//! Registry + event bridge for live port forwards (Tools → Forwards).
//!
//! Holds only *running* forwards; the saved definitions live in the
//! `ForwardStore`. Each running forward is keyed by its `ForwardId`
//! (string), its `rh_ssh` event stream is pumped into the entry's live
//! state and (when started from the UI) into a `Channel`, and the entry
//! is evicted when the actor task exits. There is no scrollback — on a
//! webview reload the UI simply re-`list()`s, reading live state via
//! `state_of`.

use std::collections::HashMap;
use std::sync::Arc;

use tauri::ipc::Channel;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use rh_ssh::{ForwardEvent, ForwardHandle, ForwardState};

/// Live state of one running forward held by the manager.
struct Entry {
    handle: ForwardHandle,
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
    /// entry state + optional UI channel) and the exit supervisor. The
    /// channel is `None` for auto-started forwards (no UI listener yet);
    /// the UI reads their state via `forward_list` polling instead.
    pub async fn register(
        &self,
        forward_id: String,
        handle: ForwardHandle,
        join: JoinHandle<()>,
        mut rx_events: tokio::sync::mpsc::UnboundedReceiver<ForwardEvent>,
        channel: Option<Channel<ForwardEvent>>,
    ) {
        let entry = Arc::new(Mutex::new(Entry {
            handle,
            state: ForwardState::Connecting,
            active: 0,
        }));
        self.registry
            .lock()
            .await
            .insert(forward_id.clone(), entry.clone());

        // Event pump: mirror state into the entry, forward to the UI if
        // a channel was supplied.
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
                if let Some(ch) = &channel {
                    let _ = ch.send(ev);
                }
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

    /// Current `(state, active)` of a running forward, or `None` if it
    /// isn't running.
    pub async fn state_of(&self, forward_id: &str) -> Option<(ForwardState, u32)> {
        let entry = self.registry.lock().await.get(forward_id).cloned();
        match entry {
            Some(e) => {
                let e = e.lock().await;
                Some((e.state, e.active))
            }
            None => None,
        }
    }

    /// Whether a forward is currently registered (running or starting).
    pub async fn is_live(&self, forward_id: &str) -> bool {
        self.registry.lock().await.contains_key(forward_id)
    }
}
