//! Trait-based wrapper around the OS keychain.
//!
//! Reasons for the trait:
//!
//! 1. **Testability** — unit tests for `CredentialStore` need a
//!    deterministic, in-memory keychain. The real `keyring-rs` crate
//!    has a global default-credential-builder setter and platform
//!    backends that don't behave the same under `cargo test`.
//! 2. **Forward compatibility** — a future "encrypted file" backend
//!    (for portable installs) drops in here without changing storage
//!    code.
//!
//! The production implementation is [`OsKeychain`]; tests use
//! [`MemoryKeychain`] (also useful in dev for quick smoke runs without
//! polluting the real OS keychain).

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use zeroize::Zeroizing;

use rh_core::{KeychainRef, RevealedSecret, SecretError, SecretValue};

/// Trait for any keychain backend.
#[async_trait]
pub trait Keychain: Send + Sync {
    /// Write a secret under the given reference. Overwrites any
    /// existing entry.
    async fn set(&self, key: &KeychainRef, value: &SecretValue) -> Result<(), SecretError>;

    /// Read a secret by reference. Returns [`SecretError::NotFound`]
    /// if no entry exists.
    async fn get(&self, key: &KeychainRef) -> Result<RevealedSecret, SecretError>;

    /// Delete an entry. Idempotent — deleting a missing entry returns
    /// `Ok(())` rather than `NotFound`, because the desired postcondition
    /// (entry does not exist) is already satisfied.
    async fn delete(&self, key: &KeychainRef) -> Result<(), SecretError>;
}

// =====================================================================
// Production implementation: OS keychain via `keyring-rs`.
// =====================================================================

/// Production keychain — talks to the real OS credential store.
#[derive(Debug, Default)]
pub struct OsKeychain;

impl OsKeychain {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Build a fresh `keyring::Entry` for a ref. Entries are cheap
    /// handles — we don't bother caching them.
    fn entry(key: &KeychainRef) -> Result<keyring::Entry, SecretError> {
        keyring::Entry::new(KeychainRef::SERVICE, key.as_str())
            .map_err(|e| map_keyring_error(&e))
    }
}

#[async_trait]
impl Keychain for OsKeychain {
    async fn set(&self, key: &KeychainRef, value: &SecretValue) -> Result<(), SecretError> {
        // keyring is blocking. Run on a blocking thread to avoid
        // stalling the async runtime.
        let key = key.clone();
        // Copy bytes once into a Zeroizing buffer the closure can own.
        // The original SecretValue stays in caller's hands and zeroizes
        // when they drop it.
        let bytes = Zeroizing::new(value.expose().to_vec());

        tokio::task::spawn_blocking(move || {
            let entry = Self::entry(&key)?;
            entry
                .set_secret(&bytes)
                .map_err(|e| map_keyring_error(&e))?;
            Ok::<_, SecretError>(())
        })
        .await
        .map_err(|e| SecretError::Backend(format!("blocking task join: {e}")))??;
        Ok(())
    }

    async fn get(&self, key: &KeychainRef) -> Result<RevealedSecret, SecretError> {
        let key = key.clone();
        let bytes = tokio::task::spawn_blocking(move || {
            let entry = Self::entry(&key)?;
            entry.get_secret().map_err(|e| map_keyring_error(&e))
        })
        .await
        .map_err(|e| SecretError::Backend(format!("blocking task join: {e}")))??;

        Ok(RevealedSecret::new(bytes))
    }

    async fn delete(&self, key: &KeychainRef) -> Result<(), SecretError> {
        let key = key.clone();
        tokio::task::spawn_blocking(move || {
            let entry = match Self::entry(&key) {
                Ok(e) => e,
                Err(SecretError::NotFound) => return Ok(()),
                Err(e) => return Err(e),
            };
            match entry.delete_credential() {
                Ok(()) => Ok(()),
                // Idempotency: missing entry is fine for delete.
                Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(map_keyring_error(&e)),
            }
        })
        .await
        .map_err(|e| SecretError::Backend(format!("blocking task join: {e}")))?
    }
}

