//! `RdpCertStore` implementation (TOFU RDP server-certificate pinning).
//!
//! The RDP analog of `known_hosts_store`. Certs are public material, so
//! they live in SQLite — not the keychain. Identity is `(hostname,
//! port)`; `remember` upserts so re-trusting a changed cert overwrites
//! the old fingerprint.

use async_trait::async_trait;
use chrono::Utc;
use sqlx::Row;
use tracing::instrument;

use rh_core::types::{RdpCertEntry, TrustedCert};
use rh_core::{RdpCertStore, StorageError};

use crate::db::Db;
use crate::host_store::{map_err, parse_datetime};

#[derive(Debug, Clone)]
pub struct SqliteRdpCertStore {
    db: Db,
}

impl SqliteRdpCertStore {
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

#[async_trait]
impl RdpCertStore for SqliteRdpCertStore {
    #[instrument(level = "debug", skip(self))]
    async fn lookup(
        &self,
        hostname: &str,
        port: u16,
    ) -> Result<Option<TrustedCert>, StorageError> {
        let row = sqlx::query(
            "SELECT fingerprint_sha256, subject, trusted_at FROM rdp_known_certs \
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
                let fingerprint_sha256: String = r
                    .try_get("fingerprint_sha256")
                    .map_err(|e| StorageError::Backend(format!("read fingerprint: {e}")))?;
                let subject: String = r
                    .try_get("subject")
                    .map_err(|e| StorageError::Backend(format!("read subject: {e}")))?;
                let trusted_at_s: String = r
                    .try_get("trusted_at")
                    .map_err(|e| StorageError::Backend(format!("read trusted_at: {e}")))?;
                Ok(Some(TrustedCert {
                    fingerprint_sha256,
                    subject,
                    trusted_at: parse_datetime("rdp_known_certs.trusted_at", &trusted_at_s)?,
                }))
            }
        }
    }

    #[instrument(level = "debug", skip(self))]
    async fn remember(
        &self,
        hostname: &str,
        port: u16,
        fingerprint_sha256: &str,
        subject: &str,
    ) -> Result<(), StorageError> {
        sqlx::query(
            r"
            INSERT INTO rdp_known_certs (hostname, port, fingerprint_sha256, subject, trusted_at)
            VALUES (?, ?, ?, ?, ?)
            ON CONFLICT (hostname, port) DO UPDATE SET
                fingerprint_sha256 = excluded.fingerprint_sha256,
                subject = excluded.subject,
                trusted_at = excluded.trusted_at
            ",
        )
        .bind(hostname)
        .bind(i64::from(port))
        .bind(fingerprint_sha256)
        .bind(subject)
        .bind(Utc::now().to_rfc3339())
        .execute(self.db.pool())
        .await
        .map_err(map_err)?;
        Ok(())
    }

    #[instrument(level = "debug", skip(self))]
    async fn forget(&self, hostname: &str, port: u16) -> Result<(), StorageError> {
        sqlx::query("DELETE FROM rdp_known_certs WHERE hostname = ? AND port = ?")
            .bind(hostname)
            .bind(i64::from(port))
            .execute(self.db.pool())
            .await
            .map_err(map_err)?;
        Ok(())
    }

    #[instrument(level = "debug", skip(self))]
    async fn list(&self) -> Result<Vec<RdpCertEntry>, StorageError> {
        let rows = sqlx::query(
            "SELECT hostname, port, fingerprint_sha256, subject, trusted_at \
             FROM rdp_known_certs ORDER BY hostname, port",
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
            let fingerprint_sha256: String = r
                .try_get("fingerprint_sha256")
                .map_err(|e| StorageError::Backend(format!("read fingerprint: {e}")))?;
            let subject: String = r
                .try_get("subject")
                .map_err(|e| StorageError::Backend(format!("read subject: {e}")))?;
            let trusted_at_s: String = r
                .try_get("trusted_at")
                .map_err(|e| StorageError::Backend(format!("read trusted_at: {e}")))?;
            let port = u16::try_from(port).map_err(|_| StorageError::Malformed {
                entity: "rdp_known_certs.port",
                reason: format!("out of range: {port}"),
            })?;
            out.push(RdpCertEntry {
                hostname,
                port,
                fingerprint_sha256,
                subject,
                trusted_at: parse_datetime("rdp_known_certs.trusted_at", &trusted_at_s)?,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn store() -> SqliteRdpCertStore {
        let db = Db::open_memory().await.unwrap();
        SqliteRdpCertStore::new(db)
    }

    #[tokio::test]
    async fn remember_then_lookup_roundtrips() {
        let s = store().await;
        s.remember("win.example.com", 3389, "FP", "CN=win").await.unwrap();
        let got = s.lookup("win.example.com", 3389).await.unwrap().unwrap();
        assert_eq!(got.fingerprint_sha256, "FP");
        assert_eq!(got.subject, "CN=win");
    }

    #[tokio::test]
    async fn remember_overwrites_on_conflict() {
        let s = store().await;
        s.remember("win.example.com", 3389, "OLD", "CN=a").await.unwrap();
        s.remember("win.example.com", 3389, "NEW", "CN=b").await.unwrap();
        let got = s.lookup("win.example.com", 3389).await.unwrap().unwrap();
        assert_eq!(got.fingerprint_sha256, "NEW");
    }

    #[tokio::test]
    async fn forget_removes() {
        let s = store().await;
        s.remember("win.example.com", 3389, "FP", "CN=win").await.unwrap();
        s.forget("win.example.com", 3389).await.unwrap();
        assert!(s.lookup("win.example.com", 3389).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn list_returns_all_sorted() {
        let s = store().await;
        s.remember("b.example.com", 3389, "B", "CN=b").await.unwrap();
        s.remember("a.example.com", 3389, "A", "CN=a").await.unwrap();
        let all = s.list().await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].hostname, "a.example.com");
        assert_eq!(all[1].hostname, "b.example.com");
    }
}
