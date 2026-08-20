//! Background automatic sync (slice 3d).
//!
//! A single actor task, spawned once at startup, keeps the local vault in sync
//! with the server with **no user interaction** beyond entering the vault
//! (master) password once — which is then cached in the OS keychain
//! (`sync_remote::vault_key_*`). The actor:
//!
//!   * runs one pass immediately on startup,
//!   * runs a pass every [`PERIODIC_SECS`] (to pull in changes from other
//!     devices), and
//!   * runs a pass shortly after any local edit — woken via
//!     [`AppState::sync_wake`], debounced by [`DEBOUNCE_MS`] so a burst of
//!     edits collapses into a single push.
//!
//! A pass is a no-op (cheap, silent) until sync is fully configured: endpoint
//! set + bearer token in the keychain + vault password cached. Overlapping
//! passes are prevented by the `sync_inflight` try-lock. Each attempted pass
//! updates `AppState::sync_status` and emits a `sync:status` event so the UI
//! can surface a quiet status indicator.

use std::sync::atomic::Ordering;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tracing::{debug, warn};

use crate::state::AppState;
use crate::sync_remote::{self, SyncConfig};

/// Periodic pull cadence — catches edits made on other devices.
const PERIODIC_SECS: u64 = 30;
/// Tight cadence used while the Notes screen is open (`AppState::notes_fast`).
/// Notes are a live scratchpad shared across devices, so half a minute of lag
/// is unusable; a pass is cheap (one small blob in, one out).
const FAST_SECS: u64 = 3;
/// After a local edit wakes us, wait this long (collapsing further edits)
/// before pushing, so rapid successive saves become one sync.
const DEBOUNCE_MS: u64 = 1_500;
/// Push debounce while in fast mode — short enough that a typed line lands on
/// the other device quickly, long enough that a burst of keystrokes is one pass.
const FAST_DEBOUNCE_MS: u64 = 400;

/// Name of the Tauri event carrying [`SyncStatusSnapshot`] to the frontend.
pub const STATUS_EVENT: &str = "sync:status";

/// The latest sync status, mirrored into `AppState::sync_status` and emitted on
/// every attempted pass. `state` is one of `idle` | `syncing` | `ok` | `error`.
#[derive(Debug, Clone, Serialize)]
pub struct SyncStatusSnapshot {
    pub state: String,
    /// Human-readable error (only when `state == "error"`).
    pub message: Option<String>,
    /// Epoch ms of the last completed pass (`ok` or `error`).
    pub at_ms: Option<i64>,
    pub had_remote: bool,
    pub hosts: u32,
    pub groups: u32,
    pub credentials: u32,
    pub snippets: u32,
    pub notes: u32,
    pub deleted: u32,
}

impl Default for SyncStatusSnapshot {
    fn default() -> Self {
        Self {
            state: "idle".to_string(),
            message: None,
            at_ms: None,
            had_remote: false,
            hosts: 0,
            groups: 0,
            credentials: 0,
            snippets: 0,
            notes: 0,
            deleted: 0,
        }
    }
}

/// Store `snap` in `AppState::sync_status` and emit it to the UI.
async fn publish(app: &AppHandle, state: &AppState, snap: SyncStatusSnapshot) {
    {
        let mut guard = state.sync_status.lock().await;
        *guard = snap.clone();
    }
    if let Err(e) = app.emit(STATUS_EVENT, &snap) {
        debug!(error = %e, "failed to emit sync:status");
    }
}

/// True when sync is fully configured (endpoint + token + a vault key, either
/// persisted in the keychain or held in memory for this session). Returned
/// tuple is `(endpoint, token, master)` ready to drive a pass.
async fn ready(state: &AppState) -> Option<(String, String, String)> {
    let cfg = SyncConfig::load();
    if cfg.endpoint.is_empty() {
        return None;
    }
    let token = sync_remote::token_get()?;
    let master = {
        let mem = state.sync_master_mem.lock().await;
        mem.clone().or_else(sync_remote::vault_key_get)?
    };
    Some((cfg.endpoint, token, master))
}

/// Run one sync pass if configured and not already running. Silent + cheap when
/// not configured (no event, status untouched).
async fn run_pass(app: &AppHandle, state: &AppState) {
    let Some((endpoint, token, master)) = ready(state).await else {
        return; // not signed in / no master yet — stay quiet
    };

    // Never overlap a periodic pass with a change-driven one.
    let _guard = match state.sync_inflight.try_lock() {
        Ok(g) => g,
        Err(_) => {
            debug!("sync pass skipped: another pass in flight");
            return;
        }
    };

    publish(
        app,
        state,
        SyncStatusSnapshot {
            state: "syncing".to_string(),
            ..Default::default()
        },
    )
    .await;

    let now_ms = chrono::Utc::now().timestamp_millis();
    match crate::api::sync::run_sync_core(state, &endpoint, &token, &master).await {
        Ok(resp) => {
            debug!(version = %resp.pushed_version, "auto-sync ok");
            publish(
                app,
                state,
                SyncStatusSnapshot {
                    state: "ok".to_string(),
                    message: None,
                    at_ms: Some(now_ms),
                    had_remote: resp.had_remote,
                    hosts: resp.hosts,
                    groups: resp.groups,
                    credentials: resp.credentials,
                    snippets: resp.snippets,
                    notes: resp.notes,
                    deleted: resp.deleted,
                },
            )
            .await;
        }
        Err(e) => {
            let message = e.to_string();
            warn!(error = %message, "auto-sync failed");
            publish(
                app,
                state,
                SyncStatusSnapshot {
                    state: "error".to_string(),
                    message: Some(message),
                    at_ms: Some(now_ms),
                    ..Default::default()
                },
            )
            .await;
        }
    }
}

/// The actor loop. Spawned once from `main.rs` setup with a clone of the
/// managed [`AppState`] and the app handle. Never returns.
pub async fn run_loop(app: AppHandle, state: AppState) {
    // Startup pass first, then wait-then-pass forever. The cadence is read at
    // the top of each wait so `notes_fast` takes effect on the next cycle.
    run_pass(&app, &state).await;

    loop {
        let fast = state.notes_fast.load(Ordering::Relaxed);
        let period = if fast { FAST_SECS } else { PERIODIC_SECS };
        let debounce = if fast { FAST_DEBOUNCE_MS } else { DEBOUNCE_MS };

        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(period)) => {}
            // A local edit happened. Debounce so a burst collapses into one pass.
            _ = state.sync_wake.notified() => {
                tokio::time::sleep(Duration::from_millis(debounce)).await;
            }
        }
        run_pass(&app, &state).await;
    }
}
