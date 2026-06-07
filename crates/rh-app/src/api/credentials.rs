//! `credential_*` Tauri commands.
//!
//! Secrets arrive base64-encoded inside DTOs and are immediately
//! decoded into `SecretValue` (zeroize-on-drop) before being passed
//! to the storage layer. The decoded bytes never appear in logs:
//! `tracing::instrument` skips secret parameters explicitly.

use tauri::{AppHandle, State};
use tracing::instrument;

use rh_core::{Credential, CredentialKind};

use crate::api::dto::{
    CredentialCreateRequest, CredentialCreateResponse, CredentialDto, CredentialIdRequest,
    CredentialLinkRequest, CredentialListResponse, CredentialRevealResponse,
    CredentialRotateSecretRequest, CredentialUnlinkRequest, CredentialUpdateRequest, SecretInput,
};
use crate::api::error::{ApiError, ApiResult};
use crate::api::events;
use crate::state::AppState;
use crate::sync_clock::KIND_CREDENTIAL;

const MAX_CRED_NAME: usize = 256;
const MAX_USERNAME: usize = 256;
const MAX_SECRET_BYTES: usize = 64 * 1024;       // 64 KiB
const MAX_PASSPHRASE_BYTES: usize = 4 * 1024;    // 4 KiB

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn credential_list(
    state: State<'_, AppState>,
) -> ApiResult<CredentialListResponse> {
    let creds = state.credentials.list().await?;
    Ok(CredentialListResponse {
        credentials: creds.into_iter().map(CredentialDto::from).collect(),
    })
}

#[tauri::command]
#[instrument(level = "debug", skip(state, app, req))]
pub async fn credential_create(
    state: State<'_, AppState>,
    app: AppHandle,
    req: CredentialCreateRequest,
) -> ApiResult<CredentialCreateResponse> {
    validate_cred_name(&req.name)?;
    validate_username(&req.username, req.kind)?;

    // Secret presence rules:
    // - Password / SshKey  → secret required.
    // - SshKeyAgent        → secret must NOT be provided.
    match req.kind {
        CredentialKind::Password | CredentialKind::SshKey => {
            if req.secret.is_none() {
                return Err(ApiError::validation(
                    "secret",
                    format!("required for kind={:?}", req.kind),
                ));
            }
        }
        CredentialKind::SshKeyAgent => {
            if req.secret.is_some() {
                return Err(ApiError::validation(
                    "secret",
                    "must be omitted for ssh_key_agent",
                ));
            }
        }
    }
    // Passphrase only meaningful for ssh_key.
    if req.passphrase.is_some() && req.kind != CredentialKind::SshKey {
        return Err(ApiError::validation(
            "passphrase",
            "only allowed for ssh_key credentials",
        ));
    }

    let secret = decode_optional_secret(req.secret.as_ref(), "secret", MAX_SECRET_BYTES)?;
    let passphrase =
        decode_optional_secret(req.passphrase.as_ref(), "passphrase", MAX_PASSPHRASE_BYTES)?;

    let cred = Credential::new(req.name, req.kind, req.username);
    let id = cred.id.clone();

    // SshKeyAgent path: we still call create() but with a zero-length
    // SecretValue that the store will recognize via `kind.requires_keychain_secret()`
    // and skip writing to keychain.
    let secret_for_store = secret.unwrap_or_else(|| rh_core::SecretValue::new(Vec::new()));
    state
        .credentials
        .create(&cred, secret_for_store, passphrase)
        .await?;
    state.stamp_live(KIND_CREDENTIAL, id.as_str()).await?;

    events::emit_credentials_changed(&app, events::Change::Created, &id);
    Ok(CredentialCreateResponse { id })
}

