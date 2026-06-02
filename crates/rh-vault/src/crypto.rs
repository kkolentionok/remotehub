//! Authenticated encryption of the vault payload (AES-256-GCM).
//!
//! We seal the serialized snapshot under the [`crate::VaultKey`] derived
//! from the master password. AES-256-GCM gives confidentiality *and*
//! integrity: any change to the ciphertext (or a wrong key from a wrong
//! password) makes tag verification fail, surfacing as
//! [`VaultError::Decrypt`].
//!
//! ## Nonce policy
//! GCM requires a unique nonce per (key, message). We generate a fresh
//! 96-bit random nonce for **every** seal and store it next to the
//! ciphertext. The vault is sealed seldom (on save/sync), so the 2^-32
//! birthday-bound concern of random 96-bit nonces under one key is not in
//! play here; even so, a re-seal after any edit gets a new nonce. We never
//! reuse a nonce with the same key.
//!
//! ## Implementation
//! Pure-Rust RustCrypto [`aes_gcm::Aes256Gcm`] (hardware AES-NI via the
//! `aes` crate when the CPU supports it). We originally specified
//! `aws-lc-rs`, but its native `aws-lc-sys` C library needs NASM + a C11
//! MSVC toolchain to build on Windows; `aes-gcm` is pure Rust and needs no
//! native build. The additional authenticated data (AAD) binds the
//! envelope header (see `envelope.rs`) so a blob can't be replayed under a
//! different header.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};

use crate::error::VaultError;
use crate::kdf::VaultKey;

/// GCM authentication tag length, in bytes.
pub const TAG_LEN: usize = 16;
/// AES-GCM nonce length, in bytes (96 bits).
pub const NONCE_LEN_BYTES: usize = 12;

/// Output of [`seal`]: the random nonce and the ciphertext-with-tag.
///
/// `ciphertext` is `encrypted_bytes || tag` (the tag is appended by GCM).
#[derive(Debug, Clone)]
pub struct Sealed {
    pub nonce: [u8; NONCE_LEN_BYTES],
    pub ciphertext: Vec<u8>,
}

/// Encrypt `plaintext` under `key`, binding `aad` (additional
/// authenticated data — covered by the tag but not encrypted; we bind the
/// envelope header so a blob can't be replayed under a different header).
pub fn seal(key: &VaultKey, plaintext: &[u8], aad: &[u8]) -> Result<Sealed, VaultError> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key.as_bytes()));

    let mut nonce_bytes = [0u8; NONCE_LEN_BYTES];
    getrandom::getrandom(&mut nonce_bytes).map_err(|_| VaultError::Encrypt)?;
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, Payload { msg: plaintext, aad })
        .map_err(|_| VaultError::Encrypt)?;

    Ok(Sealed {
        nonce: nonce_bytes,
        ciphertext,
    })
}

/// Decrypt and verify. Returns the recovered plaintext, or
/// [`VaultError::Decrypt`] if the tag fails (wrong password / tampering)
/// or [`VaultError::Malformed`] if the ciphertext is too short to contain
/// a tag.
pub fn open(
    key: &VaultKey,
    nonce_bytes: &[u8; NONCE_LEN_BYTES],
    ciphertext_and_tag: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>, VaultError> {
    if ciphertext_and_tag.len() < TAG_LEN {
        return Err(VaultError::Malformed("ciphertext shorter than tag".into()));
    }

    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key.as_bytes()));
    let nonce = Nonce::from_slice(nonce_bytes);

    cipher
        .decrypt(
            nonce,
            Payload {
                msg: ciphertext_and_tag,
                aad,
            },
        )
        .map_err(|_| VaultError::Decrypt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kdf::{derive_key, KdfAlgo, KdfParams, SALT_LEN};

    fn test_key() -> VaultKey {
        let params = KdfParams {
            algo: KdfAlgo::Argon2id,
            m_cost_kib: 64,
            t_cost: 1,
            p_cost: 1,
            salt: vec![3u8; SALT_LEN],
        };
        derive_key(b"unit-test-password", &params).unwrap()
    }

    #[test]
    fn round_trip_recovers_plaintext() {
        let key = test_key();
        let msg = b"the snapshot bytes go here";
        let sealed = seal(&key, msg, b"hdr").unwrap();
        let opened = open(&key, &sealed.nonce, &sealed.ciphertext, b"hdr").unwrap();
        assert_eq!(opened, msg);
    }

    #[test]
    fn ciphertext_is_not_plaintext_and_carries_tag() {
        let key = test_key();
        let msg = b"hello";
        let sealed = seal(&key, msg, b"").unwrap();
        assert_ne!(&sealed.ciphertext[..], &msg[..]);
        assert_eq!(sealed.ciphertext.len(), msg.len() + TAG_LEN);
        assert_eq!(sealed.nonce.len(), NONCE_LEN_BYTES);
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let key = test_key();
        let mut sealed = seal(&key, b"important", b"hdr").unwrap();
        sealed.ciphertext[0] ^= 0xFF;
        let err = open(&key, &sealed.nonce, &sealed.ciphertext, b"hdr").unwrap_err();
        assert!(matches!(err, VaultError::Decrypt));
    }

    #[test]
    fn wrong_aad_fails() {
        let key = test_key();
        let sealed = seal(&key, b"important", b"header-A").unwrap();
        let err = open(&key, &sealed.nonce, &sealed.ciphertext, b"header-B").unwrap_err();
        assert!(matches!(err, VaultError::Decrypt));
    }

    #[test]
    fn wrong_key_fails_like_wrong_password() {
        let good = test_key();
        let other = {
            let params = KdfParams {
                algo: KdfAlgo::Argon2id,
                m_cost_kib: 64,
                t_cost: 1,
                p_cost: 1,
                salt: vec![9u8; SALT_LEN],
            };
            derive_key(b"different", &params).unwrap()
        };
        let sealed = seal(&good, b"secret", b"hdr").unwrap();
        let err = open(&other, &sealed.nonce, &sealed.ciphertext, b"hdr").unwrap_err();
        assert!(matches!(err, VaultError::Decrypt));
    }

    #[test]
    fn fresh_nonce_each_seal() {
        let key = test_key();
        let a = seal(&key, b"x", b"").unwrap();
        let b = seal(&key, b"x", b"").unwrap();
        assert_ne!(a.nonce, b.nonce, "nonce must be fresh per seal");
    }
}
