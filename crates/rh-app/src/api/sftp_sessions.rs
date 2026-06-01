//! `sftp_*` Tauri commands — remote file browsing for the SFTP right pane.
//!
//! `sftp_open` resolves the host's credentials (same reveal path as SSH
//! sessions), opens a dedicated SFTP connection, and registers it.
//! `sftp_list` lists a remote directory; `sftp_close` tears the connection
//! down. Transfers (download/upload) land in a later slice.

use tauri::State;
use tracing::{info, instrument};

use rh_core::{Protocol, SessionId};
use rh_ssh::sftp::{SftpConn, SftpListing};

use crate::api::dto::{SftpListRequest, SftpOpenRequest};
use crate::api::dto::{SftpCopyRequest, SftpDownloadRequest, SftpUploadRequest};
use crate::api::dto::{SftpRemoveRequest, SftpRenameRequest};
use crate::api::dto::{
    SftpTransferCancelRequest, SftpTransferKind, SftpTransferRequest,
};
use crate::api::dto::SftpMkdirRequest;
use crate::api::dto::{SessionIdRequest, SessionOpenResponse};
use tauri::ipc::Channel;
use crate::api::error::{ApiError, ApiResult};
use crate::api::sessions::revealed_creds_for;
use crate::state::AppState;

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn sftp_open(
    state: State<'_, AppState>,
    req: SftpOpenRequest,
) -> ApiResult<SessionOpenResponse> {
    let host = state.hosts.get(&req.host_id).await?;
    if host.protocol != Protocol::Ssh {
        return Err(ApiError::validation("protocol", "host is not an SSH host"));
    }
    let creds = revealed_creds_for(state.inner(), &host).await?;
    let conn = SftpConn::connect(&host.hostname, host.port, creds)
        .await
        .map_err(|e| ApiError::Internal {
            message: e.to_string(),
        })?;

    let id = SessionId::new();
    state.sftp.insert(id.clone(), conn).await;
    info!(session_id = %id, "sftp session opened");
    Ok(SessionOpenResponse { session_id: id })
}

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn sftp_list(
    state: State<'_, AppState>,
    req: SftpListRequest,
) -> ApiResult<SftpListing> {
    let conn = state
        .sftp
        .get(&req.session_id)
        .await
        .ok_or_else(|| ApiError::not_found("sftp session"))?;
    let listing = conn
        .lock()
        .await
        .list(&req.path)
        .await
        .map_err(|e| ApiError::Internal {
            message: e.to_string(),
        })?;
    Ok(listing)
}

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn sftp_close(state: State<'_, AppState>, req: SessionIdRequest) -> ApiResult<()> {
    state.sftp.close(&req.session_id).await;
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn sftp_download(
    state: State<'_, AppState>,
    req: SftpDownloadRequest,
) -> ApiResult<()> {
    let conn = state
        .sftp
        .get(&req.session_id)
        .await
        .ok_or_else(|| ApiError::not_found("sftp session"))?;
    conn.lock()
        .await
        .download(&req.remote_path, &req.local_dir)
        .await
        .map_err(|e| ApiError::Internal {
            message: e.to_string(),
        })?;
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn sftp_upload(state: State<'_, AppState>, req: SftpUploadRequest) -> ApiResult<()> {
    let conn = state
        .sftp
        .get(&req.session_id)
        .await
        .ok_or_else(|| ApiError::not_found("sftp session"))?;
    conn.lock()
        .await
        .upload(&req.local_path, &req.remote_dir)
        .await
        .map_err(|e| ApiError::Internal {
            message: e.to_string(),
        })?;
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn sftp_copy(state: State<'_, AppState>, req: SftpCopyRequest) -> ApiResult<()> {
    let from = state
        .sftp
        .get(&req.from_session)
        .await
        .ok_or_else(|| ApiError::not_found("sftp session"))?;
    let to = state
        .sftp
        .get(&req.to_session)
        .await
        .ok_or_else(|| ApiError::not_found("sftp session"))?;

    // Read from the source (lock released at the end of this statement),
    // then write to the destination — never holding both locks at once.
    let data = from
        .lock()
        .await
        .read_file(&req.remote_path)
        .await
        .map_err(|e| ApiError::Internal {
            message: e.to_string(),
        })?;
    let name = req
        .remote_path
        .rsplit('/')
        .next()
        .unwrap_or(&req.remote_path);
    to.lock()
        .await
        .put_in_dir(&req.remote_dir, name, &data)
        .await
        .map_err(|e| ApiError::Internal {
            message: e.to_string(),
        })?;
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn sftp_rename(state: State<'_, AppState>, req: SftpRenameRequest) -> ApiResult<()> {
    let conn = state
        .sftp
        .get(&req.session_id)
        .await
        .ok_or_else(|| ApiError::not_found("sftp session"))?;
    conn.lock()
        .await
        .rename(&req.path, &req.new_name)
        .await
        .map_err(|e| ApiError::Internal {
            message: e.to_string(),
        })?;
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn sftp_remove(state: State<'_, AppState>, req: SftpRemoveRequest) -> ApiResult<()> {
    let conn = state
        .sftp
        .get(&req.session_id)
        .await
        .ok_or_else(|| ApiError::not_found("sftp session"))?;
    conn.lock()
        .await
        .remove(&req.path, req.is_dir)
        .await
        .map_err(|e| ApiError::Internal {
            message: e.to_string(),
        })?;
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug", skip(state, on_progress))]
pub async fn sftp_transfer(
    state: State<'_, AppState>,
    req: SftpTransferRequest,
    on_progress: Channel<u64>,
) -> ApiResult<()> {
    let cancel = state.sftp.register_transfer(req.transfer_id.clone()).await;
    let res = run_transfer(&state, &req, &cancel, &on_progress).await;
    state.sftp.unregister_transfer(&req.transfer_id).await;
    res
}

async fn run_transfer(
    state: &AppState,
    req: &SftpTransferRequest,
    cancel: &std::sync::atomic::AtomicBool,
    on_progress: &Channel<u64>,
) -> ApiResult<()> {
    let mut prog = |b: u64| {
        let _ = on_progress.send(b);
    };
    let to_internal = |e: rh_ssh::SshError| ApiError::Internal {
        message: e.to_string(),
    };

    match req.kind {
        SftpTransferKind::Download => {
            let conn = state
                .sftp
                .get(&req.session_id)
                .await
                .ok_or_else(|| ApiError::not_found("sftp session"))?;
            conn.lock()
                .await
                .download_stream(&req.src_path, &req.dst_dir, req.dst_name.as_deref(), cancel, &mut prog)
                .await
                .map_err(to_internal)?;
        }
        SftpTransferKind::Upload => {
            let conn = state
                .sftp
                .get(&req.session_id)
                .await
                .ok_or_else(|| ApiError::not_found("sftp session"))?;
            conn.lock()
                .await
                .upload_stream(&req.src_path, &req.dst_dir, req.dst_name.as_deref(), cancel, &mut prog)
                .await
                .map_err(to_internal)?;
        }
        SftpTransferKind::Copy => {
            let to = req
                .to_session
                .as_ref()
                .ok_or_else(|| ApiError::validation("to_session", "required for copy"))?;
            let from = state
                .sftp
                .get(&req.session_id)
                .await
                .ok_or_else(|| ApiError::not_found("sftp session"))?;
            let dst = state
                .sftp
                .get(to)
                .await
                .ok_or_else(|| ApiError::not_found("sftp session"))?;
            let data = from.lock().await.read_file(&req.src_path).await.map_err(to_internal)?;
            prog(0);
            let name = req
                .dst_name
                .as_deref()
                .unwrap_or_else(|| req.src_path.rsplit('/').next().unwrap_or(&req.src_path));
            dst.lock()
                .await
                .put_in_dir(&req.dst_dir, name, &data)
                .await
                .map_err(to_internal)?;
            prog(data.len() as u64);
        }
    }
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn sftp_transfer_cancel(
    state: State<'_, AppState>,
    req: SftpTransferCancelRequest,
) -> ApiResult<()> {
    state.sftp.cancel_transfer(&req.transfer_id).await;
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn sftp_mkdir(state: State<'_, AppState>, req: SftpMkdirRequest) -> ApiResult<()> {
    let conn = state
        .sftp
        .get(&req.session_id)
        .await
        .ok_or_else(|| ApiError::not_found("sftp session"))?;
    conn.lock()
        .await
        .mkdir(&req.parent, &req.name)
        .await
        .map_err(|e| ApiError::Internal {
            message: e.to_string(),
        })?;
    Ok(())
}
