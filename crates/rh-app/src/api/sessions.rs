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

use rh_core::{CredentialKind, Protocol, RevealError, RevealedSecret, SecretError, SessionId};
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

    // Username lives on the host so one key can serve hosts with different
    // logins. Fall back to a credential's own username for hosts saved
    // before the per-host username migration (host.username == "").
    let host_username = host.username.clone();
    let resolve_username = |cred_username: &str| -> String {
        if host_username.is_empty() {
            cred_username.to_owned()
        } else {
            host_username.clone()
        }
    };

    // Collect every auth method linked to the host. An explicit override
    // (req.credential_id) wins and restricts to that single credential;
    // otherwise we offer all linked methods and let the actor try each.
    let creds: Vec<_> = match &req.credential_id {
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
    let mut credentials: Vec<RevealedCredential> = Vec::new();

    if creds.is_empty() {
        // No stored credential. If the host has a username, try a
        // passwordless login (empty password) — covers "username only,
        // blank password" hosts. Without a username we genuinely can't.
        if host.username.is_empty() {
            return Err(ApiError::validation(
                "credential",
                "host has no credential",
            ));
        }
        credentials.push(RevealedCredential::Password {
            username: host.username.clone(),
            password: RevealedSecret::new(Vec::new()),
        });
    }

    for cred in &creds {
        let username = resolve_username(&cred.username);
        match cred.kind {
            CredentialKind::Password => {
                // Allow passwordless / empty-password hosts: a missing
                // keychain secret is treated as an empty password.
                let password = match state.credentials.reveal(&cred.id).await {
                    Ok(s) => s,
                    Err(RevealError::Secret(SecretError::NotFound)) => {
                        RevealedSecret::new(Vec::new())
                    }
                    Err(e) => return Err(e.into()),
                };
                credentials.push(RevealedCredential::Password { username, password });
            }
            CredentialKind::SshKey => {
                let private_key_pem = state.credentials.reveal(&cred.id).await?;
                let passphrase = state.credentials.reveal_passphrase(&cred.id).await?;
                credentials.push(RevealedCredential::Key {
                    username,
                    private_key_pem,
                    passphrase,
                });
            }
            CredentialKind::SshKeyAgent => {
                // Not implemented yet — skip rather than fail the whole
                // connection when other methods are available.
            }
        }
    }
    if credentials.is_empty() {
        return Err(ApiError::not_implemented(
            "no usable auth method (SSH agent not implemented)",
        ));
    }
    // Try keys before passwords (conventional, and avoids burning a
    // password attempt when a key would work).
    credentials.sort_by_key(|c| match c {
        RevealedCredential::Key { .. } => 0,
        RevealedCredential::Password { .. } => 1,
    });

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
        credentials,
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
