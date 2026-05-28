//! `session_*` Tauri commands.
//!
//! Stage 1.4: all stubs. The actual SSH/RDP actors arrive in Stage 2
//! and Stage 4 respectively. Stubs return `NotImplemented` with a
//! descriptive `feature` field so the UI can show a friendly message.

use tauri::State;
use tracing::instrument;

use crate::api::dto::{SessionIdRequest, SessionOpenOptions, SessionOpenRequest};
use crate::api::error::{ApiError, ApiResult};
use crate::state::AppState;

#[tauri::command]
#[instrument(level = "debug", skip(_state))]
pub async fn session_open(
    _state: State<'_, AppState>,
    req: SessionOpenRequest,
) -> ApiResult<serde_json::Value> {
    let feature = match req.options {
        SessionOpenOptions::Ssh { .. } => "SSH sessions (Stage 2)",
        SessionOpenOptions::Rdp { .. } => "RDP sessions (Stage 4)",
    };
    Err(ApiError::not_implemented(feature))
}

#[tauri::command]
#[instrument(level = "debug", skip(_state))]
pub async fn session_close(
    _state: State<'_, AppState>,
    _req: SessionIdRequest,
) -> ApiResult<()> {
    Err(ApiError::not_implemented("sessions (Stage 2+)"))
}

#[tauri::command]
#[instrument(level = "debug", skip(_state))]
pub async fn session_list(
    _state: State<'_, AppState>,
) -> ApiResult<serde_json::Value> {
    // For now: return an empty list rather than NotImplemented, so the
    // UI's "active sessions" view shows correctly as empty without
    // surfacing an error toast.
    Ok(serde_json::json!({ "sessions": [] }))
}
