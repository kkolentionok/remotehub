//! Storage traits.
//!
//! These describe what the persistence layer must do, not how. Concrete
//! implementations live in `rh-storage` (SQLite + OS keychain). The
//! split exists so:
//!
//! - The session crates (`rh-ssh`, `rh-rdp`) can depend on the trait
//!   without pulling in `sqlx`/`keyring`, making their tests cheap.
//! - Future alternative backends (in-memory mock, encrypted-at-rest)
//!   can drop in without touching callers.
//!
//! All traits are `Send + Sync` so they can be shared across tasks
//! behind `Arc<dyn Trait>`. All methods are `async` for I/O.

use async_trait::async_trait;

use crate::error::{SecretError, StorageError};
use crate::id::{CredentialId, GroupId, HostId};
use crate::secret::{RevealedSecret, SecretValue};
use crate::settings::Settings;
use crate::types::{Credential, Host, HostGroup, Protocol};

/// Filter passed to [`HostStore::list`]. All fields are optional; `None`
/// means "don't filter on this dimension". Combining multiple fields
/// applies them with AND semantics.
#[derive(Debug, Clone, Default)]
pub struct HostFilter {
    /// Limit to hosts in this group. `None` means all groups (including
    /// ungrouped hosts).
    pub group_id: Option<GroupId>,

    /// Limit to hosts of this protocol.
    pub protocol: Option<Protocol>,

    /// Substring match (case-insensitive) against name, hostname, or
    /// any tag. `None` means no text filter.
    pub search: Option<String>,

    /// Maximum number of rows to return. `None` means no limit (use
    /// with care — the storage layer may impose its own ceiling).
    pub limit: Option<u32>,
}

#[async_trait]
pub trait HostStore: Send + Sync {
    async fn create(&self, host: &Host) -> Result<(), StorageError>;
    async fn get(&self, id: &HostId) -> Result<Host, StorageError>;
    async fn list(&self, filter: HostFilter) -> Result<Vec<Host>, StorageError>;
    async fn update(&self, host: &Host) -> Result<(), StorageError>;
    async fn delete(&self, id: &HostId) -> Result<(), StorageError>;
}

#[async_trait]
pub trait GroupStore: Send + Sync {
    async fn create(&self, group: &HostGroup) -> Result<(), StorageError>;
    async fn get(&self, id: &GroupId) -> Result<HostGroup, StorageError>;
    /// List ALL groups (flat). UI builds the tree by `parent_id`.
    async fn list(&self) -> Result<Vec<HostGroup>, StorageError>;
    /// Rename a group. Use `move_to` to change parentage.
    async fn rename(&self, id: &GroupId, new_name: &str) -> Result<(), StorageError>;
    /// Re-parent a group. Implementations MUST reject cycles
    /// (moving A under B where B is a descendant of A).
    async fn move_to(&self, id: &GroupId, new_parent: Option<&GroupId>) -> Result<(), StorageError>;
    /// Delete a group. Implementations decide cascade vs. orphan; per
    /// the data model, child groups cascade-delete and contained hosts
    /// fall back to the root.
    async fn delete(&self, id: &GroupId) -> Result<(), StorageError>;
}

#[async_trait]
pub trait CredentialStore: Send + Sync {
    /// Create a credential and store its secret in the OS keychain.
    ///
    /// Implementations MUST write to the keychain before the database,
    /// so a DB rollback can't leave a dangling keychain entry in a
    /// state the user can't see. If keychain write succeeds but DB
    /// write fails, implementations SHOULD attempt to delete the
    /// keychain entry; failure to clean up is logged but not propagated
    /// (the user can rerun the create operation).
    async fn create(
        &self,
        credential: &Credential,
        secret: SecretValue,
        passphrase: Option<SecretValue>,
    ) -> Result<(), StorageError>;

    async fn get(&self, id: &CredentialId) -> Result<Credential, StorageError>;
    async fn list(&self) -> Result<Vec<Credential>, StorageError>;

    /// Update metadata only (name, username). Secret is not touched —
    /// use [`Self::rotate_secret`] to change it.
    async fn update(&self, credential: &Credential) -> Result<(), StorageError>;

    /// Replace the keychain-stored secret. The metadata `updated_at`
    /// timestamp is bumped as a side effect.
    async fn rotate_secret(
        &self,
        id: &CredentialId,
        secret: SecretValue,
        passphrase: Option<SecretValue>,
    ) -> Result<(), StorageError>;

    /// Delete credential metadata and the keychain entry. Cascades
    /// through `host_credentials`.
    async fn delete(&self, id: &CredentialId) -> Result<(), StorageError>;

    /// Read the secret from keychain. Returns a [`RevealedSecret`]
    /// that the caller is expected to drop (and thus zeroize) as
    /// soon as authentication completes.
    ///
    /// Distinguishes [`SecretError::NotFound`] (keychain entry missing
    /// — abnormal, the DB and keychain are out of sync) from regular
    /// not-found-by-id, which is reported as [`StorageError`].
    async fn reveal(&self, id: &CredentialId) -> Result<RevealedSecret, RevealError>;

    /// Same as [`Self::reveal`] but for an SSH key passphrase. Returns
    /// `Ok(None)` if the credential isn't an SshKey or has no passphrase.
    async fn reveal_passphrase(
        &self,
        id: &CredentialId,
    ) -> Result<Option<RevealedSecret>, RevealError>;

    /// Link a credential to a host. `set_as_default` updates
    /// `hosts.default_credential_id` atomically with the link.
    async fn link_host(
        &self,
        host_id: &HostId,
        credential_id: &CredentialId,
        set_as_default: bool,
    ) -> Result<(), StorageError>;

    async fn unlink_host(
        &self,
        host_id: &HostId,
        credential_id: &CredentialId,
    ) -> Result<(), StorageError>;
}

/// Error returned by `reveal*` methods. Distinguishes "credential row
/// missing" (storage problem) from "keychain entry missing" (sync
/// problem) so callers can react appropriately.
#[derive(Debug, thiserror::Error)]
pub enum RevealError {
    #[error("credential not found in storage")]
    Storage(#[from] StorageError),

    #[error("keychain access failed")]
    Secret(#[from] SecretError),
}

#[async_trait]
pub trait SettingsStore: Send + Sync {
    /// Load all settings into a [`Settings`] struct. Missing keys fall
    /// back to defaults from [`Settings::default()`].
    async fn load(&self) -> Result<Settings, StorageError>;

    /// Patch settings. Only fields present in `patch` are written;
    /// the rest remain at their current values.
    ///
    /// Storage layer receives a JSON object keyed by setting name
    /// (the constants in `crate::settings::keys`). Validation of
    /// individual values is the storage layer's job — it has the
    /// type information needed to check each value against its
    /// expected shape.
    async fn save(&self, patch: serde_json::Value) -> Result<(), StorageError>;
}
