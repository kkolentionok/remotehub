//! `group_*` Tauri commands.

use tauri::{AppHandle, State};
use tracing::instrument;

use rh_core::{GroupId, HostFilter, HostGroup, HostId};

use crate::api::dto::{
    GroupCreateRequest, GroupCreateResponse, GroupIdRequest, GroupListResponse, GroupMoveRequest,
    GroupRenameRequest, HostGroupDto,
};
use crate::api::error::{ApiError, ApiResult};
use crate::api::events;
use crate::state::AppState;
use crate::sync_clock::{KIND_GROUP, KIND_HOST};

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
    state.stamp_live(KIND_GROUP, id.as_str()).await?;
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
    state.stamp_live(KIND_GROUP, req.id.as_str()).await?;
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
    state.stamp_live(KIND_GROUP, req.id.as_str()).await?;
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
    // Compute the cascade BEFORE deleting. Child groups cascade-delete (FK
    // `ON DELETE CASCADE`) and hosts in any removed group fall back to root
    // (`ON DELETE SET NULL`). Both must be recorded in `sync_meta` — tombstones
    // for every removed group, a fresh stamp for every re-parented host — or
    // the deletion / re-parenting won't replicate to other devices.
    let all_groups = state.groups.list().await?;
    let victims = descendants_including(&req.id, &all_groups);
    let affected_hosts: Vec<HostId> = state
        .hosts
        .list(HostFilter::default())
        .await?
        .into_iter()
        .filter(|h| h.group_id.as_ref().is_some_and(|g| victims.contains(g)))
        .map(|h| h.id)
        .collect();

    state
        .groups
        .delete(&req.id)
        .await
        .map_err(|_| ApiError::not_found("group"))?;

    for gid in &victims {
        state.stamp_deleted(KIND_GROUP, gid.as_str()).await?;
    }
    for hid in &affected_hosts {
        state.stamp_live(KIND_HOST, hid.as_str()).await?;
    }
    events::emit_groups_changed(&app, events::Change::Deleted, &req.id);
    Ok(())
}

/// `root` plus every transitive descendant group, by walking `parent_id`. Used
/// to enumerate the rows an `ON DELETE CASCADE` will remove so each gets a
/// tombstone.
fn descendants_including(root: &GroupId, all: &[HostGroup]) -> Vec<GroupId> {
    let mut victims: Vec<GroupId> = vec![root.clone()];
    loop {
        let before = victims.len();
        for g in all {
            if let Some(parent) = &g.parent_id {
                let parent_is_victim = victims.iter().any(|v| v == parent);
                let already = victims.iter().any(|v| v == &g.id);
                if parent_is_victim && !already {
                    victims.push(g.id.clone());
                }
            }
        }
        if victims.len() == before {
            break;
        }
    }
    victims
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
