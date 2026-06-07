//! `KnownHostsStore` implementation (TOFU host-key pinning).
//!
//! Host keys are public material, so they live in SQLite — not the
//! keychain. Identity is `(hostname, port)`; `remember` upserts so
//! re-trusting a changed key just overwrites the old fingerprint.

use async_trait::async_trait;
use chrono::Utc;
use sqlx::Row;
use tracing::instrument;

use rh_core::types::{KnownHostEntry, KnownHostKey};
use rh_core::{KnownHostsStore, StorageError};

use crate::db::Db;
use crate::host_store::{map_err, parse_datetime};

#[derive(Debug, Clone)]
pub struct SqliteKnownHostsStore {
    db: Db,
}

impl SqliteKnownHostsStore {
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

#[async_trait]
impl KnownHostsStore for SqliteKnownHostsStore {
    #[instrument(level = "debug", skip(self))]
    async fn lookup(
        &self,
        hostname: &str,
        port: u16,
    ) -> Result<Option<KnownHostKey>, StorageError> {
        let row = sqlx::query(
            "SELECT key_type, fingerprint_sha256 FROM known_hosts \
             WHERE hostname = ? AND port = ?",
        )
        .bind(hostname)
        .bind(i64::from(port))
        .fetch_optional(self.db.pool())
        .await
        .map_err(map_err)?;

        match row {
            None => Ok(None),
            Some(r) => {
                let key_type: String = r
                    .try_get("key_type")
                    .map_err(|e| StorageError::Backend(format!("read key_type: {e}")))?;
                let fingerprint_sha256: String = r
                    .try_get("fingerprint_sha256")
                    .map_err(|e| StorageError::Backend(format!("read fingerprint: {e}")))?;
                Ok(Some(KnownHostKey {
                    key_type,
                    fingerprint_sha256,
                }))
            }
        }
    }

    #[instrument(level = "debug", skip(self))]
    async fn remember(
        &self,
        hostname: &str,
        port: u16,
        key: &KnownHostKey,
    ) -> Result<(), StorageError> {
        sqlx::query(
            r"
            INSERT INTO known_hosts (hostname, port, key_type, fingerprint_sha256, created_at)
            VALUES (?, ?, ?, ?, ?)
            ON CONFLICT (hostname, port) DO UPDATE SET
                key_type = excluded.key_type,
                fingerprint_sha256 = excluded.fingerprint_sha256,
                created_at = excluded.created_at
            ",
        )
        .bind(hostname)
        .bind(i64::from(port))
        .bind(&key.key_type)
        .bind(&key.fingerprint_sha256)
        .bind(Utc::now().to_rfc3339())
        .execute(self.db.pool())
        .await
        .map_err(map_err)?;
        Ok(())
    }

    #[instrument(level = "debug", skip(self))]
    async fn forget(&self, hostname: &str, port: u16) -> Result<(), StorageError> {
        sqlx::query("DELETE FROM known_hosts WHERE hostname = ? AND port = ?")
            .bind(hostname)
            .bind(i64::from(port))
            .execute(self.db.pool())
            .await
            .map_err(map_err)?;
        Ok(())
    }

    #[instrument(level = "debug", skip(self))]
    async fn list(&self) -> Result<Vec<KnownHostEntry>, StorageError> {
        let rows = sqlx::query(
            "SELECT hostname, port, key_type, fingerprint_sha256, created_at \
             FROM known_hosts ORDER BY hostname, port",
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(map_err)?;

        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            let hostname: String = r
                .try_get("hostname")
                .map_err(|e| StorageError::Backend(format!("read hostname: {e}")))?;
            let port: i64 = r
                .try_get("port")
                .map_err(|e| StorageError::Backend(format!("read port: {e}")))?;
            let key_type: String = r
                .try_get("key_type")
                .map_err(|e| StorageError::Backend(format!("read key_type: {e}")))?;
            let fingerprint_sha256: String = r
                .try_get("fingerprint_sha256")
                .map_err(|e| StorageError::Backend(format!("read fingerprint: {e}")))?;
            let created_at_s: String = r
                .try_get("created_at")
                .map_err(|e| StorageError::Backend(format!("read created_at: {e}")))?;
            let port = u16::try_from(port).map_err(|_| StorageError::Malformed {
                entity: "known_hosts.port",
                reason: format!("out of range: {port}"),
            })?;
            out.push(KnownHostEntry {
                hostname,
                port,
                key_type,
                fingerprint_sha256,
                created_at: parse_datetime("known_hosts.created_at", &created_at_s)?,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn store() -> SqliteKnownHostsStore {
        let db = Db::open_memory().await.unwrap();
        SqliteKnownHostsStore::new(db)
    }

    fn key(fp: &str) -> KnownHostKey {
        KnownHostKey {
            key_type: "ssh-ed25519".to_string(),
            fingerprint_sha256: fp.to_string(),
        }
    }

    #[tokio::test]
    async fn lookup_absent_is_none() {
        let s = store().await;
        assert!(s.lookup("example.com", 22).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn remember_then_lookup_roundtrips() {
        let s = store().await;
        s.remember("example.com", 22, &key("AAAA")).await.unwrap();
        let got = s.lookup("example.com", 22).await.unwrap().unwrap();
        assert_eq!(got.fingerprint_sha256, "AAAA");
        assert_eq!(got.key_type, "ssh-ed25519");
    }

    #[tokio::test]
    async fn remember_overwrites_on_conflict() {
        let s = store().await;
        s.remember("example.com", 22, &key("OLD")).await.unwrap();
        s.remember("example.com", 22, &key("NEW")).await.unwrap();
        let got = s.lookup("example.com", 22).await.unwrap().unwrap();
        assert_eq!(got.fingerprint_sha256, "NEW");
    }

    #[tokio::test]
    async fn port_is_part_of_identity() {
        let s = store().await;
        s.remember("example.com", 22, &key("A")).await.unwrap();
        s.remember("example.com", 2222, &key("B")).await.unwrap();
        assert_eq!(
            s.lookup("example.com", 22).await.unwrap().unwrap().fingerprint_sha256,
            "A"
        );
        assert_eq!(
            s.lookup("example.com", 2222).await.unwrap().unwrap().fingerprint_sha256,
            "B"
        );
    }

    #[tokio::test]
    async fn forget_removes() {
        let s = store().await;
        s.remember("example.com", 22, &key("A")).await.unwrap();
        s.forget("example.com", 22).await.unwrap();
        assert!(s.lookup("example.com", 22).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn list_returns_all_sorted() {
        let s = store().await;
        s.remember("b.example.com", 22, &key("B")).await.unwrap();
        s.remember("a.example.com", 22, &key("A")).await.unwrap();
        let all = s.list().await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].hostname, "a.example.com");
        assert_eq!(all[0].fingerprint_sha256, "A");
        assert_eq!(all[1].hostname, "b.example.com");
    }
}
