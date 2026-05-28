//! `group_*` Tauri commands.

use tauri::{AppHandle, State};
use tracing::instrument;

use rh_core::HostGroup;

use crate::api::dto::{
    GroupCreateRequest, GroupCreateResponse, GroupIdRequest, GroupListResponse, GroupMoveRequest,
    GroupRenameRequest, HostGroupDto,
};
use crate::api::error::{ApiError, ApiResult};
use crate::api::events;
use crate::state::AppState;

const MAX_GROUP_NAME: usize = 256;

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn group_list(state: State<'_, AppState>) -> ApiResult<GroupListResponse> {
    let groups = state.groups.list().await?;
    Ok(GroupListResponse {
        groups: groups.into_iter().map(HostGroupDto::from).collect(),
    })
}

#[tauri::command]
#[instrument(level = "debug", skip(state, app))]
pub async fn group_create(
    state: State<'_, AppState>,
    app: AppHandle,
    req: GroupCreateRequest,
) -> ApiResult<GroupCreateResponse> {
    validate_group_name(&req.name)?;
    let group = HostGroup::new(req.name, req.parent_id);
    let id = group.id.clone();
    state.groups.create(&group).await?;
    events::emit_groups_changed(&app, events::Change::Created, &id);
    Ok(GroupCreateResponse { id })
}

#[tauri::command]
#[instrument(level = "debug", skip(state, app))]
pub async fn group_rename(
    state: State<'_, AppState>,
    app: AppHandle,
    req: GroupRenameRequest,
) -> ApiResult<()> {
    validate_group_name(&req.name)?;
    state
        .groups
        .rename(&req.id, &req.name)
        .await
        .map_err(|_| ApiError::not_found("group"))?;
    events::emit_groups_changed(&app, events::Change::Updated, &req.id);
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug", skip(state, app))]
pub async fn group_move(
    state: State<'_, AppState>,
    app: AppHandle,
    req: GroupMoveRequest,
) -> ApiResult<()> {
    state
        .groups
        .move_to(&req.id, req.parent_id.as_ref())
        .await?;
    events::emit_groups_changed(&app, events::Change::Updated, &req.id);
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug", skip(state, app))]
pub async fn group_delete(
    state: State<'_, AppState>,
    app: AppHandle,
    req: GroupIdRequest,
) -> ApiResult<()> {
    state
        .groups
        .delete(&req.id)
        .await
        .map_err(|_| ApiError::not_found("group"))?;
    events::emit_groups_changed(&app, events::Change::Deleted, &req.id);
    Ok(())
}

fn validate_group_name(name: &str) -> ApiResult<()> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(ApiError::validation("name", "must not be empty"));
    }
    if name.len() > MAX_GROUP_NAME {
        return Err(ApiError::validation(
            "name",
            format!("must be at most {MAX_GROUP_NAME} characters"),
        ));
    }
    if name.contains('\0') {
        return Err(ApiError::validation("name", "must not contain NUL bytes"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_group_name_rejects_empty() {
        assert!(validate_group_name("").is_err());
        assert!(validate_group_name("   ").is_err());
    }

    #[test]
    fn validate_group_name_accepts_unicode() {
        assert!(validate_group_name("Тест группа").is_ok());
    }
}
