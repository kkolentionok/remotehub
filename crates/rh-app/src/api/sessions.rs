//! `session_*` Tauri commands — SSH sessions (Stage 2).
//!
//! `session_open` resolves the host + credentials, reveals secrets,
//! spawns an `rh-ssh` actor, and hands the actor's `mpsc` event stream
//! plus the UI `Channel` to the `SessionManager`, which mirrors events
//! into the channel and buffers output for restore-on-reload.

use std::time::Duration;

use tauri::ipc::Channel;
use tauri::State;
use tokio::sync::mpsc;
use tracing::{info, instrument};

use rh_core::{CredentialKind, Protocol, RevealError, RevealedSecret, SecretError, SessionId};
use rh_ssh::{
    JumpParams, RevealedCredential, SessionCommand, SshOpenOptions, SshSessionEvent, SshSpawnParams,
};
use rh_core::Host;

use crate::api::dto::{
    SessionAcceptHostKeyRequest, SessionIdRequest, SessionInputRequest, SessionListResponse,
    SessionOpenOptions, SessionOpenRequest, SessionOpenResponse, SessionReattachRequest,
    SessionResizeRequest, SessionSummaryDto,
};
use crate::api::error::{ApiError, ApiResult};
use crate::state::AppState;

#[tauri::command]
#[instrument(level = "debug", skip(state, app, on_event))]
pub async fn session_open(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
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

    // Connection-shaping settings (keepalive, strict host-key checking).
    let settings = state.settings.load().await.unwrap_or_default();
    let keepalive_interval = if settings.ssh_keepalive_interval_secs == 0 {
        None
    } else {
        Some(Duration::from_secs(u64::from(settings.ssh_keepalive_interval_secs)))
    };
    let strict_host_key = settings.ssh_known_hosts_strict;

    // Metadata for the session registry (restore-on-reload). Capture
    // before the host's fields are moved into the spawn params.
    let title = host
        .display_name
        .clone()
        .unwrap_or_else(|| host.name.clone());
    let hostname_for_meta = host.hostname.clone();
    let env_vars: Vec<(String, String)> = host
        .env_vars
        .iter()
        .map(|e| (e.key.clone(), e.value.clone()))
        .collect();

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
            return Err(ApiError::validation("credential", "host has no credential"));
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
                // No secret travels through us — the OS agent signs.
                credentials.push(RevealedCredential::Agent { username });
            }
        }
    }
    if credentials.is_empty() {
        return Err(ApiError::validation("credential", "host has no usable auth method"));
    }
    // Try keys, then agent, then password (conventional, and avoids
    // burning a password attempt when a key/agent would work).
    credentials.sort_by_key(|c| match c {
        RevealedCredential::Key { .. } => 0,
        RevealedCredential::Agent { .. } => 1,
        RevealedCredential::Password { .. } => 2,
    });

    // Resolve an optional jump host (ProxyJump). Its own linked
    // credentials are revealed the same way. One level only — a bastion's
    // own `jump_host_id` is ignored.
    let jump = if let Some(jid) = host.jump_host_id.clone() {
        let jhost = state
            .hosts
            .get(&jid)
            .await
            .map_err(|_| ApiError::validation("jump_host", "jump host not found"))?;
        if jhost.protocol != Protocol::Ssh {
            return Err(ApiError::validation(
                "jump_host",
                "jump host is not an SSH host",
            ));
        }
        let jcreds = revealed_creds_for(state.inner(), &jhost).await?;
        Some(JumpParams {
            hostname: jhost.hostname,
            port: jhost.port,
            host_id: jhost.id,
            credentials: jcreds,
        })
    } else {
        None
    };

    let (tx_events, rx_events) = mpsc::unbounded_channel::<SshSessionEvent>();

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
            keepalive_interval,
            strict_host_key,
        },
        startup_command: host.startup_command,
        env_vars,
        known_hosts: state.known_hosts.clone(),
        jump,
        agent_forwarding: host.agent_forwarding,
    };

    let (handle, join) = rh_ssh::spawn_session(params, tx_events);
    state
        .sessions
        .register(handle, join, hostname_for_meta, title, on_event, rx_events, state.hosts.clone(), app)
        .await;

    info!(session_id = %id, "ssh session spawned");
    Ok(SessionOpenResponse { session_id: id })
}

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn session_close(state: State<'_, AppState>, req: SessionIdRequest) -> ApiResult<()> {
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
pub async fn session_resize(state: State<'_, AppState>, req: SessionResizeRequest) -> ApiResult<()> {
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
    // Answer the actor's pending host-key prompt: trust + pin. The actor
    // already holds the fingerprint it computed, so we just signal trust.
    state
        .sessions
        .send(&req.session_id, SessionCommand::HostKeyDecision(true))
        .await;
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn session_reject_host_key(
    state: State<'_, AppState>,
    req: SessionIdRequest,
) -> ApiResult<()> {
    // Reject the pending host-key prompt; the actor aborts the handshake
    // and the session closes with `host_key_rejected`.
    state
        .sessions
        .send(&req.session_id, SessionCommand::HostKeyDecision(false))
        .await;
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn session_list(state: State<'_, AppState>) -> ApiResult<SessionListResponse> {
    let sessions = state
        .sessions
        .list()
        .await
        .into_iter()
        .map(|s| SessionSummaryDto {
            session_id: s.session_id,
            host_id: s.meta.host_id,
            hostname: s.meta.hostname,
            title: s.meta.title,
            protocol: s.meta.protocol,
            state: s.state,
            opened_at: s.meta.opened_at.to_rfc3339(),
        })
        .collect();
    Ok(SessionListResponse { sessions })
}

#[tauri::command]
#[instrument(level = "debug", skip(state, on_event))]
pub async fn session_reattach(
    state: State<'_, AppState>,
    req: SessionReattachRequest,
    on_event: Channel<SshSessionEvent>,
) -> ApiResult<bool> {
    // Swap the live session onto a fresh channel and replay buffered
    // output. Returns false if the session died between list and reattach
    // so the UI can drop the stale tab.
    Ok(state.sessions.reattach(&req.session_id, on_event).await)
}

/// Reveal all auth methods linked to a host (no override), with the same
/// passwordless fallback and key→agent→password ordering as the target
/// path. Used to log in to a jump host (bastion).
pub(crate) async fn revealed_creds_for(
    state: &AppState,
    host: &Host,
) -> ApiResult<Vec<RevealedCredential>> {
    let host_username = host.username.clone();
    let resolve_username = |cred_username: &str| -> String {
        if host_username.is_empty() {
            cred_username.to_owned()
        } else {
            host_username.clone()
        }
    };

    let mut creds = state.credentials.credentials_for_host(&host.id).await?;
    if creds.is_empty() {
        if let Some(def) = host.default_credential_id.clone() {
            creds.push(state.credentials.get(&def).await?);
        }
    }

    let mut out: Vec<RevealedCredential> = Vec::new();
    if creds.is_empty() {
        if host.username.is_empty() {
            return Err(ApiError::validation("credential", "jump host has no credential"));
        }
        out.push(RevealedCredential::Password {
            username: host.username.clone(),
            password: RevealedSecret::new(Vec::new()),
        });
    }

    for cred in &creds {
        let username = resolve_username(&cred.username);
        match cred.kind {
            CredentialKind::Password => {
                let password = match state.credentials.reveal(&cred.id).await {
                    Ok(s) => s,
                    Err(RevealError::Secret(SecretError::NotFound)) => {
                        RevealedSecret::new(Vec::new())
                    }
                    Err(e) => return Err(e.into()),
                };
                out.push(RevealedCredential::Password { username, password });
            }
            CredentialKind::SshKey => {
                let private_key_pem = state.credentials.reveal(&cred.id).await?;
                let passphrase = state.credentials.reveal_passphrase(&cred.id).await?;
                out.push(RevealedCredential::Key {
                    username,
                    private_key_pem,
                    passphrase,
                });
            }
            CredentialKind::SshKeyAgent => {
                out.push(RevealedCredential::Agent { username });
            }
        }
    }
    if out.is_empty() {
        return Err(ApiError::validation("credential", "jump host has no usable auth method"));
    }
    out.sort_by_key(|c| match c {
        RevealedCredential::Key { .. } => 0,
        RevealedCredential::Agent { .. } => 1,
        RevealedCredential::Password { .. } => 2,
    });
    Ok(out)
}
