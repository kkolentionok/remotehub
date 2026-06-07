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

    let raw_user = if host.username.is_empty() {
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
    // Split a down-level `DOMAIN\user` (incl. the synthetic `MicrosoftAccount`
    // domain Microsoft-account logins use) into separate username + domain —
    // CredSSP/sspi authenticates more reliably with them apart than with the
    // whole string crammed into the username. UPNs (`user@domain`) pass through
    // untouched. See `split_user_domain`.
    let (username, domain) = split_user_domain(&raw_user);
    let credential = RevealedRdpCredential::Password {
        username,
        domain,
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
        gfx: state.settings.load().await?.rdp_gfx,
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

/// Re-home a live RDP session's frame stream to the calling webview's channel
/// (pop-out into a separate window, or re-dock back into the tab). Returns
/// `true` if the session was live and got reattached.
#[tauri::command]
#[instrument(level = "debug", skip(state, on_event))]
pub async fn rdp_session_reattach(
    state: State<'_, AppState>,
    req: SessionIdRequest,
    on_event: Channel<RdpSessionEvent>,
) -> ApiResult<bool> {
    Ok(state.rdp_sessions.reattach(&req.session_id, on_event).await)
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

/// Split an RDP username into `(username, domain)`.
///
/// Windows accepts the down-level `DOMAIN\user` form (everything before the
/// first backslash is the domain). CredSSP/sspi authenticates more reliably
/// with the two passed separately than with the whole string as the username
/// and no domain. Crucially this also fixes **Microsoft-account** logins, which
/// use the synthetic domain `MicrosoftAccount`, e.g.
/// `MicrosoftAccount\you@outlook.com`.
///
/// The UPN form (`user@domain`) is left intact as the username with no domain —
/// sspi resolves UPNs itself, and splitting on `@` would break local accounts
/// whose name legitimately contains `@`.
fn split_user_domain(raw: &str) -> (String, Option<String>) {
    let raw = raw.trim();
    if let Some((domain, user)) = raw.split_once('\\') {
        let user = user.trim();
        let domain = domain.trim();
        if !user.is_empty() && !domain.is_empty() {
            return (user.to_string(), Some(domain.to_string()));
        }
    }
    (raw.to_string(), None)
}

#[cfg(test)]
mod tests {
    use super::split_user_domain;

    #[test]
    fn plain_username_has_no_domain() {
        assert_eq!(
            split_user_domain("Administrator"),
            ("Administrator".to_string(), None)
        );
    }

    #[test]
    fn downlevel_domain_is_split() {
        assert_eq!(
            split_user_domain("CORP\\alice"),
            ("alice".to_string(), Some("CORP".to_string()))
        );
    }

    #[test]
    fn microsoft_account_is_split() {
        assert_eq!(
            split_user_domain("MicrosoftAccount\\you@outlook.com"),
            ("you@outlook.com".to_string(), Some("MicrosoftAccount".to_string()))
        );
    }

    #[test]
    fn upn_stays_as_username() {
        assert_eq!(
            split_user_domain("you@corp.local"),
            ("you@corp.local".to_string(), None)
        );
    }

    #[test]
    fn malformed_leading_backslash_falls_back() {
        assert_eq!(split_user_domain("\\user"), ("\\user".to_string(), None));
    }
}
