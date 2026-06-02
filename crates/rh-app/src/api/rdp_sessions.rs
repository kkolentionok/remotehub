//! `rdp_session_*` Tauri commands (Stage 4).
//!
//! A parallel of `sessions.rs` for RDP. `rdp_session_open` resolves the
//! host + a password credential, spawns an `rh-rdp` actor, and hands its
//! event stream + the UI `Channel<RdpSessionEvent>` to `RdpSessionManager`.

use tauri::ipc::Channel;
use tauri::State;
use tokio::sync::mpsc;
use tracing::{info, instrument};

use rh_core::{CredentialKind, Protocol, RevealError, RevealedSecret, SecretError, SessionId};
use rh_rdp::{
    ColorDepth, RdpOpenOptions, RdpSessionEvent, RdpSpawnParams, RevealedRdpCredential,
};

use crate::api::dto::{
    RdpClipboardImageRequest, RdpClipboardRequest, RdpInputRequest, RdpKbdCaptureRequest,
    RdpResizeRequest, SessionIdRequest, SessionOpenOptions, SessionOpenRequest, SessionOpenResponse,
};
use crate::api::error::{ApiError, ApiResult};
use crate::state::AppState;

#[tauri::command]
#[instrument(level = "debug", skip(state, on_event))]
pub async fn rdp_session_open(
    state: State<'_, AppState>,
    req: SessionOpenRequest,
    on_event: Channel<RdpSessionEvent>,
) -> ApiResult<SessionOpenResponse> {
    let (width, height, color_depth, keyboard_layout) = match req.options {
        SessionOpenOptions::Rdp {
            width,
            height,
            color_depth,
            keyboard_layout,
        } => (width, height, color_depth, keyboard_layout),
        SessionOpenOptions::Ssh { .. } => {
            return Err(ApiError::validation("protocol", "use session_open for SSH"));
        }
    };

    let host = state.hosts.get(&req.host_id).await?;
    if host.protocol != Protocol::Rdp {
        return Err(ApiError::validation("protocol", "host is not an RDP host"));
    }

    // Resolve a password credential. RDP MVP is password-only; an explicit
    // override wins, else the host's linked creds, else its default.
    let creds = match &req.credential_id {
        Some(id) => vec![state.credentials.get(id).await?],
        None => {
            let mut all = state.credentials.credentials_for_host(&req.host_id).await?;
            if all.is_empty() {
                if let Some(def) = host.default_credential_id.clone() {
                    all.push(state.credentials.get(&def).await?);
                }
            }
            all
        }
    };
    let cred = creds
        .into_iter()
        .find(|c| matches!(c.kind, CredentialKind::Password))
        .ok_or_else(|| ApiError::validation("credential", "host has no password credential"))?;

    let username = if host.username.is_empty() {
        cred.username.clone()
    } else {
        host.username.clone()
    };
    // A missing keychain secret is treated as an empty password (covers
    // blank-password accounts).
    let password = match state.credentials.reveal(&cred.id).await {
        Ok(s) => s,
        Err(RevealError::Secret(SecretError::NotFound)) => RevealedSecret::new(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    // Domain: MVP keeps it None and passes the username verbatim (works for
    // local accounts like "Administrator"; "DOMAIN\\user" still negotiates).
    let credential = RevealedRdpCredential::Password {
        username,
        domain: None,
        password,
    };

    let color_depth = match color_depth {
        16 => ColorDepth::Depth16,
        24 => ColorDepth::Depth24,
        _ => ColorDepth::Depth32,
    };
    let options = RdpOpenOptions {
        width,
        height,
        color_depth,
        keyboard_layout: keyboard_layout.parse::<u32>().unwrap_or(0),
        enable_clipboard: false,
    };

    let (tx_events, rx_events) = mpsc::unbounded_channel::<RdpSessionEvent>();
    let id = SessionId::new();
    let params = RdpSpawnParams {
        id: id.clone(),
        host,
        credential,
        options,
    };

    let (tx_cmd, join) = rh_rdp::spawn_session(params, tx_events);
    state
        .rdp_sessions
        .register(id.clone(), tx_cmd, join, rx_events, on_event)
        .await;

    info!(session_id = %id, "rdp session spawned");
    Ok(SessionOpenResponse { session_id: id })
}

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn rdp_session_close(
    state: State<'_, AppState>,
    req: SessionIdRequest,
) -> ApiResult<()> {
    state.rdp_sessions.close(&req.session_id).await;
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug", skip(state, req))]
pub async fn rdp_session_input(
    state: State<'_, AppState>,
    req: RdpInputRequest,
) -> ApiResult<()> {
    state.rdp_sessions.send_input(&req.session_id, req.event).await;
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug", skip(state, req))]
pub async fn rdp_session_set_clipboard(
    state: State<'_, AppState>,
    req: RdpClipboardRequest,
) -> ApiResult<()> {
    state
        .rdp_sessions
        .set_clipboard(&req.session_id, req.text)
        .await;
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug", skip(state, req))]
pub async fn rdp_session_set_clipboard_image(
    state: State<'_, AppState>,
    req: RdpClipboardImageRequest,
) -> ApiResult<()> {
    use base64::Engine as _;
    let rgba = base64::engine::general_purpose::STANDARD
        .decode(&req.rgba_base64)
        .map_err(|e| ApiError::Internal {
            message: format!("clipboard image decode: {e}"),
        })?;
    state
        .rdp_sessions
        .set_clipboard_image(&req.session_id, req.width, req.height, rgba)
        .await;
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn rdp_session_resize(
    state: State<'_, AppState>,
    req: RdpResizeRequest,
) -> ApiResult<()> {
    state
        .rdp_sessions
        .resize(&req.session_id, req.width, req.height)
        .await;
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug")]
pub async fn rdp_session_kbd_capture(req: RdpKbdCaptureRequest) -> ApiResult<()> {
    let session = if req.on { Some(req.session_id) } else { None };
    crate::kbd_hook::set_capture(req.on, session);
    Ok(())
}
