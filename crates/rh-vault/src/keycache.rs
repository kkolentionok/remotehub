//! Process-lifetime cache of the derived vault key.
//!
//! Argon2id at the vault's parameters (64 MiB, t=3) costs on the order of a
//! quarter second. That is the right price to pay when a human types their
//! master password; it is the wrong price to pay on a timer. A sync pass used
//! to derive twice — once to open the pulled blob, once to seal the push — and
//! because [`seal_snapshot`](crate::seal_snapshot) minted a *fresh salt* on
//! every push, the other device's key was invalidated by every single edit, so
//! it re-derived too. Three derivations per round trip, purely from cadence.
//!
//! Holding a derived key in memory is the same exposure class as holding the
//! master password in memory, which the app already does for the session. The
//! cache is keyed by [`KdfParams`]: a vault re-sealed under different
//! parameters (or a different account) misses and re-derives, so a stale key
//! can never be applied to the wrong blob. [`KeyCache::clear`] on logout or a
//! master-password change.

use std::sync::{Arc, Mutex};

use crate::error::VaultError;
use crate::kdf::{derive_key, KdfParams, VaultKey};

/// Caches one derived key, keyed by the parameters it was derived from.
#[derive(Default)]
pub struct KeyCache {
    inner: Mutex<Option<(KdfParams, Arc<VaultKey>)>>,
}

impl std::fmt::Debug for KeyCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("KeyCache(<redacted>)")
    }
}

impl KeyCache {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The key for `params`, deriving it only on a miss.
    pub fn key_for(
        &self,
        password: &[u8],
        params: &KdfParams,
    ) -> Result<Arc<VaultKey>, VaultError> {
        if let Ok(guard) = self.inner.lock() {
            if let Some((cached_params, key)) = guard.as_ref() {
                if cached_params == params {
                    return Ok(Arc::clone(key));
                }
            }
        }
        let key = Arc::new(derive_key(password, params)?);
        if let Ok(mut guard) = self.inner.lock() {
            *guard = Some((params.clone(), Arc::clone(&key)));
        }
        Ok(key)
    }

    /// Forget the cached key (logout, master-password change).
    pub fn clear(&self) {
        if let Ok(mut guard) = self.inner.lock() {
            *guard = None;
        }
    }
}
