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
/// Long-lived refresh token. Kept beside the access token; lets the client
/// silently renew an expired access token (no "session expired" prompt).
/// Cleared on logout.
const KEYCHAIN_REFRESH_KEY: &str = "sync-refresh";
/// The user's vault (master) password, cached for automatic sync (slice 3d).
/// Stored in the OS keychain alongside the bearer token and all other secrets,
/// so the background actor can seal/open the E2E envelope without prompting.
/// Cleared on logout.
const KEYCHAIN_VAULT_KEY: &str = "sync-vault-key";
const CONFIG_FILE: &str = "sync-config.json";

/// The managed sync service, used by default on a fresh install. Self-hosters
/// override the endpoint in Settings → Account & Sync (or via this file).
///
/// NOTE: this is the *direct* origin subdomain (Cloudflare grey-cloud / DNS-only),
/// NOT the CDN-proxied apex `pingie.ru`. Cloudflare mangles/throttles the larger
/// `GET /v1/vault` response body (intermittent `error decoding response body`) and
/// large downloads, so all sync + update traffic must bypass the proxy.
pub const DEFAULT_ENDPOINT: &str = "https://dl.pingie.ru";

fn default_endpoint() -> String {
    DEFAULT_ENDPOINT.to_string()
}

/// Shared HTTP client with bounded timeouts. Without these, a stalled
/// connection (e.g. a CDN swallowing the response body) leaves sync spinning
/// forever; with them, the request fails and the engine reports a real error
/// the UI can show and retry from.
fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
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

/// Read the stored refresh token, if any.
pub fn refresh_get() -> Option<String> {
    keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_REFRESH_KEY)
        .ok()?
        .get_password()
        .ok()
}

/// Store the refresh token in the OS keychain (overwrites).
pub fn refresh_set(token: &str) -> Result<(), keyring::Error> {
    keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_REFRESH_KEY)?.set_password(token)
}

/// Forget the stored refresh token (logout). Idempotent.
pub fn refresh_clear() {
    if let Ok(entry) = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_REFRESH_KEY) {
        let _ = entry.delete_credential();
    }
}

/// Read the cached vault (master) password used for automatic sync.
pub fn vault_key_get() -> Option<String> {
    keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_VAULT_KEY)
        .ok()?
        .get_password()
        .ok()
}

/// Cache the vault (master) password in the OS keychain (overwrites).
pub fn vault_key_set(password: &str) -> Result<(), keyring::Error> {
    keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_VAULT_KEY)?.set_password(password)
}

/// Forget the cached vault password. Idempotent. Called on logout.
pub fn vault_key_clear() {
    if let Ok(entry) = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_VAULT_KEY) {
        let _ = entry.delete_credential();
    }
}

/// `POST /v1/register`. Returns a human-readable error string on failure.
pub async fn server_register(endpoint: &str, email: &str, password: &str) -> Result<(), String> {
    let resp = http_client()
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

/// `POST /v1/login` → (access token, optional refresh token).
/// Older servers return only `token`; then refresh is `None`.
pub async fn server_login(
    endpoint: &str,
    email: &str,
    password: &str,
) -> Result<(String, Option<String>), String> {
    let resp = http_client()
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
    let token = body
        .get("token")
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "login response had no token".to_string())?;
    let refresh = body
        .get("refresh")
        .and_then(|t| t.as_str())
        .map(|s| s.to_string());
    Ok((token, refresh))
}

/// `POST /v1/refresh` → a fresh access token from a refresh token.
pub async fn server_refresh(endpoint: &str, refresh: &str) -> Result<String, String> {
    let resp = http_client()
        .post(format!("{endpoint}/v1/refresh"))
        .json(&serde_json::json!({ "refresh": refresh }))
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("refresh failed: HTTP {}", resp.status().as_u16()));
    }
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("bad refresh response: {e}"))?;
    body.get("token")
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "refresh response had no token".to_string())
}

/// `POST /v1/oauth/exchange` → (access, optional refresh) from a one-time code.
pub async fn server_exchange(
    endpoint: &str,
    code: &str,
) -> Result<(String, Option<String>), String> {
    let resp = http_client()
        .post(format!("{endpoint}/v1/oauth/exchange"))
        .json(&serde_json::json!({ "code": code }))
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("exchange failed: HTTP {}", resp.status().as_u16()));
    }
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("bad exchange response: {e}"))?;
    let token = body
        .get("token")
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "exchange response had no token".to_string())?;
    let refresh = body
        .get("refresh")
        .and_then(|t| t.as_str())
        .map(|s| s.to_string());
    Ok((token, refresh))
}

/// `GET /v1/me` → the account's email (for "signed in as …" after OAuth).
pub async fn server_me(endpoint: &str, token: &str) -> Result<String, String> {
    let resp = http_client()
        .get(format!("{endpoint}/v1/me"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("me failed: HTTP {}", resp.status().as_u16()));
    }
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("bad me response: {e}"))?;
    body.get("email")
        .and_then(|e| e.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "me response had no email".to_string())
}