fn map_keyring_error(err: &keyring::Error) -> SecretError {
    use keyring::Error as K;
    match err {
        K::NoEntry => SecretError::NotFound,
        K::PlatformFailure(_) | K::NoStorageAccess(_) => {
            SecretError::Unavailable(err.to_string())
        }
        _ => SecretError::Backend(err.to_string()),
    }
}

// =====================================================================
// In-memory implementation for tests and dev.
// =====================================================================

/// Thread-safe in-memory keychain. Entries live until the instance is
/// dropped; data does NOT survive a process restart. Suitable for unit
/// tests and for `--no-keychain` dev mode (post-MVP idea).
#[derive(Debug, Default)]
pub struct MemoryKeychain {
    inner: Mutex<HashMap<String, Zeroizing<Vec<u8>>>>,
}

impl MemoryKeychain {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of entries currently stored. Useful in tests to assert
    /// cleanup behaviour. Not exposed as a trait method — diagnostic only.
    pub fn entry_count(&self) -> usize {
        self.inner.lock().expect("mutex poisoned").len()
    }
}

#[async_trait]
impl Keychain for MemoryKeychain {
    async fn set(&self, key: &KeychainRef, value: &SecretValue) -> Result<(), SecretError> {
        let mut guard = self.inner.lock().expect("mutex poisoned");
        guard.insert(
            key.as_str().to_string(),
            Zeroizing::new(value.expose().to_vec()),
        );
        Ok(())
    }

    async fn get(&self, key: &KeychainRef) -> Result<RevealedSecret, SecretError> {
        let guard = self.inner.lock().expect("mutex poisoned");
        match guard.get(key.as_str()) {
            Some(bytes) => Ok(RevealedSecret::new(bytes.to_vec())),
            None => Err(SecretError::NotFound),
        }
    }

    async fn delete(&self, key: &KeychainRef) -> Result<(), SecretError> {
        let mut guard = self.inner.lock().expect("mutex poisoned");
        guard.remove(key.as_str());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use rh_core::CredentialId;

    use super::*;

    fn ref_for(id: &str) -> KeychainRef {
        KeychainRef::for_credential(&CredentialId::from_raw(id))
    }

    #[tokio::test]
    async fn memory_keychain_set_get_delete_roundtrip() {
        let kc = MemoryKeychain::new();
        let key = ref_for("test1");
        let value = SecretValue::new(b"hunter2".to_vec());

        kc.set(&key, &value).await.unwrap();
        assert_eq!(kc.entry_count(), 1);

        let got = kc.get(&key).await.unwrap();
        assert_eq!(got.expose(), b"hunter2");

        kc.delete(&key).await.unwrap();
        assert_eq!(kc.entry_count(), 0);
    }

    #[tokio::test]
    async fn memory_keychain_get_missing_returns_not_found() {
        let kc = MemoryKeychain::new();
        let err = kc.get(&ref_for("nope")).await.unwrap_err();
        assert!(matches!(err, SecretError::NotFound));
    }

    #[tokio::test]
    async fn memory_keychain_delete_missing_is_ok() {
        // Idempotency contract: deleting a missing entry succeeds.
        let kc = MemoryKeychain::new();
        kc.delete(&ref_for("nope")).await.unwrap();
    }

    #[tokio::test]
    async fn memory_keychain_set_overwrites() {
        let kc = MemoryKeychain::new();
        let key = ref_for("overwrite");
        kc.set(&key, &SecretValue::new(b"old".to_vec())).await.unwrap();
        kc.set(&key, &SecretValue::new(b"new".to_vec())).await.unwrap();
        assert_eq!(kc.entry_count(), 1);
        assert_eq!(kc.get(&key).await.unwrap().expose(), b"new");
    }
}
