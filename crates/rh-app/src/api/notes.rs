//! `note_*` Tauri commands (Tools → Notes).
//!
//! Plain CRUD over `NoteStore` plus `note_set_fast_sync`, which the Notes
//! screen calls on mount/unmount to switch the background sync actor to a
//! tight cadence. Coarse mutations: create returns the new id, update/delete
//! return nothing — the UI refetches the list. Notes replicate through the
//! vault like snippets, so every signed-in device converges on the same set.

use std::sync::atomic::Ordering;

use chrono::Utc;
use tauri::State;
use tracing::instrument;

use rh_core::{Note, NoteId};

use crate::api::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::sync_clock::KIND_NOTE;

const MAX_TITLE: usize = 200;
const MAX_BODY: usize = 262_144;

fn validate(title: &str, body: &str) -> ApiResult<()> {
    if title.chars().count() > MAX_TITLE {
        return Err(ApiError::validation("title", "title too long".to_string()));
    }
    if body.len() > MAX_BODY {
        return Err(ApiError::validation("body", "note too long".to_string()));
    }
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn note_list(state: State<'_, AppState>) -> ApiResult<Vec<Note>> {
    Ok(state.notes.list().await?)
}

#[tauri::command]
#[instrument(level = "debug", skip(state, body))]
pub async fn note_create(
    state: State<'_, AppState>,
    title: String,
    body: String,
) -> ApiResult<NoteId> {
    validate(&title, &body)?;
    let note = Note::new(title.trim(), body);
    let id = note.id.clone();
    state.notes.create(&note).await?;
    state.stamp_live(KIND_NOTE, id.as_str()).await?;
    state.notes_wake.notify_one();
    Ok(id)
}

#[tauri::command]
#[instrument(level = "debug", skip(state, body))]
pub async fn note_update(
    state: State<'_, AppState>,
    id: NoteId,
    title: String,
    body: String,
) -> ApiResult<()> {
    validate(&title, &body)?;
    // `created_at` is ignored by the UPDATE (targets by id); `updated_at` bumps
    // and doubles as the sync revision source.
    let now = Utc::now();
    let note = Note {
        id,
        title: title.trim().to_string(),
        body,
        // Ignored by the UPDATE (which touches title/body/updated_at only);
        // the pin flag is owned by `note_set_pinned`.
        pinned: false,
        created_at: now,
        updated_at: now,
    };
    state.notes.update(&note).await?;
    state.stamp_live(KIND_NOTE, note.id.as_str()).await?;
    state.notes_wake.notify_one();
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn note_delete(state: State<'_, AppState>, id: NoteId) -> ApiResult<()> {
    state.notes.delete(&id).await?;
    state.stamp_deleted(KIND_NOTE, id.as_str()).await?;
    state.notes_wake.notify_one();
    Ok(())
}

/// Pin / unpin a note. Persists immediately (no debounce) like the other
/// boolean toggles in the app.
#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn note_set_pinned(
    state: State<'_, AppState>,
    id: NoteId,
    pinned: bool,
) -> ApiResult<()> {
    state.notes.set_pinned(&id, pinned).await?;
    state.stamp_live(KIND_NOTE, id.as_str()).await?;
    state.notes_wake.notify_one();
    Ok(())
}

/// Turn the tight sync cadence on (Notes screen opened) or off (closed).
/// Idempotent; the actor reads the flag at the top of every wait.
#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn note_set_fast_sync(state: State<'_, AppState>, on: bool) -> ApiResult<()> {
    state.notes_fast.store(on, Ordering::Relaxed);
    if on {
        // Pull immediately so the screen opens with fresh content.
        state.notes_wake.notify_one();
    }
    Ok(())
}

// ── pairing (access code) ───────────────────────────────────────────────────

use serde::Serialize;

#[derive(Serialize)]
pub struct PairCodeResponse {
    /// The code to read out, already grouped: `K7QD-M2XR`.
    pub code: String,
    pub expires_at: String,
}

#[derive(Serialize)]
pub struct PairedDevice {
    pub id: String,
    pub label: String,
    pub created_at: String,
    pub last_seen_at: Option<String>,
}

#[derive(Serialize)]
pub struct NotesModeResponse {
    /// True when this device holds only a notes-scoped token — signed out of
    /// the account, paired by code, notes and nothing else.
    pub notes_only: bool,
    /// True when notes can sync at all (account token or notes token present).
    pub connected: bool,
}

fn grouped(code: &str) -> String {
    let mid = code.len() / 2;
    format!("{}-{}", &code[..mid], &code[mid..])
}

