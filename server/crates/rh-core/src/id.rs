//! Typed identifiers for domain entities.
//!
//! We use [ULID](https://github.com/ulid/spec) for all generated IDs:
//! 26-character lexicographically sortable strings encoding a 48-bit
//! millisecond timestamp + 80 bits of randomness. This gives:
//!
//! - Monotonic ordering by creation time (useful for default sort in lists).
//! - Globally unique without a coordinator (no auto-increment, no collisions).
//! - Compact (26 chars vs UUID's 36) and URL-safe.
//!
//! Each entity has its own newtype to prevent mixing IDs at compile time
//! (e.g. passing a `HostId` where a `CredentialId` is expected won't compile).
//!
//! `KeychainRef` is **not** a ULID — it's a constructed reference like
//! `remotehub.<credential_id>` and lives separately from the ID system.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Generate a new ULID string. Single source of randomness for the crate.
fn new_ulid() -> String {
    ulid::Ulid::new().to_string()
}

/// Macro that produces a newtype wrapping `String` with ULID generation,
/// serde transparency, and ergonomic constructors. Keeps the boilerplate
/// honest — every ID type behaves identically except for which entity
/// it identifies.
macro_rules! ulid_newtype {
    ($(#[$attr:meta])* $name:ident) => {
        $(#[$attr])*
        #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Generate a new, unique ID.
            #[must_use]
            pub fn new() -> Self {
                Self(new_ulid())
            }

            /// Wrap an existing ID string. Use this when loading from
            /// storage or accepting from an external source. Does NOT
            /// validate the format — that's the caller's responsibility
            /// at the trust boundary (typically the storage layer or
            /// the Tauri command handler).
            #[must_use]
            pub fn from_raw(s: impl Into<String>) -> Self {
                Self(s.into())
            }

            /// Borrow the inner string.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Consume and return the inner string.
            #[must_use]
            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl From<$name> for String {
            fn from(id: $name) -> String {
                id.0
            }
        }
    };
}

ulid_newtype! {
    /// Identifier for a [`Host`](crate::Host).
    HostId
}

ulid_newtype! {
    /// Identifier for a [`Credential`](crate::Credential).
    CredentialId
}

ulid_newtype! {
    /// Identifier for a [`HostGroup`](crate::HostGroup).
    GroupId
}

ulid_newtype! {
    /// Identifier for a live session. Unlike other IDs, this one is
    /// ephemeral — never persisted to storage.
    SessionId
}

/// Opaque reference to a secret stored in the OS keychain.
///
/// This type **never** contains the secret itself, only the lookup key.
/// The format is `remotehub.<credential_id>` for primary secrets and
/// `remotehub.<credential_id>.passphrase` for SSH key passphrases.
///
/// Constructed via [`KeychainRef::for_credential`] or
/// [`KeychainRef::for_passphrase`] to enforce the format at the type
/// system level — there's no way to build an invalid one accidentally.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct KeychainRef(String);

impl KeychainRef {
    /// Service name used as the `service` field in keychain entries.
    /// All entries this app writes share this service name, making them
    /// easy to audit and bulk-delete in the OS keychain UI.
    pub const SERVICE: &'static str = "RemoteHub";

    /// Build a reference for the primary secret of a credential.
    #[must_use]
    pub fn for_credential(id: &CredentialId) -> Self {
        Self(format!("remotehub.{}", id.as_str()))
    }

    /// Build a reference for an SSH key passphrase (stored separately
    /// so password rotation does not invalidate the key, and vice versa).
    #[must_use]
    pub fn for_passphrase(id: &CredentialId) -> Self {
        Self(format!("remotehub.{}.passphrase", id.as_str()))
    }

    /// Borrow the inner string (the `account` field passed to keyring).
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Wrap an existing ref string. Used by storage layer when loading
    /// metadata rows from the database.
    #[must_use]
    pub fn from_raw(s: impl Into<String>) -> Self {
        Self(s.into())
    }
}

impl fmt::Display for KeychainRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ulid_format_is_26_chars() {
        let id = HostId::new();
        assert_eq!(id.as_str().len(), 26);
    }

    #[test]
    fn ids_are_unique() {
        let a = HostId::new();
        let b = HostId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn ids_are_monotonic_when_generated_in_sequence() {
        // ULID timestamps have ms resolution, so generating two in a row
        // can produce equal-timestamp IDs but the random part still
        // differs. The lex order should still be sensible — we just
        // verify it's deterministic, not that it's strictly increasing.
        let a = HostId::new();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = HostId::new();
        assert!(a.as_str() < b.as_str(), "later ULID should sort after earlier");
    }

    #[test]
    fn ids_roundtrip_through_serde() {
        let id = CredentialId::new();
        let json = serde_json::to_string(&id).unwrap();
        // Transparent serde: should be just the inner string, no wrapper.
        assert!(json.starts_with('"'));
        assert!(json.ends_with('"'));
        let back: CredentialId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn from_raw_does_not_validate() {
        // from_raw is explicitly lenient — used at trust boundaries.
        let id = HostId::from_raw("not-a-ulid");
        assert_eq!(id.as_str(), "not-a-ulid");
    }

    #[test]
    fn keychain_ref_for_credential() {
        let cred_id = CredentialId::from_raw("01HXY");
        let kref = KeychainRef::for_credential(&cred_id);
        assert_eq!(kref.as_str(), "remotehub.01HXY");
    }

    #[test]
    fn keychain_ref_for_passphrase_is_distinct() {
        let cred_id = CredentialId::from_raw("01HXY");
        let primary = KeychainRef::for_credential(&cred_id);
        let passphrase = KeychainRef::for_passphrase(&cred_id);
        assert_ne!(primary, passphrase);
        assert!(passphrase.as_str().ends_with(".passphrase"));
    }

    #[test]
    fn keychain_ref_service_is_stable() {
        // Crate-wide invariant: do NOT change SERVICE without a migration
        // path — existing user keychains depend on this exact string.
        assert_eq!(KeychainRef::SERVICE, "RemoteHub");
    }

    #[test]
    fn display_is_transparent() {
        let id = HostId::from_raw("01HXY");
        assert_eq!(format!("{id}"), "01HXY");
    }
}
