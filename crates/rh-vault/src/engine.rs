//! The sync engine — one transport-agnostic `pull → open → merge → seal →
//! push` pass against a [`SyncRemote`]. Slice 2 of the rollout (see
//! `docs/specs/sync.md` §11).
//!
//! This is deliberately **pure**: it depends only on this crate's crypto,
//! snapshot model, and merge — never on storage, the keychain, or Tauri. The
//! `rh-app` layer builds the local [`SyncSnapshot`] from storage, calls
//! [`sync_once`], and writes the returned merged snapshot back. The real
//! server transport (slice 3) plugs in by implementing [`SyncRemote`]; the
//! engine does not change.
//!
//! ## Concurrency
//! Two devices can sync at nearly the same moment. The transport provides
//! optimistic concurrency (a version token); if the remote moved between our
//! pull and our push, [`SyncRemote::push`] returns
//! [`VaultError::RemoteConflict`] and we re-pull, re-merge, and retry up to a
//! small bound. Because [`merge`] is convergent (a deterministic total order
//! on records), retrying always makes progress.

use crate::clock::HlcGenerator;
use crate::envelope::{from_export_string, open_envelope, seal_snapshot, to_export_string};
use crate::error::VaultError;
use crate::merge::merge;
use crate::model::SyncSnapshot;
use crate::transport::SyncRemote;

/// Bound on conflict re-merge attempts within a single [`sync_once`] call.
/// A handful is plenty: each retry observes the latest remote, so contention
/// resolves in one or two rounds in practice.
const MAX_PUSH_ATTEMPTS: usize = 5;

/// Outcome of one [`sync_once`] pass.
#[derive(Debug, Clone)]
pub struct SyncReport {
    /// The reconciled state. The caller writes this back into local storage
    /// (and keychain) so the device reflects the merge result.
    pub merged: SyncSnapshot,
    /// Remote version token after the push. Opaque; the next sync uses it as
    /// the `expected` precondition.
    pub version: String,
    /// `true` if the remote already held data, `false` on a first-ever sync
    /// (the local snapshot was pushed verbatim).
    pub had_remote: bool,
}

/// Run one sync against `remote` using `password` to seal/open the vault.
///
/// Steps: pull the current blob → if present, decrypt it and merge it into
/// `local` (record-level last-write-wins) → seal the result → push it back
/// with the pulled version as the optimistic-concurrency precondition. On
/// [`VaultError::RemoteConflict`] the loop re-pulls and re-merges.
///
/// `clock` is folded past any observed remote stamp so the caller can persist
/// its seed and keep future stamps monotonic across restarts. The merged
/// snapshot is returned rather than applied: the engine is storage-agnostic.
pub async fn sync_once(
    remote: &dyn SyncRemote,
    password: &[u8],
    local: &SyncSnapshot,
    clock: &mut HlcGenerator,
) -> Result<SyncReport, VaultError> {
    for attempt in 1..=MAX_PUSH_ATTEMPTS {
        let pulled = remote.pull().await?;
        let (merged, expected, had_remote) = match &pulled {
            Some(blob) => {
                let text = std::str::from_utf8(&blob.bytes)
                    .map_err(|_| VaultError::Transport("remote blob is not valid UTF-8".into()))?;
                let envelope = from_export_string(text)?;
                let remote_snap = open_envelope(&envelope, password)?;
                // Fold remote logical time in before stamping anything new.
                clock.observe(remote_snap.generated);
                let merged = merge(local, &remote_snap, local.node.clone());
                clock.observe(merged.generated);
                // No-op fast path: if the merge produced nothing the remote
                // doesn't already have, skip the push entirely. Re-sealing
                // would mint a fresh nonce → a "different" blob → a needless
                // rev bump, full re-upload, and (worst) multi-device conflict
                // churn where idle devices keep ping-ponging the version.
                if records_equal(&merged, &remote_snap) {
                    return Ok(SyncReport {
                        merged,
                        version: blob.version.clone(),
                        had_remote: true,
                    });
                }
                (merged, Some(blob.version.clone()), true)
            }
            // Empty remote: first-ever sync. Push local as-is.
            None => (local.clone(), None, false),
        };

        let sealed = seal_snapshot(&merged, password)?;
        let bytes = to_export_string(&sealed)?.into_bytes();
        match remote.push(&bytes, expected.as_deref()).await {
            Ok(version) => {
                return Ok(SyncReport {
                    merged,
                    version,
                    had_remote,
                })
            }
            // Someone pushed between our pull and push: re-pull and re-merge.
            Err(VaultError::RemoteConflict) if attempt < MAX_PUSH_ATTEMPTS => continue,
            Err(e) => return Err(e),
        }
    }
    // Reached only if every attempt conflicted (extreme contention).
    Err(VaultError::RemoteConflict)
}

