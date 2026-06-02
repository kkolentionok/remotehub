//! # rh-vault — portable E2E-encrypted vault + sync model
//!
//! Backend-agnostic foundation for RemoteHub's accounts/sync feature
//! (backlog item 2). This crate is deliberately independent of the
//! eventual sync backend (self-hosted server / object store / cloud-sync
//! folder file). It provides:
//!
//! - **Crypto** — a master password is stretched with Argon2id
//!   ([`kdf`]) into a 256-bit key; the replicated state is sealed with
//!   AES-256-GCM ([`crypto`]). The portable [`VaultEnvelope`] ([`envelope`])
//!   is what gets exported to a file or uploaded by a transport — the
//!   master password never leaves the device.
//! - **Model** — what we replicate and how each record is stamped for
//!   conflict resolution ([`model`]): hosts, groups, credentials (with
//!   secrets), settings, each a [`SyncRecord`] with a hybrid-logical-clock
//!   ([`clock`]) revision.
//! - **Merge** — deterministic, convergent last-write-wins reconciliation
//!   of two snapshots ([`merge`]).
//! - **Transport seam** — the [`SyncRemote`](transport::SyncRemote) trait
//!   the A/B/C backend will implement ([`transport`]).
//!
//! Nothing here touches Tauri, SQLite, or the keychain. The `rh-app` layer
//! wires this to storage (read entities → build snapshot → seal; pull →
//! open → merge → write entities) and to a concrete transport.
//!
//! ## Library policy
//! `thiserror` only (no `anyhow`), no `unwrap()` outside tests, secrets
//! zeroized and never logged. See `docs/specs/sync.md` for the full design.

#![warn(missing_debug_implementations)]
#![warn(clippy::all)]

pub mod b64;
pub mod clock;
pub mod crypto;
pub mod envelope;
pub mod error;
pub mod kdf;
pub mod merge;
pub mod model;
pub mod opt_b64;
pub mod transport;

pub use clock::{Hlc, HlcGenerator, NodeId};
pub use envelope::{
    from_export_string, open_envelope, seal_snapshot, seal_snapshot_with, to_export_string,
    VaultEnvelope, ENVELOPE_FORMAT,
};
pub use error::VaultError;
pub use kdf::{derive_key, gen_salt, KdfAlgo, KdfParams, VaultKey, KEY_LEN, SALT_LEN};
pub use merge::{merge, merge_as};
pub use model::{
    EntityKind, RecordMeta, SyncCredentialPayload, SyncRecord, SyncSnapshot, SNAPSHOT_FORMAT,
};
pub use transport::{MemoryRemote, RemoteBlob, SyncRemote};
