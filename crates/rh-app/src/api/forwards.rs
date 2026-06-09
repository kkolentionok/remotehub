//! `forward_*` Tauri commands — persisted SSH port forwarding (`-L`/`-R`/`-D`).
//!
//! Saved forward *definitions* live in the `ForwardStore`; running
//! instances live in the in-memory `ForwardManager`. A definition is
//! bound to a saved SSH host and reuses that host's credentials + one
//! level of ProxyJump (resolved at start time, mirroring `session_open`).
//! Commands: `forward_save` (persist), `forward_start` / `forward_stop`
//! (run / halt a saved def), `forward_delete`, `forward_set_auto_start`,
//! `forward_list` (defs annotated with live state). `autostart_all` brings
//! up auto-start forwards at launch.

use std::collections::HashMap;
use std::time::Duration;

use chrono::Utc;
use tauri::ipc::Channel;
use tauri::State;
use tokio::sync::mpsc;
use tracing::{instrument, warn};

use rh_core::{ForwardId, ForwardKind, HostFilter, Protocol, SavedForward};
use rh_ssh::{ForwardConnectParams, ForwardEvent, ForwardSpawnParams, ForwardSpec, JumpParams};

use crate::api::dto::{
    ForwardAutoStartRequest, ForwardListResponse, ForwardRefRequest, ForwardSaveRequest,
    ForwardSaveResponse, ForwardSavedDto,
};
use crate::api::error::{ApiError, ApiResult};
use crate::api::sessions::revealed_creds_for;
use crate::state::AppState;

/// Build the live `ForwardSpec` from a saved definition.
fn spec_of(saved: &SavedForward) -> ForwardSpec {
    ForwardSpec {
        kind: saved.kind,
        bind_host: saved.bind_host.clone(),
        bind_port: saved.bind_port,
        target_host: saved.target_host.clone(),
        target_port: saved.target_port,
    }
}

/// Resolve the host (SSH + credentials + one-level jump + keepalive) and
/// assemble the spawn params for a saved forward.
async fn build_spawn(state: &AppState, saved: &SavedForward) -> ApiResult<ForwardSpawnParams> {
    let host = state.hosts.get(&saved.host_id).await?;
    if host.protocol != Protocol::Ssh {
        return Err(ApiError::validation("protocol", "host is not an SSH host"));
    }

    let settings = state.settings.load().await.unwrap_or_default();
    let keepalive_interval = if settings.ssh_keepalive_interval_secs == 0 {
        None
    } else {
        Some(Duration::from_secs(u64::from(
            settings.ssh_keepalive_interval_secs,
        )))
    };

    let credentials = revealed_creds_for(state, &host).await?;

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
        let jcreds = revealed_creds_for(state, &jhost).await?;
        Some(JumpParams {
            hostname: jhost.hostname,
            port: jhost.port,
            host_id: jhost.id,
            credentials: jcreds,
        })
    } else {
        None
    };

    Ok(ForwardSpawnParams {
        connect: ForwardConnectParams {
            hostname: host.hostname,
            port: host.port,
            credentials,
            known_hosts: state.known_hosts.clone(),
            jump,
            keepalive_interval,
        },
        spec: spec_of(saved),
    })
}

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn forward_save(
    state: State<'_, AppState>,
    req: ForwardSaveRequest,
) -> ApiResult<ForwardSaveResponse> {
    let host = state.hosts.get(&req.host_id).await?;
    if host.protocol != Protocol::Ssh {
        return Err(ApiError::validation("protocol", "host is not an SSH host"));
    }
    // Dynamic (-D) picks its target per connection (SOCKS); local/remote
    // need an explicit target.
    if req.kind != ForwardKind::Dynamic && req.target_host.trim().is_empty() {
        return Err(ApiError::validation("target_host", "must not be empty"));
    }
    if req.bind_port == 0 {
        return Err(ApiError::validation("bind_port", "must be 1..=65535"));
    }

    let bind_host = req
        .bind_host
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "127.0.0.1".to_string());

    let saved = SavedForward {
        id: ForwardId::new(),
        host_id: req.host_id,
        kind: req.kind,
        bind_host,
        bind_port: req.bind_port,
        target_host: req.target_host,
        target_port: req.target_port,
        auto_start: req.auto_start,
        created_at: Utc::now(),
    };
    state.forward_defs.create(&saved).await?;

    Ok(ForwardSaveResponse {
        forward_id: saved.id,
    })
}

