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

use rh_vault::{HlcGenerator, SyncRemote};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};
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
    /// Persist the vault password to the OS keychain ("remember on this
    /// device"). When false, it's kept in memory for this session only and the
    /// user is re-prompted on the next launch.
    pub persist: bool,
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
    pub snippets: u32,
    pub notes: u32,
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
    let (token, refresh) = sync_remote::server_login(&cfg.endpoint, &req.email, &req.password)
        .await
        .map_err(|m| ApiError::validation("sync", m))?;
    sync_remote::token_set(&token).map_err(|e| ApiError::Internal {
        message: format!("keychain: {e}"),
    })?;
    if let Some(refresh) = refresh {
        let _ = sync_remote::refresh_set(&refresh);
    }
    cfg.email = Some(req.email.trim().to_lowercase());
    cfg.save();
    Ok(())
}

/// Log out: forget the token + cached vault password, and **purge the local
/// vault** so the account can't bleed into the next one.
///
/// The local SQLite + keychain are shared across accounts, and the sync model
/// propagates deletions via tombstones. If logout left local data and
/// tombstones in place, signing into a *different* account would push this
/// account's hosts into it (slice-3d bug #2) and, worse, replay this account's
/// deletion tombstones against it (bug #4 — cross-account data loss). So logout
/// clears hosts/groups/credentials (incl. keychain secrets) **and** all
/// tombstones. The data is preserved server-side and returns on the next
/// login + sync. (Adding hosts while signed out still works — they're adopted
/// into whichever account you next sign into.)
#[tauri::command]
#[instrument(level = "debug", skip(state, app))]
pub async fn sync_logout(app: AppHandle, state: State<'_, AppState>) -> ApiResult<()> {
    let cfg = SyncConfig::load();
    let token = sync_remote::token_get();
    let master = {
        let mem = state.sync_master_mem.lock().await.clone();
        mem.or_else(sync_remote::vault_key_get)
    };

    // Hold the inflight lock for the whole logout so no background pass races
    // the final flush or the wipe.
    let _guard = state.sync_inflight.lock().await;

    // SAFETY GATE: logout purges the local vault, trusting the server holds a
    // copy. Flush local changes to the server with one final pass FIRST; if we
    // can't confirm that push (offline, server/CDN error, or no vault password
    // set), abort and leave every local host/group/credential untouched. This
    // is the difference between "switch device cleanly" and "silently lose
    // un-synced data" — the latter was the bug.
    match (token.as_deref(), master.as_deref()) {
        (Some(token), Some(master)) if !cfg.endpoint.is_empty() => {
            run_sync_core(&state, &cfg.endpoint, token, master)
                .await
                .map_err(|e| {
                    ApiError::validation("sync", format!("logout flush failed: {e}"))
                })?;
        }
        _ => {
            return Err(ApiError::validation(
                "sync",
                "logout aborted: local data is not confirmed backed up (no vault password / not signed in)",
            ));
        }
    }

    // Confirmed on the server — now safe to forget credentials and wipe.
    sync_remote::token_clear();
    sync_remote::refresh_clear();
    sync_remote::vault_key_clear();
    *state.sync_master_mem.lock().await = None;
    // The derived key and the idle-pass cache are account-scoped: neither may
    // survive into the next login.
    state.vault_keys.clear();
    *state.sync_seen.lock().await = None;

    // Forget the account email so the UI returns cleanly to signed-out.
    let mut cfg = SyncConfig::load();
    cfg.email = None;
    cfg.save();

    // Account-scope the local vault: data + secrets + tombstones, all gone.
    crate::api::vault::wipe_local(&state).await?;

    // Tell the sidebar + tray to refetch (everything is empty now) and reset
    // the sync-status indicator.
    crate::api::events::emit_collections_reset(&app);
    let reset = crate::sync_engine::SyncStatusSnapshot::default();
    *state.sync_status.lock().await = reset.clone();
    let _ = app.emit(crate::sync_engine::STATUS_EVENT, &reset);

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
    let (token, refresh) = sync_remote::run_yandex_login(&cfg.endpoint)
        .await
        .map_err(|m| ApiError::validation("sync", m))?;
    sync_remote::token_set(&token).map_err(|e| ApiError::Internal {
        message: format!("keychain: {e}"),
    })?;
    if let Some(refresh) = refresh {
        let _ = sync_remote::refresh_set(&refresh);
    }
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

/// Run one sync pass right now and return when it has finished.
///
/// The engine is automatic; this exists for the moments it isn't fast enough —
/// the user watching a note that hasn't arrived yet wants a button, not faith.
/// Status events fire exactly as they do for a background pass.
#[tauri::command]
#[instrument(level = "debug", skip(app, state))]
pub async fn sync_refresh(app: AppHandle, state: State<'_, AppState>) -> ApiResult<()> {
    crate::sync_engine::run_pass(&app, &state).await;
    Ok(())
}

/// Order-independent fingerprint of a record set. Two snapshots holding the
/// same records fingerprint alike regardless of the order they were assembled
/// in (`build_snapshot` appends by entity type, `merge` emits sorted), so an
/// idle device produces a stable value pass after pass.
fn fingerprint(records: &[rh_vault::SyncRecord]) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut parts: Vec<String> = records
        .iter()
        .filter_map(|r| serde_json::to_string(r).ok())
        .collect();
    parts.sort_unstable();
    let mut h = DefaultHasher::new();
    parts.hash(&mut h);
    h.finish()
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
    let remote = ServerRemote::new(
        endpoint.to_string(),
        token.to_string(),
        sync_remote::refresh_get(),
    );

    let local = crate::api::vault::build_snapshot(state).await?;
    let local_fp = fingerprint(&local.records);

    // Cheap idle path. A full pass costs two Argon2id derivations (64 MiB each)
    // plus a rewrite of every record — far too much to repeat every couple of
    // seconds while the notes screen polls. If our records are byte-identical
    // to what we last applied AND the server is still on the version we last
    // saw, there is provably nothing to do: bail out after one small GET,
    // before any key derivation.
    {
        let seen = state.sync_seen.lock().await.clone();
        if let Some((seen_version, seen_fp)) = seen {
            if seen_fp == local_fp {
                if let Ok(Some(blob)) = remote.pull().await {
                    if blob.version == seen_version {
                        return Ok(SyncNowResponse {
                            had_remote: true,
                            pushed_version: seen_version,
                            hosts: 0,
                            groups: 0,
                            credentials: 0,
                            snippets: 0,
                            notes: 0,
                            deleted: 0,
                        });
                    }
                }
            }
        }
    }

    let seed = state.sync.last().await;
    let mut clock = HlcGenerator::new(seed);
    let report = rh_vault::sync_once_cached(
        &remote,
        master.as_bytes(),
        &local,
        &mut clock,
        &state.vault_keys,
    )
        .await
        .map_err(|e| ApiError::Internal {
            message: e.to_string(),
        })?;
    state.sync.observe(clock.last()).await;

    let counts = crate::api::vault::apply_snapshot(state, &report.merged).await?;
    {
        let mut seen = state.sync_seen.lock().await;
        *seen = Some((report.version.clone(), fingerprint(&report.merged.records)));
    }
    Ok(SyncNowResponse {
        had_remote: report.had_remote,
        pushed_version: report.version,
        hosts: counts.hosts,
        groups: counts.groups,
        credentials: counts.credentials,
        snippets: counts.snippets,
        notes: counts.notes,
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

    // A different password derives a different key — drop any cached one
    // before probing, or the probe would silently reuse the old key.
    state.vault_keys.clear();

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

    // Always hold it in memory for this session so auto-sync runs immediately.
    *state.sync_master_mem.lock().await = Some(req.master_password.clone());
    if req.persist {
        sync_remote::vault_key_set(&req.master_password).map_err(|e| ApiError::Internal {
            message: format!("keychain: {e}"),
        })?;
    } else {
        // Opted out of persistence — make sure no stale key lingers.
        sync_remote::vault_key_clear();
    }
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
