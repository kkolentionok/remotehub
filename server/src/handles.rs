//! SSH ID — public key handles.
//!
//! Each account may claim one public `handle` (like a username). Under it the
//! user publishes SSH **public** keys. The handle resolves at the apex root:
//!
//!   GET /<handle>            -> all keys
//!   GET /<handle>/<type>     -> keys of one type (ed25519 | rsa | ecdsa)
//!
//! Content is negotiated: a browser (`Accept: text/html`) gets a styled page;
//! anything else (curl/wget) gets `text/plain` in `authorized_keys` format, so:
//!
//!   curl -fs https://pingie.ru/<handle> >> ~/.ssh/authorized_keys
//!
//! Public keys are NOT secret and are stored server-side in plaintext (that is
//! the whole point). Private keys never leave the client keychain.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::Json;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::AuthAccount;
use crate::error::AppError;
use crate::AppState;

/// Root paths that can never be a handle — they are (or may become) real
/// endpoints/assets. Matched case-insensitively against the whole first
/// segment. Static axum routes already win in the router; this list is the
/// second guard (and keeps handles from *looking* like system paths).
const RESERVED: &[&str] = &[
    "v1", "health", "updates", "u", "api", "static", "assets", "oauth", "login", "register",
    "refresh", "vault", "me", "verify", "well-known", ".well-known", "favicon.ico", "robots.txt",
    "sitemap.xml", "admin", "www", "app", "about", "help", "support", "terms", "privacy",
];

/// Normalize + validate a candidate handle. Returns the canonical (lowercase)
/// form, or an error message describing why it's invalid.
pub fn normalize_handle(raw: &str) -> Result<String, &'static str> {
    let h = raw.trim().to_lowercase();
    let n = h.chars().count();
    if n < 2 || n > 32 {
        return Err("handle must be 2–32 characters");
    }
    if !h
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        return Err("handle may contain only a–z, 0–9, - and _");
    }
    let first = h.chars().next().unwrap();
    let last = h.chars().last().unwrap();
    if !first.is_ascii_alphanumeric() || !last.is_ascii_alphanumeric() {
        return Err("handle must start and end with a letter or digit");
    }
    if RESERVED.contains(&h.as_str()) {
        return Err("this handle is reserved");
    }
    Ok(h)
}

