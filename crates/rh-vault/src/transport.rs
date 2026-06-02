//! The backend-agnostic transport seam.
//!
//! Everything above this module — the vault crypto, the snapshot model,
//! the merge — is independent of *where* the encrypted blob lives. This
//! trait is the one place the sync backend choice (A self-hosted server /
//! B object store like S3 or WebDAV / C a file in a cloud-sync folder)
//! plugs in. Each option implements [`SyncRemote`]; the sync engine in
//! `rh-app` is written once against the trait.
//!
//! ## Contract
//! The transport moves **opaque ciphertext** ([`VaultEnvelope`] bytes). It
//! never sees plaintext and needs no knowledge of the snapshot shape. It
//! must provide *optimistic concurrency*: every stored blob has a version
//! token (an ETag, an HTTP `Last-Modified`, a file hash, a server
//! revision — whatever the backend offers), and [`SyncRemote::push`]
//! takes the version the caller based its merge on. If the remote moved
//! since, push fails with [`VaultError::RemoteConflict`] and the engine
//! pulls + re-merges + retries. This is what prevents a lost update when
//! two devices sync near-simultaneously.
//!
//! ### How A/B/C each satisfy this
//! - **A — self-hosted server:** `version` = server row revision; `push`
//!   sends `If-Match: <rev>`; the server rejects a stale rev (409).
//! - **B — object store (S3 / WebDAV):** `version` = ETag; `push` uses
//!   conditional `PUT If-Match` (S3) / `If` precondition (WebDAV).
//! - **C — cloud-sync folder file:** `version` = content hash + mtime;
//!   `push` re-reads before writing and compares (best effort; the OS sync
//!   client may still produce a "conflicted copy" the engine then merges).

use async_trait::async_trait;

use crate::error::VaultError;

/// An encrypted blob fetched from a remote, with its version token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteBlob {
    /// Serialized [`crate::VaultEnvelope`] bytes (ciphertext + header).
    pub bytes: Vec<u8>,
    /// Opaque version token for optimistic concurrency. Compare by
    /// equality only; do not parse.
    pub version: String,
}

/// A pluggable sync backend. Implementations live in `rh-app` (or a future
/// `rh-sync` crate) once the A/B/C choice is made; this crate only defines
/// the contract and a test double.
#[async_trait]
pub trait SyncRemote: Send + Sync {
    /// Fetch the current remote blob, or `None` if the remote is empty
    /// (first-ever sync). Network/IO failures map to
    /// [`VaultError::Transport`].
    async fn pull(&self) -> Result<Option<RemoteBlob>, VaultError>;

    /// Upload `bytes` as the new remote state. `expected` is the version
    /// token the caller pulled and merged against (`None` when the caller
    /// believes the remote is empty). Returns the new version token.
    ///
    /// If the remote's current version differs from `expected`, the push
    /// MUST fail with [`VaultError::RemoteConflict`] and leave the remote
    /// unchanged.
    async fn push(&self, bytes: &[u8], expected: Option<&str>) -> Result<String, VaultError>;
}

/// In-memory [`SyncRemote`] for tests and the sync-engine dry runs. Not
/// for production use. Versioning is a monotonically increasing counter.
#[derive(Debug, Default)]
pub struct MemoryRemote {
    inner: std::sync::Mutex<Option<RemoteBlob>>,
    seq: std::sync::atomic::AtomicU64,
}

impl MemoryRemote {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl SyncRemote for MemoryRemote {
    async fn pull(&self) -> Result<Option<RemoteBlob>, VaultError> {
        Ok(self.inner.lock().expect("poisoned").clone())
    }

    async fn push(&self, bytes: &[u8], expected: Option<&str>) -> Result<String, VaultError> {
        let mut guard = self.inner.lock().expect("poisoned");
        let current = guard.as_ref().map(|b| b.version.as_str());
        if current != expected {
            return Err(VaultError::RemoteConflict);
        }
        let v = self
            .seq
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        let version = format!("v{v}");
        *guard = Some(RemoteBlob {
            bytes: bytes.to_vec(),
            version: version.clone(),
        });
        Ok(version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn empty_remote_pulls_none() {
        let r = MemoryRemote::new();
        assert!(r.pull().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn push_then_pull_roundtrips_with_version() {
        let r = MemoryRemote::new();
        let v1 = r.push(b"blob-one", None).await.unwrap();
        let got = r.pull().await.unwrap().unwrap();
        assert_eq!(got.bytes, b"blob-one");
        assert_eq!(got.version, v1);
    }

    #[tokio::test]
    async fn stale_expected_version_conflicts() {
        let r = MemoryRemote::new();
        let v1 = r.push(b"one", None).await.unwrap();
        // A second writer that still thinks the remote is empty:
        let err = r.push(b"two", None).await.unwrap_err();
        assert!(matches!(err, VaultError::RemoteConflict));
        // The correct expected version succeeds:
        let v2 = r.push(b"two", Some(&v1)).await.unwrap();
        assert_ne!(v1, v2);
        assert_eq!(r.pull().await.unwrap().unwrap().bytes, b"two");
    }
}
