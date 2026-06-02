//! The sync data model: what we replicate, how each record is stamped,
//! and the snapshot container.
//!
//! This layer is **backend-agnostic**. It says nothing about *where* a
//! snapshot is stored (a server, an S3 bucket, a file in a cloud-sync
//! folder); that is the [`crate::transport::SyncRemote`] seam. It only
//! defines the replicated state and the metadata needed to merge two
//! copies deterministically.
//!
//! ## What syncs
//! Hosts, host groups, credentials (with their secret material), and
//! user settings. Each becomes a [`SyncRecord`] keyed by its stable id
//! (a ULID for entities, the setting key for settings).
//!
//! ## Opaque payloads
//! A record's `data` is the entity serialized to `serde_json::Value`,
//! not a hand-maintained shadow struct. This means adding a field to
//! `rh_core::Host` automatically flows through sync with no change here —
//! one fewer place to forget. The trade-off is that machine-set fields
//! (`detected_os`, `last_connected_at`) ride along with their record
//! rather than being reconciled independently; acceptable for v1 (see
//! `docs/specs/sync.md`, "Known semantics").
//!
//! ## Secrets
//! Credential secrets live in the OS keychain locally and are **never**
//! written to SQLite. For sync they are carried inside the record (see
//! [`SyncCredentialPayload`]) and only ever exist in plaintext inside the
//! encrypted [`crate::VaultEnvelope`] — the bytes that leave the device
//! are AES-256-GCM ciphertext.

use std::collections::BTreeMap;

use rh_core::{Credential, Host, HostGroup};
use serde::{Deserialize, Serialize};

use crate::clock::{Hlc, NodeId};
use crate::error::VaultError;

/// Bump when the snapshot wire shape changes incompatibly.
pub const SNAPSHOT_FORMAT: u32 = 1;

/// Which kind of entity a [`SyncRecord`] carries. Determines how `data`
/// is interpreted and which storage table it reconciles into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Host,
    Group,
    Credential,
    Setting,
}

/// Per-record sync metadata: the logical time of the last write, who
/// wrote it, and whether it is a tombstone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordMeta {
    /// Logical time of the last write to this record. The merge winner is
    /// the higher `rev` (ties broken by `origin`).
    pub rev: Hlc,
    /// Device that produced the write at `rev`. Deterministic tiebreaker.
    pub origin: NodeId,
    /// Tombstone flag. A deleted record is retained (not dropped) so the
    /// deletion can propagate; its `data` is `None`.
    #[serde(default)]
    pub deleted: bool,
    /// Reserved for field-level LWW (v2): per-field logical times. Empty
    /// in v1 (record-level LWW). Kept here so upgrading the conflict model
    /// is an additive change to existing snapshots, not a format break.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub field_revs: BTreeMap<String, Hlc>,
}

impl RecordMeta {
    #[must_use]
    pub fn new(rev: Hlc, origin: NodeId) -> Self {
        Self {
            rev,
            origin,
            deleted: false,
            field_revs: BTreeMap::new(),
        }
    }

    /// Total order used by the merge: higher `rev` wins; on an exact
    /// `rev` tie, the lexicographically greater `origin` wins. This is a
    /// deterministic total order, which is what makes the merge
    /// convergent (all devices pick the same winner).
    #[must_use]
    pub fn wins_over(&self, other: &RecordMeta) -> bool {
        (self.rev, &self.origin) > (other.rev, &other.origin)
    }
}

/// One replicated record. `data` is `None` exactly when `meta.deleted`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncRecord {
    pub kind: EntityKind,
    /// Stable key: entity ULID, or the setting key for `Setting`.
    pub id: String,
    pub meta: RecordMeta,
    /// Serialized entity payload, or `None` for a tombstone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// A credential plus its secret material, the payload form stored in a
/// `Credential` record. The secret bytes only exist in plaintext inside
/// the encrypted vault blob.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncCredentialPayload {
    pub credential: Credential,
    /// Primary secret (password bytes / private-key PEM). `None` for
    /// agent credentials, which carry no keychain secret.
    #[serde(with = "crate::opt_b64", default)]
    pub secret: Option<Vec<u8>>,
    /// SSH key passphrase, if the key is encrypted and a passphrase was
    /// stored separately. `None` otherwise.
    #[serde(with = "crate::opt_b64", default)]
    pub passphrase: Option<Vec<u8>>,
}

