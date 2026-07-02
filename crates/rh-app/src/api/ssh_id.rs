//! SSH ID — Tauri commands backing the Tools › SSH ID tab.
//!
//! The public key handle lives on the sync server (see server crate
//! `handles.rs`). The browser can't hold the bearer token, so these commands
//! run in `rh-app`: they call the server's `/v1/handle*` endpoints through
//! `sync_remote::api_request`, which reuses the stored endpoint + access token
//! and silently refreshes it on a 401 (same as the vault sync).

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::api::error::ApiError;
use crate::sync_remote;

type ApiResult<T> = Result<T, ApiError>;

#[derive(Serialize, Deserialize)]
pub struct SshIdKey {
    pub id: String,
    pub key_type: String,
    pub public_key: String,
    pub label: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct SshIdData {
    /// The account's handle, or null if not claimed yet.
    pub handle: Option<String>,
    pub keys: Vec<SshIdKey>,
}

#[derive(Serialize, Deserialize)]
pub struct SshIdCheck {
    pub available: bool,
    pub reason: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct SshIdAddedKey {
    pub id: String,
    pub key_type: String,
}

fn decode<T: for<'de> Deserialize<'de>>(v: serde_json::Value) -> ApiResult<T> {
    serde_json::from_value(v).map_err(|e| ApiError::Internal {
        message: format!("ssh_id decode: {e}"),
    })
}

/// Current handle + published keys for the logged-in account.
#[tauri::command]
pub async fn ssh_id_get() -> ApiResult<SshIdData> {
    let v = sync_remote::api_request(reqwest::Method::GET, "/v1/handle", None)
        .await
        .map_err(|m| ApiError::validation("ssh_id", m))?;
    decode(v)
}

/// Claim or rename the account's handle.
#[tauri::command]
pub async fn ssh_id_set_handle(handle: String) -> ApiResult<String> {
    let v = sync_remote::api_request(
        reqwest::Method::PUT,
        "/v1/handle",
        Some(json!({ "handle": handle })),
    )
    .await
    .map_err(|m| ApiError::validation("handle", m))?;
    Ok(v.get("handle")
        .and_then(|h| h.as_str())
        .unwrap_or("")
        .to_string())
}

/// Inline availability check for the create form.
#[tauri::command]
pub async fn ssh_id_check(handle: String) -> ApiResult<SshIdCheck> {
    let path = format!("/v1/handle/check?handle={}", urlencoding::encode(&handle));
    let v = sync_remote::api_request(reqwest::Method::GET, &path, None)
        .await
        .map_err(|m| ApiError::validation("handle", m))?;
    decode(v)
}

/// Publish a public key under the account's handle.
#[tauri::command]
pub async fn ssh_id_add_key(public_key: String, label: Option<String>) -> ApiResult<SshIdAddedKey> {
    let v = sync_remote::api_request(
        reqwest::Method::POST,
        "/v1/handle/keys",
        Some(json!({ "public_key": public_key, "label": label })),
    )
    .await
    .map_err(|m| ApiError::validation("public_key", m))?;
    decode(v)
}

/// Unpublish a key.
#[tauri::command]
pub async fn ssh_id_delete_key(id: String) -> ApiResult<()> {
    let path = format!("/v1/handle/keys/{id}");
    sync_remote::api_request(reqwest::Method::DELETE, &path, None)
        .await
        .map_err(|m| ApiError::validation("ssh_id", m))?;
    Ok(())
}

/// Rename a key's label.
#[tauri::command]
pub async fn ssh_id_update_label(id: String, label: Option<String>) -> ApiResult<()> {
    let path = format!("/v1/handle/keys/{id}");
    sync_remote::api_request(reqwest::Method::PATCH, &path, Some(json!({ "label": label })))
        .await
        .map_err(|m| ApiError::validation("ssh_id", m))?;
    Ok(())
}
