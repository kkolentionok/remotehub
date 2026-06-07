//! `settings_*` Tauri commands.

use tauri::{AppHandle, State};
use tracing::instrument;

use crate::api::dto::{SettingsGetAllResponse, SettingsUpdateRequest};
use crate::api::error::{ApiError, ApiResult};
use crate::api::events;
use crate::state::AppState;

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn settings_get_all(
    state: State<'_, AppState>,
) -> ApiResult<SettingsGetAllResponse> {
    let settings = state.settings.load().await?;
    Ok(SettingsGetAllResponse { settings })
}

#[tauri::command]
#[instrument(level = "debug", skip(state, app))]
pub async fn settings_update(
    state: State<'_, AppState>,
    app: AppHandle,
    req: SettingsUpdateRequest,
) -> ApiResult<()> {
    // Collect the keys being updated so we can emit a meaningful event.
    let Some(obj) = req.patches.as_object() else {
        return Err(ApiError::validation("patches", "must be a JSON object"));
    };
    let keys: Vec<&str> = obj.keys().map(String::as_str).collect();

    state.settings.save(req.patches.clone()).await?;
    events::emit_settings_changed(&app, &keys);
    Ok(())
}