impl SyncRecord {
    /// Build a live (non-deleted) host record.
    pub fn host(host: &Host, rev: Hlc, origin: NodeId) -> Result<Self, VaultError> {
        Ok(Self {
            kind: EntityKind::Host,
            id: host.id.to_string(),
            meta: RecordMeta::new(rev, origin),
            data: Some(serde_json::to_value(host)?),
        })
    }

    /// Build a live group record.
    pub fn group(group: &HostGroup, rev: Hlc, origin: NodeId) -> Result<Self, VaultError> {
        Ok(Self {
            kind: EntityKind::Group,
            id: group.id.to_string(),
            meta: RecordMeta::new(rev, origin),
            data: Some(serde_json::to_value(group)?),
        })
    }

    /// Build a live credential record (carries the secret material).
    pub fn credential(
        payload: &SyncCredentialPayload,
        rev: Hlc,
        origin: NodeId,
    ) -> Result<Self, VaultError> {
        Ok(Self {
            kind: EntityKind::Credential,
            id: payload.credential.id.to_string(),
            meta: RecordMeta::new(rev, origin),
            data: Some(serde_json::to_value(payload)?),
        })
    }

    /// Build a live setting record. `value` is the setting's JSON value.
    #[must_use]
    pub fn setting(key: &str, value: serde_json::Value, rev: Hlc, origin: NodeId) -> Self {
        Self {
            kind: EntityKind::Setting,
            id: key.to_string(),
            meta: RecordMeta::new(rev, origin),
            data: Some(value),
        }
    }

    /// Build a tombstone for a deleted entity.
    #[must_use]
    pub fn tombstone(kind: EntityKind, id: impl Into<String>, rev: Hlc, origin: NodeId) -> Self {
        let mut meta = RecordMeta::new(rev, origin);
        meta.deleted = true;
        Self {
            kind,
            id: id.into(),
            meta,
            data: None,
        }
    }

    #[must_use]
    pub fn is_deleted(&self) -> bool {
        self.meta.deleted
    }

    /// Reconstruct a [`Host`] from a live host record.
    pub fn as_host(&self) -> Result<Host, VaultError> {
        self.decode(EntityKind::Host)
    }

    /// Reconstruct a [`HostGroup`] from a live group record.
    pub fn as_group(&self) -> Result<HostGroup, VaultError> {
        self.decode(EntityKind::Group)
    }

    /// Reconstruct a [`SyncCredentialPayload`] from a live credential record.
    pub fn as_credential(&self) -> Result<SyncCredentialPayload, VaultError> {
        self.decode(EntityKind::Credential)
    }

    /// The raw setting value of a live setting record.
    pub fn as_setting(&self) -> Result<serde_json::Value, VaultError> {
        if self.kind != EntityKind::Setting {
            return Err(VaultError::Malformed(format!(
                "record {} is not a setting",
                self.id
            )));
        }
        self.data
            .clone()
            .ok_or_else(|| VaultError::Malformed(format!("setting {} is a tombstone", self.id)))
    }

    fn decode<T: for<'de> Deserialize<'de>>(&self, expect: EntityKind) -> Result<T, VaultError> {
        if self.kind != expect {
            return Err(VaultError::Malformed(format!(
                "record {} has kind {:?}, expected {:?}",
                self.id, self.kind, expect
            )));
        }
        let value = self
            .data
            .clone()
            .ok_or_else(|| VaultError::Malformed(format!("record {} is a tombstone", self.id)))?;
        serde_json::from_value(value).map_err(VaultError::from)
    }
}

/// A full point-in-time export of one device's replicated state.
///
/// This is the plaintext that gets serialized and sealed into a
/// [`crate::VaultEnvelope`]. It is also the unit that two devices merge
/// (see [`crate::merge`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncSnapshot {
    pub format: u32,
    /// Device that produced this snapshot.
    pub node: NodeId,
    /// Highest stamp this device had emitted/observed when producing the
    /// snapshot. Lets a receiver fold it into its own clock.
    pub generated: Hlc,
    pub records: Vec<SyncRecord>,
}

