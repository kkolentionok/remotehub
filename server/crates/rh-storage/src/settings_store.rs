//! `SettingsStore` implementation.
//!
//! The on-disk shape is a flat key/value table where values are
//! JSON-encoded. In-memory we reconstruct a typed [`Settings`] struct
//! by deserializing each known key from its JSON value.
//!
//! Missing keys fall back to defaults from `Settings::default()`,
//! so a fresh database immediately presents a fully-formed settings
//! view to the rest of the app — no separate "initial settings"
//! migration needed.

use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::Value;
use sqlx::Row;
use tracing::instrument;

use rh_core::settings::keys;
use rh_core::{Settings, SettingsStore, StorageError};

use crate::db::Db;
use crate::host_store::map_err;

#[derive(Debug, Clone)]
pub struct SqliteSettingsStore {
    db: Db,
}

impl SqliteSettingsStore {
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

#[async_trait]
impl SettingsStore for SqliteSettingsStore {
    #[instrument(level = "debug", skip(self))]
    async fn load(&self) -> Result<Settings, StorageError> {
        let rows = sqlx::query("SELECT key, value FROM settings")
            .fetch_all(self.db.pool())
            .await
            .map_err(map_err)?;

        let mut map: HashMap<String, Value> = HashMap::with_capacity(rows.len());
        for row in &rows {
            let key: String = row
                .try_get("key")
                .map_err(|e| StorageError::Backend(format!("read settings.key: {e}")))?;
            let raw: String = row
                .try_get("value")
                .map_err(|e| StorageError::Backend(format!("read settings.value: {e}")))?;
            let parsed: Value = serde_json::from_str(&raw).map_err(|e| StorageError::Malformed {
                entity: "settings.value",
                reason: format!("key={key} not valid JSON: {e}"),
            })?;
            map.insert(key, parsed);
        }

        let mut out = Settings::default();

        // Apply each known key if present. Unknown keys are silently
        // ignored (forward compatibility — newer client wrote a key
        // we don't know yet). Malformed values for KNOWN keys, however,
        // are an error.
        if let Some(v) = map.get(keys::LANGUAGE) {
            out.language = parse_field(keys::LANGUAGE, v)?;
        }
        if let Some(v) = map.get(keys::THEME) {
            out.theme = parse_field(keys::THEME, v)?;
        }
        if let Some(v) = map.get(keys::DEFAULT_SSH_PORT) {
            out.default_ssh_port = parse_field(keys::DEFAULT_SSH_PORT, v)?;
        }
        if let Some(v) = map.get(keys::DEFAULT_RDP_PORT) {
            out.default_rdp_port = parse_field(keys::DEFAULT_RDP_PORT, v)?;
        }
        if let Some(v) = map.get(keys::TERMINAL_FONT_FAMILY) {
            out.terminal_font_family = parse_field(keys::TERMINAL_FONT_FAMILY, v)?;
        }
        if let Some(v) = map.get(keys::TERMINAL_FONT_SIZE) {
            out.terminal_font_size = parse_field(keys::TERMINAL_FONT_SIZE, v)?;
        }
        if let Some(v) = map.get(keys::TERMINAL_COLOR_SCHEME) {
            out.terminal_color_scheme = parse_field(keys::TERMINAL_COLOR_SCHEME, v)?;
        }
        if let Some(v) = map.get(keys::TERMINAL_CURSOR_STYLE) {
            out.terminal_cursor_style = parse_field(keys::TERMINAL_CURSOR_STYLE, v)?;
        }
        if let Some(v) = map.get(keys::TERMINAL_SCROLLBACK) {
            out.terminal_scrollback = parse_field(keys::TERMINAL_SCROLLBACK, v)?;
        }
        if let Some(v) = map.get(keys::RDP_DEFAULT_RESOLUTION) {
            out.rdp_default_resolution = parse_field(keys::RDP_DEFAULT_RESOLUTION, v)?;
        }
        if let Some(v) = map.get(keys::APP_CONFIRM_CLOSE_SESSION) {
            out.app_confirm_close_session = parse_field(keys::APP_CONFIRM_CLOSE_SESSION, v)?;
        }
        if let Some(v) = map.get(keys::APP_STARTUP_SCREEN) {
            out.app_startup_screen = parse_field(keys::APP_STARTUP_SCREEN, v)?;
        }
        if let Some(v) = map.get(keys::SSH_KEEPALIVE_INTERVAL_SECS) {
            out.ssh_keepalive_interval_secs = parse_field(keys::SSH_KEEPALIVE_INTERVAL_SECS, v)?;
        }
        if let Some(v) = map.get(keys::SSH_KNOWN_HOSTS_STRICT) {
            out.ssh_known_hosts_strict = parse_field(keys::SSH_KNOWN_HOSTS_STRICT, v)?;
        }
        if let Some(v) = map.get(keys::LOCAL_SHELL) {
            out.local_shell = parse_field(keys::LOCAL_SHELL, v)?;
        }

        Ok(out)
    }

