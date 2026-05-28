//! Secret-bearing types.
//!
//! These wrap raw secret bytes and enforce three security properties:
//!
//! 1. **Zeroization on drop** — memory is overwritten with zeroes when
//!    the type goes out of scope, not just returned to the allocator.
//!    Backed by [`zeroize::Zeroizing`].
//! 2. **No accidental debug output** — `Debug` impl prints `"<redacted>"`
//!    instead of the bytes. `Display` is intentionally not implemented.
//! 3. **No equality comparison** — there is no `PartialEq` impl, since
//!    naive byte comparison is timing-attack-sensitive. If equality
//!    becomes necessary, add a constant-time `secrecy::ExposeSecret`-style
//!    method explicitly.
//!
//! Two flavours:
//!
//! - [`SecretValue`] — input side. Built from raw bytes received over
//!   IPC (after decoding from base64) on its way to keychain storage.
//! - [`RevealedSecret`] — output side. Read from keychain on its way
//!   to a session actor (SSH/RDP authentication). Lives briefly and
//!   is dropped (and zeroed) as soon as auth completes.
//!
//! Both are internally identical (`Zeroizing<Vec<u8>>`) but kept as
//! distinct types so direction of flow is visible in function signatures.

use std::fmt;

use zeroize::Zeroizing;

/// A secret value on its way to the keychain.
///
/// Construct via [`SecretValue::new`] from raw bytes; the caller is
/// expected to drop the source as soon as possible. Once inside this
/// type, the contents will be zeroed on drop.
pub struct SecretValue {
    inner: Zeroizing<Vec<u8>>,
}

impl SecretValue {
    /// Wrap raw bytes. The input `Vec<u8>` is moved (not copied) into
    /// the secret container. After this call, the only reference to
    /// these bytes lives inside `SecretValue` and will be zeroed on drop.
    #[must_use]
    pub fn new(bytes: Vec<u8>) -> Self {
        Self {
            inner: Zeroizing::new(bytes),
        }
    }

    /// Borrow the raw bytes for use at the trust boundary
    /// (e.g. passing to `keyring::Entry::set_secret`).
    ///
    /// The returned slice is valid for the lifetime of `&self`; callers
    /// must not copy it into long-lived storage outside this type.
    #[must_use]
    pub fn expose(&self) -> &[u8] {
        &self.inner
    }

    /// Length of the secret in bytes. Safe to log/expose: byte count
    /// is not considered sensitive on its own.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// True iff the secret is empty (zero bytes). Empty secrets are
    /// usually a bug — validate at the boundary.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretValue")
            .field("bytes", &"<redacted>")
            .field("len", &self.inner.len())
            .finish()
    }
}

/// A secret value freshly read from the keychain, on its way to a
/// session actor for authentication.
///
/// Same shape as [`SecretValue`] but distinct type — direction of flow
/// matters: a `RevealedSecret` should never be re-stored as such; if
/// rotation is needed, callers build a fresh `SecretValue` from new bytes.
pub struct RevealedSecret {
    inner: Zeroizing<Vec<u8>>,
}

impl RevealedSecret {
    /// Wrap raw bytes read from the keychain.
    #[must_use]
    pub fn new(bytes: Vec<u8>) -> Self {
        Self {
            inner: Zeroizing::new(bytes),
        }
    }

    /// Borrow the bytes for use during authentication. Same lifetime
    /// rules as [`SecretValue::expose`].
    #[must_use]
    pub fn expose(&self) -> &[u8] {
        &self.inner
    }

    /// View the bytes as a UTF-8 string. Returns `None` if invalid UTF-8.
    /// Useful for password authentication where the secret is a string,
    /// not a binary blob (SSH key bytes).
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.inner).ok()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl fmt::Debug for RevealedSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RevealedSecret")
            .field("bytes", &"<redacted>")
            .field("len", &self.inner.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_value_exposes_bytes_unchanged() {
        let original = vec![1_u8, 2, 3, 4, 5];
        let secret = SecretValue::new(original.clone());
        assert_eq!(secret.expose(), original.as_slice());
        assert_eq!(secret.len(), 5);
        assert!(!secret.is_empty());
    }

    #[test]
    fn debug_does_not_leak_bytes() {
        // Critical: if this ever fails, the security invariant is broken.
        let secret = SecretValue::new(b"hunter2".to_vec());
        let debug_output = format!("{secret:?}");
        assert!(!debug_output.contains("hunter2"), "bytes leaked in Debug output: {debug_output}");
        assert!(debug_output.contains("<redacted>"));
        assert!(debug_output.contains("len: 7"));
    }

    #[test]
    fn debug_does_not_leak_bytes_for_revealed_secret() {
        let secret = RevealedSecret::new(b"hunter2".to_vec());
        let debug_output = format!("{secret:?}");
        assert!(!debug_output.contains("hunter2"));
        assert!(debug_output.contains("<redacted>"));
    }

    #[test]
    fn revealed_secret_as_str_decodes_utf8() {
        let secret = RevealedSecret::new("password123".as_bytes().to_vec());
        assert_eq!(secret.as_str(), Some("password123"));
    }

    #[test]
    fn revealed_secret_as_str_returns_none_for_invalid_utf8() {
        // A valid SSH key in PEM is ASCII; this represents binary data.
        let secret = RevealedSecret::new(vec![0xFF, 0xFE, 0xFD]);
        assert_eq!(secret.as_str(), None);
    }

    #[test]
    fn empty_secret_is_flagged() {
        let secret = SecretValue::new(Vec::new());
        assert!(secret.is_empty());
        assert_eq!(secret.len(), 0);
    }

    #[test]
    fn secret_value_and_revealed_secret_are_distinct_types() {
        // Compile-time test: the following should not compile if SecretValue
        // and RevealedSecret were the same type (or aliases of each other).
        // We can't easily express "doesn't compile" in a unit test, so we
        // just exercise both in the same scope to be sure they coexist.
        let s = SecretValue::new(vec![1, 2, 3]);
        let r = RevealedSecret::new(vec![1, 2, 3]);
        // Both expose the same byte view but are nominally different.
        assert_eq!(s.expose(), r.expose());
    }
}
