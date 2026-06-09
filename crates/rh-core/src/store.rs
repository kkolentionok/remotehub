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
use crate::id::{CredentialId, ForwardId, GroupId, HostId};
use crate::secret::{RevealedSecret, SecretValue};
use crate::settings::Settings;
use crate::types::{Credential, Host, HostGroup, Protocol, SavedForward};

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

    /// Stamp `last_connected_at`. Machine-set by the session layer when a
    /// session reaches `Ready`; targeted write that doesn't disturb the
    /// rest of the row (unlike [`Self::update`]).
    async fn mark_connected(
        &self,
        id: &HostId,
        when: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), StorageError>;

    /// Stamp the auto-detected OS slug (e.g. "ubuntu"). Machine-set after
    /// a connect; targeted write that doesn't disturb the rest of the row.
    async fn mark_detected_os(&self, id: &HostId, os: &str) -> Result<(), StorageError>;
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

    /// All credentials linked to a host, default first. Used by the
    /// session layer to offer every available auth method (key(s) then
    /// password) and by the UI to render all active methods.
    async fn credentials_for_host(
        &self,
        host_id: &HostId,
    ) -> Result<Vec<Credential>, StorageError>;

    /// How many hosts a credential is currently linked to. Used to detect
    /// credentials orphaned by a host deletion (count == 0 → safe to delete
    /// the credential + its keychain secret; a positive count means it's
    /// still shared with another host and must be kept).
    async fn host_link_count(&self, id: &CredentialId) -> Result<i64, StorageError>;

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

/// Persistence for pinned SSH host keys (TOFU). Public material, so it
/// lives in SQLite alongside the other stores rather than the keychain.
///
/// Identity is `(hostname, port)`. The session actor calls [`Self::lookup`]
/// during the SSH handshake and, when the user trusts an unknown or
/// changed key, [`Self::remember`] to pin it.
#[async_trait]
pub trait KnownHostsStore: Send + Sync {
    /// The pinned key for `(hostname, port)`, or `None` if never trusted.
    async fn lookup(
        &self,
        hostname: &str,
        port: u16,
    ) -> Result<Option<crate::types::KnownHostKey>, StorageError>;

    /// Trust (or re-trust, overwriting any previous key) the given key
    /// for `(hostname, port)`. Upsert semantics.
    async fn remember(
        &self,
        hostname: &str,
        port: u16,
        key: &crate::types::KnownHostKey,
    ) -> Result<(), StorageError>;

    /// Forget the pinned key for `(hostname, port)`. No error if absent.
    async fn forget(&self, hostname: &str, port: u16) -> Result<(), StorageError>;

    /// All pinned host keys, for the management UI. Ordered by hostname.
    async fn list(&self) -> Result<Vec<crate::types::KnownHostEntry>, StorageError>;
}

/// TOFU store for RDP server certificates — the RDP analog of
/// [`KnownHostsStore`]. Pins `(hostname, port) → cert` so a changed cert
/// later can be flagged.
#[async_trait]
pub trait RdpCertStore: Send + Sync {
    /// The pinned cert for `(hostname, port)`, or `None` if never trusted.
    async fn lookup(
        &self,
        hostname: &str,
        port: u16,
    ) -> Result<Option<crate::types::TrustedCert>, StorageError>;

    /// Trust (or re-trust, overwriting) the cert for `(hostname, port)`.
    /// `trusted_at` is set to now. Upsert semantics.
    async fn remember(
        &self,
        hostname: &str,
        port: u16,
        fingerprint_sha256: &str,
        subject: &str,
    ) -> Result<(), StorageError>;

    /// Forget the pinned cert for `(hostname, port)`. No error if absent.
    async fn forget(&self, hostname: &str, port: u16) -> Result<(), StorageError>;

