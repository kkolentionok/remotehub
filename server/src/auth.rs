//! Authentication: Argon2 password hashing, JWT bearer tokens, and the
//! [`AuthAccount`] extractor that turns a valid `Authorization: Bearer <jwt>`
//! into the account id.
//!
//! This authenticates the *account* to the server. It is independent of the
//! vault's **E2E** encryption: the master password that derives the vault key
//! never reaches the server (the `blob` is already sealed). Yandex OAuth
//! (slice 3a-2) is an alternative way to obtain a token for an account; the
//! token format and everything downstream is unchanged.

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{
    PasswordHash, PasswordHasher, PasswordVerifier, SaltString,
};
use argon2::Argon2;
use axum::extract::FromRequestParts;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use chrono::Utc;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::AppState;

/// Hash a password for storage (PHC string form — carries its own salt + params).
pub fn hash_password(password: &str) -> Result<String, AppError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|_| AppError::Internal)
}

/// Constant-time-ish verify of a password against a stored PHC hash.
pub fn verify_password(password: &str, stored_hash: &str) -> bool {
    match PasswordHash::new(stored_hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

fn default_typ() -> String {
    "access".to_string()
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    /// Subject = account id.
    pub sub: String,
    /// Expiry (unix seconds).
    pub exp: usize,
    /// Token kind: "access" (default, for back-compat with pre-refresh tokens)
    /// or "refresh". Access tokens authenticate API calls; refresh tokens are
    /// only accepted by `/v1/refresh`.
    #[serde(default = "default_typ")]
    pub typ: String,
    /// Device id, present only on `typ = "notes"` tokens (see
    /// `crate::notes`). Lets revocation be checked per device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub did: Option<String>,
}

/// Mint a signed bearer (access) token for `account_id`.
pub fn issue_token(account_id: &str, secret: &str, ttl_hours: i64) -> Result<String, AppError> {
    let exp = (Utc::now() + chrono::Duration::hours(ttl_hours)).timestamp() as usize;
    let claims = Claims {
        sub: account_id.to_string(),
        exp,
        typ: "access".to_string(),
        did: None,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|_| AppError::Internal)
}

/// Mint a long-lived refresh token. The client stores it and exchanges it for
/// a fresh access token via `/v1/refresh` whenever the access token expires —
/// so the user is never asked to sign in again until the refresh itself lapses.
pub fn issue_refresh(account_id: &str, secret: &str, ttl_days: i64) -> Result<String, AppError> {
    let exp = (Utc::now() + chrono::Duration::days(ttl_days)).timestamp() as usize;
    let claims = Claims {
        sub: account_id.to_string(),
        exp,
        typ: "refresh".to_string(),
        did: None,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|_| AppError::Internal)
}

/// Mint a **notes-scoped** token: it authenticates a device that was paired by
/// code and is accepted only by the notes endpoints, never by the vault. It
/// carries the device id so a revoked device is rejected at once.
pub fn issue_notes_token(
    account_id: &str,
    device_id: &str,
    secret: &str,
    ttl_days: i64,
) -> Result<String, AppError> {
    let exp = (Utc::now() + chrono::Duration::days(ttl_days)).timestamp() as usize;
    let claims = Claims {
        sub: account_id.to_string(),
        exp,
        typ: "notes".to_string(),
        did: Some(device_id.to_string()),
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|_| AppError::Internal)
}

/// Decode any token and hand back its claims, without asserting a kind.
/// Used by the notes extractor, which accepts two kinds.
pub fn decode_claims(token: &str, secret: &str) -> Result<Claims, AppError> {
    decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .map(|d| d.claims)
    .map_err(|_| AppError::Unauthorized)
}

/// Validate a refresh token and return its account id. Rejects access tokens.
pub fn decode_refresh(token: &str, secret: &str) -> Result<String, AppError> {
    let data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .map_err(|_| AppError::Unauthorized)?;
    if data.claims.typ != "refresh" {
        return Err(AppError::Unauthorized);
    }
    Ok(data.claims.sub)
}

/// The authenticated account id, extracted from the bearer JWT. Add it as a
/// handler argument to require authentication.
pub struct AuthAccount(pub String);

#[axum::async_trait]
impl FromRequestParts<AppState> for AuthAccount {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .ok_or(AppError::Unauthorized)?;
        let token = header
            .strip_prefix("Bearer ")
            .ok_or(AppError::Unauthorized)?;
        let data = decode::<Claims>(
            token,
            &DecodingKey::from_secret(state.cfg.jwt_secret.as_bytes()),
            &Validation::default(),
        )
        .map_err(|_| AppError::Unauthorized)?;
        // A refresh token must not be usable as a bearer credential.
        if data.claims.typ != "access" {
            return Err(AppError::Unauthorized);
        }
        Ok(AuthAccount(data.claims.sub))
    }
}
