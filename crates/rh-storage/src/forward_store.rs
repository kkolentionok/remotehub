//! `ForwardStore` implementation backed by SQLite.
//!
//! Stores saved port-forward definitions (Tools → Forwards). Purely
//! metadata — no secrets here; credentials are resolved from the host at
//! start time. `host_id` has `ON DELETE CASCADE`, so deleting a host
//! drops its saved forwards automatically.

use async_trait::async_trait;
use sqlx::Row;
use tracing::instrument;

use rh_core::{ForwardId, ForwardKind, ForwardStore, HostId, SavedForward, StorageError};

use crate::db::Db;
use crate::host_store::{map_err, parse_datetime};

#[derive(Debug, Clone)]
pub struct SqliteForwardStore {
    db: Db,
}

impl SqliteForwardStore {
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ForwardStore for SqliteForwardStore {
    #[instrument(level = "debug", skip(self, f), fields(forward_id = %f.id))]
    async fn create(&self, f: &SavedForward) -> Result<(), StorageError> {
        sqlx::query(
            "INSERT INTO forwards \
             (id, host_id, kind, bind_host, bind_port, target_host, target_port, auto_start, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(f.id.as_str())
        .bind(f.host_id.as_str())
        .bind(f.kind.as_str())
        .bind(&f.bind_host)
        .bind(i64::from(f.bind_port))
        .bind(&f.target_host)
        .bind(i64::from(f.target_port))
        .bind(i64::from(f.auto_start))
        .bind(f.created_at.to_rfc3339())
        .execute(self.db.pool())
        .await
        .map_err(map_err)?;
        Ok(())
    }

    #[instrument(level = "debug", skip(self), fields(forward_id = %id))]
    async fn get(&self, id: &ForwardId) -> Result<SavedForward, StorageError> {
        let row = sqlx::query(
            "SELECT id, host_id, kind, bind_host, bind_port, target_host, target_port, \
             auto_start, created_at FROM forwards WHERE id = ?",
        )
        .bind(id.as_str())
        .fetch_optional(self.db.pool())
        .await
        .map_err(map_err)?;

        let row = row.ok_or_else(|| {
            StorageError::Backend(format!("forward {} not found", id.as_str()))
        })?;
        row_to_forward(&row)
    }

    #[instrument(level = "debug", skip(self))]
    async fn list(&self) -> Result<Vec<SavedForward>, StorageError> {
        let rows = sqlx::query(
            "SELECT id, host_id, kind, bind_host, bind_port, target_host, target_port, \
             auto_start, created_at FROM forwards ORDER BY created_at DESC",
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(map_err)?;

        rows.iter().map(row_to_forward).collect()
    }

    #[instrument(level = "debug", skip(self), fields(forward_id = %id))]
    async fn delete(&self, id: &ForwardId) -> Result<(), StorageError> {
        let result = sqlx::query("DELETE FROM forwards WHERE id = ?")
            .bind(id.as_str())
            .execute(self.db.pool())
            .await
            .map_err(map_err)?;
        if result.rows_affected() == 0 {
            return Err(StorageError::Backend(format!(
                "forward {} not found for delete",
                id.as_str()
            )));
        }
        Ok(())
    }

    #[instrument(level = "debug", skip(self), fields(forward_id = %id))]
    async fn set_auto_start(&self, id: &ForwardId, auto_start: bool) -> Result<(), StorageError> {
        let result = sqlx::query("UPDATE forwards SET auto_start = ? WHERE id = ?")
            .bind(i64::from(auto_start))
            .bind(id.as_str())
            .execute(self.db.pool())
            .await
            .map_err(map_err)?;
        if result.rows_affected() == 0 {
            return Err(StorageError::Backend(format!(
                "forward {} not found for set_auto_start",
                id.as_str()
            )));
        }
        Ok(())
    }
}

fn row_to_forward(row: &sqlx::sqlite::SqliteRow) -> Result<SavedForward, StorageError> {
    let id: String = row
        .try_get("id")
        .map_err(|e| StorageError::Backend(format!("read id: {e}")))?;
    let host_id: String = row
        .try_get("host_id")
        .map_err(|e| StorageError::Backend(format!("read host_id: {e}")))?;
    let kind_s: String = row
        .try_get("kind")
        .map_err(|e| StorageError::Backend(format!("read kind: {e}")))?;
    let kind = ForwardKind::from_tag(&kind_s).ok_or_else(|| StorageError::Malformed {
        entity: "forwards",
        reason: format!("unknown kind {kind_s:?}"),
    })?;
    let bind_host: String = row
        .try_get("bind_host")
        .map_err(|e| StorageError::Backend(format!("read bind_host: {e}")))?;
    let bind_port: i64 = row
        .try_get("bind_port")
        .map_err(|e| StorageError::Backend(format!("read bind_port: {e}")))?;
    let target_host: String = row
        .try_get("target_host")
        .map_err(|e| StorageError::Backend(format!("read target_host: {e}")))?;
    let target_port: i64 = row
        .try_get("target_port")
        .map_err(|e| StorageError::Backend(format!("read target_port: {e}")))?;
    let auto_start: i64 = row
        .try_get("auto_start")
        .map_err(|e| StorageError::Backend(format!("read auto_start: {e}")))?;
    let created_at_s: String = row
        .try_get("created_at")
        .map_err(|e| StorageError::Backend(format!("read created_at: {e}")))?;

    Ok(SavedForward {
        id: ForwardId::from_raw(id),
        host_id: HostId::from_raw(host_id),
        kind,
        bind_host,
        bind_port: u16::try_from(bind_port).unwrap_or(0),
        target_host,
        target_port: u16::try_from(target_port).unwrap_or(0),
        auto_start: auto_start != 0,
        created_at: parse_datetime("forwards.created_at", &created_at_s)?,
    })
}
