//! `snippet_*` Tauri commands (Tools → Snippets).
//!
//! Plain CRUD over `SnippetStore`. Coarse mutations: create returns the new
//! id, update/delete return nothing — the UI refetches the list. Snippets are
//! local-only for now (no sync stamping / vault); sync is a later slice.

use chrono::Utc;
use tauri::State;
use tracing::instrument;

use rh_core::{Snippet, SnippetId};

use crate::api::error::{ApiError, ApiResult};
use crate::state::AppState;

const MAX_NAME: usize = 200;
const MAX_COMMAND: usize = 16_384;

fn validate(name: &str, command: &str) -> ApiResult<()> {
    if name.trim().is_empty() {
        return Err(ApiError::validation("name", "name is required".to_string()));
    }
    if name.len() > MAX_NAME {
        return Err(ApiError::validation("name", "name too long".to_string()));
    }
    if command.len() > MAX_COMMAND {
        return Err(ApiError::validation(
            "command",
            "command too long".to_string(),
        ));
    }
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn snippet_list(state: State<'_, AppState>) -> ApiResult<Vec<Snippet>> {
    Ok(state.snippets.list().await?)
}

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn snippet_create(
    state: State<'_, AppState>,
    name: String,
    command: String,
) -> ApiResult<SnippetId> {
    validate(&name, &command)?;
    let snippet = Snippet::new(name.trim(), command);
    let id = snippet.id.clone();
    state.snippets.create(&snippet).await?;
    Ok(id)
}

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn snippet_update(
    state: State<'_, AppState>,
    id: SnippetId,
    name: String,
    command: String,
) -> ApiResult<()> {
    validate(&name, &command)?;
    // `created_at` is ignored by the UPDATE (targets by id); `updated_at` bumps.
    let now = Utc::now();
    let snippet = Snippet {
        id,
        name: name.trim().to_string(),
        command,
        created_at: now,
        updated_at: now,
    };
    state.snippets.update(&snippet).await?;
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn snippet_delete(state: State<'_, AppState>, id: SnippetId) -> ApiResult<()> {
    state.snippets.delete(&id).await?;
    Ok(())
}
