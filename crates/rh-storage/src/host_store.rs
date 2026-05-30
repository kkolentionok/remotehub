//! `HostStore` implementation backed by SQLite.
//!
//! Translation rules between `Host` and its row representation:
//!
//! - `id`, `name`, `display_name`, `group_id`, `protocol.as_str()`,
//!   `hostname`, `port`, `color`, `notes`, `startup_command`,
//!   `detected_os`, `default_credential_id` — direct.
//! - `tags` (`Vec<String>` in Rust) ↔ `tags_json` (JSON array text).
//! - `env_vars` (`Vec<EnvVar>` in Rust) ↔ `env_vars_json` (JSON array
//!   of `{key, value}` objects, order-preserving).
//! - `created_at` / `updated_at` — stored as ISO 8601 RFC 3339 strings
//!   so they're inspectable in SQLite shell tools and don't depend on
//!   SQLite's chrono affinity quirks.
//!
//! Errors are wrapped into [`StorageError`] at this layer so callers
//! don't need to know about `sqlx::Error`.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::Row;
use tracing::instrument;

use rh_core::{
    CredentialId, EnvVar, GroupId, Host, HostFilter, HostId, HostStore, Protocol, StorageError,
};

use crate::db::Db;

/// SQLite-backed implementation of [`HostStore`].
#[derive(Debug, Clone)]
pub struct SqliteHostStore {
    db: Db,
}

impl SqliteHostStore {
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

#[async_trait]
impl HostStore for SqliteHostStore {
    #[instrument(level = "debug", skip(self, host), fields(host_id = %host.id))]
    async fn create(&self, host: &Host) -> Result<(), StorageError> {
        let tags_json = serde_json::to_string(&host.tags)
            .map_err(|e| StorageError::Backend(format!("encode tags: {e}")))?;
        let env_vars_json = serde_json::to_string(&host.env_vars)
            .map_err(|e| StorageError::Backend(format!("encode env_vars: {e}")))?;

        sqlx::query(
            r"
            INSERT INTO hosts (
                id, name, display_name, group_id, protocol, hostname, port,
                username, tags_json, color, notes, startup_command, env_vars_json,
                detected_os, default_credential_id, created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ",
        )
        .bind(host.id.as_str())
        .bind(&host.name)
        .bind(host.display_name.as_deref())
        .bind(host.group_id.as_ref().map(|g| g.as_str()))
        .bind(host.protocol.as_str())
        .bind(&host.hostname)
        .bind(i32::from(host.port))
        .bind(&host.username)
        .bind(&tags_json)
        .bind(host.color.as_deref())
        .bind(host.notes.as_deref())
        .bind(host.startup_command.as_deref())
        .bind(&env_vars_json)
        .bind(host.detected_os.as_deref())
        .bind(host.default_credential_id.as_ref().map(|c| c.as_str()))
        .bind(host.created_at.to_rfc3339())
        .bind(host.updated_at.to_rfc3339())
        .execute(self.db.pool())
        .await
        .map_err(map_err)?;

        Ok(())
    }

    #[instrument(level = "debug", skip(self), fields(host_id = %id))]
    async fn get(&self, id: &HostId) -> Result<Host, StorageError> {
        let row = sqlx::query(SELECT_HOST_COLUMNS)
            .bind(id.as_str())
            .fetch_optional(self.db.pool())
            .await
            .map_err(map_err)?;

        let row = row.ok_or_else(|| {
            StorageError::Backend(format!("host {} not found", id.as_str()))
        })?;

        row_to_host(&row)
    }

    #[instrument(level = "debug", skip(self))]
    async fn list(&self, filter: HostFilter) -> Result<Vec<Host>, StorageError> {
        // Hand-build the WHERE clause. Could use a builder crate but
        // the number of filter dimensions is small and stable.
        let mut sql = String::from(SELECT_HOST_PREFIX);
        let mut clauses: Vec<&str> = Vec::new();

        if filter.group_id.is_some() {
            clauses.push("group_id = ?");
        }
        if filter.protocol.is_some() {
            clauses.push("protocol = ?");
        }
        if filter.search.is_some() {
            clauses.push("(LOWER(name) LIKE ? OR LOWER(hostname) LIKE ? OR LOWER(tags_json) LIKE ?)");
        }

        if !clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&clauses.join(" AND "));
        }

        sql.push_str(" ORDER BY name COLLATE NOCASE");

