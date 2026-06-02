//! Vault error type.
//!
//! Library-crate policy (workspace-wide): `thiserror`, never `anyhow`.
//! No variant carries secret material — passwords, key bytes, and
//! plaintext snapshot contents never appear in an error.

use thiserror::Error;

/// Errors produced by the vault / sync layer.
#[derive(Debug, Error)]
pub enum VaultError {
    /// Key derivation failed (bad Argon2 parameters, or the underlying
    /// implementation rejected them). Does NOT indicate a wrong password —
    /// a wrong password surfaces as [`VaultError::Decrypt`] when the AEAD
    /// tag fails to verify.
    #[error("key derivation failed: {0}")]
    Kdf(String),

    /// Authenticated encryption (sealing) failed. Practically only happens
    /// on a programming error (e.g. a key of the wrong length); a normal
    /// seal does not fail.
    #[error("encryption failed")]
    Encrypt,

    /// Authenticated decryption (opening) failed: the AEAD tag did not
    /// verify. This is the expected error for a **wrong password** or a
    /// **tampered / corrupted** blob — the two are cryptographically
    /// indistinguishable, by design.
    #[error("decryption failed (wrong password or corrupted vault)")]
    Decrypt,

    /// The envelope/snapshot uses a format version this build does not
    /// understand. Carries the offending version for diagnostics.
    #[error("unsupported format version: {0}")]
    UnsupportedFormat(u32),

    /// A field in the envelope was malformed (bad base64, wrong nonce
    /// length, etc.). Carries a short, secret-free reason.
    #[error("malformed vault data: {0}")]
    Malformed(String),

    /// (De)serialization of the plaintext snapshot failed.
    #[error("serialization error: {0}")]
    Serde(String),

    /// A sync transport (the A/B/C backend) failed. Carries a short,
    /// secret-free reason from the concrete transport implementation.
    #[error("sync transport error: {0}")]
    Transport(String),

    /// Optimistic-concurrency conflict on push: the remote moved since we
    /// pulled. The caller should pull again, re-merge, and retry.
    #[error("remote changed since last pull; re-merge and retry")]
    RemoteConflict,
}

impl From<serde_json::Error> for VaultError {
    fn from(e: serde_json::Error) -> Self {
        VaultError::Serde(e.to_string())
    }
}