#[tauri::command]
#[instrument(level = "debug", skip(state, on_event))]
pub async fn forward_start(
    state: State<'_, AppState>,
    req: ForwardRefRequest,
    on_event: Channel<ForwardEvent>,
) -> ApiResult<()> {
    let saved = state.forward_defs.get(&req.forward_id).await?;
    // Already running — nothing to do (avoid a second bind on the port).
    if state.forwards.is_live(saved.id.as_str()).await {
        return Ok(());
    }

    let params = build_spawn(state.inner(), &saved).await?;
    let (tx_events, rx_events) = mpsc::unbounded_channel::<ForwardEvent>();
    let (handle, join) = rh_ssh::spawn_forward(params, tx_events);

    state
        .forwards
        .register(saved.id.to_string(), handle, join, rx_events, Some(on_event))
        .await;
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn forward_stop(state: State<'_, AppState>, req: ForwardRefRequest) -> ApiResult<()> {
    state.forwards.close(req.forward_id.as_str()).await;
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn forward_delete(state: State<'_, AppState>, req: ForwardRefRequest) -> ApiResult<()> {
    state.forwards.close(req.forward_id.as_str()).await;
    state.forward_defs.delete(&req.forward_id).await?;
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn forward_set_auto_start(
    state: State<'_, AppState>,
    req: ForwardAutoStartRequest,
) -> ApiResult<()> {
    state
        .forward_defs
        .set_auto_start(&req.forward_id, req.auto_start)
        .await?;
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn forward_list(state: State<'_, AppState>) -> ApiResult<ForwardListResponse> {
    let saved = state.forward_defs.list().await?;

    // host_id -> label map (one query, then lookups).
    let hosts = state.hosts.list(HostFilter::default()).await?;
    let labels: HashMap<String, String> = hosts
        .into_iter()
        .map(|h| {
            let label = h.display_name.unwrap_or(h.name);
            (h.id.into_inner(), label)
        })
        .collect();

    let mut forwards = Vec::with_capacity(saved.len());
    for s in saved {
        let (state_now, active) = match state.forwards.state_of(s.id.as_str()).await {
            Some((st, n)) => (Some(st), n),
            None => (None, 0),
        };
        let host_label = labels
            .get(s.host_id.as_str())
            .cloned()
            .unwrap_or_else(|| s.host_id.as_str().to_string());
        forwards.push(ForwardSavedDto {
            forward_id: s.id.clone(),
            host_id: s.host_id.clone(),
            host_label,
            spec: spec_of(&s),
            auto_start: s.auto_start,
            state: state_now,
            active,
        });
    }

    Ok(ForwardListResponse { forwards })
}

/// Bring up every saved forward flagged `auto_start` (best-effort; called
/// once at launch). Failures are logged, not surfaced — the UI shows the
/// stopped state and the user can retry from the list.
pub async fn autostart_all(state: AppState) {
    let saved = match state.forward_defs.list().await {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, "forwards: autostart list failed");
            return;
        }
    };
    for s in saved.into_iter().filter(|s| s.auto_start) {
        if state.forwards.is_live(s.id.as_str()).await {
            continue;
        }
        match build_spawn(&state, &s).await {
            Ok(params) => {
                let (tx_events, rx_events) = mpsc::unbounded_channel::<ForwardEvent>();
                let (handle, join) = rh_ssh::spawn_forward(params, tx_events);
                state
                    .forwards
                    .register(s.id.to_string(), handle, join, rx_events, None)
                    .await;
            }
            Err(e) => warn!(forward_id = %s.id, error = ?e, "forwards: autostart spawn failed"),
        }
    }
}
