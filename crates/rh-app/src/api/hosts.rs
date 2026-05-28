//! `host_*` Tauri commands.
//!
//! Each handler:
//! 1. Validates input fields not enforced by serde (length limits etc.).
//! 2. Delegates to `state.hosts` (a `HostStore` impl).
//! 3. Emits `hosts:changed` event so other UI components can invalidate
//!    their caches.
//!
//! Handlers do NOT log requests at INFO — that's tracing-instrument's
//! job at DEBUG, and per-request INFO would be too noisy.

use std::collections::HashSet;

use tauri::{AppHandle, State};
use tracing::instrument;

use rh_core::{Host, HostFilter};

use crate::api::dto::{
    HostCreateRequest, HostCreateResponse, HostDto, HostFullDto, HostIdRequest, HostListRequest,
    HostListResponse, HostUpdateRequest,
};
use crate::api::error::{ApiError, ApiResult};
use crate::api::events;
use crate::state::AppState;

const MAX_NAME_LEN: usize = 256;
const MAX_HOSTNAME_LEN: usize = 253;       // DNS RFC 1035 max
const MAX_NOTES_LEN: usize = 10_000;
const MAX_TAGS: usize = 32;
const MAX_TAG_LEN: usize = 64;

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn host_list(
    state: State<'_, AppState>,
    req: HostListRequest,
) -> ApiResult<HostListResponse> {
    let filter = HostFilter {
        group_id: req.group_id,
        protocol: req.protocol,
        search: req.search,
        limit: req.limit,
    };
    let hosts = state.hosts.list(filter).await?;
    let total = u32::try_from(hosts.len()).unwrap_or(u32::MAX);
    let hosts: Vec<HostDto> = hosts.into_iter().map(Into::into).collect();
    Ok(HostListResponse { hosts, total })
}

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn host_get(
    state: State<'_, AppState>,
    req: HostIdRequest,
) -> ApiResult<HostFullDto> {
    let host = state
        .hosts
        .get(&req.id)
        .await
        .map_err(|_| ApiError::not_found("host"))?;
    Ok(host.into())
}

#[tauri::command]
#[instrument(level = "debug", skip(state, app))]
pub async fn host_create(
    state: State<'_, AppState>,
    app: AppHandle,
    req: HostCreateRequest,
) -> ApiResult<HostCreateResponse> {
    validate_name(&req.name)?;
    validate_hostname(&req.hostname)?;
    if let Some(ref n) = req.notes {
        validate_notes(n)?;
    }
    if let Some(ref tags) = req.tags {
        validate_tags(tags)?;
    }
    if let Some(p) = req.port {
        validate_port(p)?;
    }

    let mut host = Host::new(req.name, req.hostname, req.protocol, req.port);
    host.group_id = req.group_id;
    host.tags = req.tags.unwrap_or_default();
    host.color = req.color;
    host.notes = req.notes;
    host.default_credential_id = req.default_credential_id;

    let id = host.id.clone();
    state.hosts.create(&host).await?;

    events::emit_hosts_changed(&app, events::Change::Created, &id);
    Ok(HostCreateResponse { id })
}

#[tauri::command]
#[instrument(level = "debug", skip(state, app))]
pub async fn host_update(
    state: State<'_, AppState>,
    app: AppHandle,
    req: HostUpdateRequest,
) -> ApiResult<()> {
    // Load existing.
    let mut host = state
        .hosts
        .get(&req.id)
        .await
        .map_err(|_| ApiError::not_found("host"))?;

    // Apply patches.
    if let Some(name) = req.name {
        validate_name(&name)?;
        host.name = name;
    }
    if let Some(group_id_opt) = req.group_id {
        host.group_id = group_id_opt;
    }
    if let Some(protocol) = req.protocol {
        host.protocol = protocol;
    }
    if let Some(hostname) = req.hostname {
        validate_hostname(&hostname)?;
        host.hostname = hostname;
    }
    if let Some(port) = req.port {
        validate_port(port)?;
        host.port = port;
    }
    if let Some(tags) = req.tags {
        validate_tags(&tags)?;
        host.tags = tags;
    }
    if let Some(color_opt) = req.color {
        host.color = color_opt;
    }
    if let Some(notes_opt) = req.notes {
        if let Some(ref n) = notes_opt {
            validate_notes(n)?;
        }
        host.notes = notes_opt;
    }
    if let Some(cred_opt) = req.default_credential_id {
        host.default_credential_id = cred_opt;
    }
    host.touch();

    state.hosts.update(&host).await?;
    events::emit_hosts_changed(&app, events::Change::Updated, &host.id);
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug", skip(state, app))]
pub async fn host_delete(
    state: State<'_, AppState>,
    app: AppHandle,
    req: HostIdRequest,
) -> ApiResult<()> {
    state
        .hosts
        .delete(&req.id)
        .await
        .map_err(|_| ApiError::not_found("host"))?;
    events::emit_hosts_changed(&app, events::Change::Deleted, &req.id);
    Ok(())
}

// =====================================================================
// Validators
// =====================================================================

fn validate_name(name: &str) -> ApiResult<()> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(ApiError::validation("name", "must not be empty"));
    }
    if name.len() > MAX_NAME_LEN {
        return Err(ApiError::validation(
            "name",
            format!("must be at most {MAX_NAME_LEN} characters"),
        ));
    }
    if name.contains('\0') {
        return Err(ApiError::validation("name", "must not contain NUL bytes"));
    }
    Ok(())
}

