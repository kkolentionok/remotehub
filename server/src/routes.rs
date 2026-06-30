//! HTTP handlers — the §9.1 protocol: account register/login and the
//! versioned vault GET/PUT with optimistic concurrency.
//!
//! Vault concurrency is enforced with single, atomic SQL statements (no
//! read-then-write race): an update is conditional on the expected `rev`, and
//! a create is an insert that fails if a row already exists. `rev` is an
//! opaque version token to the client; here it is a monotonic integer.

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Json;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::{
    decode_refresh, hash_password, issue_refresh, issue_token, verify_password, AuthAccount,
};
use crate::error::AppError;
use crate::{oauth, AppState};

pub async fn health() -> &'static str {
    "ok"
}

#[derive(Deserialize)]
pub struct RegisterReq {
    pub email: String,
    pub password: String,
}

/// Create an email/password account. (Email verification is enforced in slice
/// 3a-2; for now the account is usable immediately.)
pub async fn register(
    State(st): State<AppState>,
    Json(req): Json<RegisterReq>,
) -> Result<StatusCode, AppError> {
    let email = req.email.trim().to_lowercase();
    if email.is_empty() || !email.contains('@') {
        return Err(AppError::BadRequest("invalid email".to_string()));
    }
    if req.password.len() < 8 {
        return Err(AppError::BadRequest("password too short (min 8)".to_string()));
    }

    let id = Uuid::new_v4().to_string();
    let hash = hash_password(&req.password)?;
    let now = Utc::now().to_rfc3339();

    let res = sqlx::query(
        "INSERT INTO accounts (id, email, password_hash, email_verified, created_at) \
         VALUES (?, ?, ?, 0, ?) ON CONFLICT(email) DO NOTHING",
    )
    .bind(&id)
    .bind(&email)
    .bind(&hash)
    .bind(&now)
    .execute(&st.pool)
    .await?;

    if res.rows_affected() == 0 {
        return Err(AppError::Conflict); // email already registered
    }

    // When verification is enforced, mint a link and log it. (SMTP delivery is
    // a deploy concern; the link is stateless — a signed token — so no table.)
    if st.cfg.require_email_verification {
        if let Some(base) = &st.cfg.public_base_url {
            if let Ok(tok) = oauth::encode_verify(&id, &st.cfg.jwt_secret) {
                tracing::info!("email verification link for {email}: {base}/v1/verify?token={tok}");
            }
        }
    }
    Ok(StatusCode::CREATED)
}