/// Content equality of two snapshots, ignoring envelope-level fields
/// (`node`, `generated`, `format`) and record order — only each record's
/// `(kind, id, meta, data)` matters. Used to decide whether a merge produced
/// anything the remote doesn't already hold; if not, the push is skipped.
fn records_equal(a: &SyncSnapshot, b: &SyncSnapshot) -> bool {
    use crate::model::EntityKind;
    use std::collections::BTreeMap;

    if a.records.len() != b.records.len() {
        return false;
    }
    let ia: BTreeMap<(EntityKind, &str), &_> = a
        .records
        .iter()
        .map(|r| ((r.kind, r.id.as_str()), r))
        .collect();
    let ib: BTreeMap<(EntityKind, &str), &_> = b
        .records
        .iter()
        .map(|r| ((r.kind, r.id.as_str()), r))
        .collect();
    ia.len() == ib.len() && ia.iter().all(|(k, ra)| ib.get(k).is_some_and(|rb| ra == rb))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::{Hlc, NodeId};
    use crate::model::SyncRecord;
    use crate::transport::{MemoryRemote, RemoteBlob};
    use std::sync::atomic::{AtomicBool, Ordering};

    fn snap(node: &str, gen: Hlc, recs: Vec<SyncRecord>) -> SyncSnapshot {
        SyncSnapshot::new(NodeId::new(node), gen, recs)
    }

    // Setting records keep the tests free of `rh_core` entity construction —
    // the engine and merge are entity-agnostic (they key on `(kind, id)` and
    // order by `meta`), so a setting exercises exactly the same paths.
    fn setting(key: &str, val: i64, rev: Hlc, origin: &str) -> SyncRecord {
        SyncRecord::setting(key, serde_json::json!(val), rev, NodeId::new(origin))
    }

    fn live_ids(s: &SyncSnapshot) -> Vec<String> {
        let mut v: Vec<String> = s
            .records
            .iter()
            .filter(|r| !r.is_deleted())
            .map(|r| r.id.clone())
            .collect();
        v.sort();
        v
    }

    #[tokio::test]
    async fn first_sync_pushes_local_to_empty_remote() {
        let remote = MemoryRemote::new();
        let local = snap("A", Hlc::new(10, 0), vec![setting("x", 1, Hlc::new(10, 0), "A")]);
        let mut clk = HlcGenerator::new(Hlc::new(10, 0));

        let r = sync_once(&remote, b"pw", &local, &mut clk).await.unwrap();

        assert!(!r.had_remote, "remote was empty");
        assert!(remote.pull().await.unwrap().is_some(), "remote now holds the blob");
    }

    #[tokio::test]
    async fn two_devices_converge() {
        let remote = MemoryRemote::new();
        let pw = b"pw";
        // A edited setting x; B independently edited setting y.
        let a = snap("A", Hlc::new(10, 0), vec![setting("x", 1, Hlc::new(10, 0), "A")]);
        let b = snap("B", Hlc::new(20, 0), vec![setting("y", 2, Hlc::new(20, 0), "B")]);
        let mut ca = HlcGenerator::new(Hlc::new(10, 0));
        let mut cb = HlcGenerator::new(Hlc::new(20, 0));

        let ra = sync_once(&remote, pw, &a, &mut ca).await.unwrap(); // pushes {x}
        let rb = sync_once(&remote, pw, &b, &mut cb).await.unwrap(); // merges -> {x,y}
        let ra2 = sync_once(&remote, pw, &ra.merged, &mut ca).await.unwrap(); // pulls -> {x,y}

        assert_eq!(live_ids(&ra2.merged), vec!["x".to_string(), "y".to_string()]);
        assert_eq!(
            live_ids(&ra2.merged),
            live_ids(&rb.merged),
            "both devices see the same set after a round"
        );
    }

    #[tokio::test]
    async fn newer_revision_wins_same_key() {
        let remote = MemoryRemote::new();
        let pw = b"pw";
        let a = snap("A", Hlc::new(10, 0), vec![setting("x", 1, Hlc::new(10, 0), "A")]);
        sync_once(&remote, pw, &a, &mut HlcGenerator::new(Hlc::new(10, 0)))
            .await
            .unwrap();

        // B holds a strictly newer edit of the same key.
        let b = snap("B", Hlc::new(50, 0), vec![setting("x", 2, Hlc::new(50, 0), "B")]);
        let rb = sync_once(&remote, pw, &b, &mut HlcGenerator::new(Hlc::new(50, 0)))
            .await
            .unwrap();

        let x = rb.merged.records.iter().find(|r| r.id == "x").unwrap();
        assert_eq!(x.meta.rev, Hlc::new(50, 0));
        assert_eq!(x.data, Some(serde_json::json!(2)));
    }

    #[tokio::test]
    async fn wrong_password_errors_and_leaves_remote() {
        let remote = MemoryRemote::new();
        let a = snap("A", Hlc::new(10, 0), vec![setting("x", 1, Hlc::new(10, 0), "A")]);
        sync_once(&remote, b"right", &a, &mut HlcGenerator::new(Hlc::new(10, 0)))
            .await
            .unwrap();
        let before = remote.pull().await.unwrap().unwrap().version;

        // A second device with the wrong password cannot open the remote blob.
        let b = snap("B", Hlc::new(20, 0), vec![setting("y", 2, Hlc::new(20, 0), "B")]);
        let res = sync_once(&remote, b"wrong", &b, &mut HlcGenerator::new(Hlc::new(20, 0))).await;
        assert!(res.is_err(), "decrypt fails on wrong password");

        // The failure happens before any push, so the remote is untouched.
        assert_eq!(remote.pull().await.unwrap().unwrap().version, before);
    }

    /// A remote that returns a single spurious conflict on its first push, then
    /// behaves like a normal [`MemoryRemote`]. Exercises the engine's retry.
    #[derive(Debug)]
    struct FlakyOnce {
        inner: MemoryRemote,
        conflicted: AtomicBool,
    }

    #[async_trait::async_trait]
    impl SyncRemote for FlakyOnce {
        async fn pull(&self) -> Result<Option<RemoteBlob>, VaultError> {
            self.inner.pull().await
        }
        async fn push(&self, bytes: &[u8], expected: Option<&str>) -> Result<String, VaultError> {
            if !self.conflicted.swap(true, Ordering::SeqCst) {
                return Err(VaultError::RemoteConflict);
            }
            self.inner.push(bytes, expected).await
        }
    }

    #[tokio::test]
    async fn second_sync_with_no_changes_skips_push() {
        let remote = MemoryRemote::new();
        let pw = b"pw";
        let a = snap("A", Hlc::new(10, 0), vec![setting("x", 1, Hlc::new(10, 0), "A")]);
        let mut clk = HlcGenerator::new(Hlc::new(10, 0));

        let r1 = sync_once(&remote, pw, &a, &mut clk).await.unwrap();
        let remote_v = remote.pull().await.unwrap().unwrap().version;

        // Re-sync the merged result: nothing new to contribute.
        let r2 = sync_once(&remote, pw, &r1.merged, &mut clk).await.unwrap();

        assert!(r2.had_remote, "remote held our blob");
        assert_eq!(r2.version, r1.version, "no-op sync keeps the same version");
        assert_eq!(
            remote.pull().await.unwrap().unwrap().version,
            remote_v,
            "no-op sync must not push a new blob (no rev bump)"
        );
    }

    #[tokio::test]
    async fn sync_with_a_local_change_still_pushes() {
        let remote = MemoryRemote::new();
        let pw = b"pw";
        let a = snap("A", Hlc::new(10, 0), vec![setting("x", 1, Hlc::new(10, 0), "A")]);
        let mut clk = HlcGenerator::new(Hlc::new(10, 0));

        sync_once(&remote, pw, &a, &mut clk).await.unwrap();
        let remote_v = remote.pull().await.unwrap().unwrap().version;

        // Local gains a record the remote doesn't have.
        let a2 = snap(
            "A",
            Hlc::new(30, 0),
            vec![
                setting("x", 1, Hlc::new(10, 0), "A"),
                setting("y", 9, Hlc::new(30, 0), "A"),
            ],
        );
        let r2 = sync_once(&remote, pw, &a2, &mut clk).await.unwrap();

        assert!(r2.had_remote);
        assert_ne!(
            remote.pull().await.unwrap().unwrap().version,
            remote_v,
            "a real change must push a new blob"
        );
        assert_eq!(live_ids(&r2.merged), vec!["x".to_string(), "y".to_string()]);
    }
}
