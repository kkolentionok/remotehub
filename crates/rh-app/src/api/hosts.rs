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

use rh_core::{EnvVar, Host, HostFilter};

use crate::api::dto::{
    HostCreateRequest, HostCreateResponse, HostDto, HostFullDto, HostIdRequest, HostListRequest,
    HostListResponse, HostUpdateRequest, KnownHostEntryDto, KnownHostForgetRequest,
    KnownHostGetResponse, KnownHostKeyDto, KnownHostsListResponse, RdpCertEntryDto,
    RdpCertsListResponse,
};
use crate::api::error::{ApiError, ApiResult};
use crate::api::events;
use crate::state::AppState;

const MAX_NAME_LEN: usize = 256;
const MAX_HOSTNAME_LEN: usize = 253;       // DNS RFC 1035 max
const MAX_NOTES_LEN: usize = 10_000;
const MAX_TAGS: usize = 32;
const MAX_TAG_LEN: usize = 64;
const MAX_DISPLAY_NAME_LEN: usize = 256;
const MAX_STARTUP_COMMAND_LEN: usize = 4_096;
const MAX_ENV_VARS: usize = 64;
const MAX_ENV_KEY_LEN: usize = 256;
const MAX_ENV_VALUE_LEN: usize = 4_096;

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
    let credential_ids = state
        .credentials
        .credentials_for_host(&req.id)
        .await
        .map(|creds| creds.into_iter().map(|c| c.id).collect())
        .unwrap_or_default();
    let mut dto = HostFullDto::from(host);
    dto.credential_ids = credential_ids;
    Ok(dto)
}

/// The pinned SSH host key for a host (resolved by its hostname+port),
/// for display in the technical-info panel. `None` until first trusted.
#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn known_host_get(
    state: State<'_, AppState>,
    req: HostIdRequest,
) -> ApiResult<KnownHostGetResponse> {
    let host = state
        .hosts
        .get(&req.id)
        .await
        .map_err(|_| ApiError::not_found("host"))?;
    let key = state
        .known_hosts
        .lookup(&host.hostname, host.port)
        .await?
        .map(|k| KnownHostKeyDto {
            key_type: k.key_type,
            fingerprint_sha256: k.fingerprint_sha256,
        });
    Ok(KnownHostGetResponse { key })
}

/// All pinned SSH host keys, for the Known Hosts management list.
#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn known_hosts_list(
    state: State<'_, AppState>,
) -> ApiResult<KnownHostsListResponse> {
    let entries = state
        .known_hosts
        .list()
        .await?
        .into_iter()
        .map(|e| KnownHostEntryDto {
            hostname: e.hostname,
            port: e.port,
            key_type: e.key_type,
            fingerprint_sha256: e.fingerprint_sha256,
            created_at: e.created_at.to_rfc3339(),
        })
        .collect();
    Ok(KnownHostsListResponse { entries })
}

/// Forget (untrust) a pinned host key. The next connect re-prompts (TOFU).
#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn known_host_forget(
    state: State<'_, AppState>,
    req: KnownHostForgetRequest,
) -> ApiResult<()> {
    state.known_hosts.forget(&req.hostname, req.port).await?;
    Ok(())
}

/// All pinned RDP server certificates, for the management list.
#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn rdp_certs_list(state: State<'_, AppState>) -> ApiResult<RdpCertsListResponse> {
    let entries = state
        .rdp_certs
        .list()
        .await?
        .into_iter()
        .map(|e| RdpCertEntryDto {
            hostname: e.hostname,
            port: e.port,
            fingerprint_sha256: e.fingerprint_sha256,
            subject: e.subject,
            trusted_at: e.trusted_at.to_rfc3339(),
        })
        .collect();
    Ok(RdpCertsListResponse { entries })
}

