//! Client side of sync (slice 3b): the HTTP transport to `rh-sync-server`,
//! the persisted endpoint/account config, and the bearer token kept in the OS
//! keychain.
//!
//! [`ServerRemote`] implements `rh_vault::SyncRemote`, so the pure engine
//! (`rh_vault::sync_once`) drives it unchanged: `pull` → `GET /v1/vault`,
//! `push` → `PUT /v1/vault` with `If-Match`. A `409`/`412` from the server
//! becomes [`VaultError::RemoteConflict`] so the engine re-pulls and re-merges.
//! The blob is the client's sealed export string, sent verbatim — the server
//! never sees plaintext.

use std::path::PathBuf;

use rh_vault::{RemoteBlob, SyncRemote, VaultError};
use serde::{Deserialize, Serialize};

use crate::paths;

const KEYCHAIN_SERVICE: &str = "RemoteHub";
const KEYCHAIN_TOKEN_KEY: &str = "sync-token";
const CONFIG_FILE: &str = "sync-config.json";

/// The managed sync service, used by default on a fresh install. Self-hosters
/// override the endpoint in Settings → Account & Sync (or via this file).
pub const DEFAULT_ENDPOINT: &str = "https://pingie.ru";

fn default_endpoint() -> String {
    DEFAULT_ENDPOINT.to_string()
}

/// Persisted, non-secret sync config (the bearer token lives in the keychain,
/// not here). Endpoint is the server base URL, e.g. `https://pingie.ru`
/// (default) or `http://localhost:8080` in dev.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncConfig {
    #[serde(default = "default_endpoint")]
    pub endpoint: String,
    #[serde(default)]
    pub email: Option<String>,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            endpoint: DEFAULT_ENDPOINT.to_string(),
            email: None,
        }
    }
}

impl SyncConfig {
    fn file() -> PathBuf {
        paths::app_data_dir().join(CONFIG_FILE)
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::file())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let path = Self::file();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, text);
        }
    }
}

/// Read the stored bearer token from the OS keychain, if logged in.
pub fn token_get() -> Option<String> {
    keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_TOKEN_KEY)
        .ok()?
        .get_password()
        .ok()
}

/// Store the bearer token in the OS keychain (overwrites).
pub fn token_set(token: &str) -> Result<(), keyring::Error> {
    keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_TOKEN_KEY)?.set_password(token)
}

/// Forget the stored token (logout). Idempotent.
pub fn token_clear() {
    if let Ok(entry) = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_TOKEN_KEY) {
        let _ = entry.delete_credential();
    }
}

/// `POST /v1/register`. Returns a human-readable error string on failure.
pub async fn server_register(endpoint: &str, email: &str, password: &str) -> Result<(), String> {
    let resp = reqwest::Client::new()
        .post(format!("{endpoint}/v1/register"))
        .json(&serde_json::json!({ "email": email, "password": password }))
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    if resp.status().is_success() {
        Ok(())
    } else if resp.status() == reqwest::StatusCode::CONFLICT {
        Err("email already registered".to_string())
    } else {
        Err(format!("register failed: HTTP {}", resp.status().as_u16()))
    }
}

/// `POST /v1/login` → bearer token.
pub async fn server_login(endpoint: &str, email: &str, password: &str) -> Result<String, String> {
    let resp = reqwest::Client::new()
        .post(format!("{endpoint}/v1/login"))
        .json(&serde_json::json!({ "email": email, "password": password }))
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err("invalid email or password".to_string());
    }
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("bad login response: {e}"))?;
    body.get("token")
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "login response had no token".to_string())
}

/// The `SyncRemote` backed by `rh-sync-server` over HTTP.
#[derive(Debug)]
pub struct ServerRemote {
    client: reqwest::Client,
    base_url: String,
    token: String,
}

impl ServerRemote {
    pub fn new(base_url: String, token: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url,
            token,
        }
    }
}

#[derive(Deserialize)]
struct VaultBody {
    blob_b64: String,
    rev: String,
}

#[derive(Serialize)]
struct PutBody {
    blob_b64: String,
}

#[derive(Deserialize)]
struct PutResult {
    rev: String,
}

#[async_trait::async_trait]
impl SyncRemote for ServerRemote {
    async fn pull(&self) -> Result<Option<RemoteBlob>, VaultError> {
        let resp = self
            .client
            .get(format!("{}/v1/vault", self.base_url))
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|e| VaultError::Transport(format!("pull: {e}")))?;

        match resp.status() {
            reqwest::StatusCode::OK => {
                let body: VaultBody = resp
                    .json()
                    .await
                    .map_err(|e| VaultError::Transport(format!("pull decode: {e}")))?;
                Ok(Some(RemoteBlob {
                    bytes: body.blob_b64.into_bytes(),
                    version: body.rev,
                }))
            }
            reqwest::StatusCode::NO_CONTENT => Ok(None),
            reqwest::StatusCode::UNAUTHORIZED => {
                Err(VaultError::Transport("unauthorized — log in again".to_string()))
            }
            s => Err(VaultError::Transport(format!("pull: server returned {s}"))),
        }
    }

    async fn push(&self, bytes: &[u8], expected: Option<&str>) -> Result<String, VaultError> {
        let blob_b64 = std::str::from_utf8(bytes)
            .map_err(|_| VaultError::Transport("blob is not UTF-8".to_string()))?
            .to_string();

        let mut req = self
            .client
            .put(format!("{}/v1/vault", self.base_url))
            .bearer_auth(&self.token)
            .json(&PutBody { blob_b64 });
        if let Some(rev) = expected {
            req = req.header("If-Match", rev);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| VaultError::Transport(format!("push: {e}")))?;

        match resp.status() {
            reqwest::StatusCode::OK => {
                let body: PutResult = resp
                    .json()
                    .await
                    .map_err(|e| VaultError::Transport(format!("push decode: {e}")))?;
                Ok(body.rev)
            }
            // Stale rev or create-collision → engine re-pulls + re-merges.
            reqwest::StatusCode::CONFLICT | reqwest::StatusCode::PRECONDITION_FAILED => {
                Err(VaultError::RemoteConflict)
            }
            reqwest::StatusCode::UNAUTHORIZED => {
                Err(VaultError::Transport("unauthorized — log in again".to_string()))
            }
            s => Err(VaultError::Transport(format!("push: server returned {s}"))),
        }
    }
}
