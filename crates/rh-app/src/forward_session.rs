//! Registry + event bridge for live port forwards (Tools → Forwards).
//!
//! Mirrors the `SessionManager` shape but lighter: each forward is keyed
//! by a generated string id, its `rh_ssh` event stream is pumped into the
//! UI `Channel`, and the entry is evicted when the actor task exits. There
//! is no output ring / reattach (a forward has no scrollback) — on a
//! webview reload the UI simply re-`list()`s the live forwards.

use std::collections::HashMap;
use std::sync::Arc;

use tauri::ipc::Channel;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use rh_core::HostId;
use rh_ssh::{ForwardEvent, ForwardHandle, ForwardSpec, ForwardState};

use crate::api::dto::ForwardSummaryDto;

/// Live state of one forward held by the manager.
struct Entry {
    handle: ForwardHandle,
    host_id: HostId,
    host_label: String,
    spec: ForwardSpec,
    state: ForwardState,
    active: u32,
}

#[derive(Clone, Default)]
pub struct ForwardManager {
    registry: Arc<Mutex<HashMap<String, Arc<Mutex<Entry>>>>>,
}

impl ForwardManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a freshly spawned forward: start the event pump (actor →
    /// entry state + UI channel) and the exit supervisor.
    #[allow(clippy::too_many_arguments)]
    pub async fn register(
        &self,
        forward_id: String,
        host_id: HostId,
        host_label: String,
        spec: ForwardSpec,
        handle: ForwardHandle,
        join: JoinHandle<()>,
        mut rx_events: tokio::sync::mpsc::UnboundedReceiver<ForwardEvent>,
        channel: Channel<ForwardEvent>,
    ) {
        let entry = Arc::new(Mutex::new(Entry {
            handle,
            host_id,
            host_label,
            spec,
            state: ForwardState::Connecting,
            active: 0,
        }));
        self.registry
            .lock()
            .await
            .insert(forward_id.clone(), entry.clone());

        // Event pump: mirror state into the entry, forward to the UI.
        let pump = entry.clone();
        tokio::spawn(async move {
            while let Some(ev) = rx_events.recv().await {
                {
                    let mut e = pump.lock().await;
                    match &ev {
                        ForwardEvent::StateChanged { state } => e.state = *state,
                        ForwardEvent::ActiveChanged { active } => e.active = *active,
                        ForwardEvent::Closed { .. } => e.state = ForwardState::Closed,
                        ForwardEvent::Error { .. } => e.state = ForwardState::Error,
                    }
                }
                let _ = channel.send(ev);
            }
        });

        // Supervisor: evict on actor exit.
        let registry = self.registry.clone();
        let supervised = forward_id.clone();
        tokio::spawn(async move {
            let _ = join.await;
            registry.lock().await.remove(&supervised);
        });
    }

    /// Stop a forward (graceful). Returns `false` if unknown.
    pub async fn close(&self, forward_id: &str) -> bool {
        let entry = self.registry.lock().await.remove(forward_id);
        match entry {
            Some(e) => {
                e.lock().await.handle.shutdown();
                true
            }
            None => false,
        }
    }

    /// Snapshot every live forward for `forward_list`.
    pub async fn list(&self) -> Vec<ForwardSummaryDto> {
        let reg = self.registry.lock().await;
        let mut out = Vec::with_capacity(reg.len());
        for (id, entry) in reg.iter() {
            let e = entry.lock().await;
            out.push(ForwardSummaryDto {
                forward_id: id.clone(),
                host_id: e.host_id.clone(),
                host_label: e.host_label.clone(),
                spec: e.spec.clone(),
                state: e.state,
                active: e.active,
            });
        }
        out
    }
}
