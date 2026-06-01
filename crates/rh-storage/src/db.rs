//! SQLite connection pool and schema management.
//!
//! Provides [`Db`] — a thin wrapper around an `sqlx::SqlitePool` that
//! handles opening (with sane PRAGMAs) and version-checking the schema.
//!
//! Alpha-mode migration policy: if the on-disk schema version differs
//! from [`CURRENT_SCHEMA_VERSION`], the entire database is dropped and
//! recreated. This is intentional and aggressive — alpha users should
//! export anything they care about before upgrading. When we ship
//! beta, [`Db::open`] gains real up-migrations instead.

use std::path::{Path, PathBuf};

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Executor, Row, SqlitePool};
use tracing::{debug, info, warn};

use rh_core::StorageError;

/// Schema version this binary expects. Bumping this triggers either an
/// incremental migration (when a path exists, e.g. v2 → v3) or, for any
/// other mismatch, a drop-recreate in alpha mode.
pub const CURRENT_SCHEMA_VERSION: u32 = 8;

/// Embedded migration script for the current version.
const V1_SQL: &str = include_str!("migrations/v1.sql");

/// Incremental, data-preserving migration v2 → v3: per-host username.
///
/// `username` used to live on the credential, which broke when one SSH key
/// is shared across hosts that log in as different users. It now lives on
/// the host. This `ALTER TABLE` keeps all existing rows intact.
const MIGRATE_V2_TO_V3: &str =
    "ALTER TABLE hosts ADD COLUMN username TEXT NOT NULL DEFAULT '';";

/// Incremental, data-preserving migration v3 → v4: pinned SSH host keys
/// (TOFU). Purely additive — a new table, no existing rows touched.
const MIGRATE_V3_TO_V4: &str = "\
CREATE TABLE known_hosts (\
    hostname            TEXT NOT NULL,\
    port                INTEGER NOT NULL,\
    key_type            TEXT NOT NULL,\
    fingerprint_sha256  TEXT NOT NULL,\
    created_at          TEXT NOT NULL,\
    PRIMARY KEY (hostname, port)\
);";

/// Incremental, data-preserving migration v4 → v5: `last_connected_at`
/// on hosts (stamped when a session reaches Ready). Additive column.
const MIGRATE_V4_TO_V5: &str = "ALTER TABLE hosts ADD COLUMN last_connected_at TEXT;";

/// Incremental, data-preserving migration v5 → v6: `jump_host_id` on
/// hosts (ProxyJump bastion reference). Additive nullable column; no FK
/// enforcement (a deleted bastion is handled at connect time).
const MIGRATE_V5_TO_V6: &str = "ALTER TABLE hosts ADD COLUMN jump_host_id TEXT;";

/// Incremental, data-preserving migration v6 → v7: `agent_forwarding`
/// on hosts (ssh -A). Additive NOT NULL column with a 0 default.
const MIGRATE_V6_TO_V7: &str =
    "ALTER TABLE hosts ADD COLUMN agent_forwarding INTEGER NOT NULL DEFAULT 0;";

/// Incremental, data-preserving migration v7 → v8: `rdp_known_certs`
/// table (TOFU pins for RDP server certificates). New table, no data loss.
const MIGRATE_V7_TO_V8: &str = "CREATE TABLE rdp_known_certs (\n    hostname            TEXT NOT NULL,\n    port                INTEGER NOT NULL,\n    fingerprint_sha256  TEXT NOT NULL,\n    subject             TEXT NOT NULL,\n    trusted_at          TEXT NOT NULL,\n    PRIMARY KEY (hostname, port)\n);";

/// Ordered forward migrations `(from_version, sql)`. The runner applies
/// every step whose `from_version >= existing` in sequence, so a DB any
/// number of versions behind (as long as the chain is contiguous from
/// its version up to current) migrates without data loss. A gap (e.g. a
/// pre-v2 database) falls back to drop-recreate in alpha mode.
const MIGRATIONS: &[(u32, &str)] = &[
    (2, MIGRATE_V2_TO_V3),
    (3, MIGRATE_V3_TO_V4),
    (4, MIGRATE_V4_TO_V5),
    (5, MIGRATE_V5_TO_V6),
    (6, MIGRATE_V6_TO_V7),
    (7, MIGRATE_V7_TO_V8),
];

