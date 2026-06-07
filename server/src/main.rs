//! rh-sync-server — RemoteHub's self-hostable sync backend.
//!
//! An authenticated, multi-tenant, versioned **opaque-blob** store. It holds
//! only the client's E2E-encrypted vault envelope per account; it never sees
//! plaintext and shares no code with the RemoteHub app. Put it behind a TLS
//! terminator (Caddy / Cloudflare / nginx) in production — see README.

mod auth;
mod config;
mod db;
mod error;
mod oauth;
mod routes;

use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;
use sqlx::SqlitePool;

use crate::config::Config;

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub cfg: Arc<Config>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cfg = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("configuration error: {e}");
            std::process::exit(1);
        }
    };

    if let Some(dir) = std::path::Path::new(&cfg.db_path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }

    let pool = match db::connect(&cfg.db_path).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("database error: {e}");
            std::process::exit(1);
        }
    };

    let bind_addr = cfg.bind_addr.clone();
    let state = AppState {
        pool,
        cfg: Arc::new(cfg),
    };

    let app = Router::new()
        .route("/health", get(routes::health))
        .route("/v1/register", post(routes::register))
        .route("/v1/login", post(routes::login))
        .route("/v1/vault", get(routes::get_vault).put(routes::put_vault))
        .route("/v1/oauth/yandex/start", get(routes::oauth_yandex_start))
        .route("/v1/oauth/yandex/callback", get(routes::oauth_yandex_callback))
        .route("/v1/verify", get(routes::verify))
        .with_state(state);

    let listener = match tokio::net::TcpListener::bind(&bind_addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("failed to bind {bind_addr}: {e}");
            std::process::exit(1);
        }
    };
    tracing::info!("rh-sync-server listening on {bind_addr}");

    if let Err(e) = axum::serve(listener, app).await {
        eprintln!("server error: {e}");
        std::process::exit(1);
    }
}