#[derive(Deserialize)]
pub struct LoginReq {
    pub email: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct LoginResp {
    pub token: String,
    /// Long-lived refresh token. The client stores it and uses `/v1/refresh`
    /// to renew `token` silently. (Older clients simply ignore this field.)
    pub refresh: String,
}

pub async fn login(
    State(st): State<AppState>,
    Json(req): Json<LoginReq>,
) -> Result<Json<LoginResp>, AppError> {
    let email = req.email.trim().to_lowercase();

    let row = sqlx::query_as::<_, (String, Option<String>, i64)>(
        "SELECT id, password_hash, email_verified FROM accounts WHERE email = ?",
    )
    .bind(&email)
    .fetch_optional(&st.pool)
    .await?;

    let (id, hash, verified) = row.ok_or(AppError::Unauthorized)?;
    // No password hash => an OAuth-only account; reject password login.
    let hash = hash.ok_or(AppError::Unauthorized)?;
    if !verify_password(&req.password, &hash) {
        return Err(AppError::Unauthorized);
    }
    if st.cfg.require_email_verification && verified == 0 {
        return Err(AppError::Forbidden("email not verified".to_string()));
    }

    let token = issue_token(&id, &st.cfg.jwt_secret, st.cfg.token_ttl_hours)?;
    let refresh = issue_refresh(&id, &st.cfg.jwt_secret, st.cfg.refresh_ttl_days)?;
    Ok(Json(LoginResp { token, refresh }))
}

#[derive(Deserialize)]
pub struct RefreshReq {
    pub refresh: String,
}

#[derive(Serialize)]
pub struct RefreshResp {
    pub token: String,
}

/// Exchange a valid refresh token for a fresh access token. The refresh token
/// itself is unchanged (kept until it lapses). 401 if the refresh is invalid
/// or expired — only then must the user sign in again.
pub async fn refresh(
    State(st): State<AppState>,
    Json(req): Json<RefreshReq>,
) -> Result<Json<RefreshResp>, AppError> {
    let account = decode_refresh(&req.refresh, &st.cfg.jwt_secret)?;
    let token = issue_token(&account, &st.cfg.jwt_secret, st.cfg.token_ttl_hours)?;
    Ok(Json(RefreshResp { token }))
}

#[derive(Deserialize)]
pub struct ExchangeReq {
    pub code: String,
}

/// Exchange a one-time OAuth `code` (from the loopback redirect) for the
/// access+refresh pair. This keeps the session token out of the browser URL.
pub async fn oauth_exchange(
    State(st): State<AppState>,
    Json(req): Json<ExchangeReq>,
) -> Result<Json<LoginResp>, AppError> {
    let account = oauth::decode_exchange(&req.code, &st.cfg.jwt_secret)?;
    let token = issue_token(&account, &st.cfg.jwt_secret, st.cfg.token_ttl_hours)?;
    let refresh = issue_refresh(&account, &st.cfg.jwt_secret, st.cfg.refresh_ttl_days)?;
    Ok(Json(LoginResp { token, refresh }))
}

#[derive(Serialize)]
pub struct VaultResp {
    pub blob_b64: String,
    pub rev: String,
}

/// Fetch the account's current vault blob, or `204 No Content` if none yet.
pub async fn get_vault(
    State(st): State<AppState>,
    AuthAccount(account): AuthAccount,
) -> Result<Response, AppError> {
    let row = sqlx::query_as::<_, (String, i64)>(
        "SELECT blob, rev FROM vaults WHERE account_id = ?",
    )
    .bind(&account)
    .fetch_optional(&st.pool)
    .await?;

    match row {
        Some((blob, rev)) => Ok(Json(VaultResp {
            blob_b64: blob,
            rev: rev.to_string(),
        })
        .into_response()),
        None => Ok(StatusCode::NO_CONTENT.into_response()),
    }
}

#[derive(Deserialize)]
pub struct PutVaultReq {
    pub blob_b64: String,
}

#[derive(Serialize)]
pub struct PutVaultResp {
    pub rev: String,
}

/// Upload a new vault blob.
///
/// - `If-Match: <rev>` present → conditional update; mismatch/absent row → 409.
/// - `If-Match` absent → create-only; if a vault already exists → 412 (the
///   client should GET first and retry with `If-Match`).
pub async fn put_vault(
    State(st): State<AppState>,
    AuthAccount(account): AuthAccount,
    headers: HeaderMap,
    Json(req): Json<PutVaultReq>,
) -> Result<Json<PutVaultResp>, AppError> {
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
                "UPDATE vaults SET blob = ?, rev = rev + 1, updated_at = ? \
                 WHERE account_id = ? AND rev = ?",
            )
            .bind(&req.blob_b64)
            .bind(&now)
            .bind(&account)
            .bind(exp_rev)
            .execute(&st.pool)
            .await?;

            if res.rows_affected() == 1 {
                Ok(Json(PutVaultResp {
                    rev: (exp_rev + 1).to_string(),
                }))
            } else {
                // No row, or the stored rev moved on → stale.
                Err(AppError::Conflict)
            }
        }
        None => {
            let res = sqlx::query(
                "INSERT INTO vaults (account_id, blob, rev, updated_at) VALUES (?, ?, 1, ?) \
                 ON CONFLICT(account_id) DO NOTHING",
            )
            .bind(&account)
            .bind(&req.blob_b64)
            .bind(&now)
            .execute(&st.pool)
            .await?;

            if res.rows_affected() == 1 {
                Ok(Json(PutVaultResp {
                    rev: "1".to_string(),
                }))
            } else {
                Err(AppError::PreconditionFailed) // already exists
            }
        }
    }
}