        // Limit with sane default ceiling. None → "no caller limit" but
        // we still clamp at 10_000 to keep memory predictable.
        let limit = filter.limit.unwrap_or(10_000).min(10_000);
        sql.push_str(" LIMIT ?");

        // Now bind parameters in the same order we appended the clauses.
        let mut query = sqlx::query(&sql);
        if let Some(ref g) = filter.group_id {
            query = query.bind(g.as_str());
        }
        if let Some(p) = filter.protocol {
            query = query.bind(p.as_str());
        }
        if let Some(ref s) = filter.search {
            let pattern = format!("%{}%", s.to_lowercase());
            query = query.bind(pattern.clone()).bind(pattern.clone()).bind(pattern);
        }
        query = query.bind(i64::from(limit));

        let rows = query
            .fetch_all(self.db.pool())
            .await
            .map_err(map_err)?;

        rows.iter().map(row_to_host).collect()
    }

    #[instrument(level = "debug", skip(self, host), fields(host_id = %host.id))]
    async fn update(&self, host: &Host) -> Result<(), StorageError> {
        let tags_json = serde_json::to_string(&host.tags)
            .map_err(|e| StorageError::Backend(format!("encode tags: {e}")))?;
        let env_vars_json = serde_json::to_string(&host.env_vars)
            .map_err(|e| StorageError::Backend(format!("encode env_vars: {e}")))?;

        let result = sqlx::query(
            r"
            UPDATE hosts SET
                name = ?,
                display_name = ?,
                group_id = ?,
                protocol = ?,
                hostname = ?,
                port = ?,
                username = ?,
                tags_json = ?,
                color = ?,
                notes = ?,
                startup_command = ?,
                env_vars_json = ?,
                detected_os = ?,
                default_credential_id = ?,
                updated_at = ?
            WHERE id = ?
            ",
        )
        .bind(&host.name)
        .bind(host.display_name.as_deref())
        .bind(host.group_id.as_ref().map(|g| g.as_str()))
        .bind(host.protocol.as_str())
        .bind(&host.hostname)
        .bind(i32::from(host.port))
        .bind(&host.username)
        .bind(&tags_json)
        .bind(host.color.as_deref())
        .bind(host.notes.as_deref())
        .bind(host.startup_command.as_deref())
        .bind(&env_vars_json)
        .bind(host.detected_os.as_deref())
        .bind(host.default_credential_id.as_ref().map(|c| c.as_str()))
        .bind(host.updated_at.to_rfc3339())
        .bind(host.id.as_str())
        .execute(self.db.pool())
        .await
        .map_err(map_err)?;

        if result.rows_affected() == 0 {
            return Err(StorageError::Backend(format!(
                "host {} not found for update",
                host.id.as_str()
            )));
        }

        Ok(())
    }

    #[instrument(level = "debug", skip(self), fields(host_id = %id))]
    async fn delete(&self, id: &HostId) -> Result<(), StorageError> {
        let result = sqlx::query("DELETE FROM hosts WHERE id = ?")
            .bind(id.as_str())
            .execute(self.db.pool())
            .await
            .map_err(map_err)?;

        if result.rows_affected() == 0 {
            return Err(StorageError::Backend(format!(
                "host {} not found for delete",
                id.as_str()
            )));
        }

        Ok(())
    }
}

// ---- Helpers -------------------------------------------------------

/// All host columns in a stable order — used by both `get` and `list`.
const SELECT_HOST_PREFIX: &str = "
    SELECT
        id, name, display_name, group_id, protocol, hostname, port,
        username, tags_json, color, notes, startup_command, env_vars_json,
        detected_os, default_credential_id, created_at, updated_at
    FROM hosts
";

const SELECT_HOST_COLUMNS: &str = "
    SELECT
        id, name, display_name, group_id, protocol, hostname, port,
        username, tags_json, color, notes, startup_command, env_vars_json,
        detected_os, default_credential_id, created_at, updated_at
    FROM hosts
    WHERE id = ?
";

