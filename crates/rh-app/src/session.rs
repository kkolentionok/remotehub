//! In-memory registry of running session actors.
//!
//! Holds one [`SshSessionHandle`] per live session. A supervisor task
//! per session awaits the actor's `JoinHandle` and evicts the entry when
//! it exits (clean, errored, or panicked), so the registry never leaks.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::error;

use rh_core::SessionId;
use rh_ssh::{SessionCommand, SshSessionHandle};

#[derive(Clone, Default)]
pub struct SessionManager {
    registry: Arc<Mutex<HashMap<SessionId, SshSessionHandle>>>,
}

impl SessionManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a freshly spawned session and start its supervisor.
    pub async fn register(&self, handle: SshSessionHandle, join: JoinHandle<()>) {
        let id = handle.id.clone();

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

        self.registry.lock().await.insert(id, handle);
    }

    /// Send a command to a session. Returns `false` if it's unknown.
    pub async fn send(&self, id: &SessionId, cmd: SessionCommand) -> bool {
        let tx = {
            let reg = self.registry.lock().await;
            reg.get(id).map(|h| h.tx_cmd.clone())
        };
        match tx {
            Some(tx) => tx.send(cmd).await.is_ok(),
            None => false,
        }
    }

    /// Request graceful shutdown and evict the entry. The supervisor's
    /// own removal becomes a harmless no-op.
    pub async fn close(&self, id: &SessionId) {
        let handle = self.registry.lock().await.remove(id);
        if let Some(h) = handle {
            // Ask the actor to wind down; fall back to abort if its
            // command queue is already gone.
            if h.tx_cmd.send(SessionCommand::Shutdown).await.is_err() {
                h.abort.abort();
            }
        }
    }
}