// ---- Yandex OAuth + email verification (slice 3a-2) -----------------------

#[derive(Deserialize)]
pub struct StartQuery {
    /// The desktop app's loopback callback, e.g. `http://127.0.0.1:53127/cb`.
    pub cb: String,
    /// Delivery mode: "code" (new clients — one-time exchange code) or absent
    /// (legacy clients — session token in the loopback URL).
    #[serde(default)]
    pub mode: Option<String>,
}

/// Begin the Yandex flow: validate the loopback `cb`, sign it into `state`,
/// and 302 to Yandex's consent screen. Opened in the user's system browser.
pub async fn oauth_yandex_start(
    State(st): State<AppState>,
    Query(q): Query<StartQuery>,
) -> Result<Redirect, AppError> {
    if !st.cfg.oauth_enabled() {
        return Err(AppError::BadRequest(
            "OAuth is not configured on this server".to_string(),
        ));
    }
    if !oauth::is_loopback_cb(&q.cb) {
        return Err(AppError::BadRequest("cb must be a loopback URL".to_string()));
    }
    let nonce = Uuid::new_v4().to_string();
    let mode = q.mode.as_deref().unwrap_or("");
    let state = oauth::encode_state(&q.cb, &nonce, mode, &st.cfg.jwt_secret)?;
    // Safe: oauth_enabled() guarantees these are Some.
    let base = st.cfg.public_base_url.as_deref().unwrap_or_default();
    let client_id = st.cfg.yandex_client_id.as_deref().unwrap_or_default();
    let redirect_uri = format!("{base}/v1/oauth/yandex/callback");
    Ok(Redirect::to(&oauth::build_authorize_url(
        client_id,
        &redirect_uri,
        &state,
    )))
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}

/// Yandex redirects here with `code` + `state`. We exchange the code, upsert
/// the account, and bounce back to the app's loopback. New clients (mode=code)
/// receive a one-time `cb?code=…` they exchange for tokens; legacy clients
/// receive `cb?token=…` (the session token). Failures go back as `cb?error=…`.
pub async fn oauth_yandex_callback(
    State(st): State<AppState>,
    Query(q): Query<CallbackQuery>,
) -> Response {
    // Recover cb + mode from state up-front so failures can be reported.
    let decoded = q
        .state
        .as_deref()
        .and_then(|s| oauth::decode_state(s, &st.cfg.jwt_secret).ok());
    let cb = decoded.as_ref().map(|(cb, _nonce, _mode)| cb.clone());
    let mode = decoded
        .as_ref()
        .map(|(_cb, _nonce, mode)| mode.clone())
        .unwrap_or_default();

    match oauth_callback_inner(&st, &q).await {
        Ok(account_id) => {
            let delivered = if mode == "code" {
                oauth::encode_exchange(&account_id, &st.cfg.jwt_secret)
                    .map(|code| ("code", code))
            } else {
                issue_token(&account_id, &st.cfg.jwt_secret, st.cfg.token_ttl_hours)
                    .map(|tok| ("token", tok))
            };
            match (cb, delivered) {
                (Some(cb), Ok((key, val))) => {
                    Redirect::to(&append_param(&cb, key, &val)).into_response()
                }
                (Some(cb), Err(e)) => {
                    Redirect::to(&append_param(&cb, "error", &e.to_string())).into_response()
                }
                (None, _) => Html(SIGNED_IN_HTML.to_string()).into_response(),
            }
        }
        Err(e) => {
            let msg = e.to_string();
            match cb {
                Some(cb) => Redirect::to(&append_param(&cb, "error", &msg)).into_response(),
                None => (
                    StatusCode::BAD_REQUEST,
                    Html(format!("<h3>Sign-in failed</h3><p>{msg}</p>")),
                )
                    .into_response(),
            }
        }
    }
}

