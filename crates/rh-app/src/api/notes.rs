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
        created_at: now,
        updated_at: now,
    };
    state.notes.update(&note).await?;
    state.stamp_live(KIND_NOTE, note.id.as_str()).await?;
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn note_delete(state: State<'_, AppState>, id: NoteId) -> ApiResult<()> {
    state.notes.delete(&id).await?;
    state.stamp_deleted(KIND_NOTE, id.as_str()).await?;
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
        state.sync_wake.notify_one();
    }
    Ok(())
}
