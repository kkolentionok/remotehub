//! Sealing for the **notes** blob.
//!
//! Notes are stored in their own container so a device can be granted access
//! to them alone. That container is sealed under a *random* 256-bit key rather
//! than one derived from the master password, which has two consequences:
//!
//! * **No Argon2 anywhere on the notes path.** The vault pays a memory-hard
//!   derivation because its key must come from something a human remembers;
//!   the notes key is generated, stored, and transported, so opening the blob
//!   is a single AES-GCM operation. That is what makes a one-second cadence
//!   sane.
//! * **The key is a thing you can hand over.** Wrapped under a short pairing
//!   code, it grants notes access without granting anything else.
//!
//! The envelope is deliberately minimal: format tag, random nonce, ciphertext.
//! There are no KDF parameters to record because there is no KDF.

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::crypto::{self, NONCE_LEN_BYTES};
use crate::error::VaultError;
use crate::kdf::VaultKey;
use crate::model::SyncSnapshot;

/// On-the-wire format version for the notes envelope.
pub const NOTES_ENVELOPE_FORMAT: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotesEnvelope {
    pub format: u32,
    #[serde(with = "crate::b64")]
    pub nonce: Vec<u8>,
    #[serde(with = "crate::b64")]
    pub ciphertext: Vec<u8>,
}

fn aad() -> Vec<u8> {
    format!("pingie-notes-v{NOTES_ENVELOPE_FORMAT}").into_bytes()
}

/// Seal a notes snapshot, returning the JSON envelope to store remotely.
pub fn seal_notes(snapshot: &SyncSnapshot, key: &VaultKey) -> Result<String, VaultError> {
    let plaintext = Zeroizing::new(serde_json::to_vec(snapshot)?);
    let sealed = crypto::seal(key, &plaintext, &aad())?;
    let env = NotesEnvelope {
        format: NOTES_ENVELOPE_FORMAT,
        nonce: sealed.nonce.to_vec(),
        ciphertext: sealed.ciphertext,
    };
    Ok(serde_json::to_string(&env)?)
}

/// Open a notes envelope produced by [`seal_notes`].
pub fn open_notes(text: &str, key: &VaultKey) -> Result<SyncSnapshot, VaultError> {
    let env: NotesEnvelope = serde_json::from_str(text)?;
    if env.format != NOTES_ENVELOPE_FORMAT {
        return Err(VaultError::UnsupportedFormat(env.format));
    }
    if env.nonce.len() != NONCE_LEN_BYTES {
        return Err(VaultError::Malformed("bad nonce length".to_string()));
    }
    let mut nonce = [0u8; NONCE_LEN_BYTES];
    nonce.copy_from_slice(&env.nonce);

    let plaintext = Zeroizing::new(crypto::open(key, &nonce, &env.ciphertext, &aad())?);
    Ok(serde_json::from_slice(&plaintext)?)
}

/// A fresh random notes key.
pub fn gen_notes_key() -> Result<VaultKey, VaultError> {
    let mut raw = [0u8; crate::kdf::KEY_LEN];
    getrandom::getrandom(&mut raw).map_err(|_| VaultError::Encrypt)?;
    Ok(VaultKey::from_bytes(raw))
}

/// Wrap the notes key under a key derived from a pairing code, ready to be
/// parked on the server. The server stores this verbatim and cannot open it:
/// it never receives the code.
pub fn wrap_notes_key(notes_key: &VaultKey, code_key: &VaultKey) -> Result<String, VaultError> {
    let sealed = crypto::seal(code_key, notes_key.as_bytes(), b"pingie-notes-wrap-v1")?;
    let env = NotesEnvelope {
        format: NOTES_ENVELOPE_FORMAT,
        nonce: sealed.nonce.to_vec(),
        ciphertext: sealed.ciphertext,
    };
    Ok(serde_json::to_string(&env)?)
}

/// Unwrap a notes key with the key derived from the code the user typed.
pub fn unwrap_notes_key(wrapped: &str, code_key: &VaultKey) -> Result<VaultKey, VaultError> {
    let env: NotesEnvelope = serde_json::from_str(wrapped)?;
    if env.nonce.len() != NONCE_LEN_BYTES {
        return Err(VaultError::Malformed("bad nonce length".to_string()));
    }
    let mut nonce = [0u8; NONCE_LEN_BYTES];
    nonce.copy_from_slice(&env.nonce);

    let raw = Zeroizing::new(crypto::open(
        code_key,
        &nonce,
        &env.ciphertext,
        b"pingie-notes-wrap-v1",
    )?);
    if raw.len() != crate::kdf::KEY_LEN {
        return Err(VaultError::Malformed("bad key length".to_string()));
    }
    let mut key = [0u8; crate::kdf::KEY_LEN];
    key.copy_from_slice(&raw);
    Ok(VaultKey::from_bytes(key))
}
