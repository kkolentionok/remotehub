//! Notes: a second opaque blob, plus pairing-by-code.
//!
//! Notes are stored apart from the vault so a device can be granted access to
//! *only* them. The blob is E2E-sealed under a notes key the server never
//! sees, exactly like the vault.
//!
//! ## The pairing handshake
//! 1. A signed-in client picks a random code, derives a key from it, wraps the
//!    notes key under that key, and POSTs `{code_hash, wrapped_key_b64}`. The
//!    **code itself never reaches the server** — only its hash — so the server
//!    cannot unwrap the key it is storing.
//! 2. The user types the code on the second device, which hashes it the same
//!    way and POSTs `/v1/pair/claim`. On success it gets the wrapped key back
//!    (unwrappable only with the code it just typed) and a notes-scoped token.
//! 3. The pairing is consumed on first claim and expires regardless.
//!
//! A code is 8 characters of Crockford base32 — about 40 bits, single-use,
//! valid for minutes. Guessing it inside that window means ~10^11 requests.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::{decode_claims, issue_notes_token, AuthAccount};
use crate::error::AppError;
use crate::AppState;

/// How long an unclaimed pairing stays valid.
const PAIRING_TTL_MINUTES: i64 = 10;
/// Lifetime of a notes-scoped token. Long, because revocation is checked
/// server-side on every request — expiry is a backstop, not the control.
const NOTES_TOKEN_TTL_DAYS: i64 = 365;

/// Access to the notes endpoints: either a full account token, or a
/// notes-scoped token whose device is still active.
pub struct NotesAccess {
    pub account: String,
    /// `Some` when the caller is a paired device rather than the account owner.
    pub device: Option<String>,
}

#[axum::async_trait]
impl axum::extract::FromRequestParts<AppState> for NotesAccess {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|h| h.strip_prefix("Bearer "))
            .ok_or(AppError::Unauthorized)?;

        let claims = decode_claims(token, &state.cfg.jwt_secret)?;
        match claims.typ.as_str() {
            "access" => Ok(NotesAccess {
                account: claims.sub,
                device: None,
            }),
            "notes" => {
                let device = claims.did.clone().ok_or(AppError::Unauthorized)?;
                // Revocation must bite immediately, so it is a lookup per
                // request rather than a property of the token.
                let row = sqlx::query_as::<_, (i64,)>(
                    "SELECT revoked FROM notes_devices WHERE id = ? AND account_id = ?",
                )
                .bind(&device)
                .bind(&claims.sub)
                .fetch_optional(&state.pool)
                .await?;
                match row {
                    Some((0,)) => {}
                    _ => return Err(AppError::Unauthorized),
                }
                let _ = sqlx::query("UPDATE notes_devices SET last_seen_at = ? WHERE id = ?")
                    .bind(Utc::now().to_rfc3339())
                    .bind(&device)
                    .execute(&state.pool)
                    .await;
                Ok(NotesAccess {
                    account: claims.sub,
                    device: Some(device),
                })
            }
            _ => Err(AppError::Unauthorized),
        }
    }
}

#[derive(Serialize)]
pub struct NotesResp {
    pub blob_b64: String,
    pub rev: String,
}

#[derive(Deserialize)]
pub struct PutNotesReq {
    pub blob_b64: String,
}

#[derive(Serialize)]
pub struct PutNotesResp {
    pub rev: String,
}

pub async fn get_notes(
    State(st): State<AppState>,
    access: NotesAccess,
) -> Result<Response, AppError> {
    let row = sqlx::query_as::<_, (String, i64)>(
        "SELECT blob, rev FROM notes_blobs WHERE account_id = ?",
    )
    .bind(&access.account)
    .fetch_optional(&st.pool)
    .await?;

    match row {
        Some((blob, rev)) => Ok(Json(NotesResp {
            blob_b64: blob,
            rev: rev.to_string(),
        })
        .into_response()),
        None => Ok(StatusCode::NO_CONTENT.into_response()),
    }
}

