//! In-memory registry of running session actors.
//!
//! Each entry is a [`Hub`]: it owns the actor's command channel, mirrors
//! the actor's event stream into the current UI [`Channel`], and keeps a
//! bounded ring of recent terminal output. The ring is what makes
//! *restore-on-reload* work — the Rust process outlives a webview reload,
//! so after the UI reloads it calls [`SessionManager::list`] to discover
//! live sessions and [`SessionManager::reattach`] to swap in a fresh
//! channel and replay the buffered scrollback.
//!
//! A supervisor task per session awaits the actor's `JoinHandle` and
//! evicts the entry when it exits (clean, errored, or panicked), so the
//! registry never leaks.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use tauri::ipc::Channel;
use tauri::AppHandle;
use tokio::sync::Mutex;
use tokio::task::{AbortHandle, JoinHandle};
use tracing::{error, warn};

use rh_core::{HostId, HostStore, Protocol, SessionId};
use rh_ssh::{SessionCommand, SessionState, SshSessionEvent, SshSessionHandle};

use crate::api::events::{self, Change};

/// How much raw terminal output to retain per session for replay on
/// reattach. Enough for a screen or two of scrollback without unbounded
/// growth on chatty sessions.
const OUTPUT_RING_BYTES: usize = 256 * 1024;

/// Stable, non-secret metadata about a live session, surfaced to the UI
/// by [`SessionManager::list`] so it can rebuild tabs after a reload.
#[derive(Debug, Clone)]
pub struct SessionMeta {
    pub host_id: HostId,
    pub hostname: String,
    pub title: String,
    pub protocol: Protocol,
    pub opened_at: DateTime<Utc>,
}

/// A snapshot of one live session for `session_list`.
#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub session_id: SessionId,
    pub meta: SessionMeta,
    pub state: SessionState,
}

/// Per-session live state held by the manager.
struct Hub {
    tx_cmd: tokio::sync::mpsc::Sender<SessionCommand>,
    abort: AbortHandle,
    meta: SessionMeta,
    state: SessionState,
    /// Recent raw output bytes (ANSI included) for reattach replay.
    output: VecDeque<u8>,
    /// Current UI channel, or `None` after a reload until reattach.
    sink: Option<Channel<SshSessionEvent>>,
    /// Set once we've stamped the host's `last_connected_at` (on first Ready).
    connected_stamped: bool,
}

impl Hub {
    /// Update local state from an outgoing event before it's forwarded.
    fn record(&mut self, ev: &SshSessionEvent) {
        match ev {
            SshSessionEvent::StateChanged { state } => self.state = *state,
            SshSessionEvent::Data { bytes } => {
                self.output.extend(bytes.iter().copied());
                let overflow = self.output.len().saturating_sub(OUTPUT_RING_BYTES);
                if overflow > 0 {
                    self.output.drain(0..overflow);
                }
            }
            SshSessionEvent::Closed { .. } => self.state = SessionState::Closed,
            _ => {}
        }
    }
}

#[derive(Clone, Default)]
pub struct SessionManager {
    registry: Arc<Mutex<HashMap<SessionId, Arc<Mutex<Hub>>>>>,
}