impl SyncSnapshot {
    #[must_use]
    pub fn new(node: NodeId, generated: Hlc, records: Vec<SyncRecord>) -> Self {
        Self {
            format: SNAPSHOT_FORMAT,
            node,
            generated,
            records,
        }
    }

    /// Validate the format version after deserialization.
    pub fn check_format(&self) -> Result<(), VaultError> {
        if self.format != SNAPSHOT_FORMAT {
            return Err(VaultError::UnsupportedFormat(self.format));
        }
        Ok(())
    }

    /// Count of live (non-tombstone) records of a given kind. Handy for UI
    /// ("12 hosts, 4 credentials") and tests.
    #[must_use]
    pub fn live_count(&self, kind: EntityKind) -> usize {
        self.records
            .iter()
            .filter(|r| r.kind == kind && !r.is_deleted())
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rh_core::{CredentialKind, Protocol};

    fn node() -> NodeId {
        NodeId::new("test-node")
    }

    #[test]
    fn host_record_roundtrips() {
        let h = Host::new("web-1", "10.0.0.1", Protocol::Ssh, Some(22));
        let rec = SyncRecord::host(&h, Hlc::new(5, 0), node()).unwrap();
        assert_eq!(rec.kind, EntityKind::Host);
        assert_eq!(rec.id, h.id.to_string());
        let back = rec.as_host().unwrap();
        assert_eq!(back, h);
    }

    #[test]
    fn credential_payload_carries_secret() {
        let c = Credential::new("root pw", CredentialKind::Password, "root");
        let payload = SyncCredentialPayload {
            credential: c.clone(),
            secret: Some(b"hunter2".to_vec()),
            passphrase: None,
        };
        let rec = SyncRecord::credential(&payload, Hlc::new(1, 0), node()).unwrap();
        let back = rec.as_credential().unwrap();
        assert_eq!(back.credential, c);
        assert_eq!(back.secret.as_deref(), Some(&b"hunter2"[..]));
    }

    #[test]
    fn tombstone_has_no_data_and_reports_deleted() {
        let rec = SyncRecord::tombstone(EntityKind::Host, "01ABC", Hlc::new(9, 0), node());
        assert!(rec.is_deleted());
        assert!(rec.data.is_none());
        assert!(rec.as_host().is_err());
    }

    #[test]
    fn wrong_kind_decode_is_error() {
        let g = HostGroup::new("prod", None);
        let rec = SyncRecord::group(&g, Hlc::new(1, 0), node()).unwrap();
        assert!(rec.as_host().is_err());
        assert!(rec.as_group().is_ok());
    }

    #[test]
    fn meta_total_order_breaks_ties_by_origin() {
        let a = RecordMeta::new(Hlc::new(10, 0), NodeId::new("aaa"));
        let b = RecordMeta::new(Hlc::new(10, 0), NodeId::new("bbb"));
        assert!(b.wins_over(&a));
        assert!(!a.wins_over(&b));
        // Strictly higher rev beats any origin.
        let c = RecordMeta::new(Hlc::new(11, 0), NodeId::new("aaa"));
        assert!(c.wins_over(&b));
    }

    #[test]
    fn snapshot_format_and_counts() {
        let h = Host::new("h", "1.1.1.1", Protocol::Rdp, None);
        let snap = SyncSnapshot::new(
            node(),
            Hlc::new(3, 0),
            vec![
                SyncRecord::host(&h, Hlc::new(3, 0), node()).unwrap(),
                SyncRecord::tombstone(EntityKind::Host, "gone", Hlc::new(2, 0), node()),
            ],
        );
        assert_eq!(snap.format, SNAPSHOT_FORMAT);
        snap.check_format().unwrap();
        assert_eq!(snap.live_count(EntityKind::Host), 1);
    }

    #[test]
    fn setting_record_roundtrips() {
        let rec = SyncRecord::setting(
            "theme",
            serde_json::json!("navy"),
            Hlc::new(1, 0),
            node(),
        );
        assert_eq!(rec.as_setting().unwrap(), serde_json::json!("navy"));
    }
}
