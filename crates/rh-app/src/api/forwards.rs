//! `forward_*` Tauri commands — SSH **local port forwarding** (`-L`).
//!
//! `forward_open` resolves the host + credentials (reusing
//! `sessions::revealed_creds_for` and the same one-level ProxyJump
//! resolution as `session_open`), spawns an `rh-ssh` forward actor, and
//! hands its event stream + the UI `Channel` to the `ForwardManager`.
//! Forwards are in-memory only in this slice (no DB persistence).

use std::time::Duration;

use tauri::ipc::Channel;
use tauri::State;
use tokio::sync::mpsc;
use tracing::instrument;

use rh_core::{Protocol, SessionId};
use rh_ssh::{ForwardConnectParams, ForwardEvent, ForwardKind, ForwardSpawnParams, ForwardSpec, JumpParams};

use crate::api::dto::{
    ForwardCloseRequest, ForwardListResponse, ForwardOpenRequest, ForwardOpenResponse,
};
use crate::api::error::{ApiError, ApiResult};
use crate::api::sessions::revealed_creds_for;
use crate::state::AppState;

#[tauri::command]
#[instrument(level = "debug", skip(state, on_event))]
pub async fn forward_open(
    state: State<'_, AppState>,
    req: ForwardOpenRequest,
    on_event: Channel<ForwardEvent>,
) -> ApiResult<ForwardOpenResponse> {
    let host = state.hosts.get(&req.host_id).await?;
    if host.protocol != Protocol::Ssh {
        return Err(ApiError::validation("protocol", "host is not an SSH host"));
    }
    // Dynamic (-D) chooses its target per connection (SOCKS); local/remote
    // need an explicit target.
    if req.kind != ForwardKind::Dynamic && req.target_host.trim().is_empty() {
        return Err(ApiError::validation("target_host", "must not be empty"));
    }

    let host_label = host
        .display_name
        .clone()
        .unwrap_or_else(|| host.name.clone());

    // Connection-shaping (keepalive) — same source as session_open.
    let settings = state.settings.load().await.unwrap_or_default();
    let keepalive_interval = if settings.ssh_keepalive_interval_secs == 0 {
        None
    } else {
        Some(Duration::from_secs(u64::from(
            settings.ssh_keepalive_interval_secs,
        )))
    };

    let credentials = revealed_creds_for(state.inner(), &host).await?;

    // Optional ProxyJump (one level), mirrors session_open.
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

    let spec = ForwardSpec {
        kind: req.kind,
        bind_host: req
            .bind_host
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "127.0.0.1".to_string()),
        bind_port: req.bind_port,
        target_host: req.target_host,
        target_port: req.target_port,
    };

    let params = ForwardSpawnParams {
        connect: ForwardConnectParams {
            hostname: host.hostname,
            port: host.port,
            credentials,
            known_hosts: state.known_hosts.clone(),
            jump,
            keepalive_interval,
        },
        spec: spec.clone(),
    };

    let (tx_events, rx_events) = mpsc::unbounded_channel::<ForwardEvent>();
    let (handle, join) = rh_ssh::spawn_forward(params, tx_events);

    let forward_id = SessionId::new().to_string();
    state
        .forwards
        .register(
            forward_id.clone(),
            req.host_id,
            host_label,
            spec,
            handle,
            join,
            rx_events,
            on_event,
        )
        .await;

    Ok(ForwardOpenResponse { forward_id })
}

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn forward_close(
    state: State<'_, AppState>,
    req: ForwardCloseRequest,
) -> ApiResult<()> {
    state.forwards.close(&req.forward_id).await;
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn forward_list(state: State<'_, AppState>) -> ApiResult<ForwardListResponse> {
    Ok(ForwardListResponse {
        forwards: state.forwards.list().await,
    })
}
