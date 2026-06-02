//! The portable, E2E-encrypted vault envelope.
//!
//! A [`VaultEnvelope`] is the on-the-wire / on-disk form: a small
//! cleartext header (format version, KDF parameters, AEAD nonce) plus the
//! AES-256-GCM ciphertext of a serialized [`SyncSnapshot`]. It is what the
//! user exports to a file, and what a [`crate::transport::SyncRemote`]
//! uploads/downloads. The bytes that leave the device are ciphertext; the
//! master password never does.
//!
//! ```text
//! VaultEnvelope (JSON)
//! ├─ format        u32                      (cleartext)
//! ├─ kdf           { argon2id, m, t, p, salt }  (cleartext, needed to re-derive)
//! ├─ nonce         base64 12 bytes          (cleartext)
//! └─ ciphertext    base64 (snapshot_json ‖ gcm_tag)   (secret)
//! ```
//!
//! The cleartext header is bound into the AEAD as additional authenticated
//! data (AAD), so an attacker cannot, say, swap in weaker KDF parameters
//! and have it still verify.

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::crypto::{self, NONCE_LEN_BYTES};
use crate::error::VaultError;
use crate::kdf::{derive_key, KdfParams};
use crate::model::SyncSnapshot;

/// Bump when the envelope wire shape changes incompatibly.
pub const ENVELOPE_FORMAT: u32 = 1;

/// A sealed, portable vault. Serializes to/from JSON for export files and
/// for transport payloads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultEnvelope {
    pub format: u32,
    pub kdf: KdfParams,
    /// AEAD nonce, base64. Exactly 12 bytes once decoded.
    #[serde(with = "crate::b64")]
    pub nonce: Vec<u8>,
    /// `snapshot_json || gcm_tag`, base64.
    #[serde(with = "crate::b64")]
    pub ciphertext: Vec<u8>,
}

impl VaultEnvelope {
    /// Derive the additional-authenticated-data binding: the cleartext
    /// header fields that must not be tampered with. We bind a canonical
    /// JSON of `{format, kdf}` so the salt/params/version are integrity-
    /// protected by the same tag that protects the payload.
    fn aad(&self) -> Result<Vec<u8>, VaultError> {
        Self::aad_of(self.format, &self.kdf)
    }

    fn aad_of(format: u32, kdf: &KdfParams) -> Result<Vec<u8>, VaultError> {
        let header = serde_json::json!({ "format": format, "kdf": kdf });
        Ok(serde_json::to_vec(&header)?)
    }
}

/// Seal a snapshot into a portable envelope under `password`, generating a
/// fresh salt + nonce. This is the export / "save the vault" path.
pub fn seal_snapshot(snapshot: &SyncSnapshot, password: &[u8]) -> Result<VaultEnvelope, VaultError> {
    let kdf = KdfParams::new_default();
    seal_snapshot_with(snapshot, password, kdf)
}

/// Seal using caller-supplied KDF parameters (used by re-encrypt flows and
/// by tests that want cheap parameters).
pub fn seal_snapshot_with(
    snapshot: &SyncSnapshot,
    password: &[u8],
    kdf: KdfParams,
) -> Result<VaultEnvelope, VaultError> {
    // Serialize the plaintext snapshot; keep it in a zeroizing buffer so
    // the cleartext (which includes secret bytes) is wiped after sealing.
    let plaintext = Zeroizing::new(serde_json::to_vec(snapshot)?);

    let key = derive_key(password, &kdf)?;
    let aad = VaultEnvelope::aad_of(ENVELOPE_FORMAT, &kdf)?;
    let sealed = crypto::seal(&key, &plaintext, &aad)?;

    Ok(VaultEnvelope {
        format: ENVELOPE_FORMAT,
        kdf,
        nonce: sealed.nonce.to_vec(),
        ciphertext: sealed.ciphertext,
    })
}

/// Open an envelope with `password`, recovering the snapshot.
///
/// A wrong password (or any tampering) surfaces as [`VaultError::Decrypt`]
/// — the AEAD tag will not verify. An unknown format is
/// [`VaultError::UnsupportedFormat`].
pub fn open_envelope(envelope: &VaultEnvelope, password: &[u8]) -> Result<SyncSnapshot, VaultError> {
    if envelope.format != ENVELOPE_FORMAT {
        return Err(VaultError::UnsupportedFormat(envelope.format));
    }
    if envelope.nonce.len() != NONCE_LEN_BYTES {
        return Err(VaultError::Malformed(format!(
            "nonce length {} (expected {})",
            envelope.nonce.len(),
            NONCE_LEN_BYTES
        )));
    }
    let mut nonce = [0u8; NONCE_LEN_BYTES];
    nonce.copy_from_slice(&envelope.nonce);

    let key = derive_key(password, &envelope.kdf)?;
    let aad = envelope.aad()?;
    let plaintext = Zeroizing::new(crypto::open(&key, &nonce, &envelope.ciphertext, &aad)?);

    let snapshot: SyncSnapshot = serde_json::from_slice(&plaintext)?;
    snapshot.check_format()?;
    Ok(snapshot)
}