#[tauri::command]
#[instrument(level = "debug", skip(state, app))]
pub async fn credential_update(
    state: State<'_, AppState>,
    app: AppHandle,
    req: CredentialUpdateRequest,
) -> ApiResult<()> {
    let mut cred = state
        .credentials
        .get(&req.id)
        .await
        .map_err(|_| ApiError::not_found("credential"))?;

    if let Some(name) = req.name {
        validate_cred_name(&name)?;
        cred.name = name;
    }
    if let Some(username) = req.username {
        validate_username(&username, cred.kind)?;
        cred.username = username;
    }
    cred.touch();

    state.credentials.update(&cred).await?;
    state.stamp_live(KIND_CREDENTIAL, cred.id.as_str()).await?;
    events::emit_credentials_changed(&app, events::Change::Updated, &cred.id);
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug", skip(state, app, req))]
pub async fn credential_rotate_secret(
    state: State<'_, AppState>,
    app: AppHandle,
    req: CredentialRotateSecretRequest,
) -> ApiResult<()> {
    // Load to check kind — rotation makes no sense for SshKeyAgent.
    let cred = state
        .credentials
        .get(&req.id)
        .await
        .map_err(|_| ApiError::not_found("credential"))?;

    if cred.kind == CredentialKind::SshKeyAgent {
        return Err(ApiError::validation(
            "kind",
            "ssh_key_agent credentials have no secret to rotate",
        ));
    }

    if req.passphrase.is_some() && cred.kind != CredentialKind::SshKey {
        return Err(ApiError::validation(
            "passphrase",
            "only allowed for ssh_key credentials",
        ));
    }

    let secret = decode_secret(&req.secret, "secret", MAX_SECRET_BYTES)?;
    let passphrase =
        decode_optional_secret(req.passphrase.as_ref(), "passphrase", MAX_PASSPHRASE_BYTES)?;

    state
        .credentials
        .rotate_secret(&req.id, secret, passphrase)
        .await?;
    state.stamp_live(KIND_CREDENTIAL, req.id.as_str()).await?;
    events::emit_credentials_changed(&app, events::Change::Updated, &req.id);
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug", skip(state, app))]
pub async fn credential_delete(
    state: State<'_, AppState>,
    app: AppHandle,
    req: CredentialIdRequest,
) -> ApiResult<()> {
    state
        .credentials
        .delete(&req.id)
        .await
        .map_err(|_| ApiError::not_found("credential"))?;
    events::emit_credentials_changed(&app, events::Change::Deleted, &req.id);
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug", skip(state))]
pub async fn credential_reveal(
    state: State<'_, AppState>,
    req: CredentialIdRequest,
) -> ApiResult<CredentialRevealResponse> {
    // Load metadata for the kind/username (UI uses this to render the right
    // field), then reveal the secret. We never return raw bytes through IPC
    // when the credential is ssh_key_agent — there's nothing to reveal.
    let cred = state
        .credentials
        .get(&req.id)
        .await
        .map_err(|_| ApiError::not_found("credential"))?;

    if cred.kind == CredentialKind::SshKeyAgent {
        return Ok(CredentialRevealResponse {
            kind: cred.kind,
            username: cred.username,
            secret: None,
        });
    }

    let revealed = state.credentials.reveal(&req.id).await?;
    // Convert raw bytes to UTF-8 — passwords are always strings in practice.
    // For ssh_key the caller is expected to render in mono and accept that
    // a binary key would surface as replacement chars (we don't have binary
    // SSH keys in practice; openssh keys are textual PEM).
    let secret = String::from_utf8_lossy(revealed.expose()).into_owned();

    Ok(CredentialRevealResponse {
        kind: cred.kind,
        username: cred.username,
        secret: Some(secret),
    })
}

#[tauri::command]
#[instrument(level = "debug", skip(state, app))]
pub async fn credential_link_host(
    state: State<'_, AppState>,
    app: AppHandle,
    req: CredentialLinkRequest,
) -> ApiResult<()> {
    state
        .credentials
        .link_host(&req.host_id, &req.credential_id, req.set_as_default)
        .await?;
    // Two entities changed: the credential link and (possibly) the host's
    // default. Emit both so UI invalidates both views.
    events::emit_credentials_changed(&app, events::Change::Updated, &req.credential_id);
    events::emit_hosts_changed(&app, events::Change::Updated, &req.host_id);
    Ok(())
}

#[tauri::command]
#[instrument(level = "debug", skip(state, app))]
pub async fn credential_unlink_host(
    state: State<'_, AppState>,
    app: AppHandle,
    req: CredentialUnlinkRequest,
) -> ApiResult<()> {
    state
        .credentials
        .unlink_host(&req.host_id, &req.credential_id)
        .await?;
    events::emit_credentials_changed(&app, events::Change::Updated, &req.credential_id);
    events::emit_hosts_changed(&app, events::Change::Updated, &req.host_id);
    Ok(())
}

// =====================================================================
// Helpers
// =====================================================================

fn validate_cred_name(name: &str) -> ApiResult<()> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(ApiError::validation("name", "must not be empty"));
    }
    if name.len() > MAX_CRED_NAME {
        return Err(ApiError::validation(
            "name",
            format!("must be at most {MAX_CRED_NAME} characters"),
        ));
    }
    if name.contains('\0') {
        return Err(ApiError::validation("name", "must not contain NUL bytes"));
    }
    Ok(())
}

fn validate_username(username: &str, _kind: CredentialKind) -> ApiResult<()> {
    // Empty is allowed: the login lives on the host now, not the
    // credential (one key can serve hosts with different users). The
    // credential username is optional metadata / legacy fallback.
    if username.len() > MAX_USERNAME {
        return Err(ApiError::validation(
            "username",
            format!("must be at most {MAX_USERNAME} characters"),
        ));
    }
    if username.contains('\0') {
        return Err(ApiError::validation("username", "must not contain NUL bytes"));
    }
    Ok(())
}

fn decode_secret(
    input: &SecretInput,
    field_name: &'static str,
    max_bytes: usize,
) -> ApiResult<rh_core::SecretValue> {
    let value = input.decode().map_err(|_| {
        ApiError::validation(field_name, "must be valid base64")
    })?;
    if value.is_empty() {
        return Err(ApiError::validation(field_name, "must not be empty"));
    }
    if value.len() > max_bytes {
        return Err(ApiError::validation(
            field_name,
            format!("exceeds maximum size ({max_bytes} bytes)"),
        ));
    }
    Ok(value)
}

fn decode_optional_secret(
    input: Option<&SecretInput>,
    field_name: &'static str,
    max_bytes: usize,
) -> ApiResult<Option<rh_core::SecretValue>> {
    match input {
        Some(i) => Ok(Some(decode_secret(i, field_name, max_bytes)?)),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_username_allows_empty_any_kind() {
        // Login now lives on the host; an empty credential username is fine.
        assert!(validate_username("", CredentialKind::SshKeyAgent).is_ok());
        assert!(validate_username("", CredentialKind::Password).is_ok());
        assert!(validate_username("", CredentialKind::SshKey).is_ok());
    }

    #[test]
    fn validate_username_rejects_nul() {
        assert!(validate_username("a\0b", CredentialKind::Password).is_err());
    }

    #[test]
    fn decode_secret_rejects_empty_bytes() {
        // Empty base64 ("") decodes to empty bytes — should be rejected.
        let inp = SecretInput("".to_string());
        let err = decode_secret(&inp, "test", 100).unwrap_err();
        assert!(matches!(err, ApiError::Validation { .. }));
    }

    #[test]
    fn decode_secret_rejects_oversized() {
        // 100 bytes of 'a' = 'YWFhYWFh...' — base64 of larger payload.
        let big = "A".repeat(200); // base64 padding may shift exact size
        let inp = SecretInput(big);
        let err = decode_secret(&inp, "test", 50).unwrap_err();
        // Either too-big (Validation) or invalid base64 (Validation) —
        // both acceptable; the field is invalid.
        assert!(matches!(err, ApiError::Validation { .. }));
    }

    #[test]
    fn decode_secret_accepts_valid_base64() {
        // "hunter2" in base64
        let inp = SecretInput("aHVudGVyMg==".to_string());
        let s = decode_secret(&inp, "test", 100).unwrap();
        assert_eq!(s.expose(), b"hunter2");
    }
}
