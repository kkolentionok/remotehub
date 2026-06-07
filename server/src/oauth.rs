//! Yandex OAuth (slice 3a-2) + email-verification tokens.
//!
//! Desktop flow (server-mediated loopback): the app opens the system browser
//! at `/v1/oauth/yandex/start?cb=http://127.0.0.1:<port>/cb`; we 302 to Yandex
//! carrying a signed `state`; Yandex calls our `/v1/oauth/yandex/callback`; we
//! exchange the code (with our client secret, which never ships to the app),
//! read the user's id + email, upsert the account, mint our bearer JWT, and
//! 302 back to the app's loopback `cb?token=<jwt>`. The client secret stays on
//! the server; the app only ever receives its own bearer token.
//!
//! `state` and verification links are stateless signed JWTs (same secret as
//! the bearer tokens), so no extra table is needed.

use chrono::Utc;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

use crate::error::AppError;

pub const YANDEX_AUTHORIZE: &str = "https://oauth.yandex.ru/authorize";
pub const YANDEX_TOKEN: &str = "https://oauth.yandex.ru/token";
pub const YANDEX_USERINFO: &str = "https://login.yandex.ru/info";

// ---- OAuth `state` (CSRF + carries the app loopback) ---------------------

#[derive(Serialize, Deserialize)]
struct StateClaims {
    cb: String,
    nonce: String,
    purpose: String,
    exp: usize,
}

pub fn encode_state(cb: &str, nonce: &str, secret: &str) -> Result<String, AppError> {
    let exp = (Utc::now() + chrono::Duration::minutes(10)).timestamp() as usize;
    let claims = StateClaims {
        cb: cb.to_string(),
        nonce: nonce.to_string(),
        purpose: "oauth_state".to_string(),
        exp,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|_| AppError::Internal)
}

/// Returns `(cb, nonce)`.
pub fn decode_state(state: &str, secret: &str) -> Result<(String, String), AppError> {
    let data = decode::<StateClaims>(
        state,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .map_err(|_| AppError::BadRequest("invalid or expired OAuth state".to_string()))?;
    if data.claims.purpose != "oauth_state" {
        return Err(AppError::BadRequest("invalid OAuth state".to_string()));
    }
    Ok((data.claims.cb, data.claims.nonce))
}

/// `cb` must be a loopback `http://` URL (`127.0.0.1` / `localhost`). This
/// prevents the endpoint being abused as an open redirector.
pub fn is_loopback_cb(cb: &str) -> bool {
    let Some(rest) = cb.strip_prefix("http://") else {
        return false;
    };
    let host = rest.split(['/', ':']).next().unwrap_or("");
    host == "127.0.0.1" || host == "localhost"
}

pub fn build_authorize_url(client_id: &str, redirect_uri: &str, state: &str) -> String {
    format!(
        "{YANDEX_AUTHORIZE}?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}&force_confirm=yes",
        urlencoding::encode(client_id),
        urlencoding::encode(redirect_uri),
        urlencoding::encode("login:email login:info"),
        urlencoding::encode(state),
    )
}

// ---- Yandex token + userinfo ---------------------------------------------

#[derive(Deserialize)]
struct TokenResp {
    access_token: String,
}

pub async fn exchange_code(
    client_id: &str,
    client_secret: &str,
    code: &str,
) -> Result<String, AppError> {
    let resp = reqwest::Client::new()
        .post(YANDEX_TOKEN)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("client_id", client_id),
            ("client_secret", client_secret),
        ])
        .send()
        .await
        .map_err(|e| {
            tracing::error!("yandex token request failed: {e}");
            AppError::Internal
        })?;
    if !resp.status().is_success() {
        tracing::error!("yandex token http {}", resp.status());
        return Err(AppError::BadRequest("OAuth token exchange failed".to_string()));
    }
    let t: TokenResp = resp.json().await.map_err(|e| {
        tracing::error!("yandex token decode: {e}");
        AppError::Internal
    })?;
    Ok(t.access_token)
}

#[derive(Deserialize)]
struct UserInfo {
    id: String,
    #[serde(default)]
    default_email: Option<String>,
    #[serde(default)]
    login: Option<String>,
}

pub struct YandexUser {
    pub sub: String,
    pub email: String,
}

pub async fn fetch_userinfo(access_token: &str) -> Result<YandexUser, AppError> {
    let resp = reqwest::Client::new()
        .get(format!("{YANDEX_USERINFO}?format=json"))
        .header("Authorization", format!("OAuth {access_token}"))
        .send()
        .await
        .map_err(|e| {
            tracing::error!("yandex userinfo request failed: {e}");
            AppError::Internal
        })?;
    if !resp.status().is_success() {
        return Err(AppError::BadRequest("OAuth userinfo failed".to_string()));
    }
    let u: UserInfo = resp.json().await.map_err(|e| {
        tracing::error!("yandex userinfo decode: {e}");
        AppError::Internal
    })?;
    let email = u
        .default_email
        .or_else(|| u.login.map(|l| format!("{l}@yandex.ru")))
        .ok_or_else(|| AppError::BadRequest("Yandex account has no email".to_string()))?
        .trim()
        .to_lowercase();
    Ok(YandexUser { sub: u.id, email })
}

// ---- email-verification tokens -------------------------------------------

#[derive(Serialize, Deserialize)]
struct VerifyClaims {
    sub: String,
    purpose: String,
    exp: usize,
}

pub fn encode_verify(account_id: &str, secret: &str) -> Result<String, AppError> {
    let exp = (Utc::now() + chrono::Duration::days(2)).timestamp() as usize;
    let claims = VerifyClaims {
        sub: account_id.to_string(),
        purpose: "verify".to_string(),
        exp,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|_| AppError::Internal)
}

pub fn decode_verify(token: &str, secret: &str) -> Result<String, AppError> {
    let data = decode::<VerifyClaims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .map_err(|_| AppError::BadRequest("invalid or expired verification link".to_string()))?;
    if data.claims.purpose != "verify" {
        return Err(AppError::BadRequest("invalid verification link".to_string()));
    }
    Ok(data.claims.sub)
}