impl SessionManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a freshly spawned session: start the event pump (actor →
    /// ring + current channel) and the exit supervisor.
    pub async fn register(
        &self,
        handle: SshSessionHandle,
        join: JoinHandle<()>,
        hostname: String,
        title: String,
        channel: Channel<SshSessionEvent>,
        mut rx_events: tokio::sync::mpsc::UnboundedReceiver<SshSessionEvent>,
        hosts: Arc<dyn HostStore>,
        app: AppHandle,
    ) {
        let id = handle.id.clone();
        let meta = SessionMeta {
            host_id: handle.host_id.clone(),
            hostname,
            title,
            protocol: handle.protocol,
            opened_at: handle.opened_at,
        };
        let hub = Arc::new(Mutex::new(Hub {
            tx_cmd: handle.tx_cmd,
            abort: handle.abort,
            meta,
            state: SessionState::Connecting,
            output: VecDeque::new(),
            sink: Some(channel),
            connected_stamped: false,
        }));

        self.registry.lock().await.insert(id.clone(), hub.clone());

        // Event pump: record into the ring, forward to the live channel,
        // and stamp the host's last-connected time on first Ready.
        let pump_hub = hub.clone();
        tokio::spawn(async move {
            while let Some(ev) = rx_events.recv().await {
                // OS detection is backend-only: persist it and don't
                // forward to the UI channel (the chip refreshes on the
                // next host_get).
                if let SshSessionEvent::DetectedOs { os } = &ev {
                    let hosts = hosts.clone();
                    let host_id = {
                        let h = pump_hub.lock().await;
                        h.meta.host_id.clone()
                    };
                    let os = os.clone();
                    let app = app.clone();
                    tokio::spawn(async move {
                        if let Err(e) = hosts.mark_detected_os(&host_id, &os).await {
                            warn!(error = %e, "failed to persist detected OS");
                        } else {
                            // Refresh the UI host list so the sidebar icon
                            // switches to the detected OS immediately.
                            events::emit_hosts_changed(&app, Change::Updated, &host_id);
                        }
                    });
                    continue;
                }
                let mut h = pump_hub.lock().await;
                h.record(&ev);
                if !h.connected_stamped
                    && matches!(
                        ev,
                        SshSessionEvent::StateChanged {
                            state: SessionState::Ready
                        }
                    )
                {
                    h.connected_stamped = true;
                    let hosts = hosts.clone();
                    let host_id = h.meta.host_id.clone();
                    tokio::spawn(async move {
                        if let Err(e) =
                            hosts.mark_connected(&host_id, chrono::Utc::now()).await
                        {
                            warn!(error = %e, "failed to stamp last_connected_at");
                        }
                    });
                }
                if let Some(sink) = &h.sink {
                    // Send failure just means the UI channel went away
                    // (e.g. a reload); the ring keeps accumulating so a
                    // reattach can replay it.
                    let _ = sink.send(ev);
                }
            }
        });

        // Supervisor: evict on actor exit.
        let registry = self.registry.clone();
        let supervised_id = id.clone();
        tokio::spawn(async move {
            if let Err(e) = join.await {
                if e.is_panic() {
                    error!(session_id = %supervised_id, "session actor panicked: {e:?}");
                }
            }
            registry.lock().await.remove(&supervised_id);
        });
    }

    /// Send a command to a session. Returns `false` if it's unknown.
    pub async fn send(&self, id: &SessionId, cmd: SessionCommand) -> bool {
        let tx = {
            let reg = self.registry.lock().await;
            match reg.get(id) {
                Some(hub) => hub.lock().await.tx_cmd.clone(),
                None => return false,
            }
        };
        tx.send(cmd).await.is_ok()
    }

    /// Request graceful shutdown and evict the entry. The supervisor's
    /// own removal becomes a harmless no-op.
    pub async fn close(&self, id: &SessionId) {
        let hub = self.registry.lock().await.remove(id);
        if let Some(hub) = hub {
            let h = hub.lock().await;
            if h.tx_cmd.send(SessionCommand::Shutdown).await.is_err() {
                h.abort.abort();
            }
        }
    }

    /// Snapshot every live session for restore-on-reload.
    pub async fn list(&self) -> Vec<SessionSummary> {
        let reg = self.registry.lock().await;
        let mut out = Vec::with_capacity(reg.len());
        for (id, hub) in reg.iter() {
            let h = hub.lock().await;
            out.push(SessionSummary {
                session_id: id.clone(),
                meta: h.meta.clone(),
                state: h.state,
            });
        }
        // Stable order by open time so tabs come back in a sensible order.
        out.sort_by_key(|s| s.meta.opened_at);
        out
    }

    /// Point a session at a fresh UI channel (after a reload) and replay
    /// its buffered output + current state so the terminal repaints.
    /// Returns `false` if the session is unknown (already gone).
    pub async fn reattach(&self, id: &SessionId, channel: Channel<SshSessionEvent>) -> bool {
        let hub = {
            let reg = self.registry.lock().await;
            match reg.get(id) {
                Some(hub) => hub.clone(),
                None => return false,
            }
        };
        let mut h = hub.lock().await;
        // Replay buffered scrollback as a single data burst, then the
        // current lifecycle state, before wiring the channel for live
        // events — order guarantees the UI paints history then status.
        if !h.output.is_empty() {
            let bytes: Vec<u8> = h.output.iter().copied().collect();
            let _ = channel.send(SshSessionEvent::Data { bytes });
        }
        let _ = channel.send(SshSessionEvent::StateChanged { state: h.state });
        h.sink = Some(channel);
        true
    }
}
