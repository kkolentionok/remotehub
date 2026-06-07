//! Misc / meta commands.

use tracing::instrument;

use crate::api::dto::AppVersionResponse;
use crate::api::error::ApiResult;

#[tauri::command]
#[instrument(level = "debug")]
pub async fn app_version() -> ApiResult<AppVersionResponse> {
    Ok(AppVersionResponse {
        version: env!("CARGO_PKG_VERSION").to_string(),
        target: std::env::consts::OS.to_string(),
    })
}
