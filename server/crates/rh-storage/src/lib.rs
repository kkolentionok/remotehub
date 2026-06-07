//! SQLite + OS keychain storage layer for RemoteHub.
//!
//! Implements the storage traits defined in [`rh_core::store`]:
//!
//! - [`SqliteHostStore`]   — hosts (CRUD + filtered list).
//! - [`SqliteGroupStore`]  — host groups (tree with cycle prevention).
//! - [`SqliteCredentialStore`] — credentials (metadata in SQLite,
//!   secrets in OS keychain via the [`Keychain`] trait).
//! - [`SqliteSettingsStore`]   — user settings as JSON kv.
//!
//! The [`Db`] handle wraps an `sqlx::SqlitePool` and runs schema
//! init / migration on open. In alpha mode, schema mismatch triggers
//! a drop-recreate (see [`db::InitOutcome::Recreated`]).
//!
//! ## Typical wiring
//!
//! ```no_run
//! # use std::sync::Arc;
//! # use rh_storage::*;
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let (db, _outcome) = Db::open("remotehub.db").await?;
//! let hosts       = SqliteHostStore::new(db.clone());
//! let groups      = SqliteGroupStore::new(db.clone());
//! let keychain    = Arc::new(OsKeychain::new());
//! let credentials = SqliteCredentialStore::new(db.clone(), keychain);
//! let settings    = SqliteSettingsStore::new(db);
//! # Ok(()) }
//! ```

#![warn(missing_debug_implementations)]
#![warn(clippy::all)]

pub mod credential_store;
pub mod db;
pub mod group_store;
pub mod host_store;
pub mod keychain;
pub mod known_hosts_store;
pub mod rdp_cert_store;
pub mod settings_store;

pub use credential_store::SqliteCredentialStore;
pub use db::{Db, InitOutcome, CURRENT_SCHEMA_VERSION};
pub use group_store::SqliteGroupStore;
pub use host_store::SqliteHostStore;
pub use keychain::{Keychain, MemoryKeychain, OsKeychain};
pub use known_hosts_store::SqliteKnownHostsStore;
pub use rdp_cert_store::SqliteRdpCertStore;
pub use settings_store::SqliteSettingsStore;