/// What [`Db::open`] decided to do with the schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitOutcome {
    /// File didn't exist (or had no schema). We created v1 from scratch.
    Created,
    /// File existed at the expected version. Nothing to do.
    AlreadyCurrent,
    /// File existed at a different version. We dropped and recreated.
    /// User-visible data was lost.
    Recreated { old_version: u32 },
    /// File existed one version behind and we applied an incremental,
    /// data-preserving migration. No data lost.
    Migrated { old_version: u32 },
}

/// Opaque pool wrapper. Cloning is cheap (Arc inside).
#[derive(Debug, Clone)]
pub struct Db {
    pool: SqlitePool,
    /// Path the pool was opened against, for diagnostics. `:memory:`
    /// for in-memory databases.
    source: String,
}

impl Db {
    /// Open the database at `path`, creating it if missing, and ensure
    /// the schema is at [`CURRENT_SCHEMA_VERSION`].
    ///
    /// The parent directory must exist; we won't create it here (that's
    /// the responsibility of the application's startup code).
    pub async fn open(path: impl AsRef<Path>) -> Result<(Self, InitOutcome), StorageError> {
        let path = path.as_ref().to_path_buf();
        Self::open_with(SourceKind::File(path)).await
    }

    /// Open an in-memory database. Each call gets a fresh database;
    /// useful for tests.
    pub async fn open_memory() -> Result<Self, StorageError> {
        let (db, _) = Self::open_with(SourceKind::Memory).await?;
        Ok(db)
    }

    async fn open_with(source: SourceKind) -> Result<(Self, InitOutcome), StorageError> {
        let (options, source_str) = match &source {
            SourceKind::File(path) => {
                let opts = SqliteConnectOptions::new()
                    .filename(path)
                    .create_if_missing(true)
                    .foreign_keys(true)
                    .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
                    .synchronous(sqlx::sqlite::SqliteSynchronous::Normal);
                (opts, path.display().to_string())
            }
            SourceKind::Memory => {
                let opts = SqliteConnectOptions::new()
                    .in_memory(true)
                    .foreign_keys(true);
                (opts, ":memory:".to_string())
            }
        };

        let max_conns = match &source {
            // Each `:memory:` connection is an isolated database, so a
            // multi-connection pool would let the second connection see
            // an EMPTY db. Cap at 1 for memory; tests are fine with that.
            SourceKind::Memory => 1,
            // For file-backed DBs, SQLite serializes writes anyway, but
            // a small pool lets reads parallelize.
            SourceKind::File(_) => 5,
        };

        let pool = SqlitePoolOptions::new()
            .max_connections(max_conns)
            .connect_with(options)
            .await
            .map_err(|e| StorageError::Io(format!("connect: {e}")))?;

        let db = Db {
            pool,
            source: source_str,
        };

        let outcome = db.init_or_migrate().await?;
        Ok((db, outcome))
    }

