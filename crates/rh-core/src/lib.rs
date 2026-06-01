//! RemoteHub core domain types, traits, and errors.
//!
//! This crate has no I/O dependencies and is consumed by all other
//! crates in the workspace as the source of truth for domain concepts.
//!
//! ## Modules
//!
//! - [`id`] — typed identifiers (ULID newtypes) and keychain references.
//! - [`types`] — domain entities: `Host`, `HostGroup`, `Credential`,
//!   `Protocol`, `CredentialKind`, `EnvVar`.
//! - [`secret`] — secret-bearing types with zeroize-on-drop semantics.
//! - [`settings`] — user-facing settings with defaults.
//! - [`error`] — error enums for storage, secrets, sessions, and the
//!   umbrella [`error::CoreError`].
//! - [`store`] — async storage traits implemented by `rh-storage`.
//!
//! ## Re-exports
//!
//! The most commonly-used types are re-exported at the crate root for
//! ergonomic `use rh_core::Host` style imports. The full surface lives
//! in the modules above for the cases where namespacing is preferred.

#![warn(missing_debug_implementations)]
#![warn(clippy::all)]
// Pedantic / nursery lints are not enforced at the crate level — they
// produce too many false positives across rustc versions. Add them
// per-module if needed.

pub mod error;
pub mod id;
pub mod secret;
pub mod settings;
pub mod store;
pub mod types;

// Convenience re-exports — keep this list short and stable. Anything
// niche stays behind its module path.
pub use error::{CoreError, SecretError, SessionError, StorageError};
pub use id::{CredentialId, GroupId, HostId, KeychainRef, SessionId};
pub use secret::{RevealedSecret, SecretValue};
pub use settings::{Language, Settings, Theme};
pub use store::{
    CredentialStore, GroupStore, HostFilter, HostStore, KnownHostsStore, RdpCertStore, RevealError,
    SettingsStore,
};
pub use types::{
    Credential, CredentialKind, EnvVar, Host, HostGroup, KnownHostEntry, KnownHostKey, Protocol,
    RdpCertEntry, TrustedCert,
};
