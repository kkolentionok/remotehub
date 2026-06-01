//! `local_session_*` Tauri commands — local shell PTY sessions.
//!
//! A thin parallel of `sessions.rs` with no host/credential resolution:
//! `local_session_open` spawns a shell PTY and hands its event stream +
//! the UI `Channel<SshSessionEvent>` to `LocalPtyManager`. Input/resize/
//! close reuse the same `SshSessionEvent`/`SessionCommand` contract as SSH,
//! so the frontend renders it with the existing terminal component.

use tauri::ipc::Channel;
use tauri::State;
use tracing::{info, instrument};

use rh_core::SessionId;
use rh_ssh::{SessionCommand, SshSessionEvent};

use crate::api::dto::{
    LocalSessionOpenRequest, SessionIdRequest, SessionInputRequest, SessionOpenResponse,
    SessionResizeRequest,
};
use crate::api::error::ApiError;
use crate::api::error::ApiResult;
use crate::state::AppState;

#[tauri::command]
#[instrument(level = "debug", skip(state, on_event))]
pub async fn local_session_open(
    state: State<'_, AppState>,
    req: LocalSessionOpenRequest,
    on_event: Channel<SshSessionEvent>,
) -> ApiResult<SessionOpenResponse> {
    let id = SessionId::new();
    let shell = state
        .settings
        .load()
        .await
        .ok()
        .map(|s| s.local_shell)
        .filter(|s| !s.trim().is_empty());
    state
        .local_sessions
        .open(id.clone(), req.cols, req.rows, shell, on_event)
        .await
        .map_err(|message| ApiError::Internal { message })?;
    info!(session_id = %id, "local pty session spawned");
    Ok(SessionOpenResponse { session_id: id })
}

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn local_session_close(
    state: State<'_, AppState>,
    req: SessionIdRequest,
) -> ApiResult<()> {
    state.local_sessions.close(&req.session_id).await;
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug", skip(state, req))]
pub async fn local_session_input(
    state: State<'_, AppState>,
    req: SessionInputRequest,
) -> ApiResult<()> {
    state
        .local_sessions
        .send(&req.session_id, SessionCommand::SshInput(req.data))
        .await;
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn local_session_resize(
    state: State<'_, AppState>,
    req: SessionResizeRequest,
) -> ApiResult<()> {
    state
        .local_sessions
        .send(
            &req.session_id,
            SessionCommand::Resize {
                cols: req.width,
                rows: req.height,
            },
        )
        .await;
    Ok(())
}
