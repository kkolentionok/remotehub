//! `CredentialStore` implementation: SQLite metadata + keychain secrets.
//!
//! ## Create flow (atomicity)
//!
//! The data model splits a credential across two systems:
//! - **Keychain**: the actual secret bytes.
//! - **SQLite**: metadata + a `keychain_ref` pointing to the secret.
//!
//! We have no two-phase-commit available. The chosen ordering is
//! **keychain first, database second**, and on database failure we
//! attempt to delete the keychain entry. Rationale:
//!
//! - If keychain write fails → no DB row exists yet → nothing to clean.
//!   User retries with a fresh attempt.
//! - If keychain write succeeds, DB write fails → we try to delete the
//!   keychain entry. If that cleanup also fails, the user has an orphan
//!   keychain entry visible in OS UI but no corresponding row. This is
//!   a benign leak: user can remove it manually, and the secret is
//!   still under our service name so it's identifiable.
//!
//! The opposite order (DB first) would leave a dangling DB row pointing
//! to a missing keychain entry, which breaks `reveal()` for that
//! credential and is harder to clean up.

use std::sync::Arc;

use async_trait::async_trait;
use sqlx::Row;
use tracing::{instrument, warn};

use rh_core::{
    Credential, CredentialId, CredentialKind, CredentialStore, HostId, KeychainRef, RevealError,
    RevealedSecret, SecretValue, StorageError,
};

use crate::db::Db;
use crate::host_store::{map_err, parse_datetime};
use crate::keychain::Keychain;

/// SQLite + keychain backed `CredentialStore`.
#[derive(Clone)]
pub struct SqliteCredentialStore {
    db: Db,
    keychain: Arc<dyn Keychain>,
}

impl std::fmt::Debug for SqliteCredentialStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteCredentialStore")
            .field("db", &self.db.source())
            .field("keychain", &"<dyn Keychain>")
            .finish()
    }
}

impl SqliteCredentialStore {
    #[must_use]
    pub fn new(db: Db, keychain: Arc<dyn Keychain>) -> Self {
        Self { db, keychain }
    }

    /// Write the secret(s) for a credential to keychain. Symmetric for
    /// create and rotate paths.
    async fn write_secrets(
        &self,
        cred: &Credential,
        secret: SecretValue,
        passphrase: Option<SecretValue>,
    ) -> Result<(), StorageError> {
        if cred.kind.requires_keychain_secret() {
            self.keychain
                .set(&cred.keychain_ref, &secret)
                .await
                .map_err(|e| StorageError::Backend(format!("keychain set: {e}")))?;
        }

        if let Some(pp) = passphrase {
            let pp_ref = KeychainRef::for_passphrase(&cred.id);
            self.keychain
                .set(&pp_ref, &pp)
                .await
                .map_err(|e| StorageError::Backend(format!("keychain passphrase set: {e}")))?;
        }
        Ok(())
    }

    /// Best-effort cleanup of orphaned keychain entries. Used when a DB
    /// transaction fails partway through. Errors are logged, not
    /// propagated — the caller is already failing for a different reason.
    async fn cleanup_secrets(&self, cred: &Credential) {
        if cred.kind.requires_keychain_secret() {
            if let Err(e) = self.keychain.delete(&cred.keychain_ref).await {
                warn!(
                    keychain_ref = %cred.keychain_ref,
                    error = %e,
                    "failed to clean up orphaned keychain entry; user may need to remove manually"
                );
            }
        }
        let pp_ref = KeychainRef::for_passphrase(&cred.id);
        if let Err(e) = self.keychain.delete(&pp_ref).await {
            warn!(
                keychain_ref = %pp_ref,
                error = %e,
                "failed to clean up orphaned passphrase entry"
            );
        }
    }
}

#[async_trait]
impl CredentialStore for SqliteCredentialStore {
    #[instrument(level = "debug", skip(self, credential, secret, passphrase), fields(cred_id = %credential.id))]
    async fn create(
        &self,
        credential: &Credential,
        secret: SecretValue,
        passphrase: Option<SecretValue>,
    ) -> Result<(), StorageError> {
        // Step 1: keychain first.
        self.write_secrets(credential, secret, passphrase).await?;

        // Step 2: DB. On failure, attempt cleanup.
        let result = sqlx::query(
            r"
            INSERT INTO credentials (
                id, name, kind, username, keychain_ref, created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?)
            ",
        )
        .bind(credential.id.as_str())
        .bind(&credential.name)
        .bind(credential_kind_to_str(credential.kind))
        .bind(&credential.username)
        .bind(credential.keychain_ref.as_str())
        .bind(credential.created_at.to_rfc3339())
        .bind(credential.updated_at.to_rfc3339())
        .execute(self.db.pool())
        .await;

        match result {
            Ok(_) => Ok(()),
            Err(e) => {
                let err = map_err(e);
                self.cleanup_secrets(credential).await;
                Err(err)
            }
        }
    }