    #[instrument(level = "debug", skip(self, patch))]
    async fn save(&self, patch: Value) -> Result<(), StorageError> {
        let obj = patch.as_object().ok_or_else(|| StorageError::Backend(
            "settings patch must be a JSON object".to_string(),
        ))?;

        let mut tx = self
            .db
            .pool()
            .begin()
            .await
            .map_err(|e| StorageError::Backend(format!("begin tx: {e}")))?;

        for (key, value) in obj {
            // The UI sends Settings field names (e.g. `terminal_color_scheme`);
            // the table stays namespaced (`terminal.color_scheme`). Translate
            // here, rejecting anything we don't recognize so UI bugs can't
            // write trash into the table.
            let storage_key = storage_key_for(key).ok_or_else(|| {
                StorageError::Backend(format!("unknown setting key: {key}"))
            })?;
            // We don't deeply validate the value type here — Settings::load
            // will catch type mismatches on next load, and the UI is
            // expected to send well-typed values via the typed wrapper.
            let encoded = serde_json::to_string(value).map_err(|e| {
                StorageError::Backend(format!("encode setting {key}: {e}"))
            })?;
            sqlx::query(
                r"
                INSERT INTO settings (key, value) VALUES (?, ?)
                ON CONFLICT (key) DO UPDATE SET value = excluded.value
                ",
            )
            .bind(storage_key)
            .bind(&encoded)
            .execute(&mut *tx)
            .await
            .map_err(map_err)?;
        }

        tx.commit()
            .await
            .map_err(|e| StorageError::Backend(format!("commit: {e}")))?;
        Ok(())
    }
}

fn parse_field<T: serde::de::DeserializeOwned>(
    key: &'static str,
    value: &Value,
) -> Result<T, StorageError> {
    serde_json::from_value::<T>(value.clone()).map_err(|e| StorageError::Malformed {
        entity: "settings",
        reason: format!("key={key}: {e}"),
    })
}

/// Map a Settings struct field name (as the UI sends it) to the
/// namespaced key used in the `settings` table. Returns `None` for
/// unknown fields so `save` can reject them.
fn storage_key_for(field: &str) -> Option<&'static str> {
    Some(match field {
        "language" => keys::LANGUAGE,
        "theme" => keys::THEME,
        "default_ssh_port" => keys::DEFAULT_SSH_PORT,
        "default_rdp_port" => keys::DEFAULT_RDP_PORT,
        "terminal_font_family" => keys::TERMINAL_FONT_FAMILY,
        "terminal_font_size" => keys::TERMINAL_FONT_SIZE,
        "terminal_color_scheme" => keys::TERMINAL_COLOR_SCHEME,
        "terminal_cursor_style" => keys::TERMINAL_CURSOR_STYLE,
        "terminal_scrollback" => keys::TERMINAL_SCROLLBACK,
        "rdp_default_resolution" => keys::RDP_DEFAULT_RESOLUTION,
        "app_confirm_close_session" => keys::APP_CONFIRM_CLOSE_SESSION,
        "app_startup_screen" => keys::APP_STARTUP_SCREEN,
        "ssh_keepalive_interval_secs" => keys::SSH_KEEPALIVE_INTERVAL_SECS,
        "ssh_known_hosts_strict" => keys::SSH_KNOWN_HOSTS_STRICT,
        "local_shell" => keys::LOCAL_SHELL,
        _ => return None,
    })
}
