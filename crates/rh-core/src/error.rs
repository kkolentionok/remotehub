//! Domain error types.
//!
//! Layered design: each subsystem has its own error enum, and the
//! umbrella [`CoreError`] flattens them via `#[from]` for use in
//! library APIs that touch multiple subsystems.
//!
//! - [`StorageError`] — SQLite / persistence failures.
//! - [`SecretError`] — OS keychain failures (entry missing, locked, etc.).
//! - [`SessionError`] — network / protocol failures during a session.
//! - [`CoreError`] — umbrella, plus validation errors.
//!
//! Crate boundary policy:
//! - Library crates (`rh-storage`, `rh-ssh`, `rh-rdp`) return the
//!   matching specific error.
//! - Binary crate (`rh-app`) catches them, maps to `ApiError` (defined
//!   in that crate), strips internal details, and returns to UI.
//!
//! Crucially, **no error type carries secret material**. Hostnames,
//! credential names, and IDs are fine; password bytes, key material,
//! and PTY payload content are not.

use thiserror::Error;

use crate::id::{CredentialId, GroupId, HostId};

/// Persistence-layer error. Wraps SQLite-specific failures behind
/// a stable surface so callers don't need to depend on `sqlx`.
#[derive(Debug, Error)]
pub enum StorageError {
    /// Entity exists but was expected not to (e.g. unique-name violation).
    #[error("conflict: {0}")]
    Conflict(String),

    /// A foreign key constraint failed — e.g. host references a
    /// group_id that doesn't exist.
    #[error("foreign key violation: {0}")]
    ForeignKey(String),

    /// SQLite I/O, including file open / lock errors.
    #[error("database I/O: {0}")]
    Io(String),

    /// Backend-specific error (sqlx, query syntax, migration failure).
    #[error("database backend: {0}")]
    Backend(String),

    /// Data on disk is malformed — e.g. tags_json is not a JSON array.
    /// Indicates either a corrupted DB or a bug in a previous write.
    #[error("malformed data in {entity}: {reason}")]
    Malformed { entity: &'static str, reason: String },
}

/// OS keychain access error.
#[derive(Debug, Error)]
pub enum SecretError {
    /// No entry under the requested keychain ref.
    #[error("secret not found")]
    NotFound,

    /// Keychain backend is unavailable on this system. On Windows this
    /// would be Credential Manager being disabled; on Linux it's the
    /// Secret Service daemon not running.
    #[error("keychain unavailable: {0}")]
    Unavailable(String),

    /// User denied access (e.g. macOS Keychain prompt cancelled).
    #[error("access denied by user")]
    Denied,

    /// Other backend error.
    #[error("keychain backend: {0}")]
    Backend(String),
}

/// Live-session error. Reported by SSH/RDP actors when something goes
/// wrong during a connection. Distinct from `StorageError` because the
/// failure modes are operationally different — storage errors mean
/// "the app is broken", session errors mean "this server is unreachable".
#[derive(Debug, Error)]
pub enum SessionError {
    /// TCP / DNS / TLS failure before authentication.
    #[error("network: {0}")]
    Network(String),

    /// Authentication was rejected by the server.
    #[error("authentication failed")]
    AuthFailed,

    /// Server host key did not match the known-hosts entry (potential
    /// MITM, or legitimate server-side rotation). Distinct from
    /// "rejected by user" — this one is automatic.
    #[error("host key mismatch")]
    HostKeyMismatch,

    /// User declined to trust a previously-unknown host key.
    #[error("host key rejected")]
    HostKeyRejected,

    /// Protocol error — server sent something we couldn't parse, or
    /// negotiation failed in a non-auth way.
    #[error("protocol: {0}")]
    Protocol(String),

    /// Session closed by the server in an expected way (e.g. user typed
    /// `exit` in SSH shell). Not an error per se; included here so the
    /// session actor can use a single error channel.
    #[error("server disconnected: {0}")]
    ServerDisconnected(String),

    /// The session actor panicked. Carries the panic message (no
    /// backtrace; that's in the logs).
    #[error("session crashed: {0}")]
    Crashed(String),
}

/// Umbrella error for the core crate. Use this in trait signatures
/// that may span multiple subsystems (e.g. opening a session reads
/// from storage AND keychain AND network).
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("host not found: {0}")]
    HostNotFound(HostId),

    #[error("credential not found: {0}")]
    CredentialNotFound(CredentialId),

    #[error("group not found: {0}")]
    GroupNotFound(GroupId),

    /// Field-level validation failure surfaced from an entity
    /// constructor or a write path.
    #[error("validation: {field}: {reason}")]
    Validation { field: &'static str, reason: String },

    #[error("storage: {0}")]
    Storage(#[from] StorageError),

    #[error("secret: {0}")]
    Secret(#[from] SecretError),

    #[error("session: {0}")]
    Session(#[from] SessionError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_error_displays_cleanly() {
        let err = StorageError::Conflict("host name already taken".to_string());
        assert_eq!(format!("{err}"), "conflict: host name already taken");
    }

    #[test]
    fn core_error_wraps_storage_via_from() {
        let storage = StorageError::Io("disk full".to_string());
        let core: CoreError = storage.into();
        assert!(matches!(core, CoreError::Storage(_)));
    }

    #[test]
    fn core_error_wraps_secret_via_from() {
        let secret = SecretError::NotFound;
        let core: CoreError = secret.into();
        assert!(matches!(core, CoreError::Secret(SecretError::NotFound)));
    }

    #[test]
    fn core_error_wraps_session_via_from() {
        let session = SessionError::AuthFailed;
        let core: CoreError = session.into();
        assert!(matches!(core, CoreError::Session(_)));
    }

    #[test]
    fn validation_error_carries_field_name() {
        let err = CoreError::Validation {
            field: "port",
            reason: "must be between 1 and 65535".to_string(),
        };
        let s = format!("{err}");
        assert!(s.contains("port"));
        assert!(s.contains("65535"));
    }

    #[test]
    fn host_not_found_displays_id() {
        let id = HostId::from_raw("01HXYZ");
        let err = CoreError::HostNotFound(id);
        let s = format!("{err}");
        assert!(s.contains("01HXYZ"));
    }
}
