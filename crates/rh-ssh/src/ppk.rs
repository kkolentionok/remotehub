//! PuTTY private-key (`.ppk`) → OpenSSH private-key conversion.
//!
//! russh only understands OpenSSH / PKCS#8 keys, so when a credential's
//! stored secret is a PuTTY `.ppk` file we convert it in memory before
//! handing it to `russh::keys::decode_secret_key`.
//!
//! Supported: PPK v2 and v3, encrypted (`aes256-cbc`) and unencrypted,
//! for algorithms `ssh-rsa`, `ssh-dss`, `ecdsa-sha2-nistp{256,384,521}`
//! and `ssh-ed25519`.
//!
//! The format is documented authoritatively in PuTTY's manual, Appendix C.
//! Summary of what we rely on:
//!
//! * The file is text: header lines, then base64 `Public-Lines`, then (for
//!   v3 encrypted) Argon2 headers, then base64 `Private-Lines`, then
//!   `Private-MAC`.
//! * The public blob is the ordinary SSH wire-format public key.
//! * The private blob holds only the *private* components (the public ones
//!   are not repeated). Its layout is per-algorithm (see `assemble_*`).
//! * MAC preimage = ssh-string(algorithm) ++ ssh-string(encryption) ++
//!   ssh-string(comment) ++ ssh-string(public_blob) ++
//!   ssh-string(private_blob_plaintext)  — HMAC-SHA1 (v2) / HMAC-SHA256 (v3).
//! * Key material (encrypted): v2 derives the AES key from SHA-1 hashes of
//!   the passphrase and a zero IV; v3 runs Argon2 → 32-byte key ++ 16-byte
//!   IV ++ 32-byte MAC key. Unencrypted keys use zero-length key/iv/mac-key.
//!
//! We then hand-assemble an unencrypted `openssh-key-v1` container and base64
//! it into a PEM block, which russh can read directly.

use aes::Aes256;
use base64::Engine as _;
use cbc::Decryptor;
use cipher::{block_padding::NoPadding, BlockDecryptMut, KeyIvInit};
use hmac::{Hmac, Mac};
use sha1::{Digest, Sha1};
use sha2::Sha256;

type Aes256CbcDec = Decryptor<Aes256>;

#[derive(Debug)]
pub enum PpkError {
    /// Structural problem (missing field, bad base64, truncated blob).
    Format(&'static str),
    /// Key algorithm we don't convert.
    UnsupportedAlgorithm,
    /// Encrypted key but no passphrase was supplied.
    NeedsPassphrase,
    /// MAC didn't match — wrong passphrase or a corrupted file.
    MacMismatch,
    /// A cryptographic primitive failed unexpectedly.
    Crypto(&'static str),
}

impl std::fmt::Display for PpkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PpkError::Format(m) => write!(f, "malformed .ppk: {m}"),
            PpkError::UnsupportedAlgorithm => write!(f, "unsupported key algorithm in .ppk"),
            PpkError::NeedsPassphrase => {
                write!(f, ".ppk is encrypted but no passphrase was supplied")
            }
            PpkError::MacMismatch => {
                write!(f, ".ppk MAC mismatch (wrong passphrase or corrupt file)")
            }
            PpkError::Crypto(m) => write!(f, ".ppk crypto error: {m}"),
        }
    }
}

impl std::error::Error for PpkError {}

/// Returns `true` if `text` looks like a PuTTY `.ppk` file.
pub fn is_ppk(text: &str) -> bool {
    text.contains("PuTTY-User-Key-File")
}