/// Serialize an envelope to a pretty JSON string for an export file.
pub fn to_export_string(envelope: &VaultEnvelope) -> Result<String, VaultError> {
    Ok(serde_json::to_string_pretty(envelope)?)
}

/// Parse an envelope from an export-file string.
pub fn from_export_string(s: &str) -> Result<VaultEnvelope, VaultError> {
    Ok(serde_json::from_str(s)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::{Hlc, NodeId};
    use crate::kdf::{gen_salt, KdfAlgo};
    use crate::model::{SyncCredentialPayload, SyncRecord};
    use rh_core::{Credential, CredentialKind, Host, Protocol};

    fn cheap_kdf() -> KdfParams {
        KdfParams {
            algo: KdfAlgo::Argon2id,
            m_cost_kib: 64,
            t_cost: 1,
            p_cost: 1,
            salt: gen_salt(),
        }
    }

    fn sample_snapshot() -> SyncSnapshot {
        let node = NodeId::new("device-1");
        let h = Host::new("web", "10.0.0.5", Protocol::Ssh, Some(22));
        let cred = Credential::new("root", CredentialKind::Password, "root");
        let payload = SyncCredentialPayload {
            credential: cred,
            secret: Some(b"s3cr3t-bytes".to_vec()),
            passphrase: None,
        };
        SyncSnapshot::new(
            node.clone(),
            Hlc::new(10, 0),
            vec![
                SyncRecord::host(&h, Hlc::new(9, 0), node.clone()).unwrap(),
                SyncRecord::credential(&payload, Hlc::new(10, 0), node).unwrap(),
            ],
        )
    }

    #[test]
    fn seal_open_roundtrip() {
        let snap = sample_snapshot();
        let env = seal_snapshot_with(&snap, b"master-pw", cheap_kdf()).unwrap();
        let back = open_envelope(&env, b"master-pw").unwrap();
        assert_eq!(back, snap);
    }

    #[test]
    fn wrong_password_fails_to_open() {
        let snap = sample_snapshot();
        let env = seal_snapshot_with(&snap, b"correct", cheap_kdf()).unwrap();
        let err = open_envelope(&env, b"wrong").unwrap_err();
        assert!(matches!(err, VaultError::Decrypt));
    }

    #[test]
    fn ciphertext_does_not_leak_the_secret() {
        let snap = sample_snapshot();
        let env = seal_snapshot_with(&snap, b"pw", cheap_kdf()).unwrap();
        // The plaintext secret bytes must not appear anywhere in the
        // serialized envelope.
        let exported = to_export_string(&env).unwrap();
        assert!(!exported.contains("s3cr3t-bytes"));
        // base64 of the secret also must not appear (it's inside GCM).
        use base64::{engine::general_purpose::STANDARD, Engine};
        let b64_secret = STANDARD.encode(b"s3cr3t-bytes");
        assert!(!exported.contains(&b64_secret));
    }

    #[test]
    fn tampering_with_kdf_params_is_detected() {
        let snap = sample_snapshot();
        let mut env = seal_snapshot_with(&snap, b"pw", cheap_kdf()).unwrap();
        // Attacker bumps t_cost in the cleartext header. Because the header
        // is bound as AAD, the tag no longer verifies — and the derived key
        // changes too. Either way: Decrypt error, no silent acceptance.
        env.kdf.t_cost += 1;
        let err = open_envelope(&env, b"pw").unwrap_err();
        assert!(matches!(err, VaultError::Decrypt));
    }

    #[test]
    fn export_string_roundtrips() {
        let snap = sample_snapshot();
        let env = seal_snapshot_with(&snap, b"pw", cheap_kdf()).unwrap();
        let s = to_export_string(&env).unwrap();
        let parsed = from_export_string(&s).unwrap();
        let back = open_envelope(&parsed, b"pw").unwrap();
        assert_eq!(back, snap);
    }

    #[test]
    fn unsupported_format_rejected() {
        let snap = sample_snapshot();
        let mut env = seal_snapshot_with(&snap, b"pw", cheap_kdf()).unwrap();
        env.format = 999;
        let err = open_envelope(&env, b"pw").unwrap_err();
        assert!(matches!(err, VaultError::UnsupportedFormat(999)));
    }
}
