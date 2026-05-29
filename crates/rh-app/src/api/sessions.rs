//! `session_*` Tauri commands — SSH sessions (Stage 2).
//!
//! `session_open` resolves the host + credential, reveals the secret,
//! spawns an `rh-ssh` actor, and bridges its `mpsc` event stream into the
//! Tauri `Channel` the UI passed in. Input/resize/close are routed to the
//! actor by session id via the `SessionManager`.

use tauri::ipc::Channel;
use tauri::State;
use tokio::sync::mpsc;
use tracing::{info, instrument};

use rh_core::{CredentialKind, Protocol, SessionId};
use rh_ssh::{RevealedCredential, SessionCommand, SshOpenOptions, SshSessionEvent, SshSpawnParams};

use crate::api::dto::{
    SessionAcceptHostKeyRequest, SessionIdRequest, SessionInputRequest, SessionOpenOptions,
    SessionOpenRequest, SessionOpenResponse, SessionResizeRequest,
};
use crate::api::error::{ApiError, ApiResult};
use crate::state::AppState;

#[tauri::command]
#[instrument(level = "debug", skip(state, on_event))]
pub async fn session_open(
    state: State<'_, AppState>,
    req: SessionOpenRequest,
    on_event: Channel<SshSessionEvent>,
) -> ApiResult<SessionOpenResponse> {
    // SSH only in Stage 2.
    let (cols, rows, term) = match req.options {
        SessionOpenOptions::Ssh { cols, rows, term } => (cols, rows, term),
        SessionOpenOptions::Rdp { .. } => {
            return Err(ApiError::not_implemented("RDP sessions (Stage 4)"));
        }
    };

    let host = state.hosts.get(&req.host_id).await?;
    if host.protocol != Protocol::Ssh {
        return Err(ApiError::validation("protocol", "host is not an SSH host"));
    }

    // Resolve credential: explicit override, else the host default.
    let cred_id = req
        .credential_id
        .or(host.default_credential_id.clone())
        .ok_or_else(|| ApiError::validation("credential", "host has no credential"))?;

    let cred = state.credentials.get(&cred_id).await?;
    if cred.kind != CredentialKind::Password {
        return Err(ApiError::not_implemented(
            "SSH key / agent authentication (Stage 2 follow-up)",
        ));
    }
    let password = state.credentials.reveal(&cred_id).await?;
    let credential = RevealedCredential::Password {
        username: cred.username,
        password,
    };

    // Bridge the actor's mpsc events into the UI Channel.
    let (tx_events, mut rx_events) = mpsc::unbounded_channel::<SshSessionEvent>();
    tokio::spawn(async move {
        while let Some(ev) = rx_events.recv().await {
            if on_event.send(ev).is_err() {
                break;
            }
        }
    });

    let id = SessionId::new();
    let params = SshSpawnParams {
        id: id.clone(),
        hostname: host.hostname,
        port: host.port,
        host_id: req.host_id,
        credential,
        options: SshOpenOptions {
            cols,
            rows,
            term,
            keepalive_interval: None,
        },
        startup_command: host.startup_command,
    };

    let (handle, join) = rh_ssh::spawn_session(params, tx_events);
    state.sessions.register(handle, join).await;

    info!(session_id = %id, "ssh session spawned");
    Ok(SessionOpenResponse { session_id: id })
}

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn session_close(
    state: State<'_, AppState>,
    req: SessionIdRequest,
) -> ApiResult<()> {
    state.sessions.close(&req.session_id).await;
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn session_send_input(
    state: State<'_, AppState>,
    req: SessionInputRequest,
) -> ApiResult<()> {
    state
        .sessions
        .send(&req.session_id, SessionCommand::SshInput(req.data))
        .await;
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn session_resize(
    state: State<'_, AppState>,
    req: SessionResizeRequest,
) -> ApiResult<()> {
    state
        .sessions
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

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn session_accept_host_key(
    state: State<'_, AppState>,
    req: SessionAcceptHostKeyRequest,
) -> ApiResult<()> {
    // v1 auto-accepts host keys in the actor, so this is a no-op. The
    // command exists for the future interactive TOFU flow.
    let _ = (&state, req.session_id);
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn session_reject_host_key(
    state: State<'_, AppState>,
    req: SessionIdRequest,
) -> ApiResult<()> {
    // Rejecting an unknown key means we won't trust the server — close.
    state.sessions.close(&req.session_id).await;
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug", skip(_state))]
pub async fn session_list(_state: State<'_, AppState>) -> ApiResult<serde_json::Value> {
    // The UI tracks live sessions itself; restore-on-reload is a later
    // refinement. Return empty so no error toast surfaces.
    Ok(serde_json::json!({ "sessions": [] }))
}