/// Mint a pairing code. The code never leaves this device: the server gets
/// its hash and the notes key wrapped under a key derived from it.
#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn notes_pair_create(state: State<'_, AppState>) -> ApiResult<PairCodeResponse> {
    let cfg = crate::sync_remote::SyncConfig::load();
    let token = crate::sync_remote::token_get().ok_or_else(|| {
        ApiError::validation("account", "sign in first".to_string())
    })?;
    let notes_key = crate::notes_sync::key_from_keychain().ok_or_else(|| {
        ApiError::validation("notes", "notes are not synced yet — wait for a sync pass".to_string())
    })?;

    let code = rh_vault::gen_pairing_code().map_err(|e| ApiError::Internal {
        message: format!("code: {e}"),
    })?;
    let ckey = rh_vault::code_key(&code).map_err(|e| ApiError::Internal {
        message: format!("code key: {e}"),
    })?;
    let wrapped = rh_vault::wrap_notes_key(&notes_key, &ckey).map_err(|e| ApiError::Internal {
        message: format!("wrap: {e}"),
    })?;

    let expires_at = crate::sync_remote::pair_create(
        &cfg.endpoint,
        &token,
        &rh_vault::code_hash(&code),
        &wrapped,
    )
    .await
    .map_err(|e| ApiError::Internal { message: e })?;

    let _ = state;
    Ok(PairCodeResponse {
        code: grouped(&code),
        expires_at,
    })
}

/// Redeem a code on the second device: fetch the wrapped notes key, unwrap it
/// with the code, and keep the notes-scoped token.
#[tauri::command]
#[instrument(level = "debug", skip(state, code))]
pub async fn notes_pair_claim(
    state: State<'_, AppState>,
    code: String,
    label: String,
) -> ApiResult<()> {
    let cfg = crate::sync_remote::SyncConfig::load();
    if cfg.endpoint.is_empty() {
        return Err(ApiError::validation(
            "endpoint",
            "no server configured".to_string(),
        ));
    }
    let normalized = rh_vault::normalize_code(&code);
    if normalized.len() != rh_vault::CODE_LEN {
        return Err(ApiError::validation(
            "code",
            "the code is 8 characters".to_string(),
        ));
    }

    let (wrapped, token) = crate::sync_remote::pair_claim(
        &cfg.endpoint,
        &rh_vault::code_hash(&normalized),
        &label,
    )
    .await
    .map_err(|e| ApiError::validation("code", e))?;

    let ckey = rh_vault::code_key(&normalized).map_err(|e| ApiError::Internal {
        message: format!("code key: {e}"),
    })?;
    // A wrong code fails here rather than at the server: the AEAD tag simply
    // doesn't verify.
    let notes_key = rh_vault::unwrap_notes_key(&wrapped, &ckey)
        .map_err(|_| ApiError::validation("code", "this code did not unlock the notes".to_string()))?;

    crate::notes_sync::store_key(&notes_key)?;
    crate::sync_remote::notes_token_set(&token).map_err(|e| ApiError::Internal {
        message: format!("keychain: {e}"),
    })?;

    // Pull straight away so the screen fills in.
    crate::notes_sync::run_pass_quiet(&state).await;
    state.notes_wake.notify_one();
    Ok(())
}

/// Devices currently paired to this account.
#[tauri::command]
#[instrument(level = "debug", skip(_state))]
pub async fn notes_pair_devices(_state: State<'_, AppState>) -> ApiResult<Vec<PairedDevice>> {
    let cfg = crate::sync_remote::SyncConfig::load();
    let Some(token) = crate::sync_remote::token_get() else {
        return Ok(Vec::new());
    };
    let v = crate::sync_remote::pair_devices(&cfg.endpoint, &token)
        .await
        .map_err(|e| ApiError::Internal { message: e })?;
    let list = v.as_array().cloned().unwrap_or_default();
    Ok(list
        .into_iter()
        .filter_map(|d| {
            Some(PairedDevice {
                id: d.get("id")?.as_str()?.to_string(),
                label: d.get("label")?.as_str().unwrap_or_default().to_string(),
                created_at: d.get("created_at")?.as_str().unwrap_or_default().to_string(),
                last_seen_at: d
                    .get("last_seen_at")
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string()),
            })
        })
        .collect())
}

/// Revoke a paired device. Takes effect on that device's next request.
#[tauri::command]
#[instrument(level = "debug", skip(_state))]
pub async fn notes_pair_revoke(_state: State<'_, AppState>, id: String) -> ApiResult<()> {
    let cfg = crate::sync_remote::SyncConfig::load();
    let token = crate::sync_remote::token_get().ok_or_else(|| {
        ApiError::validation("account", "sign in first".to_string())
    })?;
    crate::sync_remote::pair_revoke(&cfg.endpoint, &token, &id)
        .await
        .map_err(|e| ApiError::Internal { message: e })
}

/// Whether this device is in notes-only mode, and whether notes can sync.
#[tauri::command]
#[instrument(level = "debug", skip(_state))]
pub async fn notes_mode(_state: State<'_, AppState>) -> ApiResult<NotesModeResponse> {
    let account = crate::sync_remote::token_get().is_some();
    let paired = crate::sync_remote::notes_token_get().is_some();
    Ok(NotesModeResponse {
        notes_only: paired && !account,
        connected: account || paired,
    })
}

/// Leave notes-only mode on this device (forget the code-granted access).
#[tauri::command]
#[instrument(level = "debug", skip(_state))]
pub async fn notes_unpair(_state: State<'_, AppState>) -> ApiResult<()> {
    crate::sync_remote::notes_token_clear();
    crate::sync_remote::notes_key_clear();
    Ok(())
}