    /// Borrow the underlying pool. Storage adapters use this to run
    /// queries. Intentionally `pub(crate)` to keep `sqlx::Pool` out of
    /// the public API — callers should use the trait-based store APIs.
    pub(crate) fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Path or `:memory:` for diagnostics.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    async fn init_or_migrate(&self) -> Result<InitOutcome, StorageError> {
        let existing = self.read_schema_version().await?;

        match existing {
            None => {
                info!(source = %self.source, "initializing fresh schema v{CURRENT_SCHEMA_VERSION}");
                self.apply_v1().await?;
                Ok(InitOutcome::Created)
            }
            Some(v) if v == CURRENT_SCHEMA_VERSION => {
                info!(source = %self.source, version = v, "schema already at current version");
                Ok(InitOutcome::AlreadyCurrent)
            }
            // Forward, data-preserving migration when a contiguous chain
            // exists from `v` up to current (see MIGRATIONS).
            Some(v) if v < CURRENT_SCHEMA_VERSION && has_migration_chain(v) => {
                info!(
                    source = %self.source,
                    old_version = v,
                    new_version = CURRENT_SCHEMA_VERSION,
                    "migrating schema (data preserved)"
                );
                for (from, sql) in MIGRATIONS.iter().filter(|(from, _)| *from >= v) {
                    debug!(from, to = from + 1, "applying migration");
                    self.apply_migration(sql).await?;
                }
                self.set_schema_version(CURRENT_SCHEMA_VERSION).await?;
                Ok(InitOutcome::Migrated { old_version: v })
            }
            Some(v) => {
                warn!(
                    source = %self.source,
                    old_version = v,
                    new_version = CURRENT_SCHEMA_VERSION,
                    "schema version mismatch — alpha mode: recreating database (data loss)"
                );
                self.drop_all_tables().await?;
                self.apply_v1().await?;
                Ok(InitOutcome::Recreated { old_version: v })
            }
        }
    }

    /// Read `schema_meta.version`. Returns `None` if the table doesn't
    /// exist (fresh database) or the row is missing.
    async fn read_schema_version(&self) -> Result<Option<u32>, StorageError> {
        // Check if the table exists first to avoid noisy errors on fresh DBs.
        let table_exists: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'schema_meta'",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| StorageError::Backend(format!("probe schema_meta: {e}")))?;

        if table_exists == 0 {
            return Ok(None);
        }

        let row = sqlx::query("SELECT value FROM schema_meta WHERE key = 'version'")
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| StorageError::Backend(format!("read schema version: {e}")))?;

        match row {
            Some(r) => {
                let raw: String = r
                    .try_get(0)
                    .map_err(|e| StorageError::Backend(format!("decode version column: {e}")))?;
                let parsed: u32 = raw.parse().map_err(|_| StorageError::Malformed {
                    entity: "schema_meta.version",
                    reason: format!("expected integer, got {raw:?}"),
                })?;
                Ok(Some(parsed))
            }
            None => Ok(None),
        }
    }

    /// Run a single incremental migration script (one or more statements).
    async fn apply_migration(&self, sql: &str) -> Result<(), StorageError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| StorageError::Backend(format!("acquire for migration: {e}")))?;
        conn.execute(sql)
            .await
            .map_err(|e| StorageError::Backend(format!("apply migration: {e}")))?;
        Ok(())
    }

    /// Update the recorded schema version after an incremental migration.
    async fn set_schema_version(&self, version: u32) -> Result<(), StorageError> {
        sqlx::query("UPDATE schema_meta SET value = ? WHERE key = 'version'")
            .bind(version.to_string())
            .execute(&self.pool)
            .await
            .map_err(|e| StorageError::Backend(format!("set schema version: {e}")))?;
        Ok(())
    }

    async fn apply_v1(&self) -> Result<(), StorageError> {
        // execute_many handles multi-statement scripts. We run inside an
        // implicit transaction by virtue of each statement committing
        // independently in SQLite; explicit BEGIN would actually wrap
        // DDL in DDL on most engines, but SQLite allows it.
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| StorageError::Backend(format!("acquire for migration: {e}")))?;

        conn.execute(V1_SQL)
            .await
            .map_err(|e| StorageError::Backend(format!("apply v1: {e}")))?;

        Ok(())
    }

    /// Drop every user table. Called only when schema version mismatch
    /// forces a recreate. We don't bother with selective drops — alpha.
    async fn drop_all_tables(&self) -> Result<(), StorageError> {
        // Order matters: drop dependent tables before referenced ones,
        // OR disable foreign_keys temporarily. We do the latter — it's
        // a one-shot operation and avoids re-encoding the dependency
        // graph here.
        let drop_script = "
            PRAGMA foreign_keys = OFF;
            DROP TABLE IF EXISTS host_credentials;
            DROP TABLE IF EXISTS hosts;
            DROP TABLE IF EXISTS credentials;
            DROP TABLE IF EXISTS host_groups;
            DROP TABLE IF EXISTS known_hosts;
            DROP TABLE IF EXISTS settings;
            DROP TABLE IF EXISTS schema_meta;
            PRAGMA foreign_keys = ON;
        ";

        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| StorageError::Backend(format!("acquire for drop: {e}")))?;

        conn.execute(drop_script)
            .await
            .map_err(|e| StorageError::Backend(format!("drop tables: {e}")))?;

        Ok(())
    }
}