/// Same optimistic-concurrency contract as the vault: `If-Match: <rev>` for an
/// update, absent for a first create.
pub async fn put_notes(
    State(st): State<AppState>,
    access: NotesAccess,
    headers: HeaderMap,
    Json(req): Json<PutNotesReq>,
) -> Result<Json<PutNotesResp>, AppError> {
    if req.blob_b64.len() > st.cfg.max_blob_bytes {
        return Err(AppError::TooLarge);
    }
    let now = Utc::now().to_rfc3339();
    let expected = headers
        .get("if-match")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().trim_matches('"').to_string());

    match expected {
        Some(exp) => {
            let exp_rev: i64 = exp
                .parse()
                .map_err(|_| AppError::BadRequest("malformed If-Match".to_string()))?;
            let res = sqlx::query(
                "UPDATE notes_blobs SET blob = ?, rev = rev + 1, updated_at = ? \
                 WHERE account_id = ? AND rev = ?",
            )
            .bind(&req.blob_b64)
            .bind(&now)
            .bind(&access.account)
            .bind(exp_rev)
            .execute(&st.pool)
            .await?;

            if res.rows_affected() == 1 {
                Ok(Json(PutNotesResp {
                    rev: (exp_rev + 1).to_string(),
                }))
            } else {
                Err(AppError::Conflict)
            }
        }
        None => {
            let res = sqlx::query(
                "INSERT INTO notes_blobs (account_id, blob, rev, updated_at) \
                 VALUES (?, ?, 1, ?) ON CONFLICT(account_id) DO NOTHING",
            )
            .bind(&access.account)
            .bind(&req.blob_b64)
            .bind(&now)
            .execute(&st.pool)
            .await?;

            if res.rows_affected() == 1 {
                Ok(Json(PutNotesResp {
                    rev: "1".to_string(),
                }))
            } else {
                Err(AppError::PreconditionFailed)
            }
        }
    }
}

#[derive(Deserialize)]
pub struct PairCreateReq {
    /// Hex SHA-256 of the code, computed on the client.
    pub code_hash: String,
    /// The notes key, sealed under a key derived from the code.
    pub wrapped_key_b64: String,
}

#[derive(Serialize)]
pub struct PairCreateResp {
    pub expires_at: String,
}

/// Register a pending pairing. Replaces any earlier one for the same hash.
pub async fn pair_create(
    State(st): State<AppState>,
    AuthAccount(account): AuthAccount,
    Json(req): Json<PairCreateReq>,
) -> Result<Json<PairCreateResp>, AppError> {
    if req.code_hash.len() < 32 || req.code_hash.len() > 128 {
        return Err(AppError::BadRequest("malformed code_hash".to_string()));
    }
    if req.wrapped_key_b64.is_empty() || req.wrapped_key_b64.len() > 4096 {
        return Err(AppError::BadRequest("malformed wrapped_key".to_string()));
    }

    let now = Utc::now();
    let expires_at = (now + Duration::minutes(PAIRING_TTL_MINUTES)).to_rfc3339();

    // Housekeeping: expired or claimed rows are dead weight and a needless
    // window if the file is ever read.
    let _ = sqlx::query("DELETE FROM pairings WHERE expires_at < ? OR claimed_at IS NOT NULL")
        .bind(now.to_rfc3339())
        .execute(&st.pool)
        .await;

    sqlx::query(
        "INSERT INTO pairings (code_hash, account_id, wrapped_key, expires_at, created_at) \
         VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT(code_hash) DO UPDATE SET \
            account_id = excluded.account_id, \
            wrapped_key = excluded.wrapped_key, \
            expires_at = excluded.expires_at, \
            claimed_at = NULL, \
            created_at = excluded.created_at",
    )
    .bind(&req.code_hash)
    .bind(&account)
    .bind(&req.wrapped_key_b64)
    .bind(&expires_at)
    .bind(now.to_rfc3339())
    .execute(&st.pool)
    .await?;

    Ok(Json(PairCreateResp { expires_at }))
}

