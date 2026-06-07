//! Application state shared across Tauri command handlers.
//!
//! Tauri passes a single `State<AppState>` to each command via the
//! `tauri::State` extractor. We hold our stores behind `Arc<dyn …>` so
//! tests can substitute mock implementations without changing handler
//! signatures.

use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

use rh_core::{
    CredentialStore, GroupStore, HostStore, KnownHostsStore, RdpCertStore, SettingsStore,
    StorageError, SyncMetaStore,
};

use tokio::sync::{Mutex, Notify};

use crate::local_pty::LocalPtyManager;
use crate::rdp_session::RdpSessionManager;
use crate::sftp_session::SftpManager;
use crate::session::SessionManager;
use crate::sync_clock::SyncClock;
use crate::sync_engine::SyncStatusSnapshot;

/// Bundle of every async store the command layer needs.
///
/// Held inside `tauri::State<AppState>`. Cloning is cheap (each field
/// is already an `Arc`). The struct itself is `Send + Sync` because
/// all fields are `Arc<dyn Trait + Send + Sync>` which `async_trait`
/// guarantees.
#[derive(Clone)]
pub struct AppState {
    pub hosts: Arc<dyn HostStore>,
    pub groups: Arc<dyn GroupStore>,
    pub credentials: Arc<dyn CredentialStore>,
    pub settings: Arc<dyn SettingsStore>,
    pub known_hosts: Arc<dyn KnownHostsStore>,
    pub rdp_certs: Arc<dyn RdpCertStore>,
    /// Per-record sync provenance + tombstones (slice 2b). Mutations bump it;
    /// `vault::build_snapshot` reads it so a snapshot reflects each record's
    /// last *edit*, not when it was assembled.
    pub sync_meta: Arc<dyn SyncMetaStore>,
    /// Process-wide device identity + monotonic HLC source.
    pub sync: Arc<SyncClock>,
    pub sessions: SessionManager,
    pub rdp_sessions: RdpSessionManager,
    pub local_sessions: LocalPtyManager,
    pub sftp: SftpManager,
    /// Live session count last reported by the UI (the session tabs the user
    /// can see). Read by the tray Quit handler to decide whether to ask for
    /// confirmation, and mirrored into the tray tooltip.
    pub session_count: Arc<AtomicUsize>,
    /// Background auto-sync coordination (slice 3d).
    ///
    /// `sync_wake` is pulsed by every local mutation (via `stamp_live` /
    /// `stamp_deleted`) so the background sync actor can push promptly,
    /// debounced. `sync_inflight` is a try-locked guard so periodic and
    /// change-driven passes never overlap. `sync_status` holds the latest
    /// status snapshot the UI can read on first paint (live updates arrive
    /// via the `sync:status` event).
    pub sync_wake: Arc<Notify>,
    pub sync_inflight: Arc<Mutex<()>>,
    pub sync_status: Arc<Mutex<SyncStatusSnapshot>>,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("hosts", &"<dyn HostStore>")
            .field("groups", &"<dyn GroupStore>")
            .field("credentials", &"<dyn CredentialStore>")
            .field("settings", &"<dyn SettingsStore>")
            .field("known_hosts", &"<dyn KnownHostsStore>")
            .field("rdp_certs", &"<dyn RdpCertStore>")
            .field("sync_meta", &"<dyn SyncMetaStore>")
            .field("sync", &self.sync)
            .field("sessions", &"<SessionManager>")
            .field("rdp_sessions", &"<RdpSessionManager>")
            .field("local_sessions", &"<LocalPtyManager>")
            .field("sftp", &"<SftpManager>")
            .field("session_count", &self.session_count)
            .finish()
    }
}

impl AppState {
    /// Build state by wiring concrete SQLite + keychain implementations.
    /// Used by `main.rs`; tests can construct `AppState` directly with
    /// any combination of traits.
    pub fn new(
        hosts: Arc<dyn HostStore>,
        groups: Arc<dyn GroupStore>,
        credentials: Arc<dyn CredentialStore>,
        settings: Arc<dyn SettingsStore>,
        known_hosts: Arc<dyn KnownHostsStore>,
        rdp_certs: Arc<dyn RdpCertStore>,
        sync_meta: Arc<dyn SyncMetaStore>,
        sync: Arc<SyncClock>,
    ) -> Self {
        Self {
            hosts,
            groups,
            credentials,
            settings,
            known_hosts,
            rdp_certs,
            sync_meta,
            sync,
            sessions: SessionManager::new(),
            rdp_sessions: RdpSessionManager::new(),
            local_sessions: LocalPtyManager::new(),
            sftp: SftpManager::new(),
            session_count: Arc::new(AtomicUsize::new(0)),
            sync_wake: Arc::new(Notify::new()),
            sync_inflight: Arc::new(Mutex::new(())),
            sync_status: Arc::new(Mutex::new(SyncStatusSnapshot::default())),
        }
    }

    /// Stamp a created/updated record's provenance with a fresh monotonic
    /// stamp (the record becomes/stays live). Call after a successful
    /// create/update mutation. `kind` is one of the `sync_clock::KIND_*`.
    pub async fn stamp_live(&self, kind: &str, id: &str) -> Result<(), StorageError> {
        let stamp = self.sync.next_stamp().await;
        self.sync_meta.bump(kind, id, &stamp).await?;
        // A local edit happened — nudge the background sync actor (coalesced,
        // debounced on its side). No-op if sync isn't configured yet.
        self.sync_wake.notify_one();
        Ok(())
    }

    /// Record a tombstone for a deleted record (so the deletion replicates).
    /// Call after a successful delete.
    pub async fn stamp_deleted(&self, kind: &str, id: &str) -> Result<(), StorageError> {
        let stamp = self.sync.next_stamp().await;
        self.sync_meta.tombstone(kind, id, &stamp).await?;
        self.sync_wake.notify_one();
        Ok(())
    }
}