enum SourceKind {
    File(PathBuf),
    Memory,
}

/// Whether a contiguous forward migration chain exists from `from` up to
/// [`CURRENT_SCHEMA_VERSION`] (every intermediate step has an entry in
/// [`MIGRATIONS`]). If not, the caller falls back to drop-recreate.
fn has_migration_chain(from: u32) -> bool {
    (from..CURRENT_SCHEMA_VERSION).all(|k| MIGRATIONS.iter().any(|(f, _)| *f == k))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn opens_fresh_memory_db_at_current_version() {
        let db = Db::open_memory().await.unwrap();
        let v = db.read_schema_version().await.unwrap();
        assert_eq!(v, Some(CURRENT_SCHEMA_VERSION));
    }

    #[tokio::test]
    async fn fresh_open_reports_created() {
        let (_, outcome) = Db::open_with(SourceKind::Memory).await.unwrap();
        assert_eq!(outcome, InitOutcome::Created);
    }

    #[tokio::test]
    async fn fresh_db_has_all_expected_tables() {
        let db = Db::open_memory().await.unwrap();
        let names: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name",
        )
        .fetch_all(db.pool())
        .await
        .unwrap();

        for expected in [
            "credentials",
            "host_credentials",
            "host_groups",
            "hosts",
            "known_hosts",
            "schema_meta",
            "settings",
        ] {
            assert!(
                names.iter().any(|n| n == expected),
                "missing table {expected}; have: {names:?}"
            );
        }
    }

    #[tokio::test]
    async fn opens_file_db_and_persists() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.db");

        // First open: creates schema.
        let (db, outcome) = Db::open(&path).await.unwrap();
        assert_eq!(outcome, InitOutcome::Created);

        // Write something so we can verify persistence.
        sqlx::query("INSERT INTO settings (key, value) VALUES ('marker', '\"hi\"')")
            .execute(db.pool())
            .await
            .unwrap();
        drop(db); // close pool

        // Second open: should report AlreadyCurrent and preserve data.
        let (db, outcome) = Db::open(&path).await.unwrap();
        assert_eq!(outcome, InitOutcome::AlreadyCurrent);

        let val: String = sqlx::query_scalar("SELECT value FROM settings WHERE key = 'marker'")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(val, "\"hi\"");
    }

    #[tokio::test]
    async fn malformed_schema_version_returns_error() {
        let db = Db::open_memory().await.unwrap();
        sqlx::query("UPDATE schema_meta SET value = 'not-a-number' WHERE key = 'version'")
            .execute(db.pool())
            .await
            .unwrap();

        let result = db.read_schema_version().await;
        assert!(matches!(result, Err(StorageError::Malformed { .. })));
    }

    #[tokio::test]
    async fn version_mismatch_triggers_recreate() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.db");

        // Create db at v1, insert data.
        let (db, _) = Db::open(&path).await.unwrap();
        sqlx::query("INSERT INTO settings (key, value) VALUES ('keep_me', '\"data\"')")
            .execute(db.pool())
            .await
            .unwrap();
        // Bump version artificially to simulate a future schema.
        sqlx::query("UPDATE schema_meta SET value = '99' WHERE key = 'version'")
            .execute(db.pool())
            .await
            .unwrap();
        drop(db);

        // Reopen: this binary expects version 1, finds 99 → recreate.
        let (db, outcome) = Db::open(&path).await.unwrap();
        assert_eq!(outcome, InitOutcome::Recreated { old_version: 99 });

        // Data should be gone.
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM settings")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
}