#[derive(Deserialize)]
pub struct PairClaimReq {
    pub code_hash: String,
    /// Shown in the owner's device list, e.g. a hostname.
    #[serde(default)]
    pub label: String,
}

#[derive(Serialize)]
pub struct PairClaimResp {
    pub wrapped_key_b64: String,
    pub token: String,
    pub device_id: String,
}

/// Consume a pairing: hand back the wrapped notes key and a notes-scoped
/// token. Unauthenticated by design — the code *is* the credential.
pub async fn pair_claim(
    State(st): State<AppState>,
    Json(req): Json<PairClaimReq>,
) -> Result<Json<PairClaimResp>, AppError> {
    let now = Utc::now();
    let now_s = now.to_rfc3339();

    // Single-use: the UPDATE both claims the row and proves it was unclaimed
    // and unexpired, so two devices racing the same code cannot both win.
    let claimed = sqlx::query(
        "UPDATE pairings SET claimed_at = ? \
         WHERE code_hash = ? AND claimed_at IS NULL AND expires_at > ?",
    )
    .bind(&now_s)
    .bind(&req.code_hash)
    .bind(&now_s)
    .execute(&st.pool)
    .await?;

    if claimed.rows_affected() != 1 {
        // Wrong, already used, or expired — one answer for all three, so a
        // guesser learns nothing from the distinction.
        return Err(AppError::Unauthorized);
    }

    let (account, wrapped) = sqlx::query_as::<_, (String, String)>(
        "SELECT account_id, wrapped_key FROM pairings WHERE code_hash = ?",
    )
    .bind(&req.code_hash)
    .fetch_optional(&st.pool)
    .await?
    .ok_or(AppError::Unauthorized)?;

    let device_id = Uuid::new_v4().to_string();
    let label = {
        let l = req.label.trim();
        if l.is_empty() {
            "Device".to_string()
        } else {
            l.chars().take(64).collect::<String>()
        }
    };

    sqlx::query(
        "INSERT INTO notes_devices (id, account_id, label, created_at, revoked) \
         VALUES (?, ?, ?, ?, 0)",
    )
    .bind(&device_id)
    .bind(&account)
    .bind(&label)
    .bind(&now_s)
    .execute(&st.pool)
    .await?;

    let token = issue_notes_token(
        &account,
        &device_id,
        &st.cfg.jwt_secret,
        NOTES_TOKEN_TTL_DAYS,
    )?;

    Ok(Json(PairClaimResp {
        wrapped_key_b64: wrapped,
        token,
        device_id,
    }))
}

#[derive(Serialize)]
pub struct DeviceDto {
    pub id: String,
    pub label: String,
    pub created_at: String,
    pub last_seen_at: Option<String>,
}

pub async fn devices_list(
    State(st): State<AppState>,
    AuthAccount(account): AuthAccount,
) -> Result<Json<Vec<DeviceDto>>, AppError> {
    let rows = sqlx::query_as::<_, (String, String, String, Option<String>)>(
        "SELECT id, label, created_at, last_seen_at FROM notes_devices \
         WHERE account_id = ? AND revoked = 0 ORDER BY created_at DESC",
    )
    .bind(&account)
    .fetch_all(&st.pool)
    .await?;

    Ok(Json(
        rows.into_iter()
            .map(|(id, label, created_at, last_seen_at)| DeviceDto {
                id,
                label,
                created_at,
                last_seen_at,
            })
            .collect(),
    ))
}

/// Revoke a paired device. Its token stops working on the next request.
pub async fn device_revoke(
    State(st): State<AppState>,
    AuthAccount(account): AuthAccount,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    let res = sqlx::query("UPDATE notes_devices SET revoked = 1 WHERE id = ? AND account_id = ?")
        .bind(&id)
        .bind(&account)
        .execute(&st.pool)
        .await?;
    if res.rows_affected() == 1 {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(AppError::NotFound)
    }
}

/// Parse an RFC3339 stamp, for callers that need to compare times.
#[allow(dead_code)]
fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}