/// Infer the key-type tag from an SSH public-key line. `None` if it doesn't
/// look like a supported SSH public key.
pub fn key_type_of(pubkey: &str) -> Option<&'static str> {
    let p = pubkey.trim_start();
    if p.starts_with("ssh-ed25519") || p.starts_with("sk-ssh-ed25519@openssh.com") {
        Some("ed25519")
    } else if p.starts_with("ssh-rsa") {
        Some("rsa")
    } else if p.starts_with("ecdsa-sha2-") || p.starts_with("sk-ecdsa-sha2-") {
        Some("ecdsa")
    } else if p.starts_with("ssh-dss") {
        Some("dsa")
    } else {
        None
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

// ---- authed CRUD (called from the app) -----------------------------------

#[derive(Serialize)]
pub struct KeyItem {
    pub id: String,
    pub key_type: String,
    pub public_key: String,
    pub label: Option<String>,
}

#[derive(Serialize)]
pub struct HandleResp {
    /// The account's handle, or null if not claimed yet.
    pub handle: Option<String>,
    pub keys: Vec<KeyItem>,
}

/// GET /v1/handle — current handle + published keys for the logged-in account.
pub async fn handle_get(
    State(st): State<AppState>,
    AuthAccount(account): AuthAccount,
) -> Result<Json<HandleResp>, AppError> {
    let handle: Option<String> =
        sqlx::query_scalar("SELECT handle FROM handles WHERE account_id = ?")
            .bind(&account)
            .fetch_optional(&st.pool)
            .await?;

    let rows = sqlx::query_as::<_, (String, String, String, Option<String>)>(
        "SELECT id, key_type, public_key, label FROM handle_keys WHERE account_id = ? ORDER BY created_at",
    )
    .bind(&account)
    .fetch_all(&st.pool)
    .await?;

    let keys = rows
        .into_iter()
        .map(|(id, key_type, public_key, label)| KeyItem {
            id,
            key_type,
            public_key,
            label,
        })
        .collect();

    Ok(Json(HandleResp { handle, keys }))
}

#[derive(Deserialize)]
pub struct SetHandleReq {
    pub handle: String,
}

#[derive(Serialize)]
pub struct SetHandleResp {
    pub handle: String,
}

/// PUT /v1/handle — claim or rename the account's handle.
pub async fn handle_set(
    State(st): State<AppState>,
    AuthAccount(account): AuthAccount,
    Json(req): Json<SetHandleReq>,
) -> Result<Json<SetHandleResp>, AppError> {
    let handle = normalize_handle(&req.handle).map_err(|m| AppError::BadRequest(m.to_string()))?;

    // Taken by someone else?
    let owner: Option<String> =
        sqlx::query_scalar("SELECT account_id FROM handles WHERE handle = ? COLLATE NOCASE")
            .bind(&handle)
            .fetch_optional(&st.pool)
            .await?;
    if let Some(owner) = owner {
        if owner != account {
            return Err(AppError::BadRequest("this handle is taken".to_string()));
        }
    }

    // Upsert (one handle per account; account_id is the PK).
    sqlx::query(
        "INSERT INTO handles (account_id, handle, created_at) VALUES (?, ?, ?)
         ON CONFLICT(account_id) DO UPDATE SET handle = excluded.handle",
    )
    .bind(&account)
    .bind(&handle)
    .bind(Utc::now().to_rfc3339())
    .execute(&st.pool)
    .await?;

    Ok(Json(SetHandleResp { handle }))
}

#[derive(Deserialize)]
pub struct CheckQuery {
    pub handle: String,
}

#[derive(Serialize)]
pub struct CheckResp {
    pub available: bool,
    pub reason: Option<String>,
}

/// GET /v1/handle/check?handle=… — inline availability for the create form.
pub async fn handle_check(
    State(st): State<AppState>,
    AuthAccount(account): AuthAccount,
    Query(q): Query<CheckQuery>,
) -> Result<Json<CheckResp>, AppError> {
    let handle = match normalize_handle(&q.handle) {
        Ok(h) => h,
        Err(m) => {
            return Ok(Json(CheckResp {
                available: false,
                reason: Some(m.to_string()),
            }))
        }
    };
    let owner: Option<String> =
        sqlx::query_scalar("SELECT account_id FROM handles WHERE handle = ? COLLATE NOCASE")
            .bind(&handle)
            .fetch_optional(&st.pool)
            .await?;
    let available = match owner {
        None => true,
        Some(o) => o == account, // the account's own current handle counts as free
    };
    Ok(Json(CheckResp {
        available,
        reason: if available {
            None
        } else {
            Some("taken".to_string())
        },
    }))
}

#[derive(Deserialize)]
pub struct AddKeyReq {
    pub public_key: String,
    pub label: Option<String>,
}

#[derive(Serialize)]
pub struct AddKeyResp {
    pub id: String,
    pub key_type: String,
}

/// POST /v1/handle/keys — publish a public key under the account's handle.
pub async fn handle_add_key(
    State(st): State<AppState>,
    AuthAccount(account): AuthAccount,
    Json(req): Json<AddKeyReq>,
) -> Result<Json<AddKeyResp>, AppError> {
    let public_key = req.public_key.trim().to_string();
    let key_type = key_type_of(&public_key)
        .ok_or_else(|| AppError::BadRequest("not a recognized SSH public key".to_string()))?;
    if public_key.len() > 8192 {
        return Err(AppError::BadRequest("public key too long".to_string()));
    }
    let id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO handle_keys (id, account_id, key_type, public_key, label, created_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&account)
    .bind(key_type)
    .bind(&public_key)
    .bind(req.label.as_deref())
    .bind(Utc::now().to_rfc3339())
    .execute(&st.pool)
    .await?;

    Ok(Json(AddKeyResp {
        id,
        key_type: key_type.to_string(),
    }))
}

/// DELETE /v1/handle/keys/:id — unpublish a key (scoped to the account).
pub async fn handle_delete_key(
    State(st): State<AppState>,
    AuthAccount(account): AuthAccount,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    sqlx::query("DELETE FROM handle_keys WHERE id = ? AND account_id = ?")
        .bind(&id)
        .bind(&account)
        .execute(&st.pool)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct UpdateKeyReq {
    pub label: Option<String>,
}

/// PATCH /v1/handle/keys/:id — rename a key's label (scoped to the account).
pub async fn handle_update_key(
    State(st): State<AppState>,
    AuthAccount(account): AuthAccount,
    Path(id): Path<String>,
    Json(req): Json<UpdateKeyReq>,
) -> Result<StatusCode, AppError> {
    let label = req.label.as_deref().map(|s| s.trim()).filter(|s| !s.is_empty());
    sqlx::query("UPDATE handle_keys SET label = ? WHERE id = ? AND account_id = ?")
        .bind(label)
        .bind(&id)
        .bind(&account)
        .execute(&st.pool)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- public resolution (curl + browser) -----------------------------------

async fn resolve(
    st: &AppState,
    handle: &str,
    type_filter: Option<&str>,
) -> Result<Option<Vec<(String, String, Option<String>)>>, AppError> {
    // Validate shape first; reserved/invalid handles simply don't exist.
    let handle = match normalize_handle(handle) {
        Ok(h) => h,
        Err(_) => return Ok(None),
    };
    let account: Option<String> =
        sqlx::query_scalar("SELECT account_id FROM handles WHERE handle = ? COLLATE NOCASE")
            .bind(&handle)
            .fetch_optional(&st.pool)
            .await?;
    let Some(account) = account else {
        return Ok(None);
    };

    let rows = if let Some(t) = type_filter {
        sqlx::query_as::<_, (String, String, Option<String>)>(
            "SELECT key_type, public_key, label FROM handle_keys
             WHERE account_id = ? AND key_type = ? ORDER BY created_at",
        )
        .bind(&account)
        .bind(t)
        .fetch_all(&st.pool)
        .await?
    } else {
        sqlx::query_as::<_, (String, String, Option<String>)>(
            "SELECT key_type, public_key, label FROM handle_keys
             WHERE account_id = ? ORDER BY created_at",
        )
        .bind(&account)
        .fetch_all(&st.pool)
        .await?
    };
    Ok(Some(rows))
}

fn authorized_keys_text(keys: &[(String, String, Option<String>)]) -> String {
    let mut out = String::new();
    for (_t, pubkey, _label) in keys {
        out.push_str(pubkey.trim());
        out.push('\n');
    }
    out
}

fn wants_html(headers: &HeaderMap) -> bool {
    headers
        .get(axum::http::header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|a| a.contains("text/html"))
        .unwrap_or(false)
}

fn host_of(headers: &HeaderMap, st: &AppState) -> String {
    headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .or_else(|| {
            st.cfg
                .public_base_url
                .as_ref()
                .map(|u| u.trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/').to_string())
        })
        .unwrap_or_else(|| "pingie.ru".to_string())
}

fn plain(body: String, status: StatusCode) -> Response {
    (
        status,
        [
            (axum::http::header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (axum::http::header::CACHE_CONTROL, "no-store"),
        ],
        body,
    )
        .into_response()
}

/// GET /:handle
pub async fn public_handle(
    State(st): State<AppState>,
    Path(handle): Path<String>,
    headers: HeaderMap,
) -> Response {
    serve_public(&st, &handle, None, &headers).await
}

/// GET /:handle/:type
pub async fn public_handle_type(
    State(st): State<AppState>,
    Path((handle, ktype)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let t = ktype.to_lowercase();
    let filter = match t.as_str() {
        "ed25519" | "rsa" | "ecdsa" | "dsa" => Some(t),
        // Unknown type segment → treat as "no such thing".
        _ => return not_found(&headers),
    };
    serve_public(&st, &handle, filter.as_deref(), &headers).await
}

async fn serve_public(
    st: &AppState,
    handle: &str,
    type_filter: Option<&str>,
    headers: &HeaderMap,
) -> Response {
    match resolve(st, handle, type_filter).await {
        Ok(Some(keys)) => {
            if wants_html(headers) {
                let host = host_of(headers, st);
                Html(render_page(handle, &keys, type_filter, &host)).into_response()
            } else {
                plain(authorized_keys_text(&keys), StatusCode::OK)
            }
        }
        Ok(None) => not_found(headers),
        Err(e) => e.into_response(),
    }
}

fn not_found(headers: &HeaderMap) -> Response {
    if wants_html(headers) {
        (
            StatusCode::NOT_FOUND,
            Html(NOT_FOUND_HTML.to_string()),
        )
            .into_response()
    } else {
        plain("not found\n".to_string(), StatusCode::NOT_FOUND)
    }
}

// ---- public HTML (designer markup — PublicPage.dc.html) --------------------

/// Full-shell 404 (matches the designer's not-found frame: header + theme
/// toggle + create CTA), themed light/dark.
const NOT_FOUND_HTML: &str = PAGE_404;

/// Server-rendered public page, ported from the designer's `PublicPage.dc.html`
/// + `CommandBlock.dc.html`. Self-contained: class-based CSS with light/dark via
/// `data-theme` (system default + persisted toggle), vanilla JS for the key-type
/// dropdown (recomputes the curl command + context line), FAQ accordion, copy.
pub fn render_page(
    handle: &str,
    keys: &[(String, String, Option<String>)],
    type_filter: Option<&str>,
    host: &str,
) -> String {
    let total = keys.len();
    let count_of = |t: &str| keys.iter().filter(|(kt, _, _)| kt == t).count();
    let (ne, nr, nc) = (count_of("ed25519"), count_of("rsa"), count_of("ecdsa"));
    let mut present: Vec<&str> = Vec::new();
    if ne > 0 {
        present.push("ED25519");
    }
    if nr > 0 {
        present.push("RSA");
    }
    if nc > 0 {
        present.push("ECDSA");
    }
    let types_str = present.join(", ");
    let is_empty = total == 0;
    let init_type = type_filter.unwrap_or("all");

    let data = serde_json::json!({
        "host": host,
        "handle": handle,
        "initType": init_type,
        "typesStr": types_str,
        "counts": { "all": total, "ed25519": ne, "rsa": nr, "ecdsa": nc },
    })
    .to_string();

    let filter_badge = match type_filter {
        Some(t) => format!(
            "<span class=\"fbadge\">{}</span>",
            esc(&t.to_uppercase())
        ),
        None => String::new(),
    };

    let main = if is_empty {
        format!(
            r#"<div class="emptycard">
  <div class="emptyicon"><svg width="19" height="19" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="7.5" cy="15.5" r="5.5"></circle><path d="m21 2-9.6 9.6"></path><path d="m15.5 7.5 3 3L22 7l-3-3"></path></svg></div>
  <div class="emptytitle">У <span class="mono acc">@{handle}</span> пока нет опубликованных ключей</div>
  <div class="emptysub">Загляните позже или свяжитесь с владельцем.</div>
</div>"#,
            handle = esc(handle)
        )
    } else {
        CMD_BLOCK.to_string()
    };

    PAGE_TEMPLATE
        .replace("__DATA__", &data)
        .replace("__FBADGE__", &filter_badge)
        .replace("__MAIN__", &main)
        .replace("__HANDLE__", &esc(handle))
}

const CMD_BLOCK: &str = r#"<div class="cmd">
  <div class="cmdbar">
    <div class="drop">
      <button class="dropbtn" onclick="tgl(this,'fmt')" type="button">
        <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="4 17 10 11 4 5"></polyline><line x1="12" y1="19" x2="20" y2="19"></line></svg>
        <span>Shell command</span>
        <svg class="chev" width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m6 9 6 6 6-6"></path></svg>
      </button>
      <div class="menu" data-menu="fmt" style="left:0">
        <div class="mi on"><span>Shell command</span><svg class="chk" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="var(--accent)" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6 9 17l-5-5"></path></svg></div>
        <div class="mihint">Другие форматы — скоро</div>
      </div>
    </div>
    <div class="drop">
      <button class="dropbtn" onclick="tgl(this,'typ')" type="button">
        <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="7.5" cy="15.5" r="5.5"></circle><path d="m21 2-9.6 9.6"></path><path d="m15.5 7.5 3 3L22 7l-3-3"></path></svg>
        <span class="mono" id="typlabel">All</span>
        <svg class="chev" width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m6 9 6 6 6-6"></path></svg>
      </button>
      <div class="menu" data-menu="typ" style="right:0">
        <button class="mi" type="button" data-t="all" onclick="pick('all')"><span>All</span><svg class="chk" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="var(--accent)" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6 9 17l-5-5"></path></svg></button>
        <button class="mi" type="button" data-t="ed25519" onclick="pick('ed25519')"><span>ED25519</span><svg class="chk" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="var(--accent)" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6 9 17l-5-5"></path></svg></button>
        <button class="mi" type="button" data-t="rsa" onclick="pick('rsa')"><span>RSA</span><svg class="chk" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="var(--accent)" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6 9 17l-5-5"></path></svg></button>
        <button class="mi" type="button" data-t="ecdsa" onclick="pick('ecdsa')"><span>ECDSA</span><svg class="chk" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="var(--accent)" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6 9 17l-5-5"></path></svg></button>
      </div>
    </div>
  </div>
  <div class="cmdbody">
    <div class="cmdcode"><span class="kw">curl</span><span class="dim"> -fs </span><span id="ub"></span><span class="acc" id="us"></span><span class="dim">  &gt;&gt;  </span><span class="dim2">~/.ssh/authorized_keys</span></div>
    <button class="copybtn" onclick="cp()" title="Скопировать" type="button"><span id="cpi"><svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2"></rect><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path></svg></span></button>
  </div>
</div>
<p class="ctx" id="ctx"></p>
<div class="stepsWrap">
  <div class="kicker">Дальнейшие шаги</div>
  <div class="steps">
    <div class="step"><span class="stepnum">1</span><span class="steptext">Скопируйте ключи в <span class="kbd">~/.ssh/authorized_keys</span> одной командой выше.</span></div>
    <div class="step dim55"><span class="stepnum2">2</span><span class="steptext">В Pingie выберите <span class="tx2">«SSH ID»</span> как метод аутентификации хоста. <span class="soon">скоро</span></span></div>
  </div>
</div>"#;

const PAGE_404: &str = r####"<!doctype html><html lang="ru"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>SSH ID · Pingie</title>
<style>
:root{--accent:#4c8eff;--canvas:#0a0a0d;--surface:#16161a;--bd:rgba(255,255,255,.09);--bd2:rgba(255,255,255,.06);--hover:rgba(255,255,255,.045);--tx:rgba(255,255,255,.92);--tx2:rgba(255,255,255,.6);--tx3:rgba(255,255,255,.4);--sans:-apple-system,BlinkMacSystemFont,"Segoe UI",system-ui,sans-serif}
:root[data-theme=light]{--canvas:#f7f8fa;--surface:#fff;--bd:rgba(0,0,0,.11);--bd2:rgba(0,0,0,.07);--hover:rgba(0,0,0,.035);--tx:rgba(10,10,13,.92);--tx2:rgba(10,10,13,.6);--tx3:rgba(10,10,13,.42)}
*{box-sizing:border-box}
body{margin:0;font-family:var(--sans);background:var(--canvas);color:var(--tx);min-height:100vh}
.col{max-width:680px;margin:0 auto;padding:0 28px}
header{display:flex;align-items:center;justify-content:space-between;gap:16px;padding:22px 0 0}
.brand{display:flex;align-items:center;gap:10px}
.brand b{font-size:15px;font-weight:600;letter-spacing:-.01em}
.sep{width:1px;height:14px;background:var(--bd)}
.kick{font-size:10.5px;font-weight:500;letter-spacing:.08em;text-transform:uppercase;color:var(--tx3)}
.toggle{width:32px;height:32px;border:1px solid var(--bd);border-radius:6px;background:transparent;color:var(--tx2);cursor:pointer;display:inline-flex;align-items:center;justify-content:center}
.toggle:hover{background:var(--hover);color:var(--tx)}
.wrap404{display:flex;flex-direction:column;align-items:center;text-align:center;padding:96px 0 90px}
.code{font-family:ui-monospace,monospace;font-size:12px;color:var(--tx3);margin-bottom:18px;letter-spacing:.04em}
h1{font-size:22px;font-weight:600;margin:0 0 10px;letter-spacing:-.01em}
.sub{font-size:14px;color:var(--tx2);margin:0 0 22px;max-width:360px;line-height:1.55}
.cta{display:inline-flex;align-items:center;height:36px;padding:0 16px;border-radius:6px;background:var(--accent);color:#fff;text-decoration:none;font-size:13px;font-weight:500}
.cta:hover{background:#5b99ff}
</style></head><body>
<div class="col">
  <header>
    <div class="brand">🔑 <b>Pingie</b><span class="sep"></span><span class="kick">SSH ID</span></div>
    <button class="toggle" onclick="tt()" title="Тема" type="button" id="tg">◐</button>
  </header>
  <div class="wrap404">
    <div class="code">404</div>
    <h1>Такого SSH ID нет</h1>
    <p class="sub">Проверьте адрес — возможно, в хэндле опечатка. Или создайте собственный SSH ID в Pingie.</p>
    <a class="cta" href="https://pingie.ru">Создать SSH ID</a>
  </div>
</div>
<script>
(function(){var r=document.documentElement,K='pingie-theme',s=localStorage.getItem(K);
if(s)r.setAttribute('data-theme',s);else if(matchMedia('(prefers-color-scheme: light)').matches)r.setAttribute('data-theme','light');
window.tt=function(){var c=r.getAttribute('data-theme')==='light'?'dark':'light';r.setAttribute('data-theme',c);localStorage.setItem(K,c);};})();
</script>
</body></html>"####;

const PAGE_TEMPLATE: &str = r####"<!doctype html><html lang="ru"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>SSH-ключи @__HANDLE__ · Pingie</title>
<style>
:root{--accent:#4c8eff;--accentH:#5b99ff;--danger:#f87171;--success:#4ade80;
--canvas:#0a0a0d;--surface:#16161a;--elevated:#1f1f24;--hover:rgba(255,255,255,.045);
--tx:rgba(255,255,255,.92);--tx2:rgba(255,255,255,.6);--tx3:rgba(255,255,255,.4);
--bd:rgba(255,255,255,.09);--bd2:rgba(255,255,255,.06);--shadow:0 10px 28px rgba(0,0,0,.5);
--mono:"JetBrains Mono",ui-monospace,"SF Mono","Cascadia Code",Menlo,Consolas,monospace;
--sans:-apple-system,BlinkMacSystemFont,"Segoe UI",system-ui,sans-serif}
:root[data-theme=light]{--canvas:#f7f8fa;--surface:#fff;--elevated:#fff;--hover:rgba(0,0,0,.035);
--tx:rgba(10,10,13,.92);--tx2:rgba(10,10,13,.6);--tx3:rgba(10,10,13,.42);
--bd:rgba(0,0,0,.11);--bd2:rgba(0,0,0,.07);--shadow:0 8px 24px rgba(0,0,0,.14)}
*{box-sizing:border-box}
body{margin:0;font-family:var(--sans);background:var(--canvas);color:var(--tx);min-height:100vh}
.mono{font-family:var(--mono)}.acc{color:var(--accent)}.tx2{color:var(--tx2)}.dim55{opacity:.55}
.col{max-width:680px;margin:0 auto;padding:0 28px}
@media (max-width:640px){.col{padding:0 18px}}
header{display:flex;align-items:center;justify-content:space-between;gap:16px;padding:22px 0 0}
.brand{display:flex;align-items:center;gap:10px;min-width:0}
.brand b{font-size:15px;font-weight:600;letter-spacing:-.01em}
.sep{width:1px;height:14px;background:var(--bd);flex:none}
.kick{font-size:10.5px;font-weight:500;letter-spacing:.08em;text-transform:uppercase;color:var(--tx3)}
.hactions{display:flex;align-items:center;gap:8px;flex:none}
.toggle{width:32px;height:32px;border:1px solid var(--bd);border-radius:6px;background:transparent;color:var(--tx2);cursor:pointer;display:inline-flex;align-items:center;justify-content:center}
.toggle:hover{background:var(--hover);color:var(--tx)}
.createlink{display:inline-flex;align-items:center;height:32px;padding:0 12px;border:1px solid var(--bd);border-radius:6px;background:transparent;color:var(--tx);text-decoration:none;font-size:13px;font-weight:500;white-space:nowrap}
.createlink:hover{background:var(--hover)}
.content{padding:44px 0 0}
.titleRow{display:flex;align-items:center;gap:10px;flex-wrap:wrap;margin-bottom:12px}
h1{font-size:22px;font-weight:600;margin:0;letter-spacing:-.01em}
h1 .at{font-family:var(--mono);font-weight:500;color:var(--accent)}
.fbadge{font-family:var(--mono);font-size:11px;font-weight:500;letter-spacing:.03em;color:var(--accent);background:rgba(76,142,255,.13);border:1px solid rgba(76,142,255,.25);padding:3px 8px;border-radius:5px}
.sub{font-size:15px;color:var(--tx2);margin:0 0 26px;line-height:1.55;max-width:560px}
.cmd{border:1px solid var(--bd);border-radius:8px;background:var(--surface);position:relative}
.cmdbar{display:flex;align-items:center;justify-content:space-between;gap:8px;padding:7px 8px;border-bottom:1px solid var(--bd2)}
.drop{position:relative}
.dropbtn{display:inline-flex;align-items:center;gap:6px;height:28px;padding:0 8px;border:1px solid var(--bd);border-radius:6px;background:transparent;color:var(--tx2);cursor:pointer;font-family:var(--sans);font-size:12px}
.dropbtn:hover{background:var(--hover);color:var(--tx)}
.chev{opacity:.6}
.menu{display:none;position:absolute;top:calc(100% + 6px);min-width:150px;background:var(--elevated);border:1px solid var(--bd);border-radius:8px;box-shadow:var(--shadow);padding:4px;z-index:30}
.menu.open{display:block}
.mi{display:flex;align-items:center;justify-content:space-between;gap:12px;width:100%;padding:7px 10px;border:none;border-radius:5px;background:transparent;color:var(--tx);cursor:pointer;font-family:var(--sans);font-size:13px;text-align:left}
.mi:hover{background:var(--hover)}
.mi .chk{opacity:0}
.mi.on .chk{opacity:1}.mi.on{background:var(--hover)}
.mihint{padding:8px 10px 6px;color:var(--tx3);font-size:11px}
.cmdbody{position:relative}
.cmdcode{font-family:var(--mono);font-size:13px;line-height:1.5;padding:15px 48px 15px 15px;overflow-x:auto;white-space:nowrap}
.cmdcode .kw{color:var(--accent);font-weight:500}.cmdcode .dim{color:var(--tx3)}.cmdcode .dim2{color:var(--tx2)}
.copybtn{position:absolute;top:9px;right:9px;display:inline-flex;align-items:center;gap:6px;height:28px;padding:0 9px;border:1px solid var(--bd);border-radius:6px;background:var(--elevated);color:var(--tx2);cursor:pointer;font-family:var(--sans);font-size:12px}
.copybtn:hover{background:var(--hover);color:var(--tx)}
.copybtn .ok{color:var(--success);display:inline-flex;align-items:center;gap:5px}
.ctx{font-size:12px;color:var(--tx3);margin:11px 2px 0}
.stepsWrap{margin-top:38px}
.kicker{font-size:11px;font-weight:500;letter-spacing:.07em;text-transform:uppercase;color:var(--tx3);margin-bottom:16px}
.steps{display:flex;flex-direction:column;gap:16px}
.step{display:flex;gap:13px;align-items:flex-start}
.stepnum,.stepnum2{flex:none;width:22px;height:22px;border-radius:50%;font-size:12px;font-weight:600;display:inline-flex;align-items:center;justify-content:center;font-family:var(--mono)}
.stepnum{background:rgba(76,142,255,.14);color:var(--accent)}
.stepnum2{background:var(--hover);color:var(--tx2)}
.steptext{font-size:14px;color:var(--tx);line-height:1.5;padding-top:1px}
.kbd{font-family:var(--mono);font-size:12.5px;color:var(--tx2)}
.soon{font-family:var(--mono);font-size:10px;letter-spacing:.04em;text-transform:uppercase;color:var(--tx3);border:1px solid var(--bd);border-radius:4px;padding:1px 5px;margin-left:4px}
.emptycard{border:1px solid var(--bd);border-radius:12px;background:var(--surface);padding:40px 24px;display:flex;flex-direction:column;align-items:center;text-align:center}
.emptyicon{width:40px;height:40px;border-radius:10px;background:var(--hover);display:inline-flex;align-items:center;justify-content:center;color:var(--tx3);margin-bottom:14px}
.emptytitle{font-size:15px;color:var(--tx);margin-bottom:6px}
.emptysub{font-size:13px;color:var(--tx3)}
.faq{margin-top:46px}
.faqlist{border-top:1px solid var(--bd2)}
.faqitem{border-bottom:1px solid var(--bd2)}
.faqbtn{display:flex;align-items:center;justify-content:space-between;gap:16px;width:100%;padding:16px 2px;border:none;background:transparent;color:var(--tx);cursor:pointer;text-align:left;font-family:var(--sans);font-size:14.5px;font-weight:500}
.faqsign{flex:none;color:var(--tx3);font-size:18px;line-height:1;width:16px;text-align:center}
.faqans{margin:0;padding:0 2px 18px;font-size:14px;color:var(--tx2);line-height:1.6;max-width:600px;display:none}
.faqitem.open .faqans{display:block}
footer{margin-top:52px;padding:22px 0 30px;border-top:1px solid var(--bd2);display:flex;align-items:center;gap:6px}
footer span{font-size:12px;color:var(--tx3)}
footer a{font-size:12px;color:var(--tx2);text-decoration:none;font-weight:500}
footer a:hover{color:var(--accent)}
</style></head>
<body>
<div class="col">
  <header>
    <div class="brand">🔑 <b>Pingie</b><span class="sep"></span><span class="kick">SSH ID</span></div>
    <div class="hactions">
      <button class="toggle" onclick="tt()" title="Тема" type="button">◐</button>
      <a class="createlink" href="https://pingie.ru">Создать свой SSH ID</a>
    </div>
  </header>
  <div class="content">
    <div class="titleRow">
      <h1>SSH-ключи <span class="at">@__HANDLE__</span></h1>
      __FBADGE__
    </div>
    <p class="sub">Единое место для публичных ключей — работают как один распределённый ключ.</p>
    __MAIN__
    <div class="faq">
      <div class="kicker">Частые вопросы</div>
      <div class="faqlist">
        <div class="faqitem open"><button class="faqbtn" onclick="fq(this)" type="button"><span>Откуда эти ключи?</span><span class="faqsign">–</span></button><p class="faqans">Это публичные SSH-ключи, которые @__HANDLE__ добавил в свой SSH ID в приложении Pingie. Каждый ключ привязан к конкретному устройству владельца.</p></div>
        <div class="faqitem"><button class="faqbtn" onclick="fq(this)" type="button"><span>Почему они доступны публично?</span><span class="faqsign">+</span></button><p class="faqans">Публичный ключ по своей природе не секрет — он и предназначен для передачи серверам. Приватные ключи никогда не покидают устройство и системный keychain.</p></div>
        <div class="faqitem"><button class="faqbtn" onclick="fq(this)" type="button"><span>Почему им можно доверять?</span><span class="faqsign">+</span></button><p class="faqans">Ключи публикует сам владелец через приложение. Доверие остаётся за вами: вы решаете, какой SSH ID добавить в authorized_keys своего сервера.</p></div>
        <div class="faqitem"><button class="faqbtn" onclick="fq(this)" type="button"><span>Какой вариант самый безопасный?</span><span class="faqsign">+</span></button><p class="faqans">Ed25519 — современный, компактный и быстрый алгоритм, рекомендуемый по умолчанию. Используйте фильтр ED25519, чтобы принять только его.</p></div>
      </div>
    </div>
  </div>
  <footer><span>Powered by</span><a href="https://pingie.ru">Pingie</a><span>· 2026</span></footer>
</div>
<script>
var D=__DATA__;
(function(){
  var r=document.documentElement,K='pingie-theme',s=localStorage.getItem(K);
  if(s)r.setAttribute('data-theme',s);
  else if(matchMedia('(prefers-color-scheme: light)').matches)r.setAttribute('data-theme','light');
  window.tt=function(){var c=r.getAttribute('data-theme')==='light'?'dark':'light';r.setAttribute('data-theme',c);localStorage.setItem(K,c);};
  window.fq=function(b){var it=b.parentNode,was=it.classList.contains('open');
    var all=document.querySelectorAll('.faqitem');for(var i=0;i<all.length;i++){all[i].classList.remove('open');all[i].querySelector('.faqsign').textContent='+';}
    if(!was){it.classList.add('open');it.querySelector('.faqsign').textContent='–';}};
  window.tgl=function(btn,name){var m=btn.parentNode.querySelector('[data-menu="'+name+'"]');var open=m.classList.contains('open');
    var ms=document.querySelectorAll('.menu');for(var i=0;i<ms.length;i++)ms[i].classList.remove('open');
    if(!open)m.classList.add('open');};
  document.addEventListener('click',function(e){if(!e.target.closest('.drop')){var ms=document.querySelectorAll('.menu');for(var i=0;i<ms.length;i++)ms[i].classList.remove('open');}});
  var LBL={all:'All',ed25519:'ED25519',rsa:'RSA',ecdsa:'ECDSA'};
  function cmd(t){var suf=t==='all'?'':'/'+t;return 'curl -fs https://'+D.host+'/'+D.handle+suf+' >> ~/.ssh/authorized_keys';}
  function ctxText(t){if(t==='all'){var n=D.counts.all;return n===0?'Пока нет ключей':('Ключей: '+n+(D.typesStr?' · '+D.typesStr:''));}
    return 'Только '+LBL[t]+' · '+(D.counts[t]||0)+' шт.';}
  window.pick=function(t){
    var us=document.getElementById('us');if(us)us.textContent=t==='all'?'':'/'+t;
    var tl=document.getElementById('typlabel');if(tl)tl.textContent=LBL[t];
    var ctx=document.getElementById('ctx');if(ctx)ctx.textContent=ctxText(t);
    var mis=document.querySelectorAll('.mi[data-t]');for(var i=0;i<mis.length;i++)mis[i].classList.toggle('on',mis[i].getAttribute('data-t')===t);
    window.__t=t;
    var ms=document.querySelectorAll('.menu');for(var j=0;j<ms.length;j++)ms[j].classList.remove('open');
  };
  window.cp=function(){var t=window.__t||'all';var full=cmd(t);
    try{navigator.clipboard.writeText(full);}catch(e){}
    var i=document.getElementById('cpi');if(!i)return;var h=i.innerHTML;
    i.innerHTML='<span class="ok"><svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6 9 17l-5-5"></path></svg>Скопировано</span>';
    setTimeout(function(){i.innerHTML=h;},1600);};
  var ub=document.getElementById('ub');if(ub)ub.textContent='https://'+D.host+'/'+D.handle;
  if(document.getElementById('us'))pick(D.initType);
})();
</script>
</body></html>"####;