    /// All pinned certs, for the management UI. Ordered by hostname.
    async fn list(&self) -> Result<Vec<crate::types::RdpCertEntry>, StorageError>;
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

/// Provenance of a replicated record: the logical time of its last write
/// (`rev_wall`/`rev_counter` — a Hybrid Logical Clock stamp split into its
/// two integer fields for storage) and the device that produced it
/// (`origin`). This is the storage-neutral form of the sync layer's
/// `(Hlc, NodeId)`; `rh-vault` converts between them. Kept in `rh-core` so
/// the storage trait does not depend on the sync crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncStamp {
    pub rev_wall: u64,
    pub rev_counter: u32,
    pub origin: String,
}

/// Per-record sync provenance: one row per replicated record keyed by
/// `(kind, id)`, carrying its [`SyncStamp`] and a tombstone flag.
///
/// This sits beside the entity tables rather than adding `rev`/`origin`
/// columns to each one — a single generic table means deletions survive the
/// entity row (a tombstone is a `deleted = true` row whose entity is gone) so
/// the deletion can propagate to other devices, and no per-entity SQL needs
/// to learn about sync. `kind` is a free string (`"host"`, `"group"`,
/// `"credential"`, `"setting"`); `id` is the entity ULID or the setting key.
///
/// The merge model (record-level LWW by `rev` then `origin`) lives in
/// `rh-vault`; this trait only persists the stamps so a rebuilt snapshot
/// reflects when each record was actually last *edited*, not when the
/// snapshot was assembled.
#[async_trait]
pub trait SyncMetaStore: Send + Sync {
    /// Record or refresh the provenance of a *live* record (upsert with
    /// `deleted = false`). Call after a successful create/update. Resurrects
    /// a prior tombstone for the same `(kind, id)`.
    async fn bump(&self, kind: &str, id: &str, stamp: &SyncStamp) -> Result<(), StorageError>;

    /// Mark a record deleted (upsert with `deleted = true`). Call after a
    /// successful delete; the entity row is gone but this tombstone remains
    /// so the deletion replicates.
    async fn tombstone(&self, kind: &str, id: &str, stamp: &SyncStamp)
        -> Result<(), StorageError>;

    /// The stamp for one record, or `None` if never stamped.
    async fn stamp_of(&self, kind: &str, id: &str) -> Result<Option<SyncStamp>, StorageError>;

    /// All live (non-tombstone) stamps for `kind`, as `(id, stamp)`.
    async fn live_stamps(&self, kind: &str) -> Result<Vec<(String, SyncStamp)>, StorageError>;

    /// All tombstones across every kind, as `(kind, id, stamp)`.
    async fn tombstones(&self) -> Result<Vec<(String, String, SyncStamp)>, StorageError>;

    /// Forget a record's provenance entirely (both live rows and tombstones).
    /// Used when a local wipe (replace-import) clears everything.
    async fn clear(&self, kind: &str, id: &str) -> Result<(), StorageError>;

    /// Forget **all** provenance — every live stamp and tombstone. Used by a
    /// replace-import wipe so stale stamps for now-deleted entities can't
    /// resurface in the next snapshot.
    async fn clear_all(&self) -> Result<(), StorageError>;
}

/// Persistence for saved port-forward definitions (Tools → Forwards).
///
/// Stores only the *definition* (which host to tunnel through + bind /
/// target / kind + auto-start). Running instances live in memory in the
/// app layer's forward manager; starting a saved forward resolves the
/// host's credentials at start time (secrets are never stored here).
#[async_trait]
pub trait ForwardStore: Send + Sync {
    /// Insert a new saved forward.
    async fn create(&self, f: &SavedForward) -> Result<(), StorageError>;

    /// One saved forward by id.
    async fn get(&self, id: &ForwardId) -> Result<SavedForward, StorageError>;

    /// All saved forwards, newest first.
    async fn list(&self) -> Result<Vec<SavedForward>, StorageError>;

    /// Delete a saved forward.
    async fn delete(&self, id: &ForwardId) -> Result<(), StorageError>;

    /// Toggle the auto-start flag.
    async fn set_auto_start(&self, id: &ForwardId, auto_start: bool) -> Result<(), StorageError>;
}