/// Run the desktop Yandex sign-in: bind a one-shot loopback listener, open the
/// system browser at the server's `/v1/oauth/yandex/start?cb=…&mode=code`, and
/// wait for the server to redirect a one-time `code` back to our loopback. We
/// exchange that code for the access+refresh pair over HTTPS — the session
/// token never appears in the browser URL. Returns `(access, refresh?)`.
///
/// Back-compat: if an older server redirects `?token=…` instead, we accept the
/// token directly (no refresh).
pub async fn run_yandex_login(endpoint: &str) -> Result<(String, Option<String>), String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("could not open loopback listener: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| e.to_string())?
        .port();
    let cb = format!("http://127.0.0.1:{port}/cb");

    let start_url = format!(
        "{endpoint}/v1/oauth/yandex/start?cb={}&mode=code",
        urlencoding::encode(&cb)
    );
    open::that(&start_url).map_err(|e| format!("could not open browser: {e}"))?;

    // Wait for exactly one callback hit (the user completing consent), bounded.
    let (mut sock, _peer) =
        tokio::time::timeout(std::time::Duration::from_secs(180), listener.accept())
            .await
            .map_err(|_| "sign-in timed out".to_string())?
            .map_err(|e| format!("accept failed: {e}"))?;

    // The request line — `GET /cb?code=…&… HTTP/1.1` — sits at the very start,
    // so a single read is enough for a bare GET on loopback.
    let mut buf = [0u8; 8192];
    let n = sock
        .read(&mut buf)
        .await
        .map_err(|e| format!("read failed: {e}"))?;
    let req = String::from_utf8_lossy(&buf[..n]);
    let line = req.lines().next().unwrap_or("");
    let query = line
        .split_whitespace()
        .nth(1)
        .and_then(|path| path.split('?').nth(1))
        .unwrap_or("");

    let mut code: Option<String> = None;
    let mut token: Option<String> = None;
    let mut err: Option<String> = None;
    for pair in query.split('&') {
        let mut it = pair.splitn(2, '=');
        match (it.next(), it.next()) {
            (Some("code"), Some(v)) => code = Some(urldecode(v)),
            (Some("token"), Some(v)) => token = Some(urldecode(v)),
            (Some("error"), Some(v)) => err = Some(urldecode(v)),
            _ => {}
        }
    }

    // Branded page; the inline script strips the query (code/token) out of the
    // address bar + history so nothing sensitive lingers there.
    let body = "<!doctype html><meta charset=utf-8><title>Pingie</title><body style=\"font-family:system-ui;\
        background:#0a0a0d;color:#e6e6e6;display:flex;align-items:center;justify-content:center;\
        height:100vh;margin:0\"><div style=\"text-align:center\"><h2 style=\"font-weight:600\">Pingie</h2>\
        <p style=\"color:#9aa0a6\">Готово. Можете закрыть это окно и вернуться в приложение.</p></div>\
        <script>history.replaceState({},'',location.pathname)</script>";
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = sock.write_all(resp.as_bytes()).await;
    let _ = sock.flush().await;

    if let Some(e) = err {
        return Err(e);
    }
    // New flow: exchange the one-time code for the token pair.
    if let Some(code) = code {
        return server_exchange(endpoint, &code).await;
    }
    // Legacy flow: server handed us the session token directly.
    if let Some(token) = token {
        return Ok((token, None));
    }
    Err("no code in the OAuth callback".to_string())
}

fn urldecode(s: &str) -> String {
    urlencoding::decode(s)
        .map(|c| c.into_owned())
        .unwrap_or_else(|_| s.to_string())
}

/// The `SyncRemote` backed by `rh-sync-server` over HTTP.
#[derive(Debug)]
pub struct ServerRemote {
    client: reqwest::Client,
    base_url: String,
    /// Current access token. Swapped in place when refreshed, so `&self`
    /// methods (the `SyncRemote` trait) can renew it transparently.
    token: tokio::sync::Mutex<String>,
    /// Refresh token, if we have one. Used to silently renew `token` on 401.
    refresh: Option<String>,
}

impl ServerRemote {
    pub fn new(base_url: String, token: String, refresh: Option<String>) -> Self {
        Self {
            client: http_client(),
            base_url,
            token: tokio::sync::Mutex::new(token),
            refresh,
        }
    }

    async fn current_token(&self) -> String {
        self.token.lock().await.clone()
    }

    /// On a 401, mint a fresh access token from the stored refresh token,
    /// persist it to the keychain, and return it. `None` if there is no refresh
    /// token or it is no longer valid — only then must the user sign in again.
    async fn try_refresh(&self) -> Option<String> {
        let refresh = self.refresh.as_ref()?;
        let fresh = server_refresh(&self.base_url, refresh).await.ok()?;
        *self.token.lock().await = fresh.clone();
        let _ = token_set(&fresh);
        Some(fresh)
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
        let url = format!("{}/v1/vault", self.base_url);
        let token = self.current_token().await;
        let mut resp = self
            .client
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| VaultError::Transport(format!("pull: {e}")))?;

        // Access token expired → renew via refresh token and retry once.
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            if let Some(fresh) = self.try_refresh().await {
                resp = self
                    .client
                    .get(&url)
                    .bearer_auth(&fresh)
                    .send()
                    .await
                    .map_err(|e| VaultError::Transport(format!("pull: {e}")))?;
            }
        }

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
        let url = format!("{}/v1/vault", self.base_url);

        // Build the PUT fresh each attempt (the body/headers are consumed on send).
        let build = |token: &str| {
            let mut req = self
                .client
                .put(&url)
                .bearer_auth(token)
                .json(&PutBody {
                    blob_b64: blob_b64.clone(),
                });
            if let Some(rev) = expected {
                req = req.header("If-Match", rev);
            }
            req
        };

        let token = self.current_token().await;
        let mut resp = build(&token)
            .send()
            .await
            .map_err(|e| VaultError::Transport(format!("push: {e}")))?;

        // Access token expired → renew via refresh token and retry once.
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            if let Some(fresh) = self.try_refresh().await {
                resp = build(&fresh)
                    .send()
                    .await
                    .map_err(|e| VaultError::Transport(format!("push: {e}")))?;
            }
        }

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