/// Runs the Yandex exchange + account upsert. Returns the account id; the
/// caller decides how to deliver the session (code vs token).
async fn oauth_callback_inner(st: &AppState, q: &CallbackQuery) -> Result<String, AppError> {
    if !st.cfg.oauth_enabled() {
        return Err(AppError::BadRequest("OAuth is not configured".to_string()));
    }
    if let Some(err) = &q.error {
        return Err(AppError::BadRequest(format!("Yandex denied access: {err}")));
    }
    let code = q
        .code
        .as_deref()
        .ok_or_else(|| AppError::BadRequest("missing code".to_string()))?;
    let state = q
        .state
        .as_deref()
        .ok_or_else(|| AppError::BadRequest("missing state".to_string()))?;
    // Validate (and discard) the state — its only job was CSRF + carrying cb.
    let _ = oauth::decode_state(state, &st.cfg.jwt_secret)?;

    let client_id = st.cfg.yandex_client_id.as_deref().unwrap_or_default();
    let client_secret = st.cfg.yandex_client_secret.as_deref().unwrap_or_default();
    let access = oauth::exchange_code(client_id, client_secret, code).await?;
    let user = oauth::fetch_userinfo(&access).await?;
    upsert_oauth_account(st, &user).await
}

/// Find the account by Yandex subject; else by email (link it); else create a
/// password-less, email-verified account. Returns the account id.
async fn upsert_oauth_account(st: &AppState, user: &oauth::YandexUser) -> Result<String, AppError> {
    if let Some((id,)) =
        sqlx::query_as::<_, (String,)>("SELECT id FROM accounts WHERE yandex_sub = ?")
            .bind(&user.sub)
            .fetch_optional(&st.pool)
            .await?
    {
        return Ok(id);
    }
    if let Some((id,)) = sqlx::query_as::<_, (String,)>("SELECT id FROM accounts WHERE email = ?")
        .bind(&user.email)
        .fetch_optional(&st.pool)
        .await?
    {
        sqlx::query("UPDATE accounts SET yandex_sub = ?, email_verified = 1 WHERE id = ?")
            .bind(&user.sub)
            .bind(&id)
            .execute(&st.pool)
            .await?;
        return Ok(id);
    }
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO accounts (id, email, password_hash, yandex_sub, email_verified, created_at) \
         VALUES (?, ?, NULL, ?, 1, ?)",
    )
    .bind(&id)
    .bind(&user.email)
    .bind(&user.sub)
    .bind(&now)
    .execute(&st.pool)
    .await?;
    Ok(id)
}

#[derive(Deserialize)]
pub struct VerifyQuery {
    pub token: String,
}

/// Mark an account's email verified (from the link in the verification email).
pub async fn verify(
    State(st): State<AppState>,
    Query(q): Query<VerifyQuery>,
) -> Result<Html<String>, AppError> {
    let account_id = oauth::decode_verify(&q.token, &st.cfg.jwt_secret)?;
    sqlx::query("UPDATE accounts SET email_verified = 1 WHERE id = ?")
        .bind(&account_id)
        .execute(&st.pool)
        .await?;
    Ok(Html(
        "<h3>Email verified</h3><p>You can return to RemoteHub and sign in.</p>".to_string(),
    ))
}

const SIGNED_IN_HTML: &str = "<h3>Signed in</h3><p>You can close this window and return to RemoteHub.</p>";

/// Append `key=value` to a URL, choosing `?` or `&` correctly.
fn append_param(url: &str, key: &str, value: &str) -> String {
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}{key}={}", urlencoding::encode(value))
}

#[derive(Serialize)]
pub struct MeResp {
    pub email: String,
    pub email_verified: bool,
}

/// The authenticated account's email + verification flag. Used by the desktop
/// app to display "signed in as …" after an OAuth login (where it never typed
/// an email).
pub async fn me(
    State(st): State<AppState>,
    AuthAccount(account): AuthAccount,
) -> Result<Json<MeResp>, AppError> {
    let row = sqlx::query_as::<_, (String, i64)>(
        "SELECT email, email_verified FROM accounts WHERE id = ?",
    )
    .bind(&account)
    .fetch_optional(&st.pool)
    .await?;
    let (email, verified) = row.ok_or(AppError::Unauthorized)?;
    Ok(Json(MeResp {
        email,
        email_verified: verified != 0,
    }))
}
