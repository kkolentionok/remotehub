//! `SyncMetaStore` implementation backed by SQLite.
//!
//! Stores one row per replicated record in the generic `sync_meta` table
//! (keyed by `kind, id`), carrying the record's last-write HLC stamp split
//! into `rev_wall` / `rev_counter`, its `origin` device, and a `deleted`
//! tombstone flag. See `rh_core::store::SyncMetaStore` for the contract and
//! `docs/specs/sync.md` for why provenance lives in its own table.
//!
//! All writes are UPSERTs on the `(kind, id)` primary key, so `bump` and
//! `tombstone` are idempotent and either one flips the `deleted` flag of an
//! existing row (a `bump` resurrects a tombstone; a `tombstone` buries a live
//! record). Runtime `query()` + `bind` is used (matching `host_store`) so no
//! compile-time database is needed.

use async_trait::async_trait;
use sqlx::Row;
use tracing::instrument;

use rh_core::{StorageError, SyncMetaStore, SyncStamp};

use crate::db::Db;

/// SQLite-backed implementation of [`SyncMetaStore`].
#[derive(Debug, Clone)]
pub struct SqliteSyncMetaStore {
    db: Db,
}

impl SqliteSyncMetaStore {
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Shared UPSERT for both `bump` (deleted = false) and `tombstone`
    /// (deleted = true).
    async fn upsert(
        &self,
        kind: &str,
        id: &str,
        stamp: &SyncStamp,
        deleted: bool,
    ) -> Result<(), StorageError> {
        sqlx::query(
            r"
            INSERT INTO sync_meta (kind, id, rev_wall, rev_counter, origin, deleted)
            VALUES (?, ?, ?, ?, ?, ?)
            ON CONFLICT(kind, id) DO UPDATE SET
                rev_wall    = excluded.rev_wall,
                rev_counter = excluded.rev_counter,
                origin      = excluded.origin,
                deleted     = excluded.deleted
            ",
        )
        .bind(kind)
        .bind(id)
        .bind(stamp.rev_wall as i64)
        .bind(i64::from(stamp.rev_counter))
        .bind(&stamp.origin)
        .bind(i64::from(deleted))
        .execute(self.db.pool())
        .await
        .map_err(|e| StorageError::Backend(format!("sync_meta upsert: {e}")))?;
        Ok(())
    }
}

/// Decode a `sync_meta` row's stamp columns into a [`SyncStamp`].
fn row_stamp(row: &sqlx::sqlite::SqliteRow) -> Result<SyncStamp, StorageError> {
    let rev_wall: i64 = row
        .try_get("rev_wall")
        .map_err(|e| StorageError::Backend(format!("decode rev_wall: {e}")))?;
    let rev_counter: i64 = row
        .try_get("rev_counter")
        .map_err(|e| StorageError::Backend(format!("decode rev_counter: {e}")))?;
    let origin: String = row
        .try_get("origin")
        .map_err(|e| StorageError::Backend(format!("decode origin: {e}")))?;
    Ok(SyncStamp {
        rev_wall: rev_wall as u64,
        rev_counter: rev_counter as u32,
        origin,
    })
}

#[async_trait]
impl SyncMetaStore for SqliteSyncMetaStore {
    #[instrument(level = "debug", skip(self, stamp))]
    async fn bump(&self, kind: &str, id: &str, stamp: &SyncStamp) -> Result<(), StorageError> {
        self.upsert(kind, id, stamp, false).await
    }

    #[instrument(level = "debug", skip(self, stamp))]
    async fn tombstone(
        &self,
        kind: &str,
        id: &str,
        stamp: &SyncStamp,
    ) -> Result<(), StorageError> {
        self.upsert(kind, id, stamp, true).await
    }

    #[instrument(level = "debug", skip(self))]
    async fn stamp_of(&self, kind: &str, id: &str) -> Result<Option<SyncStamp>, StorageError> {
        let row = sqlx::query(
            "SELECT rev_wall, rev_counter, origin FROM sync_meta WHERE kind = ? AND id = ?",
        )
        .bind(kind)
        .bind(id)
        .fetch_optional(self.db.pool())
        .await
        .map_err(|e| StorageError::Backend(format!("sync_meta stamp_of: {e}")))?;

        match row {
            Some(r) => Ok(Some(row_stamp(&r)?)),
            None => Ok(None),
        }
    }