    #[instrument(level = "debug", skip(self), fields(cred_id = %id))]
    async fn get(&self, id: &CredentialId) -> Result<Credential, StorageError> {
        let row = sqlx::query(SELECT_CRED_BY_ID)
            .bind(id.as_str())
            .fetch_optional(self.db.pool())
            .await
            .map_err(map_err)?;

        let row = row.ok_or_else(|| {
            StorageError::Backend(format!("credential {} not found", id.as_str()))
        })?;
        row_to_credential(&row)
    }

    #[instrument(level = "debug", skip(self))]
    async fn list(&self) -> Result<Vec<Credential>, StorageError> {
        let rows = sqlx::query(SELECT_CRED_PREFIX)
            .fetch_all(self.db.pool())
            .await
            .map_err(map_err)?;
        rows.iter().map(row_to_credential).collect()
    }

    #[instrument(level = "debug", skip(self, credential), fields(cred_id = %credential.id))]
    async fn update(&self, credential: &Credential) -> Result<(), StorageError> {
        // Metadata only — kind and keychain_ref are NOT updateable.
        let result = sqlx::query(
            "UPDATE credentials SET name = ?, username = ?, updated_at = ? WHERE id = ?",
        )
        .bind(&credential.name)
        .bind(&credential.username)
        .bind(credential.updated_at.to_rfc3339())
        .bind(credential.id.as_str())
        .execute(self.db.pool())
        .await
        .map_err(map_err)?;

        if result.rows_affected() == 0 {
            return Err(StorageError::Backend(format!(
                "credential {} not found for update",
                credential.id.as_str()
            )));
        }
        Ok(())
    }

    #[instrument(level = "debug", skip(self, secret, passphrase), fields(cred_id = %id))]
    async fn rotate_secret(
        &self,
        id: &CredentialId,
        secret: SecretValue,
        passphrase: Option<SecretValue>,
    ) -> Result<(), StorageError> {
        // Load current credential to know its kind (we need to decide
        // whether to write a primary secret or skip it for SshKeyAgent).
        let credential = self.get(id).await?;
        self.write_secrets(&credential, secret, passphrase).await?;

        let now = chrono::Utc::now().to_rfc3339();
        let result = sqlx::query("UPDATE credentials SET updated_at = ? WHERE id = ?")
            .bind(&now)
            .bind(id.as_str())
            .execute(self.db.pool())
            .await
            .map_err(map_err)?;

        if result.rows_affected() == 0 {
            return Err(StorageError::Backend(format!(
                "credential {} disappeared during rotation",
                id.as_str()
            )));
        }
        Ok(())
    }

    #[instrument(level = "debug", skip(self), fields(cred_id = %id))]
    async fn delete(&self, id: &CredentialId) -> Result<(), StorageError> {
        // We need keychain_ref before the row vanishes. Look it up,
        // then delete row, then delete keychain entries.
        let credential = self.get(id).await?;

        // DB delete first this time — the DB row is the canonical
        // existence marker. If keychain cleanup fails afterwards, the
        // credential is functionally gone (user can't reach the secret),
        // just slightly wasteful in keychain storage.
        let result = sqlx::query("DELETE FROM credentials WHERE id = ?")
            .bind(id.as_str())
            .execute(self.db.pool())
            .await
            .map_err(map_err)?;

        if result.rows_affected() == 0 {
            return Err(StorageError::Backend(format!(
                "credential {} disappeared before delete",
                id.as_str()
            )));
        }

        // Best-effort cleanup. Idempotent (Keychain::delete returns Ok
        // for missing entries) so we just log any unexpected error.
        self.cleanup_secrets(&credential).await;
        Ok(())
    }

    #[instrument(level = "debug", skip(self), fields(cred_id = %id))]
    async fn reveal(&self, id: &CredentialId) -> Result<RevealedSecret, RevealError> {
        let credential = self.get(id).await?;

        if !credential.kind.requires_keychain_secret() {
            // SshKeyAgent: no secret in keychain. Return empty as a
            // sentinel — caller checks `credential.kind` separately.
            return Ok(RevealedSecret::new(Vec::new()));
        }

        let secret = self.keychain.get(&credential.keychain_ref).await?;
        Ok(secret)
    }

