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

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    /// Subject = account id.
    pub sub: String,
    /// Expiry (unix seconds).
    pub exp: usize,
}

/// Mint a signed bearer token for `account_id`.
pub fn issue_token(account_id: &str, secret: &str, ttl_hours: i64) -> Result<String, AppError> {
    let exp = (Utc::now() + chrono::Duration::hours(ttl_hours)).timestamp() as usize;
    let claims = Claims {
        sub: account_id.to_string(),
        exp,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|_| AppError::Internal)
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
        Ok(AuthAccount(data.claims.sub))
    }
}
