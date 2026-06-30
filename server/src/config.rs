//! Runtime configuration, read from environment variables.
//!
//! `JWT_SECRET` is the only required variable; everything else has a sane
//! default suitable for a single-container deployment. Yandex OAuth /
//! email-verification settings (slice 3a-2) will be added here.

use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    /// `host:port` to bind the HTTP listener to.
    pub bind_addr: String,
    /// Filesystem path to the SQLite database (created if missing).
    pub db_path: String,
    /// Secret used to sign/verify the bearer JWTs. Must be long + random.
    pub jwt_secret: String,
    /// Token lifetime in hours.
    pub token_ttl_hours: i64,
    /// Refresh-token lifetime in days. Refresh tokens let the client silently
    /// renew an expired access token (no "session expired" prompt). Default is
    /// effectively "never" for a personal tool.
    pub refresh_ttl_days: i64,
    /// Reject vault blobs larger than this (defense against abuse).
    pub max_blob_bytes: usize,
    /// Public base URL (e.g. `https://pingie.ru`) — used to build the Yandex
    /// OAuth `redirect_uri` and email-verification links. Required for OAuth.
    pub public_base_url: Option<String>,
    /// Yandex OAuth application credentials. OAuth is enabled only when both
    /// these and `public_base_url` are set.
    pub yandex_client_id: Option<String>,
    pub yandex_client_secret: Option<String>,
    /// When true, email/password accounts must verify their email before they
    /// can log in. Default false (the dev flow stays frictionless).
    pub require_email_verification: bool,
}

impl Config {
    /// True when all three Yandex/OAuth settings are present.
    pub fn oauth_enabled(&self) -> bool {
        self.public_base_url.is_some()
            && self.yandex_client_id.is_some()
            && self.yandex_client_secret.is_some()
    }
    pub fn from_env() -> Result<Self, String> {
        let jwt_secret =
            env::var("JWT_SECRET").map_err(|_| "JWT_SECRET must be set".to_string())?;
        if jwt_secret.len() < 16 {
            return Err("JWT_SECRET must be at least 16 characters".to_string());
        }
        Ok(Self {
            bind_addr: env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string()),
            db_path: env::var("DB_PATH").unwrap_or_else(|_| "/data/sync.db".to_string()),
            jwt_secret,
            token_ttl_hours: env::var("TOKEN_TTL_HOURS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(168),
            refresh_ttl_days: env::var("REFRESH_TTL_DAYS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(3650),
            max_blob_bytes: env::var("MAX_BLOB_BYTES")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(5 * 1024 * 1024),
            public_base_url: env::var("PUBLIC_BASE_URL")
                .ok()
                .map(|s| s.trim().trim_end_matches('/').to_string())
                .filter(|s| !s.is_empty()),
            yandex_client_id: env::var("YANDEX_CLIENT_ID").ok().filter(|s| !s.is_empty()),
            yandex_client_secret: env::var("YANDEX_CLIENT_SECRET").ok().filter(|s| !s.is_empty()),
            require_email_verification: env::var("REQUIRE_EMAIL_VERIFICATION")
                .map(|s| s == "1" || s.eq_ignore_ascii_case("true"))
                .unwrap_or(false),
        })
    }
}
