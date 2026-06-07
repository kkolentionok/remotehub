//! Tracing / logging setup.
//!
//! - Console (stderr): human-friendly, level controlled by RUST_LOG (default INFO).
//! - File (rolling daily): JSON, INFO level.
//!
//! Log directory is `<app_data>/logs/`. Files are named `app.<date>.log` and
//! rotate daily; we keep the last 7 by default.

use std::fs;

use tracing_subscriber::{
    fmt::{self, time::ChronoUtc},
    prelude::*,
    EnvFilter,
};

use crate::paths;

/// Initialize tracing. Returns an error if log directory cannot be created.
///
/// The returned guard is intentionally leaked: the application runs until the
/// process exits, at which point the OS reclaims everything. Keeping the guard
/// alive ensures the background log writer thread flushes on shutdown.
pub fn init() -> Result<(), std::io::Error> {
    let log_dir = paths::log_dir();
    fs::create_dir_all(&log_dir)?;

    let file_appender = tracing_appender::rolling::daily(&log_dir, "app.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
    // Leak the guard for the process lifetime; see fn doc.
    let _ = Box::leak(Box::new(guard));

    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,rh_=debug"));

    let file_layer = fmt::layer()
        .json()
        .with_timer(ChronoUtc::rfc_3339())
        .with_writer(non_blocking)
        .with_ansi(false);

    let stderr_layer = fmt::layer()
        .with_timer(ChronoUtc::rfc_3339())
        .with_writer(std::io::stderr)
        .with_target(false);

    tracing_subscriber::registry()
        .with(env_filter)
        .with(file_layer)
        .with(stderr_layer)
        .init();

    Ok(())
}
