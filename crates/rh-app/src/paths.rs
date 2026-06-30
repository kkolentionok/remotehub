//! Platform-specific paths for application data, logs, and config.
//!
//! Stage 1.1 uses simple computation; Stage 1.4 may switch to Tauri's
//! `app_data_dir` once we have an `AppHandle` available at the point of use.

use std::path::PathBuf;

/// Base directory for application data on this OS.
///
/// - Windows: `%APPDATA%\Pingie\`
/// - macOS:   `~/Library/Application Support/Pingie/`
/// - Linux:   `$XDG_DATA_HOME/pingie/` or `~/.local/share/pingie/`
pub fn app_data_dir() -> PathBuf {
    if let Some(dir) = platform_app_data_dir() {
        return dir;
    }
    // Fallback: cwd. This should never happen on a normal install.
    PathBuf::from(".").join("pingie-data")
}

/// Directory for rotating log files.
pub fn log_dir() -> PathBuf {
    app_data_dir().join("logs")
}

#[cfg(target_os = "windows")]
fn platform_app_data_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|p| PathBuf::from(p).join("Pingie"))
}

#[cfg(target_os = "macos")]
fn platform_app_data_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(|p| PathBuf::from(p).join("Library/Application Support/Pingie"))
}

#[cfg(target_os = "linux")]
fn platform_app_data_dir() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        Some(PathBuf::from(xdg).join("pingie"))
    } else {
        std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/share/pingie"))
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn platform_app_data_dir() -> Option<PathBuf> {
    None
}
