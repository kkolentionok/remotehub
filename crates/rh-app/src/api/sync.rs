//! Sync IPC commands (slice 3b) — endpoint/account config, register/login,
//! and `sync_now`, which drives the engine against the server.
//!
//! `sync_now` is the whole loop: build the local snapshot, run
//! `rh_vault::sync_once` against a [`ServerRemote`] (pull → merge → seal →
//! push, with conflict-retry handled inside the engine), then apply the merged
//! result back to storage. The master password seals/opens the E2E envelope
//! and never leaves the process; the bearer token comes from the keychain.

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
pub struct SyncNowRequest {
    pub master_password: String,
}

#[derive(Debug, Serialize)]
pub struct SyncConfigResponse {
    pub endpoint: String,
    pub email: Option<String>,
    pub logged_in: bool,
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

/// Forget the stored token.
#[tauri::command]
#[instrument(level = "debug")]
pub async fn sync_logout() -> ApiResult<()> {
    sync_remote::token_clear();
    Ok(())
}

/// Run one sync against the server: build local snapshot → engine
/// (pull/merge/seal/push, conflict-retry) → apply merged back to storage.
#[tauri::command]
#[instrument(level = "debug", skip(state, req))]
pub async fn sync_now(
    state: State<'_, AppState>,
    req: SyncNowRequest,
) -> ApiResult<SyncNowResponse> {
    let cfg = SyncConfig::load();
    if cfg.endpoint.is_empty() {
        return Err(ApiError::validation("sync", "endpoint not set"));
    }
    let token =
        sync_remote::token_get().ok_or_else(|| ApiError::validation("sync", "not logged in"))?;
    let remote = ServerRemote::new(cfg.endpoint, token);

    let local = crate::api::vault::build_snapshot(&state).await?;

    // Drive the engine on a generator seeded from the shared clock, then fold
    // the advance back — so we never hold the clock lock across network I/O.
    let seed = state.sync.last().await;
    let mut clock = HlcGenerator::new(seed);
    let report = rh_vault::sync_once(&remote, req.master_password.as_bytes(), &local, &mut clock)
        .await
        .map_err(|e| ApiError::Internal {
            message: e.to_string(),
        })?;
    state.sync.observe(clock.last()).await;

    let counts = crate::api::vault::apply_snapshot(&state, &report.merged).await?;
    Ok(SyncNowResponse {
        had_remote: report.had_remote,
        pushed_version: report.version,
        hosts: counts.hosts,
        groups: counts.groups,
        credentials: counts.credentials,
        deleted: counts.deleted,
    })
}
