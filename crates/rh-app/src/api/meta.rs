//! Misc / meta commands.

use std::sync::atomic::Ordering;

use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;
use tracing::instrument;

use crate::api::dto::AppVersionResponse;
use crate::api::error::{ApiError, ApiResult};
use crate::state::AppState;

#[tauri::command]
#[instrument(level = "debug")]
pub async fn app_version() -> ApiResult<AppVersionResponse> {
    Ok(AppVersionResponse {
        version: env!("CARGO_PKG_VERSION").to_string(),
        target: std::env::consts::OS.to_string(),
    })
}

/// The UI reports how many live session tabs it currently shows. We stash the
/// value (read by the tray Quit handler to decide whether to confirm) and
/// refresh the tray tooltip so it reflects activity even when hidden.
#[tauri::command]
#[instrument(level = "debug", skip(state, app))]
pub async fn ui_sessions_report(
    state: tauri::State<'_, AppState>,
    app: AppHandle,
    count: u32,
) -> ApiResult<()> {
    state.session_count.store(count as usize, Ordering::Relaxed);
    crate::tray::update_tooltip(&app, count as usize);
    Ok(())
}

/// Quit the app for real. Used by the UI after the user confirms quitting with
/// live sessions open (the tray Quit menu bounces through a confirm first).
#[tauri::command]
#[instrument(level = "debug")]
pub async fn app_quit(app: AppHandle) -> ApiResult<()> {
    app.exit(0);
    Ok(())
}

/// Open a URL in the user's default browser. Called from the terminal's
/// Ctrl/Cmd+click link handler. Goes through the opener plugin's *Rust* API
/// (not the JS `plugin:opener|open_url` command) so it isn't gated by the
/// IPC URL scope — any link the user explicitly Ctrl-clicks should open.
#[tauri::command]
#[instrument(level = "debug")]
pub async fn open_external(app: AppHandle, url: String) -> ApiResult<()> {
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| ApiError::Internal { message: format!("open url: {e}") })?;
    Ok(())
}
