//! Master-password -> vault-key derivation (Argon2id).
//!
//! The vault is encrypted under a 256-bit key derived from the user's
//! master password with **Argon2id**, a memory-hard KDF that resists
//! GPU/ASIC brute force. The derivation parameters (memory, iterations,
//! parallelism) and the random salt are stored *in the clear* alongside
//! the ciphertext (in the [`crate::VaultEnvelope`] header): they are not
//! secret, and a future build must read the parameters a past build used
//! in order to derive the same key.
//!
//! ## Why `argon2` and not `aws-lc-rs`
//! `aws-lc-rs` 1.17 does not expose Argon2id in its public Rust API
//! (it offers PBKDF2/HKDF, but not Argon2). The RustCrypto `argon2`
//! crate is well-audited and already vendored in this workspace
//! (`rh-ssh/ppk.rs`). We use it *only* for this derivation; the actual
//! payload encryption uses RustCrypto `aes-gcm` (see `crypto.rs`).

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::error::VaultError;

/// Length of the derived symmetric key, in bytes (AES-256).
pub const KEY_LEN: usize = 32;
/// Length of the KDF salt, in bytes.
pub const SALT_LEN: usize = 16;

/// KDF identifier persisted in the envelope. Only Argon2id is supported;
/// the field exists so the format can evolve (e.g. a future scrypt or a
/// parameter-policy bump) without a silent reinterpretation of old data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KdfAlgo {
    Argon2id,
}

/// Argon2id cost parameters + salt, stored alongside the ciphertext.
///
/// Defaults follow the OWASP "first recommended" Argon2id configuration
/// (a sensible interactive-login target): 64 MiB memory, 3 iterations,
/// 1 lane. Tunable later via the policy in `rh-app` without breaking old
/// vaults, since each vault records the parameters it was sealed with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdfParams {
    pub algo: KdfAlgo,
    /// Memory cost in KiB (Argon2 `m_cost`). 65536 KiB = 64 MiB.
    pub m_cost_kib: u32,
    /// Iteration count (Argon2 `t_cost`).
    pub t_cost: u32,
    /// Degree of parallelism / lanes (Argon2 `p_cost`).
    pub p_cost: u32,
    /// Random per-vault salt, base64 (standard, padded) in the JSON form.
    #[serde(with = "crate::b64")]
    pub salt: Vec<u8>,
}

impl KdfParams {
    /// Build interactive-login defaults with a freshly generated salt.
    #[must_use]
    pub fn new_default() -> Self {
        Self {
            algo: KdfAlgo::Argon2id,
            m_cost_kib: 64 * 1024,
            t_cost: 3,
            p_cost: 1,
            salt: gen_salt(),
        }
    }
}

/// Generate a cryptographically random salt.
#[must_use]
pub fn gen_salt() -> Vec<u8> {
    let mut salt = vec![0u8; SALT_LEN];
    // getrandom only fails if the OS RNG is unavailable, which on a desktop is
    // unrecoverable; a panic-free zero-salt would be a security bug, so we
    // expect() — a failure here means the platform RNG is broken and the app
    // cannot operate securely anyway.
    getrandom::getrandom(&mut salt).expect("system RNG unavailable");
    salt
}

/// A derived 256-bit vault key. Zeroized on drop.
///
/// Deliberately has no `Debug` that prints bytes and no `Clone`: a key
/// should exist in exactly the places that hold it. Borrow the bytes via
/// [`VaultKey::as_bytes`] only at the AEAD boundary.
pub struct VaultKey(Zeroizing<[u8; KEY_LEN]>);

impl VaultKey {
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }

    /// Wrap raw key material.
    ///
    /// The vault key comes from a password via [`derive_key`], but the notes
    /// blob is sealed under a *random* key that is stored and transported
    /// rather than derived — no password, and so no Argon2 on its path.
    #[must_use]
    pub fn from_bytes(raw: [u8; KEY_LEN]) -> Self {
        Self(Zeroizing::new(raw))
    }
}

impl std::fmt::Debug for VaultKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("VaultKey(<redacted 32 bytes>)")
    }
}

/// Derive the vault key from a master password and stored KDF params.
///
/// Deterministic: same password + same params + same salt => same key.
/// A wrong password yields a *different* key, which then fails AEAD tag
/// verification at decrypt time (surfacing as [`VaultError::Decrypt`]).
pub fn derive_key(password: &[u8], params: &KdfParams) -> Result<VaultKey, VaultError> {
    use argon2::{Algorithm, Argon2, Params, Version};

    let KdfAlgo::Argon2id = params.algo; // exhaustive today; guards future variants

    let argon_params = Params::new(
        params.m_cost_kib,
        params.t_cost,
        params.p_cost,
        Some(KEY_LEN),
    )
    .map_err(|e| VaultError::Kdf(e.to_string()))?;

    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, argon_params);

    let mut out = Zeroizing::new([0u8; KEY_LEN]);
    argon
        .hash_password_into(password, &params.salt, out.as_mut_slice())
        .map_err(|e| VaultError::Kdf(e.to_string()))?;

    Ok(VaultKey(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Cheap params for tests — real defaults (64 MiB) make the test suite
    // crawl. Production code uses KdfParams::new_default().
    fn fast_params(salt: Vec<u8>) -> KdfParams {
        KdfParams {
            algo: KdfAlgo::Argon2id,
            m_cost_kib: 64,
            t_cost: 1,
            p_cost: 1,
            salt,
        }
    }

    #[test]
    fn derivation_is_deterministic() {
        let params = fast_params(vec![7u8; SALT_LEN]);
        let a = derive_key(b"correct horse", &params).unwrap();
        let b = derive_key(b"correct horse", &params).unwrap();
        assert_eq!(a.as_bytes(), b.as_bytes());
        assert_eq!(a.as_bytes().len(), KEY_LEN);
    }

    #[test]
    fn different_password_yields_different_key() {
        let params = fast_params(vec![7u8; SALT_LEN]);
        let a = derive_key(b"password-a", &params).unwrap();
        let b = derive_key(b"password-b", &params).unwrap();
        assert_ne!(a.as_bytes(), b.as_bytes());
    }

    #[test]
    fn different_salt_yields_different_key() {
        let a = derive_key(b"same", &fast_params(vec![1u8; SALT_LEN])).unwrap();
        let b = derive_key(b"same", &fast_params(vec![2u8; SALT_LEN])).unwrap();
        assert_ne!(a.as_bytes(), b.as_bytes());
    }

    #[test]
    fn gen_salt_is_right_length_and_not_all_zero() {
        let s = gen_salt();
        assert_eq!(s.len(), SALT_LEN);
        assert!(s.iter().any(|&b| b != 0), "salt should not be all zeroes");
    }

    #[test]
    fn key_debug_is_redacted() {
        let k = derive_key(b"x", &fast_params(vec![0u8; SALT_LEN])).unwrap();
        assert!(format!("{k:?}").contains("redacted"));
    }
}