    #[instrument(level = "debug", skip(self))]
    async fn live_stamps(&self, kind: &str) -> Result<Vec<(String, SyncStamp)>, StorageError> {
        let rows = sqlx::query(
            "SELECT id, rev_wall, rev_counter, origin FROM sync_meta \
             WHERE kind = ? AND deleted = 0 ORDER BY id",
        )
        .bind(kind)
        .fetch_all(self.db.pool())
        .await
        .map_err(|e| StorageError::Backend(format!("sync_meta live_stamps: {e}")))?;

        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            let id: String = r
                .try_get("id")
                .map_err(|e| StorageError::Backend(format!("decode id: {e}")))?;
            out.push((id, row_stamp(r)?));
        }
        Ok(out)
    }

    #[instrument(level = "debug", skip(self))]
    async fn tombstones(&self) -> Result<Vec<(String, String, SyncStamp)>, StorageError> {
        let rows = sqlx::query(
            "SELECT kind, id, rev_wall, rev_counter, origin FROM sync_meta \
             WHERE deleted = 1 ORDER BY kind, id",
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(|e| StorageError::Backend(format!("sync_meta tombstones: {e}")))?;

        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            let kind: String = r
                .try_get("kind")
                .map_err(|e| StorageError::Backend(format!("decode kind: {e}")))?;
            let id: String = r
                .try_get("id")
                .map_err(|e| StorageError::Backend(format!("decode id: {e}")))?;
            out.push((kind, id, row_stamp(r)?));
        }
        Ok(out)
    }

    #[instrument(level = "debug", skip(self))]
    async fn clear(&self, kind: &str, id: &str) -> Result<(), StorageError> {
        sqlx::query("DELETE FROM sync_meta WHERE kind = ? AND id = ?")
            .bind(kind)
            .bind(id)
            .execute(self.db.pool())
            .await
            .map_err(|e| StorageError::Backend(format!("sync_meta clear: {e}")))?;
        Ok(())
    }

    #[instrument(level = "debug", skip(self))]
    async fn clear_all(&self) -> Result<(), StorageError> {
        sqlx::query("DELETE FROM sync_meta")
            .execute(self.db.pool())
            .await
            .map_err(|e| StorageError::Backend(format!("sync_meta clear_all: {e}")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp(wall: u64, ctr: u32, origin: &str) -> SyncStamp {
        SyncStamp {
            rev_wall: wall,
            rev_counter: ctr,
            origin: origin.to_string(),
        }
    }

    async fn store() -> SqliteSyncMetaStore {
        let db = Db::open_memory().await.unwrap();
        SqliteSyncMetaStore::new(db)
    }

    #[tokio::test]
    async fn bump_then_stamp_of_roundtrips() {
        let s = store().await;
        s.bump("host", "h1", &stamp(1234, 7, "node-A")).await.unwrap();
        let got = s.stamp_of("host", "h1").await.unwrap().unwrap();
        assert_eq!(got, stamp(1234, 7, "node-A"));
        assert!(s.stamp_of("host", "missing").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn bump_upsert_overwrites() {
        let s = store().await;
        s.bump("host", "h1", &stamp(10, 0, "A")).await.unwrap();
        s.bump("host", "h1", &stamp(20, 3, "B")).await.unwrap();
        assert_eq!(s.stamp_of("host", "h1").await.unwrap().unwrap(), stamp(20, 3, "B"));
        // Still a single row (UPSERT, not insert).
        assert_eq!(s.live_stamps("host").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn tombstone_is_excluded_from_live_and_listed_as_tombstone() {
        let s = store().await;
        s.bump("host", "h1", &stamp(10, 0, "A")).await.unwrap();
        s.tombstone("host", "h1", &stamp(30, 0, "A")).await.unwrap();

        assert!(s.live_stamps("host").await.unwrap().is_empty());
        let ts = s.tombstones().await.unwrap();
        assert_eq!(ts, vec![("host".to_string(), "h1".to_string(), stamp(30, 0, "A"))]);
        // stamp_of still returns the (tombstone) stamp.
        assert_eq!(s.stamp_of("host", "h1").await.unwrap().unwrap(), stamp(30, 0, "A"));
    }

    #[tokio::test]
    async fn bump_resurrects_a_tombstone() {
        let s = store().await;
        s.tombstone("group", "g1", &stamp(10, 0, "A")).await.unwrap();
        assert_eq!(s.tombstones().await.unwrap().len(), 1);

        s.bump("group", "g1", &stamp(40, 0, "B")).await.unwrap();
        assert!(s.tombstones().await.unwrap().is_empty(), "no longer a tombstone");
        assert_eq!(
            s.live_stamps("group").await.unwrap(),
            vec![("g1".to_string(), stamp(40, 0, "B"))]
        );
    }

    #[tokio::test]
    async fn live_stamps_filters_by_kind() {
        let s = store().await;
        s.bump("host", "h1", &stamp(10, 0, "A")).await.unwrap();
        s.bump("host", "h2", &stamp(11, 0, "A")).await.unwrap();
        s.bump("credential", "c1", &stamp(12, 0, "A")).await.unwrap();

        let hosts = s.live_stamps("host").await.unwrap();
        assert_eq!(hosts.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), vec!["h1", "h2"]);
        assert_eq!(s.live_stamps("credential").await.unwrap().len(), 1);
        assert!(s.live_stamps("setting").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn clear_removes_the_row() {
        let s = store().await;
        s.tombstone("host", "h1", &stamp(10, 0, "A")).await.unwrap();
        s.clear("host", "h1").await.unwrap();
        assert!(s.stamp_of("host", "h1").await.unwrap().is_none());
        assert!(s.tombstones().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn large_wall_ms_survives_the_i64_roundtrip() {
        let s = store().await;
        // A realistic epoch-ms value (well within i63) must round-trip exactly.
        let big = 1_900_000_000_000u64;
        s.bump("setting", "k", &stamp(big, 4_000_000_000, "A")).await.unwrap();
        let got = s.stamp_of("setting", "k").await.unwrap().unwrap();
        assert_eq!(got.rev_wall, big);
        assert_eq!(got.rev_counter, 4_000_000_000);
    }
}
