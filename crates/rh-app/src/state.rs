//! Application state shared across Tauri command handlers.
//!
//! Tauri passes a single `State<AppState>` to each command via the
//! `tauri::State` extractor. We hold our stores behind `Arc<dyn …>` so
//! tests can substitute mock implementations without changing handler
//! signatures.

use std::sync::Arc;

use rh_core::{CredentialStore, GroupStore, HostStore, SettingsStore};

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
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("hosts", &"<dyn HostStore>")
            .field("groups", &"<dyn GroupStore>")
            .field("credentials", &"<dyn CredentialStore>")
            .field("settings", &"<dyn SettingsStore>")
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
    ) -> Self {
        Self {
            hosts,
            groups,
            credentials,
            settings,
        }
    }
}
