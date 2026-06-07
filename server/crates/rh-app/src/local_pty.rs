//! Local PTY session: spawns the system shell in a pseudo-terminal
//! (ConPTY on Windows, openpty on Unix via `portable-pty`) and bridges it
//! to the **same** `SshSessionEvent` / `SessionCommand` contract the UI
//! terminal already speaks. That lets the frontend render a local shell
//! with the existing xterm.js `Terminal` component unchanged — only the
//! IPC entry points differ.
//!
//! Like RDP, this gets its own thin registry rather than going through the
//! SSH `SessionManager` (no host, no host-key TOFU). It *does* keep a bounded
//! output ring + swappable sink, mirroring the SSH hub, so local shells also
//! survive a webview reload via [`LocalPtyManager::list`] + `reattach`.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use tauri::ipc::Channel;
use tokio::sync::{mpsc, Mutex};
use tracing::instrument;

use rh_core::SessionId;
use rh_ssh::{CloseReason, SessionCommand, SessionState, SshSessionEvent};

/// How much raw PTY output to retain per session for replay on reattach.
const OUTPUT_RING_BYTES: usize = 256 * 1024;

/// A snapshot of one live local shell for `local_session_list`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LocalSessionSummary {
    pub session_id: SessionId,
    pub title: String,
}

/// Per-session live state: the command sender, a bounded output ring for
/// replay, and the current UI sink (None between a reload and reattach).
struct LocalHub {
    tx_cmd: mpsc::Sender<SessionCommand>,
    title: String,
    output: VecDeque<u8>,
    sink: Option<Channel<SshSessionEvent>>,
    state: SessionState,
    opened_at: DateTime<Utc>,
}

impl LocalHub {
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
pub struct LocalPtyManager {
    inner: Arc<Mutex<HashMap<SessionId, Arc<Mutex<LocalHub>>>>>,
}

impl LocalPtyManager {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Spawn a shell PTY and start forwarding its byte stream to the UI.
    /// `shell` overrides the system default when non-empty.
    #[instrument(level = "debug", skip(self, on_event))]
    pub async fn open(
        &self,
        id: SessionId,
        cols: u16,
        rows: u16,
        shell: Option<String>,
        on_event: Channel<SshSessionEvent>,
    ) -> Result<(), String> {
        let (tx_events, mut rx_events) = mpsc::unbounded_channel::<SshSessionEvent>();
        let (tx_cmd, rx_cmd) = mpsc::channel::<SessionCommand>(64);

        // Title for the restored tab: the chosen shell's basename (or the
        // system default). Computed before `spawn_pty` consumes `shell`.
        let program = shell
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(default_shell);
        let title = program
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(&program)
            .to_string();

        // Open the PTY + spawn worker threads. Fails fast (e.g. shell not
        // found) so the caller can surface an error before registering.
        spawn_pty(cols, rows, shell, rx_cmd, tx_events)?;

        let hub = Arc::new(Mutex::new(LocalHub {
            tx_cmd,
            title,
            output: VecDeque::new(),
            sink: Some(on_event),
            state: SessionState::Ready,
            opened_at: Utc::now(),
        }));
        self.inner.lock().await.insert(id.clone(), hub.clone());

        // Pump: record into the ring, forward to the live sink. When the
        // worker's event stream ends (shell gone), evict the entry.
        let inner = self.inner.clone();
        let pump_hub = hub.clone();
        tokio::spawn(async move {
            while let Some(ev) = rx_events.recv().await {
                let mut h = pump_hub.lock().await;
                h.record(&ev);
                if let Some(sink) = &h.sink {
                    let _ = sink.send(ev);
                }
            }
            inner.lock().await.remove(&id);
        });
        Ok(())
    }

    #[instrument(level = "debug", skip(self, cmd))]
    pub async fn send(&self, id: &SessionId, cmd: SessionCommand) {
        let tx = {
            let reg = self.inner.lock().await;
            match reg.get(id) {
                Some(hub) => hub.lock().await.tx_cmd.clone(),
                None => return,
            }
        };
        let _ = tx.send(cmd).await;
    }

    #[instrument(level = "debug", skip(self))]
    pub async fn close(&self, id: &SessionId) {
        let hub = self.inner.lock().await.remove(id);
        if let Some(hub) = hub {
            let tx = hub.lock().await.tx_cmd.clone();
            let _ = tx.send(SessionCommand::Shutdown).await;
        }
    }

