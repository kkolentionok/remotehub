//! User-facing settings.
//!
//! Settings are persisted as a flat key/value map in SQLite (table
//! `settings`, see `data-model.md`). The keys themselves are defined
//! as string constants in this module — single source of truth, used
//! by `rh-storage` for reads/writes and by `rh-app` for default
//! initialization.
//!
//! Values are stored as JSON strings to keep the schema simple while
//! supporting structured values (e.g. resolution tuples). Type
//! validation happens at the storage layer when a value is read.

use serde::{Deserialize, Serialize};

/// User-selected UI language.
///
/// Persisted as the lowercase variant name. Adding a language is just
/// a matter of adding a variant here, updating defaults, and adding
/// the corresponding catalogue file in the UI's `i18n/` directory.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    En,
    Ru,
}

/// User-selected color theme.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Light,
    Dark,
    /// Follow the OS theme.
    #[default]
    System,
}

/// Terminal cursor rendering style.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CursorStyle {
    #[default]
    Block,
    Underline,
    Bar,
}

/// Named color scheme for the terminal. We ship a small set; users
/// who want custom palettes can add them in a later iteration.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TerminalColorScheme {
    #[default]
    Default,
    SolarizedDark,
    SolarizedLight,
    Dracula,
    Nord,
}

/// RDP startup resolution. Either an explicit `width × height`, or
/// `Fit` to match the host window.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RdpResolution {
    #[default]
    Fit,
    Fixed { width: u16, height: u16 },
}

/// What to show when the application starts. `Home` is a blank state;
/// `LastHosts` restores the tabs that were open at last shutdown
/// (informational only — sessions themselves don't survive a restart).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StartupScreen {
    #[default]
    Home,
    LastHosts,
}

/// Setting keys — string constants used as the primary key in the
/// `settings` table. Centralizing them here means typos surface at
/// compile time and the full list of supported settings is one
/// `grep` away.
pub mod keys {
    pub const LANGUAGE: &str = "language";
    pub const THEME: &str = "theme";
    pub const DEFAULT_SSH_PORT: &str = "default_ssh_port";
    pub const DEFAULT_RDP_PORT: &str = "default_rdp_port";
    pub const TERMINAL_FONT_FAMILY: &str = "terminal.font_family";
    pub const TERMINAL_FONT_SIZE: &str = "terminal.font_size";
    pub const TERMINAL_COLOR_SCHEME: &str = "terminal.color_scheme";
    pub const TERMINAL_CURSOR_STYLE: &str = "terminal.cursor_style";
    pub const TERMINAL_SCROLLBACK: &str = "terminal.scrollback";
    pub const RDP_DEFAULT_RESOLUTION: &str = "rdp.default_resolution";
    pub const APP_CONFIRM_CLOSE_SESSION: &str = "app.confirm_close_session";
    pub const APP_STARTUP_SCREEN: &str = "app.startup_screen";
    pub const SSH_KEEPALIVE_INTERVAL_SECS: &str = "ssh.keepalive_interval_secs";
    pub const SSH_KNOWN_HOSTS_STRICT: &str = "ssh.known_hosts_strict";
}

/// Typed view of all settings with their default values.
///
/// This struct is the canonical representation in memory; the SQLite
/// table is the persistence format. Storage layer is responsible for
/// loading rows into this struct and writing changes back.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Settings {
    pub language: Language,
    pub theme: Theme,
    pub default_ssh_port: u16,
    pub default_rdp_port: u16,
    pub terminal_font_family: String,
    pub terminal_font_size: u16,
    pub terminal_color_scheme: TerminalColorScheme,
    pub terminal_cursor_style: CursorStyle,
    pub terminal_scrollback: u32,
    pub rdp_default_resolution: RdpResolution,
    pub app_confirm_close_session: bool,
    pub app_startup_screen: StartupScreen,
    pub ssh_keepalive_interval_secs: u32,
    pub ssh_known_hosts_strict: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: Language::default(),
            theme: Theme::default(),
            default_ssh_port: 22,
            default_rdp_port: 3389,
            terminal_font_family: default_terminal_font().to_string(),
            terminal_font_size: 14,
            terminal_color_scheme: TerminalColorScheme::default(),
            terminal_cursor_style: CursorStyle::default(),
            terminal_scrollback: 10_000,
            rdp_default_resolution: RdpResolution::default(),
            app_confirm_close_session: true,
            app_startup_screen: StartupScreen::default(),
            ssh_keepalive_interval_secs: 30,
            ssh_known_hosts_strict: true,
        }
    }
}

