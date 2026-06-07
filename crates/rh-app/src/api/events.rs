//! Global event emission to the UI.
//!
//! UI subscribes once at startup to events like `hosts:changed` and
//! invalidates relevant local caches when they fire. Payloads carry
//! only the `kind` of change and the affected `id`; for the new state
//! the UI is expected to call the relevant `*_get` / `*_list` command.
//!
//! Centralizing emission here gives us:
//! - One place to change event naming or payload shape.
//! - Symmetric behaviour: every CRUD command emits exactly one event.
//! - A natural place for cross-cutting concerns (debouncing, batching)
//!   if they're ever needed.

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tracing::warn;

use rh_core::{CredentialId, GroupId, HostId};

/// Discriminator for `*Changed` events.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Change {
    Created,
    Updated,
    Deleted,
}

#[derive(Debug, Serialize)]
struct ChangePayload<'a, Id: Serialize> {
    kind: Change,
    id: &'a Id,
}

/// Event names as constants — single source of truth, used both for
/// emission and (eventually) for any subscription logic on the Rust
/// side (we don't have any yet, but the constants document the API).
pub mod names {
    pub const HOSTS_CHANGED: &str = "hosts:changed";
    pub const GROUPS_CHANGED: &str = "groups:changed";
    pub const CREDENTIALS_CHANGED: &str = "credentials:changed";
    pub const SETTINGS_CHANGED: &str = "settings:changed";
}

pub fn emit_hosts_changed(app: &AppHandle, kind: Change, id: &HostId) {
    emit(app, names::HOSTS_CHANGED, &ChangePayload { kind, id });
}

pub fn emit_groups_changed(app: &AppHandle, kind: Change, id: &GroupId) {
    emit(app, names::GROUPS_CHANGED, &ChangePayload { kind, id });
}

pub fn emit_credentials_changed(app: &AppHandle, kind: Change, id: &CredentialId) {
    emit(app, names::CREDENTIALS_CHANGED, &ChangePayload { kind, id });
}

/// Emit all three collection-changed events at once. Used after a bulk local
/// mutation (logout purge / vault replace) where there is no single affected
/// id — the UI listens per-collection and refetches, ignoring the payload, so a
/// sentinel id is fine (`HostId::from_raw` etc. do not validate).
pub fn emit_collections_reset(app: &AppHandle) {
    emit_hosts_changed(app, Change::Deleted, &HostId::from_raw(""));
    emit_groups_changed(app, Change::Deleted, &GroupId::from_raw(""));
    emit_credentials_changed(app, Change::Deleted, &CredentialId::from_raw(""));
}

pub fn emit_settings_changed(app: &AppHandle, keys: &[&str]) {
    #[derive(Serialize)]
    struct Payload<'a> {
        keys: &'a [&'a str],
    }
    emit(app, names::SETTINGS_CHANGED, &Payload { keys });
}

fn emit<P: Serialize>(app: &AppHandle, name: &str, payload: &P) {
    if let Err(e) = app.emit(name, payload) {
        // Failure here is non-fatal: UI just won't refresh that one
        // change. The next command-issued query will still see the
        // correct state from storage.
        warn!(event = name, error = %e, "failed to emit event to UI");
    }
}
