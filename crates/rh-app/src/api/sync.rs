//! Sync IPC commands — endpoint/account config, register/login, Yandex OAuth,
//! and the vault-password handoff for **automatic** sync (slice 3d).
//!
//! There is no manual "sync now": once the user is signed in and has set the
//! vault (master) password once via [`sync_set_master`], the background actor
//! in [`crate::sync_engine`] keeps everything in sync (on startup, after every
//! local edit, and periodically). [`run_sync_core`] is the shared pass the
//! actor and `sync_set_master` both drive; [`sync_status`] exposes the latest
//! status for first paint. The master password seals/opens the E2E envelope and
//! is cached only in the OS keychain; the bearer token comes from there too.

use rh_vault::HlcGenerator;
use serde::{Deserialize, Serialize};
use tauri::State;
use tracing::instrument;

use crate::api::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::sync_remote::{self, ServerRemote, SyncConfig};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyncEndpointRequest {
    pub endpoint: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyncAuthRequest {
    pub email: String,
    pub password: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyncMasterRequest {
    pub master_password: String,
}

#[derive(Debug, Serialize)]
pub struct SyncConfigResponse {
    pub endpoint: String,
    pub email: Option<String>,
    pub logged_in: bool,
    /// Whether the vault (master) password is cached for automatic sync.
    /// When `logged_in && !has_master`, the UI prompts for it.
    pub has_master: bool,
}

#[derive(Debug, Serialize)]
pub struct SyncNowResponse {
    pub had_remote: bool,
    pub pushed_version: String,
    pub hosts: u32,
    pub groups: u32,
    pub credentials: u32,
    pub deleted: u32,
}

/// Current endpoint/account + whether a token is stored.
#[tauri::command]
#[instrument(level = "debug")]
pub async fn sync_get_config() -> ApiResult<SyncConfigResponse> {
    let cfg = SyncConfig::load();
    Ok(SyncConfigResponse {
        endpoint: cfg.endpoint,
        email: cfg.email,
        logged_in: sync_remote::token_get().is_some(),
        has_master: sync_remote::vault_key_get().is_some(),
    })
}

/// Set the server base URL (trailing slash trimmed).
#[tauri::command]
#[instrument(level = "debug")]
pub async fn sync_set_endpoint(req: SyncEndpointRequest) -> ApiResult<()> {
    let mut cfg = SyncConfig::load();
    cfg.endpoint = req.endpoint.trim().trim_end_matches('/').to_string();
    cfg.save();
    Ok(())
}

/// Create an account on the configured server.
#[tauri::command]
#[instrument(level = "debug", skip(req))]
pub async fn sync_register(req: SyncAuthRequest) -> ApiResult<()> {
    let cfg = SyncConfig::load();
    if cfg.endpoint.is_empty() {
        return Err(ApiError::validation("sync", "endpoint not set"));
    }
    sync_remote::server_register(&cfg.endpoint, &req.email, &req.password)
        .await
        .map_err(|m| ApiError::validation("sync", m))
}

/// Log in; store the bearer token in the OS keychain and remember the email.
#[tauri::command]
#[instrument(level = "debug", skip(req))]
pub async fn sync_login(req: SyncAuthRequest) -> ApiResult<()> {
    let mut cfg = SyncConfig::load();
    if cfg.endpoint.is_empty() {
        return Err(ApiError::validation("sync", "endpoint not set"));
    }
    let token = sync_remote::server_login(&cfg.endpoint, &req.email, &req.password)
        .await
        .map_err(|m| ApiError::validation("sync", m))?;
    sync_remote::token_set(&token).map_err(|e| ApiError::Internal {
        message: format!("keychain: {e}"),
    })?;
    cfg.email = Some(req.email.trim().to_lowercase());
    cfg.save();
    Ok(())
}

/// Forget the stored token and cached vault password.
#[tauri::command]
#[instrument(level = "debug")]
pub async fn sync_logout() -> ApiResult<()> {
    sync_remote::token_clear();
    sync_remote::vault_key_clear();
    Ok(())
}

/// Desktop Yandex sign-in: opens the browser, runs the server-mediated loopback
/// flow, stores the returned bearer token, and fetches the email for display.
#[tauri::command]
#[instrument(level = "debug")]
pub async fn sync_oauth_yandex() -> ApiResult<SyncConfigResponse> {
    let cfg = SyncConfig::load();
    if cfg.endpoint.is_empty() {
        return Err(ApiError::validation("sync", "endpoint not set"));
    }
    let token = sync_remote::run_yandex_login(&cfg.endpoint)
        .await
        .map_err(|m| ApiError::validation("sync", m))?;
    sync_remote::token_set(&token).map_err(|e| ApiError::Internal {
        message: format!("keychain: {e}"),
    })?;
    // Best-effort: remember the email for the "signed in as …" display.
    let mut cfg = SyncConfig::load();
    if let Ok(email) = sync_remote::server_me(&cfg.endpoint, &token).await {
        cfg.email = Some(email);
        cfg.save();
    }
    Ok(SyncConfigResponse {
        endpoint: cfg.endpoint,
        email: cfg.email,
        logged_in: true,
        has_master: sync_remote::vault_key_get().is_some(),
    })
}

/// Core sync pass, shared by the background actor and `sync_set_master`:
/// build local snapshot → engine (pull/merge/seal/push, conflict-retry) →
/// apply merged back to storage. Never holds the clock lock across network I/O.
pub(crate) async fn run_sync_core(
    state: &AppState,
    endpoint: &str,
    token: &str,
    master: &str,
) -> ApiResult<SyncNowResponse> {
    let remote = ServerRemote::new(endpoint.to_string(), token.to_string());

    let local = crate::api::vault::build_snapshot(state).await?;

    let seed = state.sync.last().await;
    let mut clock = HlcGenerator::new(seed);
    let report = rh_vault::sync_once(&remote, master.as_bytes(), &local, &mut clock)
        .await
        .map_err(|e| ApiError::Internal {
            message: e.to_string(),
        })?;
    state.sync.observe(clock.last()).await;

    let counts = crate::api::vault::apply_snapshot(state, &report.merged).await?;
    Ok(SyncNowResponse {
        had_remote: report.had_remote,
        pushed_version: report.version,
        hosts: counts.hosts,
        groups: counts.groups,
        credentials: counts.credentials,
        deleted: counts.deleted,
    })
}

/// Cache the vault (master) password so automatic sync can run unattended.
///
/// We validate it first by running one real pass: if the password is wrong the
/// engine fails to open the remote envelope (`decryption failed`) and we reject
/// without storing. Any other failure (network/server) still stores the key —
/// it's plausibly correct and the background actor will retry — so a transient
/// hiccup doesn't block the user. On success the background actor takes over.
#[tauri::command]
#[instrument(level = "debug", skip(state, req))]
pub async fn sync_set_master(
    state: State<'_, AppState>,
    req: SyncMasterRequest,
) -> ApiResult<SyncConfigResponse> {
    let cfg = SyncConfig::load();
    if cfg.endpoint.is_empty() {
        return Err(ApiError::validation("sync", "endpoint not set"));
    }
    let token =
        sync_remote::token_get().ok_or_else(|| ApiError::validation("sync", "not logged in"))?;

    // Serialize with the background actor so the validation pass can't race a
    // periodic one (held only for this single pass).
    let probe = {
        let _guard = state.sync_inflight.lock().await;
        run_sync_core(&state, &cfg.endpoint, &token, &req.master_password).await
    };
    match probe {
        Ok(_) => {}
        Err(ApiError::Internal { message }) if message.to_lowercase().contains("decrypt") => {
            return Err(ApiError::validation(
                "master_password",
                "wrong vault password",
            ));
        }
        // Non-decrypt failure (e.g. offline): keep the key; actor will retry.
        Err(_) => {}
    }

    sync_remote::vault_key_set(&req.master_password).map_err(|e| ApiError::Internal {
        message: format!("keychain: {e}"),
    })?;
    // Nudge the actor so any further reconciliation happens promptly.
    state.sync_wake.notify_one();

    Ok(SyncConfigResponse {
        endpoint: cfg.endpoint,
        email: cfg.email,
        logged_in: true,
        has_master: true,
    })
}

/// Current background-sync status (for first paint; live updates come via the
/// `sync:status` event).
#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn sync_status(
    state: State<'_, AppState>,
) -> ApiResult<crate::sync_engine::SyncStatusSnapshot> {
    Ok(state.sync_status.lock().await.clone())
}