/// Platform-conventional monospace font, used as the terminal default
/// when the user hasn't picked one explicitly.
#[must_use]
const fn default_terminal_font() -> &'static str {
    if cfg!(target_os = "windows") {
        "Cascadia Mono"
    } else if cfg!(target_os = "macos") {
        "SF Mono"
    } else {
        "DejaVu Sans Mono"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_serde_lowercase() {
        assert_eq!(serde_json::to_string(&Theme::System).unwrap(), "\"system\"");
        let parsed: Theme = serde_json::from_str("\"dark\"").unwrap();
        assert_eq!(parsed, Theme::Dark);
    }

    #[test]
    fn color_scheme_serde_kebab_case() {
        let json = serde_json::to_string(&TerminalColorScheme::SolarizedDark).unwrap();
        assert_eq!(json, "\"solarized-dark\"");
    }

    #[test]
    fn rdp_resolution_serde_tagged() {
        let fit = RdpResolution::Fit;
        let json = serde_json::to_string(&fit).unwrap();
        assert_eq!(json, r#"{"kind":"fit"}"#);

        let fixed = RdpResolution::Fixed { width: 1920, height: 1080 };
        let json = serde_json::to_string(&fixed).unwrap();
        assert_eq!(json, r#"{"kind":"fixed","width":1920,"height":1080}"#);
    }

    #[test]
    fn default_settings_match_spec() {
        // These defaults come from data-model.md and are user-visible
        // contracts. Changing any of them should be a deliberate decision.
        let s = Settings::default();
        assert_eq!(s.language, Language::En);
        assert_eq!(s.theme, Theme::System);
        assert_eq!(s.default_ssh_port, 22);
        assert_eq!(s.default_rdp_port, 3389);
        assert_eq!(s.terminal_font_size, 14);
        assert_eq!(s.terminal_scrollback, 10_000);
        assert_eq!(s.terminal_color_scheme, TerminalColorScheme::Default);
        assert_eq!(s.terminal_cursor_style, CursorStyle::Block);
        assert!(s.app_confirm_close_session);
        assert_eq!(s.app_startup_screen, StartupScreen::Home);
        assert_eq!(s.ssh_keepalive_interval_secs, 30);
        assert!(s.ssh_known_hosts_strict);
        assert_eq!(s.rdp_default_resolution, RdpResolution::Fit);
    }

    #[test]
    fn language_serde_lowercase() {
        assert_eq!(serde_json::to_string(&Language::Ru).unwrap(), "\"ru\"");
        let parsed: Language = serde_json::from_str("\"en\"").unwrap();
        assert_eq!(parsed, Language::En);
    }

    #[test]
    fn keys_are_unique() {
        // Sanity: a typo in one of the keys constants would silently
        // shadow another setting. Check pairwise distinctness.
        let all = [
            keys::LANGUAGE,
            keys::THEME,
            keys::DEFAULT_SSH_PORT,
            keys::DEFAULT_RDP_PORT,
            keys::TERMINAL_FONT_FAMILY,
            keys::TERMINAL_FONT_SIZE,
            keys::TERMINAL_COLOR_SCHEME,
            keys::TERMINAL_CURSOR_STYLE,
            keys::TERMINAL_SCROLLBACK,
            keys::RDP_DEFAULT_RESOLUTION,
            keys::APP_CONFIRM_CLOSE_SESSION,
            keys::APP_STARTUP_SCREEN,
            keys::SSH_KEEPALIVE_INTERVAL_SECS,
            keys::SSH_KNOWN_HOSTS_STRICT,
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b, "duplicate key: {a}");
            }
        }
    }

    #[test]
    fn settings_serde_roundtrip() {
        let s = Settings::default();
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }
}