    #[instrument(level = "debug", skip(self), fields(cred_id = %id))]
    async fn reveal_passphrase(
        &self,
        id: &CredentialId,
    ) -> Result<Option<RevealedSecret>, RevealError> {
        let credential = self.get(id).await?;

        if credential.kind != CredentialKind::SshKey {
            return Ok(None);
        }

        let pp_ref = KeychainRef::for_passphrase(id);
        match self.keychain.get(&pp_ref).await {
            Ok(s) => Ok(Some(s)),
            Err(rh_core::SecretError::NotFound) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    #[instrument(level = "debug", skip(self), fields(host_id = %host_id, cred_id = %credential_id, default = set_as_default))]
    async fn link_host(
        &self,
        host_id: &HostId,
        credential_id: &CredentialId,
        set_as_default: bool,
    ) -> Result<(), StorageError> {
        // Single transaction: link row + (optionally) host.default_credential_id.
        let mut tx = self
            .db
            .pool()
            .begin()
            .await
            .map_err(|e| StorageError::Backend(format!("begin tx: {e}")))?;

        // If this is the default, clear is_default on any other links
        // for the same host first (we want at most one default).
        if set_as_default {
            sqlx::query("UPDATE host_credentials SET is_default = 0 WHERE host_id = ?")
                .bind(host_id.as_str())
                .execute(&mut *tx)
                .await
                .map_err(map_err)?;
        }

        sqlx::query(
            r"
            INSERT INTO host_credentials (host_id, credential_id, is_default)
            VALUES (?, ?, ?)
            ON CONFLICT (host_id, credential_id) DO UPDATE SET is_default = excluded.is_default
            ",
        )
        .bind(host_id.as_str())
        .bind(credential_id.as_str())
        .bind(i32::from(set_as_default))
        .execute(&mut *tx)
        .await
        .map_err(map_err)?;

        if set_as_default {
            sqlx::query("UPDATE hosts SET default_credential_id = ? WHERE id = ?")
                .bind(credential_id.as_str())
                .bind(host_id.as_str())
                .execute(&mut *tx)
                .await
                .map_err(map_err)?;
        }

        tx.commit()
            .await
            .map_err(|e| StorageError::Backend(format!("commit: {e}")))?;
        Ok(())
    }

    #[instrument(level = "debug", skip(self), fields(host_id = %host_id, cred_id = %credential_id))]
    async fn unlink_host(
        &self,
        host_id: &HostId,
        credential_id: &CredentialId,
    ) -> Result<(), StorageError> {
        let mut tx = self
            .db
            .pool()
            .begin()
            .await
            .map_err(|e| StorageError::Backend(format!("begin tx: {e}")))?;

        // Was this the default? If so, clear host.default_credential_id.
        let was_default: Option<i64> = sqlx::query_scalar(
            "SELECT is_default FROM host_credentials WHERE host_id = ? AND credential_id = ?",
        )
        .bind(host_id.as_str())
        .bind(credential_id.as_str())
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_err)?;

        sqlx::query(
            "DELETE FROM host_credentials WHERE host_id = ? AND credential_id = ?",
        )
        .bind(host_id.as_str())
        .bind(credential_id.as_str())
        .execute(&mut *tx)
        .await
        .map_err(map_err)?;

        if was_default == Some(1) {
            sqlx::query("UPDATE hosts SET default_credential_id = NULL WHERE id = ?")
                .bind(host_id.as_str())
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

// ---- Helpers -------------------------------------------------------

const SELECT_CRED_PREFIX: &str = "
    SELECT id, name, kind, username, keychain_ref, created_at, updated_at
    FROM credentials
    ORDER BY name COLLATE NOCASE
";

const SELECT_CRED_BY_ID: &str = "
    SELECT id, name, kind, username, keychain_ref, created_at, updated_at
    FROM credentials
    WHERE id = ?
";

fn credential_kind_to_str(kind: CredentialKind) -> &'static str {
    match kind {
        CredentialKind::Password => "password",
        CredentialKind::SshKey => "ssh_key",
        CredentialKind::SshKeyAgent => "ssh_key_agent",
    }
}

fn credential_kind_from_str(raw: &str) -> Result<CredentialKind, StorageError> {
    match raw {
        "password" => Ok(CredentialKind::Password),
        "ssh_key" => Ok(CredentialKind::SshKey),
        "ssh_key_agent" => Ok(CredentialKind::SshKeyAgent),
        other => Err(StorageError::Malformed {
            entity: "credentials.kind",
            reason: format!("unknown value {other:?}"),
        }),
    }
}

fn row_to_credential(row: &sqlx::sqlite::SqliteRow) -> Result<Credential, StorageError> {
    let id: String = row
        .try_get("id")
        .map_err(|e| StorageError::Backend(format!("read id: {e}")))?;
    let name: String = row
        .try_get("name")
        .map_err(|e| StorageError::Backend(format!("read name: {e}")))?;
    let kind_s: String = row
        .try_get("kind")
        .map_err(|e| StorageError::Backend(format!("read kind: {e}")))?;
    let username: String = row
        .try_get("username")
        .map_err(|e| StorageError::Backend(format!("read username: {e}")))?;
    let keychain_ref_s: String = row
        .try_get("keychain_ref")
        .map_err(|e| StorageError::Backend(format!("read keychain_ref: {e}")))?;
    let created_at_s: String = row
        .try_get("created_at")
        .map_err(|e| StorageError::Backend(format!("read created_at: {e}")))?;
    let updated_at_s: String = row
        .try_get("updated_at")
        .map_err(|e| StorageError::Backend(format!("read updated_at: {e}")))?;

    Ok(Credential {
        id: CredentialId::from_raw(id),
        name,
        kind: credential_kind_from_str(&kind_s)?,
        username,
        keychain_ref: KeychainRef::from_raw(keychain_ref_s),
        created_at: parse_datetime("credentials.created_at", &created_at_s)?,
        updated_at: parse_datetime("credentials.updated_at", &updated_at_s)?,
    })
}
