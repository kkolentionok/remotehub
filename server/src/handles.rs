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

// ---- HTML page (functional; designer's markup can replace this later) ------

const NOT_FOUND_HTML: &str = r#"<!doctype html><html><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>SSH ID · Pingie</title><style>
:root{color-scheme:light dark}
body{font-family:system-ui,-apple-system,Segoe UI,sans-serif;background:#0a0a0d;color:rgba(255,255,255,.92);
display:flex;align-items:center;justify-content:center;height:100vh;margin:0;text-align:center}
@media (prefers-color-scheme: light){body{background:#fff;color:#0a0a0d}}
a{color:#4c8eff;text-decoration:none}
</style></head><body><div><h2 style="font-weight:600">SSH ID не найден</h2>
<p style="opacity:.6">Такого хэндла нет. <a href="https://pingie.ru">Создать свой →</a></p></div></body></html>"#;

/// Server-rendered public page. Self-contained (inline CSS/JS): system-default
/// theme with a manual light/dark toggle (persisted in localStorage). Styled in
/// the Pingie language (accent #4c8eff, mono for keys/commands). Intended to be
/// superseded by the designer's markup, but fully functional as-is.
pub fn render_page(
    handle: &str,
    keys: &[(String, String, Option<String>)],
    type_filter: Option<&str>,
    host: &str,
) -> String {
    let base = format!("https://{host}/{handle}");
    let url = match type_filter {
        Some(t) => format!("{base}/{t}"),
        None => base.clone(),
    };
    let curl = format!("curl -fs {url} >> ~/.ssh/authorized_keys");

    let key_rows = if keys.is_empty() {
        "<p class=\"muted\">У этого SSH ID пока нет опубликованных ключей.</p>".to_string()
    } else {
        keys.iter()
            .map(|(t, pubkey, label)| {
                let lbl = label
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .map(|s| format!("<span class=\"lbl\">{}</span>", esc(s)))
                    .unwrap_or_default();
                format!(
                    "<div class=\"key\"><span class=\"badge\">{}</span>{}<code>{}</code></div>",
                    esc(&t.to_uppercase()),
                    lbl,
                    esc(pubkey.trim())
                )
            })
            .collect::<Vec<_>>()
            .join("")
    };

    format!(
        r####"<!doctype html><html lang="ru"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>SSH-ключи @{h} · Pingie</title>
<style>
:root{{
  --accent:#4c8eff; --bg:#0a0a0d; --surface:#16161a; --border:rgba(255,255,255,.10);
  --fg:rgba(255,255,255,.92); --fg2:rgba(255,255,255,.6); --fg3:rgba(255,255,255,.4);
}}
[data-theme="light"]{{
  --bg:#f7f8fa; --surface:#ffffff; --border:rgba(0,0,0,.10);
  --fg:rgba(0,0,0,.92); --fg2:rgba(0,0,0,.6); --fg3:rgba(0,0,0,.45);
}}
*{{box-sizing:border-box}}
body{{font-family:system-ui,-apple-system,Segoe UI,Roboto,sans-serif;background:var(--bg);color:var(--fg);
margin:0;line-height:1.5}}
.wrap{{max-width:720px;margin:0 auto;padding:32px 20px 64px}}
.top{{display:flex;align-items:center;justify-content:space-between;margin-bottom:40px}}
.brand{{display:flex;align-items:center;gap:8px;color:var(--fg2);font-size:13px}}
.brand b{{color:var(--fg);font-weight:600}}
.toggle{{background:var(--surface);border:1px solid var(--border);color:var(--fg2);border-radius:6px;
padding:6px 10px;font-size:13px;cursor:pointer}}
h1{{font-size:22px;font-weight:600;margin:0 0 6px}}
h1 .at{{color:var(--accent);font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace}}
.sub{{color:var(--fg2);margin:0 0 28px}}
.card{{background:var(--surface);border:1px solid var(--border);border-radius:12px;padding:16px;margin-bottom:16px}}
.cmd{{display:flex;align-items:center;gap:12px}}
.cmd code{{font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;font-size:13px;
overflow-x:auto;white-space:nowrap;flex:1}}
.cmd code .kw{{color:var(--accent)}}
.copy{{background:transparent;border:1px solid var(--border);color:var(--fg2);border-radius:6px;
padding:6px 10px;font-size:12px;cursor:pointer;white-space:nowrap}}
.copy:hover{{color:var(--fg)}}
.sect{{text-transform:uppercase;letter-spacing:.06em;font-size:11px;color:var(--fg3);font-weight:600;
margin:28px 0 10px}}
.key{{display:flex;align-items:center;gap:10px;padding:8px 0;border-top:1px solid var(--border);flex-wrap:wrap}}
.key:first-child{{border-top:none}}
.badge{{font-family:ui-monospace,monospace;font-size:11px;color:var(--accent);
border:1px solid var(--border);border-radius:4px;padding:2px 6px}}
.lbl{{color:var(--fg2);font-size:13px}}
.key code{{font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;font-size:12px;color:var(--fg2);
overflow:hidden;text-overflow:ellipsis;white-space:nowrap;flex:1;min-width:120px}}
.muted{{color:var(--fg3)}}
.foot{{margin-top:40px;color:var(--fg3);font-size:12px}}
.foot a{{color:var(--accent);text-decoration:none}}
</style></head>
<body>
<div class="wrap">
  <div class="top">
    <div class="brand">🔑 <b>Pingie</b> · SSH ID</div>
    <button class="toggle" id="tg">Тема</button>
  </div>
  <h1>SSH-ключи <span class="at">@{h}</span></h1>
  <p class="sub">Единое место для публичных ключей — работают как один распределённый ключ.</p>
  <div class="card">
    <div class="cmd">
      <code><span class="kw">curl</span> -fs {url} &gt;&gt; ~/.ssh/authorized_keys</code>
      <button class="copy" id="cp" data-cmd="{curl_attr}">Копировать</button>
    </div>
  </div>
  <div class="sect">Опубликованные ключи</div>
  {key_rows}
  <div class="sect">Дальше</div>
  <ol style="color:var(--fg2);padding-left:20px;margin:0">
    <li>Скопируй ключи в <code>~/.ssh/authorized_keys</code> одной командой.</li>
    <li>Это публичные ключи — приватные никогда не покидают устройство.</li>
  </ol>
  <div class="foot">Powered by <a href="https://pingie.ru">Pingie</a></div>
</div>
<script>
(function(){{
  var root=document.documentElement, K='pingie-theme';
  var saved=localStorage.getItem(K);
  if(saved) root.setAttribute('data-theme',saved);
  else if(matchMedia('(prefers-color-scheme: light)').matches) root.setAttribute('data-theme','light');
  document.getElementById('tg').onclick=function(){{
    var cur=root.getAttribute('data-theme')==='light'?'dark':'light';
    root.setAttribute('data-theme',cur); localStorage.setItem(K,cur);
  }};
  var cp=document.getElementById('cp');
  cp.onclick=function(){{navigator.clipboard.writeText(cp.dataset.cmd).then(function(){{
    var t=cp.textContent; cp.textContent='Скопировано'; setTimeout(function(){{cp.textContent=t;}},1200);
  }});}};
}})();
</script>
</body></html>"####,
        h = esc(handle),
        url = esc(&url),
        curl_attr = esc(&curl),
        key_rows = key_rows,
    )
}
