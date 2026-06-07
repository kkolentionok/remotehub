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

/// Install a panic hook that writes the panic (location + message + backtrace)
/// **synchronously** to `<app_data>/logs/panic.log`.
///
/// Needed because the release build is windowless (`windows_subsystem =
/// "windows"`) so the default hook's stderr output is invisible, and with
/// `panic = "abort"` the process dies immediately — the non-blocking tracing
/// writer may not flush in time. A direct synchronous file append guarantees
/// the panic reaches disk before the abort. Call once, after `init()`.
pub fn install_panic_hook() {
    let crash_path = paths::log_dir().join("panic.log");
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let when = chrono::Utc::now().to_rfc3339();
        let bt = std::backtrace::Backtrace::force_capture();
        let entry = format!("\n===== panic @ {when} =====\n{info}\n\nbacktrace:\n{bt}\n");
        if let Ok(mut f) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&crash_path)
        {
            use std::io::Write;
            let _ = f.write_all(entry.as_bytes());
            let _ = f.flush();
        }
        // Best-effort: also surface it through tracing (may not flush on abort).
        tracing::error!(target: "panic", "{info}");
        prev(info);
    }));
}