/// Convert a `.ppk` file's text into an OpenSSH PEM private key. The result
/// is *unencrypted* (we've already decrypted with `passphrase`), so the
/// caller should pass `None` as the passphrase to `decode_secret_key`.
pub fn ppk_to_openssh(text: &str, passphrase: Option<&str>) -> Result<String, PpkError> {
    let p = parse(text)?;
    let pass = passphrase.unwrap_or("");

    if p.encrypted && passphrase.is_none() {
        return Err(PpkError::NeedsPassphrase);
    }

    // Derive key material and decrypt the private blob if needed.
    let (priv_plain, mac_key) = if p.encrypted {
        let (cipher_key, iv, mac_key) = derive_encrypted(&p, pass)?;
        let plain = aes_cbc_decrypt(&cipher_key, &iv, &p.private_blob)?;
        (plain, mac_key)
    } else {
        // Unencrypted: zero-length key/iv/mac-key (v3) or, for v2, a MAC key
        // derived from the (empty) passphrase. See `mac_key_unencrypted`.
        (p.private_blob.clone(), mac_key_unencrypted(&p, pass))
    };

    verify_mac(&p, &priv_plain, &mac_key)?;

    let openssh = assemble_openssh(&p.algorithm, &p.public_blob, &priv_plain, &p.comment)?;
    Ok(pem_wrap(&openssh))
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

struct Ppk {
    version: u8,
    algorithm: String,
    encrypted: bool,
    comment: String,
    public_blob: Vec<u8>,
    private_blob: Vec<u8>,
    private_mac: Vec<u8>,
    // v3 Argon2 params (only meaningful when encrypted && version >= 3)
    argon2: Option<Argon2Params>,
}

struct Argon2Params {
    flavour: argon2::Algorithm,
    memory_kib: u32,
    passes: u32,
    parallelism: u32,
    salt: Vec<u8>,
}

fn parse(text: &str) -> Result<Ppk, PpkError> {
    // Tolerate CRLF / CR by trimming trailing CR on each line.
    let lines: Vec<&str> = text.lines().map(|l| l.trim_end_matches('\r')).collect();
    let mut i = 0usize;
    let next = |i: &mut usize| -> Option<&str> {
        let l = lines.get(*i).copied();
        *i += 1;
        l
    };

    let first = next(&mut i).ok_or(PpkError::Format("empty file"))?;
    let (vstr, algorithm) = first
        .strip_prefix("PuTTY-User-Key-File-")
        .and_then(|rest| rest.split_once(": "))
        .ok_or(PpkError::Format("missing PuTTY-User-Key-File header"))?;
    let version: u8 = vstr.parse().map_err(|_| PpkError::Format("bad version"))?;
    if version < 2 {
        return Err(PpkError::Format("PPK v1 not supported"));
    }
    let algorithm = algorithm.to_string();

    let mut encryption = String::new();
    let mut comment = String::new();
    let mut public_blob = Vec::new();
    let mut private_blob = Vec::new();
    let mut private_mac = Vec::new();
    let mut kd_flavour: Option<argon2::Algorithm> = None;
    let mut a_mem = 0u32;
    let mut a_pass = 0u32;
    let mut a_par = 0u32;
    let mut a_salt: Vec<u8> = Vec::new();

    while i < lines.len() {
        let line = next(&mut i).unwrap();
        if line.is_empty() {
            continue;
        }
        let (key, val) = line
            .split_once(": ")
            .ok_or(PpkError::Format("malformed header line"))?;
        match key {
            "Encryption" => encryption = val.to_string(),
            "Comment" => comment = val.to_string(),
            "Public-Lines" => {
                public_blob = read_b64_block(&lines, &mut i, val)?;
            }
            "Private-Lines" => {
                private_blob = read_b64_block(&lines, &mut i, val)?;
            }
            "Private-MAC" => {
                private_mac = hex_decode(val).ok_or(PpkError::Format("bad MAC hex"))?;
            }
            "Key-Derivation" => {
                kd_flavour = Some(match val {
                    "Argon2d" => argon2::Algorithm::Argon2d,
                    "Argon2i" => argon2::Algorithm::Argon2i,
                    "Argon2id" => argon2::Algorithm::Argon2id,
                    _ => return Err(PpkError::Format("unknown Argon2 flavour")),
                });
            }
            "Argon2-Memory" => a_mem = val.parse().map_err(|_| PpkError::Format("Argon2-Memory"))?,
            "Argon2-Passes" => a_pass = val.parse().map_err(|_| PpkError::Format("Argon2-Passes"))?,
            "Argon2-Parallelism" => {
                a_par = val.parse().map_err(|_| PpkError::Format("Argon2-Parallelism"))?
            }
            "Argon2-Salt" => {
                a_salt = hex_decode(val).ok_or(PpkError::Format("bad Argon2-Salt"))?
            }
            // Ignore unknown headers for forward-compat.
            _ => {}
        }
    }

    if public_blob.is_empty() || private_blob.is_empty() {
        return Err(PpkError::Format("missing key blobs"));
    }
    let encrypted = match encryption.as_str() {
        "none" | "" => false,
        "aes256-cbc" => true,
        _ => return Err(PpkError::Format("unsupported encryption")),
    };

    let argon2 = if encrypted && version >= 3 {
        Some(Argon2Params {
            flavour: kd_flavour.ok_or(PpkError::Format("missing Key-Derivation"))?,
            memory_kib: a_mem,
            passes: a_pass,
            parallelism: a_par,
            salt: a_salt,
        })
    } else {
        None
    };

    Ok(Ppk {
        version,
        algorithm,
        encrypted,
        comment,
        public_blob,
        private_blob,
        private_mac,
        argon2,
    })
}

fn read_b64_block(lines: &[&str], i: &mut usize, count: &str) -> Result<Vec<u8>, PpkError> {
    let n: usize = count.parse().map_err(|_| PpkError::Format("bad line count"))?;
    let mut b64 = String::new();
    for _ in 0..n {
        let l = lines.get(*i).ok_or(PpkError::Format("truncated blob"))?;
        b64.push_str(l.trim_end_matches('\r'));
        *i += 1;
    }
    base64::engine::general_purpose::STANDARD
        .decode(b64.as_bytes())
        .map_err(|_| PpkError::Format("bad base64"))
}

// ---------------------------------------------------------------------------
// Key derivation
// ---------------------------------------------------------------------------

/// Encrypted: derive (cipher_key[32], iv[16], mac_key[32]).
fn derive_encrypted(p: &Ppk, pass: &str) -> Result<([u8; 32], [u8; 16], Vec<u8>), PpkError> {
    if p.version >= 3 {
        let a = p.argon2.as_ref().ok_or(PpkError::Format("missing Argon2 params"))?;
        let params = argon2::Params::new(a.memory_kib, a.passes, a.parallelism, Some(80))
            .map_err(|_| PpkError::Crypto("argon2 params"))?;
        // PPK uses empty secret + empty associated data. Argon2 version 0x13.
        let ctx = argon2::Argon2::new(a.flavour, argon2::Version::V0x13, params);
        let mut out = [0u8; 80];
        ctx.hash_password_into(pass.as_bytes(), &a.salt, &mut out)
            .map_err(|_| PpkError::Crypto("argon2"))?;
        let mut key = [0u8; 32];
        let mut iv = [0u8; 16];
        key.copy_from_slice(&out[0..32]);
        iv.copy_from_slice(&out[32..48]);
        let mac_key = out[48..80].to_vec();
        Ok((key, iv, mac_key))
    } else {
        // v2: cipher key = first 32 bytes of SHA1(0||pass) ++ SHA1(1||pass).
        let mut key = [0u8; 32];
        let h0 = {
            let mut h = Sha1::new();
            h.update([0, 0, 0, 0]);
            h.update(pass.as_bytes());
            h.finalize()
        };
        let h1 = {
            let mut h = Sha1::new();
            h.update([0, 0, 0, 1]);
            h.update(pass.as_bytes());
            h.finalize()
        };
        key[0..20].copy_from_slice(&h0);
        key[20..32].copy_from_slice(&h1[0..12]);
        // v2 IV is all zeroes; MAC key is SHA1("...mac-key" || pass).
        let iv = [0u8; 16];
        let mac_key = mac_key_v2(pass);
        Ok((key, iv, mac_key))
    }
}

/// Unencrypted MAC key. v3 → empty (zero-length). v2 → SHA1 of the fixed
/// string and the (empty) passphrase.
fn mac_key_unencrypted(p: &Ppk, pass: &str) -> Vec<u8> {
    if p.version >= 3 {
        Vec::new()
    } else {
        mac_key_v2(pass)
    }
}

fn mac_key_v2(pass: &str) -> Vec<u8> {
    let mut h = Sha1::new();
    h.update(b"putty-private-key-file-mac-key");
    h.update(pass.as_bytes());
    h.finalize().to_vec()
}

fn aes_cbc_decrypt(key: &[u8; 32], iv: &[u8; 16], ct: &[u8]) -> Result<Vec<u8>, PpkError> {
    if ct.is_empty() || ct.len() % 16 != 0 {
        return Err(PpkError::Format("private blob not block-aligned"));
    }
    let mut buf = ct.to_vec();
    let cipher = Aes256CbcDec::new(&(*key).into(), &(*iv).into());
    let pt = cipher
        .decrypt_padded_mut::<NoPadding>(&mut buf)
        .map_err(|_| PpkError::Crypto("aes-cbc"))?;
    Ok(pt.to_vec())
}

// ---------------------------------------------------------------------------
// MAC verification
// ---------------------------------------------------------------------------

fn verify_mac(p: &Ppk, private_plain: &[u8], mac_key: &[u8]) -> Result<(), PpkError> {
    // Preimage: 5 ssh-strings.
    let mut data = Vec::new();
    put_string(&mut data, p.algorithm.as_bytes());
    put_string(&mut data, if p.encrypted { b"aes256-cbc" } else { b"none" });
    put_string(&mut data, p.comment.as_bytes());
    put_string(&mut data, &p.public_blob);
    put_string(&mut data, private_plain);

    let computed = if p.version >= 3 {
        let mut m =
            <Hmac<Sha256>>::new_from_slice(mac_key).map_err(|_| PpkError::Crypto("hmac key"))?;
        m.update(&data);
        m.finalize().into_bytes().to_vec()
    } else {
        let mut m =
            <Hmac<Sha1>>::new_from_slice(mac_key).map_err(|_| PpkError::Crypto("hmac key"))?;
        m.update(&data);
        m.finalize().into_bytes().to_vec()
    };

    if computed == p.private_mac {
        Ok(())
    } else {
        Err(PpkError::MacMismatch)
    }
}

// ---------------------------------------------------------------------------
// OpenSSH container assembly
// ---------------------------------------------------------------------------

fn assemble_openssh(
    algorithm: &str,
    public_blob: &[u8],
    private_plain: &[u8],
    comment: &str,
) -> Result<Vec<u8>, PpkError> {
    // Build the per-key "private section" body: type-specific public fields
    // followed by the private fields (this mirrors OpenSSH's layout).
    let body = match algorithm {
        "ssh-rsa" => assemble_rsa(public_blob, private_plain)?,
        "ssh-dss" => assemble_dss(public_blob, private_plain)?,
        "ssh-ed25519" => assemble_ed25519(public_blob, private_plain)?,
        a if a.starts_with("ecdsa-sha2-nistp") => assemble_ecdsa(public_blob, private_plain)?,
        _ => return Err(PpkError::UnsupportedAlgorithm),
    };

    // Private section: checkint twice, body, comment, then 1..n padding.
    let mut priv_section = Vec::new();
    let check: u32 = 0x5247_4248; // arbitrary; OpenSSH only checks the two match
    put_u32(&mut priv_section, check);
    put_u32(&mut priv_section, check);
    priv_section.extend_from_slice(&body);
    put_string(&mut priv_section, comment.as_bytes());
    let mut pad: u8 = 1;
    while priv_section.len() % 8 != 0 {
        priv_section.push(pad);
        pad = pad.wrapping_add(1);
    }

    let mut out = Vec::new();
    out.extend_from_slice(b"openssh-key-v1\0");
    put_string(&mut out, b"none"); // ciphername
    put_string(&mut out, b"none"); // kdfname
    put_string(&mut out, b""); // kdfoptions
    put_u32(&mut out, 1); // number of keys
    put_string(&mut out, public_blob); // public key
    put_string(&mut out, &priv_section); // (unencrypted) private section
    Ok(out)
}

fn assemble_rsa(public_blob: &[u8], private_plain: &[u8]) -> Result<Vec<u8>, PpkError> {
    // public:  string "ssh-rsa", mpint e, mpint n
    // private: mpint d, mpint p, mpint q, mpint iqmp
    // openssh: string "ssh-rsa", mpint n, mpint e, mpint d, mpint iqmp, mpint p, mpint q
    let mut pr = Reader::new(public_blob);
    let _alg = pr.string()?;
    let e = pr.string()?;
    let n = pr.string()?;
    let mut sr = Reader::new(private_plain);
    let d = sr.string()?;
    let p = sr.string()?;
    let q = sr.string()?;
    let iqmp = sr.string()?;

    let mut out = Vec::new();
    put_string(&mut out, b"ssh-rsa");
    put_string(&mut out, n);
    put_string(&mut out, e);
    put_string(&mut out, d);
    put_string(&mut out, iqmp);
    put_string(&mut out, p);
    put_string(&mut out, q);
    Ok(out)
}

fn assemble_dss(public_blob: &[u8], private_plain: &[u8]) -> Result<Vec<u8>, PpkError> {
    // public:  string "ssh-dss", mpint p, q, g, y
    // private: mpint x
    // openssh: string "ssh-dss", mpint p, q, g, y, x
    let mut pr = Reader::new(public_blob);
    let _alg = pr.string()?;
    let p = pr.string()?;
    let q = pr.string()?;
    let g = pr.string()?;
    let y = pr.string()?;
    let mut sr = Reader::new(private_plain);
    let x = sr.string()?;

    let mut out = Vec::new();
    put_string(&mut out, b"ssh-dss");
    put_string(&mut out, p);
    put_string(&mut out, q);
    put_string(&mut out, g);
    put_string(&mut out, y);
    put_string(&mut out, x);
    Ok(out)
}

fn assemble_ecdsa(public_blob: &[u8], private_plain: &[u8]) -> Result<Vec<u8>, PpkError> {
    // public:  string alg, string curve, string Q
    // private: mpint d
    // openssh: string alg, string curve, string Q, mpint d
    let mut pr = Reader::new(public_blob);
    let alg = pr.string()?;
    let curve = pr.string()?;
    let qpoint = pr.string()?;
    let mut sr = Reader::new(private_plain);
    let d = sr.string()?;

    let mut out = Vec::new();
    put_string(&mut out, alg);
    put_string(&mut out, curve);
    put_string(&mut out, qpoint);
    put_string(&mut out, d);
    Ok(out)
}

fn assemble_ed25519(public_blob: &[u8], private_plain: &[u8]) -> Result<Vec<u8>, PpkError> {
    // public:  string "ssh-ed25519", string A(32)
    // private: mpint privatescalar  (PuTTY stores the seed as an mpint)
    // openssh: string "ssh-ed25519", string A, string (seed32 || A32)
    let mut pr = Reader::new(public_blob);
    let _alg = pr.string()?;
    let a = pr.string()?;
    if a.len() != 32 {
        return Err(PpkError::Format("ed25519 public key not 32 bytes"));
    }
    let mut sr = Reader::new(private_plain);
    let seed_mpint = sr.string()?;
    let seed = normalize_fixed(seed_mpint, 32).ok_or(PpkError::Format("ed25519 seed length"))?;

    let mut skpk = Vec::with_capacity(64);
    skpk.extend_from_slice(&seed);
    skpk.extend_from_slice(a);

    let mut out = Vec::new();
    put_string(&mut out, b"ssh-ed25519");
    put_string(&mut out, a);
    put_string(&mut out, &skpk);
    Ok(out)
}

/// Turn an mpint (possibly with a leading sign zero, or with stripped leading
/// zeros) into a fixed-width big-endian byte array of length `n`.
fn normalize_fixed(mpint: &[u8], n: usize) -> Option<Vec<u8>> {
    let mut b = mpint;
    // Drop a single leading 0x00 used as the mpint sign byte.
    while b.len() > n && b.first() == Some(&0) {
        b = &b[1..];
    }
    if b.len() > n {
        return None;
    }
    let mut out = vec![0u8; n - b.len()];
    out.extend_from_slice(b);
    Some(out)
}

// ---------------------------------------------------------------------------
// SSH wire helpers
// ---------------------------------------------------------------------------

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }
    fn string(&mut self) -> Result<&'a [u8], PpkError> {
        if self.pos + 4 > self.buf.len() {
            return Err(PpkError::Format("truncated length"));
        }
        let len = u32::from_be_bytes([
            self.buf[self.pos],
            self.buf[self.pos + 1],
            self.buf[self.pos + 2],
            self.buf[self.pos + 3],
        ]) as usize;
        self.pos += 4;
        if self.pos + len > self.buf.len() {
            return Err(PpkError::Format("truncated string"));
        }
        let s = &self.buf[self.pos..self.pos + len];
        self.pos += len;
        Ok(s)
    }
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn put_string(out: &mut Vec<u8>, bytes: &[u8]) {
    put_u32(out, bytes.len() as u32);
    out.extend_from_slice(bytes);
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let hi = (bytes[i] as char).to_digit(16)?;
        let lo = (bytes[i + 1] as char).to_digit(16)?;
        out.push(((hi << 4) | lo) as u8);
        i += 2;
    }
    Some(out)
}