    /// Snapshot live local shells for restore-on-reload.
    pub async fn list(&self) -> Vec<LocalSessionSummary> {
        let reg = self.inner.lock().await;
        let mut out = Vec::with_capacity(reg.len());
        let mut rows: Vec<(DateTime<Utc>, LocalSessionSummary)> = Vec::new();
        for (id, hub) in reg.iter() {
            let h = hub.lock().await;
            rows.push((
                h.opened_at,
                LocalSessionSummary {
                    session_id: id.clone(),
                    title: h.title.clone(),
                },
            ));
        }
        rows.sort_by_key(|(t, _)| *t);
        out.extend(rows.into_iter().map(|(_, s)| s));
        out
    }

    /// Point a session at a fresh UI channel after a reload: replay the
    /// buffered output + current state, then wire the channel for live
    /// events. Returns `false` if the session is gone.
    pub async fn reattach(&self, id: &SessionId, channel: Channel<SshSessionEvent>) -> bool {
        let hub = {
            let reg = self.inner.lock().await;
            match reg.get(id) {
                Some(hub) => hub.clone(),
                None => return false,
            }
        };
        let mut h = hub.lock().await;
        if !h.output.is_empty() {
            let bytes: Vec<u8> = h.output.iter().copied().collect();
            let _ = channel.send(SshSessionEvent::Data { bytes });
        }
        let _ = channel.send(SshSessionEvent::StateChanged { state: h.state });
        h.sink = Some(channel);
        true
    }
}

/// Pick the system shell. PowerShell on Windows; `$SHELL` (or bash) on Unix.
fn default_shell() -> String {
    #[cfg(windows)]
    {
        "powershell.exe".to_string()
    }
    #[cfg(not(windows))]
    {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string())
    }
}

fn home_dir() -> Option<String> {
    #[cfg(windows)]
    {
        std::env::var("USERPROFILE").ok().filter(|s| !s.is_empty())
    }
    #[cfg(not(windows))]
    {
        std::env::var("HOME").ok().filter(|s| !s.is_empty())
    }
}

/// Open a PTY, spawn the shell, and wire two workers:
/// - a blocking **reader thread** that pumps PTY output → `Data` events,
///   and emits `Closed` on EOF (shell exited / PTY torn down);
/// - a **command task** that owns the write side + child and applies
///   `SshInput` / `Resize` / `Shutdown`.
fn spawn_pty(
    cols: u16,
    rows: u16,
    shell: Option<String>,
    mut rx_cmd: mpsc::Receiver<SessionCommand>,
    tx_events: mpsc::UnboundedSender<SshSessionEvent>,
) -> Result<(), String> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("openpty: {e}"))?;

    let program = shell
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(default_shell);
    let mut cmd = CommandBuilder::new(program);
    if let Some(home) = home_dir() {
        cmd.cwd(home);
    }
    cmd.env("TERM", "xterm-256color");

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("spawn shell: {e}"))?;

    // Read + write handles are cloned/taken off the master before it moves
    // into the command task (where resize lives).
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("clone reader: {e}"))?;
    let mut writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("take writer: {e}"))?;
    let master = pair.master;
    // Drop the slave so the child holds the only slave end — required for a
    // clean EOF on the reader when the shell exits.
    drop(pair.slave);

    // The shell is up; tell the UI it's ready (no connect phase locally).
    let _ = tx_events.send(SshSessionEvent::StateChanged {
        state: SessionState::Ready,
    });

    // Reader thread (blocking I/O): PTY output → Data; EOF/err → Closed.
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx_events
                        .send(SshSessionEvent::Data {
                            bytes: buf[..n].to_vec(),
                        })
                        .is_err()
                    {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = tx_events.send(SshSessionEvent::Closed {
            reason: CloseReason::ServerDisconnected { message: None },
        });
        // `tx_events` drops here → the forwarder's stream ends → cleanup.
    });

    // Command task: owns writer + master + child. Writes are tiny
    // (keystrokes), so doing them inline in the async task is fine.
    tokio::spawn(async move {
        // Keep `master` alive for the lifetime of the task (resize target).
        let master = master;
        while let Some(cmd) = rx_cmd.recv().await {
            match cmd {
                SessionCommand::SshInput(bytes) => {
                    let _ = writer.write_all(&bytes);
                    let _ = writer.flush();
                }
                SessionCommand::Resize { cols, rows } => {
                    let _ = master.resize(PtySize {
                        rows,
                        cols,
                        pixel_width: 0,
                        pixel_height: 0,
                    });
                }
                SessionCommand::Shutdown => {
                    let _ = child.kill();
                    break;
                }
                // No host-key TOFU for a local shell.
                SessionCommand::HostKeyDecision(_) => {}
            }
        }
        let _ = child.kill();
    });

    Ok(())
}