/// Forget (untrust) a pinned RDP cert. The next connect re-prompts (TOFU).
#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn rdp_cert_forget(
    state: State<'_, AppState>,
    req: KnownHostForgetRequest,
) -> ApiResult<()> {
    state.rdp_certs.forget(&req.hostname, req.port).await?;
    Ok(())
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
    if let Some(ref dn) = req.display_name {
        validate_display_name(dn)?;
    }
    if let Some(ref cmd) = req.startup_command {
        validate_startup_command(cmd)?;
    }
    if let Some(ref env) = req.env_vars {
        validate_env_vars(env)?;
    }

    let mut host = Host::new(req.name, req.hostname, req.protocol, req.port);
    host.display_name = normalize_optional(req.display_name);
    host.group_id = req.group_id;
    host.username = req.username.unwrap_or_default();
    host.tags = req.tags.unwrap_or_default();
    host.color = req.color;
    host.notes = req.notes;
    host.startup_command = normalize_optional(req.startup_command);
    host.env_vars = req.env_vars.unwrap_or_default();
    host.default_credential_id = req.default_credential_id;
    host.jump_host_id = req.jump_host_id;
    host.agent_forwarding = req.agent_forwarding.unwrap_or(false);
    host.favorite = req.favorite.unwrap_or(false);

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
    if let Some(display_name_opt) = req.display_name {
        if let Some(ref dn) = display_name_opt {
            validate_display_name(dn)?;
        }
        host.display_name = display_name_opt.and_then(normalize_str);
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
    if let Some(username) = req.username {
        host.username = username;
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
    if let Some(startup_opt) = req.startup_command {
        if let Some(ref cmd) = startup_opt {
            validate_startup_command(cmd)?;
        }
        host.startup_command = startup_opt.and_then(normalize_str);
    }
    if let Some(env) = req.env_vars {
        validate_env_vars(&env)?;
        host.env_vars = env;
    }
    if let Some(cred_opt) = req.default_credential_id {
        host.default_credential_id = cred_opt;
    }
    if let Some(jump_opt) = req.jump_host_id {
        // Guard against self-reference; the UI also excludes it.
        if jump_opt.as_ref() == Some(&host.id) {
            return Err(ApiError::validation("jump_host", "a host cannot jump through itself"));
        }
        host.jump_host_id = jump_opt;
    }
    if let Some(af) = req.agent_forwarding {
        host.agent_forwarding = af;
    }
    if let Some(fav) = req.favorite {
        host.favorite = fav;
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
    state.hosts.delete(&req.id).await?;
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

fn validate_display_name(display_name: &str) -> ApiResult<()> {
    if display_name.len() > MAX_DISPLAY_NAME_LEN {
        return Err(ApiError::validation(
            "display_name",
            format!("must be at most {MAX_DISPLAY_NAME_LEN} characters"),
        ));
    }
    if display_name.contains('\0') {
        return Err(ApiError::validation(
            "display_name",
            "must not contain NUL bytes",
        ));
    }
    Ok(())
}

fn validate_startup_command(cmd: &str) -> ApiResult<()> {
    if cmd.len() > MAX_STARTUP_COMMAND_LEN {
        return Err(ApiError::validation(
            "startup_command",
            format!("must be at most {MAX_STARTUP_COMMAND_LEN} characters"),
        ));
    }
    if cmd.contains('\0') {
        return Err(ApiError::validation(
            "startup_command",
            "must not contain NUL bytes",
        ));
    }
    Ok(())
}

fn validate_env_vars(env: &[EnvVar]) -> ApiResult<()> {
    if env.len() > MAX_ENV_VARS {
        return Err(ApiError::validation(
            "env_vars",
            format!("at most {MAX_ENV_VARS} variables allowed"),
        ));
    }
    let mut seen = HashSet::with_capacity(env.len());
    for (i, ev) in env.iter().enumerate() {
        let key = ev.key.trim();
        if key.is_empty() {
            return Err(ApiError::validation(
                "env_vars",
                format!("variable #{} has an empty key", i + 1),
            ));
        }
        if ev.key.len() > MAX_ENV_KEY_LEN {
            return Err(ApiError::validation(
                "env_vars",
                format!("variable #{} key too long (max {MAX_ENV_KEY_LEN})", i + 1),
            ));
        }
        if ev.value.len() > MAX_ENV_VALUE_LEN {
            return Err(ApiError::validation(
                "env_vars",
                format!(
                    "variable {key:?} value too long (max {MAX_ENV_VALUE_LEN})"
                ),
            ));
        }
        if ev.key.contains('\0') || ev.value.contains('\0') {
            return Err(ApiError::validation(
                "env_vars",
                format!("variable {key:?} must not contain NUL bytes"),
            ));
        }
        if !seen.insert(key) {
            return Err(ApiError::validation(
                "env_vars",
                format!("variable key {key:?} is duplicated"),
            ));
        }
    }
    Ok(())
}

/// Trim a string; map empty → `None`. Used so a blank label/command
/// arriving from the UI is stored as SQL NULL rather than `""`.
fn normalize_str(s: String) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Normalize an optional create-path string (`Option<String>`): treat a
/// present-but-blank value as absent.
fn normalize_optional(s: Option<String>) -> Option<String> {
    s.and_then(normalize_str)
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

    #[test]
    fn display_name_length_enforced() {
        assert!(validate_display_name("Prod DB").is_ok());
        assert!(validate_display_name(&"x".repeat(MAX_DISPLAY_NAME_LEN)).is_ok());
        assert!(validate_display_name(&"x".repeat(MAX_DISPLAY_NAME_LEN + 1)).is_err());
    }

    #[test]
    fn startup_command_length_enforced() {
        assert!(validate_startup_command("tmux attach").is_ok());
        assert!(validate_startup_command(&"x".repeat(MAX_STARTUP_COMMAND_LEN + 1)).is_err());
    }

    #[test]
    fn env_vars_validation() {
        assert!(validate_env_vars(&[EnvVar::new("LANG", "C")]).is_ok());
        // empty key rejected
        assert!(validate_env_vars(&[EnvVar::new("  ", "x")]).is_err());
        // duplicate key rejected
        assert!(
            validate_env_vars(&[EnvVar::new("A", "1"), EnvVar::new("A", "2")]).is_err()
        );
        // too many
        let many: Vec<EnvVar> = (0..MAX_ENV_VARS + 1)
            .map(|i| EnvVar::new(format!("K{i}"), "v"))
            .collect();
        assert!(validate_env_vars(&many).is_err());
    }

    #[test]
    fn normalize_str_blanks_to_none() {
        assert_eq!(normalize_str("  ".to_string()), None);
        assert_eq!(normalize_str("  hi ".to_string()), Some("hi".to_string()));
    }
}
