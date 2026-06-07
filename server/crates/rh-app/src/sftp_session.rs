//! Live SFTP connection registry.
//!
//! Each entry is one `SftpConn` (its own SSH transport + sftp subsystem),
//! guarded by an async `Mutex` so listing calls serialize per connection
//! (a file browser doesn't need concurrent requests on one session, and it
//! sidesteps any `Sync` requirement on the underlying client).

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::instrument;

use rh_core::SessionId;
use rh_ssh::sftp::SftpConn;

#[derive(Clone, Default)]
pub struct SftpManager {
    inner: Arc<Mutex<HashMap<SessionId, Arc<Mutex<SftpConn>>>>>,
    /// Per-transfer cancel flags, keyed by the UI-supplied transfer id.
    cancels: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
}

impl SftpManager {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[instrument(level = "debug", skip(self, conn))]
    pub async fn insert(&self, id: SessionId, conn: SftpConn) {
        self.inner
            .lock()
            .await
            .insert(id, Arc::new(Mutex::new(conn)));
    }

    pub async fn get(&self, id: &SessionId) -> Option<Arc<Mutex<SftpConn>>> {
        self.inner.lock().await.get(id).cloned()
    }

    #[instrument(level = "debug", skip(self))]
    pub async fn close(&self, id: &SessionId) {
        self.inner.lock().await.remove(id);
    }

    /// Register a fresh cancel flag for a transfer and return it.
    pub async fn register_transfer(&self, transfer_id: String) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        self.cancels.lock().await.insert(transfer_id, flag.clone());
        flag
    }

    pub async fn unregister_transfer(&self, transfer_id: &str) {
        self.cancels.lock().await.remove(transfer_id);
    }

    /// Flip the cancel flag for a running transfer, if present.
    pub async fn cancel_transfer(&self, transfer_id: &str) {
        if let Some(flag) = self.cancels.lock().await.get(transfer_id) {
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}