fn validate_hostname(hostname: &str) -> ApiResult<()> {
    let trimmed = hostname.trim();
    if trimmed.is_empty() {
        return Err(ApiError::validation("hostname", "must not be empty"));
    }
    if hostname.len() > MAX_HOSTNAME_LEN {
        return Err(ApiError::validation(
            "hostname",
            format!("must be at most {MAX_HOSTNAME_LEN} characters"),
        ));
    }
    if hostname.contains(char::is_whitespace) {
        return Err(ApiError::validation(
            "hostname",
            "must not contain whitespace",
        ));
    }
    if hostname.contains('\0') {
        return Err(ApiError::validation(
            "hostname",
            "must not contain NUL bytes",
        ));
    }
    Ok(())
}

fn validate_port(port: u16) -> ApiResult<()> {
    if port == 0 {
        return Err(ApiError::validation("port", "must be between 1 and 65535"));
    }
    Ok(())
}

fn validate_notes(notes: &str) -> ApiResult<()> {
    if notes.len() > MAX_NOTES_LEN {
        return Err(ApiError::validation(
            "notes",
            format!("must be at most {MAX_NOTES_LEN} characters"),
        ));
    }
    Ok(())
}

fn validate_tags(tags: &[String]) -> ApiResult<()> {
    if tags.len() > MAX_TAGS {
        return Err(ApiError::validation(
            "tags",
            format!("at most {MAX_TAGS} tags allowed"),
        ));
    }
    let mut seen = HashSet::with_capacity(tags.len());
    for (i, t) in tags.iter().enumerate() {
        let trimmed = t.trim();
        if trimmed.is_empty() {
            return Err(ApiError::validation(
                "tags",
                format!("tag #{} is empty", i + 1),
            ));
        }
        if t.len() > MAX_TAG_LEN {
            return Err(ApiError::validation(
                "tags",
                format!("tag #{} too long (max {MAX_TAG_LEN})", i + 1),
            ));
        }
        if !seen.insert(t.as_str()) {
            return Err(ApiError::validation(
                "tags",
                format!("tag {t:?} is duplicated"),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_name_rejected() {
        assert!(validate_name("").is_err());
        assert!(validate_name("   ").is_err());
    }

    #[test]
    fn long_name_rejected() {
        let long = "x".repeat(MAX_NAME_LEN + 1);
        assert!(validate_name(&long).is_err());
    }

    #[test]
    fn name_with_nul_rejected() {
        assert!(validate_name("hello\0world").is_err());
    }

    #[test]
    fn good_name_accepted() {
        assert!(validate_name("prod-db-01").is_ok());
        assert!(validate_name("Sébastien's server").is_ok()); // unicode ok
    }

    #[test]
    fn hostname_with_whitespace_rejected() {
        assert!(validate_hostname("db .example.com").is_err());
        assert!(validate_hostname("\texample").is_err());
    }

    #[test]
    fn port_zero_rejected() {
        assert!(validate_port(0).is_err());
        assert!(validate_port(1).is_ok());
        assert!(validate_port(65_535).is_ok());
    }

    #[test]
    fn too_many_tags_rejected() {
        let many: Vec<String> = (0..MAX_TAGS + 1).map(|i| format!("tag{i}")).collect();
        assert!(validate_tags(&many).is_err());
    }

    #[test]
    fn duplicate_tags_rejected() {
        let dup = vec!["prod".to_string(), "prod".to_string()];
        assert!(validate_tags(&dup).is_err());
    }

    #[test]
    fn empty_tag_rejected() {
        let with_empty = vec!["prod".to_string(), "".to_string()];
        assert!(validate_tags(&with_empty).is_err());
    }
}