fn row_to_host(row: &sqlx::sqlite::SqliteRow) -> Result<Host, StorageError> {
    let id: String = row
        .try_get("id")
        .map_err(|e| StorageError::Backend(format!("read id: {e}")))?;
    let name: String = row
        .try_get("name")
        .map_err(|e| StorageError::Backend(format!("read name: {e}")))?;
    let display_name: Option<String> = row
        .try_get("display_name")
        .map_err(|e| StorageError::Backend(format!("read display_name: {e}")))?;
    let group_id: Option<String> = row
        .try_get("group_id")
        .map_err(|e| StorageError::Backend(format!("read group_id: {e}")))?;
    let protocol_str: String = row
        .try_get("protocol")
        .map_err(|e| StorageError::Backend(format!("read protocol: {e}")))?;
    let hostname: String = row
        .try_get("hostname")
        .map_err(|e| StorageError::Backend(format!("read hostname: {e}")))?;
    let username: String = row
        .try_get("username")
        .map_err(|e| StorageError::Backend(format!("read username: {e}")))?;
    let port: i64 = row
        .try_get("port")
        .map_err(|e| StorageError::Backend(format!("read port: {e}")))?;
    let tags_json: String = row
        .try_get("tags_json")
        .map_err(|e| StorageError::Backend(format!("read tags_json: {e}")))?;
    let color: Option<String> = row
        .try_get("color")
        .map_err(|e| StorageError::Backend(format!("read color: {e}")))?;
    let notes: Option<String> = row
        .try_get("notes")
        .map_err(|e| StorageError::Backend(format!("read notes: {e}")))?;
    let startup_command: Option<String> = row
        .try_get("startup_command")
        .map_err(|e| StorageError::Backend(format!("read startup_command: {e}")))?;
    let env_vars_json: String = row
        .try_get("env_vars_json")
        .map_err(|e| StorageError::Backend(format!("read env_vars_json: {e}")))?;
    let detected_os: Option<String> = row
        .try_get("detected_os")
        .map_err(|e| StorageError::Backend(format!("read detected_os: {e}")))?;
    let default_credential_id: Option<String> = row
        .try_get("default_credential_id")
        .map_err(|e| StorageError::Backend(format!("read default_credential_id: {e}")))?;
    let created_at_s: String = row
        .try_get("created_at")
        .map_err(|e| StorageError::Backend(format!("read created_at: {e}")))?;
    let updated_at_s: String = row
        .try_get("updated_at")
        .map_err(|e| StorageError::Backend(format!("read updated_at: {e}")))?;

    let protocol = match protocol_str.as_str() {
        "ssh" => Protocol::Ssh,
        "rdp" => Protocol::Rdp,
        other => {
            return Err(StorageError::Malformed {
                entity: "hosts.protocol",
                reason: format!("unknown value {other:?}"),
            });
        }
    };

    let port = u16::try_from(port).map_err(|_| StorageError::Malformed {
        entity: "hosts.port",
        reason: format!("out of range: {port}"),
    })?;

    let tags: Vec<String> = serde_json::from_str(&tags_json).map_err(|e| StorageError::Malformed {
        entity: "hosts.tags_json",
        reason: format!("invalid JSON: {e}"),
    })?;

    let env_vars: Vec<EnvVar> =
        serde_json::from_str(&env_vars_json).map_err(|e| StorageError::Malformed {
            entity: "hosts.env_vars_json",
            reason: format!("invalid JSON: {e}"),
        })?;

    let created_at = parse_datetime("hosts.created_at", &created_at_s)?;
    let updated_at = parse_datetime("hosts.updated_at", &updated_at_s)?;

    Ok(Host {
        id: HostId::from_raw(id),
        name,
        display_name,
        group_id: group_id.map(GroupId::from_raw),
        protocol,
        hostname,
        port,
        username,
        tags,
        color,
        notes,
        startup_command,
        env_vars,
        detected_os,
        default_credential_id: default_credential_id.map(CredentialId::from_raw),
        created_at,
        updated_at,
    })
}

pub(crate) fn parse_datetime(field: &'static str, raw: &str) -> Result<DateTime<Utc>, StorageError> {
    DateTime::parse_from_rfc3339(raw)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| StorageError::Malformed {
            entity: field,
            reason: format!("invalid RFC3339 {raw:?}: {e}"),
        })
}

/// Map sqlx errors to StorageError variants. Specifically distinguishes
/// FK violations and UNIQUE conflicts, which UI handles differently
/// from generic backend failures.
pub(crate) fn map_err(err: sqlx::Error) -> StorageError {
    // SQLite returns extended result codes when configured. We match
    // on the textual error message because sqlx's typed error variants
    // don't always carry the SQLite extended code through.
    let msg = err.to_string();
    if msg.contains("FOREIGN KEY constraint failed") {
        return StorageError::ForeignKey(msg);
    }
    if msg.contains("UNIQUE constraint failed") {
        return StorageError::Conflict(msg);
    }
    if msg.contains("CHECK constraint failed") {
        return StorageError::Backend(format!("check constraint: {msg}"));
    }
    StorageError::Backend(msg)
}