fn pem_wrap(der: &[u8]) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(der);
    let mut s = String::from("-----BEGIN OPENSSH PRIVATE KEY-----\n");
    for chunk in b64.as_bytes().chunks(70) {
        s.push_str(std::str::from_utf8(chunk).unwrap());
        s.push('\n');
    }
    s.push_str("-----END OPENSSH PRIVATE KEY-----\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        assert_eq!(hex_decode("00ff10"), Some(vec![0x00, 0xff, 0x10]));
        assert_eq!(hex_decode("0"), None);
        assert_eq!(hex_decode("zz"), None);
    }

    #[test]
    fn normalize_pads_and_strips() {
        assert_eq!(normalize_fixed(&[0x01, 0x02], 4), Some(vec![0, 0, 1, 2]));
        // leading sign byte dropped
        assert_eq!(normalize_fixed(&[0x00, 0xff], 1), Some(vec![0xff]));
        // too long
        assert_eq!(normalize_fixed(&[1, 2, 3], 2), None);
    }

    #[test]
    fn rejects_v1() {
        let err = parse("PuTTY-User-Key-File-1: ssh-rsa\n");
        assert!(matches!(err, Err(PpkError::Format(_))));
    }

    #[test]
    fn detects_ppk() {
        assert!(is_ppk("PuTTY-User-Key-File-3: ssh-ed25519\n..."));
        assert!(!is_ppk("-----BEGIN OPENSSH PRIVATE KEY-----"));
    }
}
