//! Application state shared across Tauri command handlers.
//!
//! Tauri passes a single `State<AppState>` to each command via the
//! `tauri::State` extractor. We hold our stores behind `Arc<dyn …>` so
//! tests can substitute mock implementations without changing handler
//! signatures.

use std::sync::Arc;

use rh_core::{CredentialStore, GroupStore, HostStore, KnownHostsStore, SettingsStore, RdpCertStore};

use crate::local_pty::LocalPtyManager;
use crate::rdp_session::RdpSessionManager;
use crate::sftp_session::SftpManager;
use crate::session::SessionManager;

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
    pub sessions: SessionManager,
    pub rdp_sessions: RdpSessionManager,
    pub local_sessions: LocalPtyManager,
    pub sftp: SftpManager,
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
            .field("sessions", &"<SessionManager>")
            .field("rdp_sessions", &"<RdpSessionManager>")
            .field("local_sessions", &"<LocalPtyManager>")
            .field("sftp", &"<SftpManager>")
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
    ) -> Self {
        Self {
            hosts,
            groups,
            credentials,
            settings,
            known_hosts,
            rdp_certs,
            sessions: SessionManager::new(),
            rdp_sessions: RdpSessionManager::new(),
            local_sessions: LocalPtyManager::new(),
            sftp: SftpManager::new(),
        }
    }
}
