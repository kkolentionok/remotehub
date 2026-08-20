//! Shared, persisted per-device sync identity + monotonic clock.
//!
//! One [`HlcGenerator`] for the whole process behind an async mutex, plus a
//! stable [`NodeId`], persisted to `sync-identity.json` in the app-data dir
//! (local, **never** synced). Every mutation stamps a record's provenance
//! with a fresh stamp from here, so revisions are monotonic across the app and
//! across restarts — which is what makes the merge true last-edit-wins (see
//! `docs/specs/sync.md`, slice 2b). Replaces slice 1/2's per-call identity.

use std::path::{Path, PathBuf};

use rh_core::SyncStamp;
use rh_vault::{Hlc, HlcGenerator, NodeId};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

/// `sync_meta.kind` labels. Internal identifiers for the provenance table —
/// not the wire `EntityKind`. Centralized so the bump/tombstone call sites and
/// the snapshot builder can't drift apart on a typo.
pub const KIND_HOST: &str = "host";
pub const KIND_GROUP: &str = "group";
pub const KIND_CREDENTIAL: &str = "credential";
pub const KIND_SNIPPET: &str = "snippet";
pub const KIND_NOTE: &str = "note";
// `KIND_SETTING` will return when settings replication lands (settings are not
// yet part of the snapshot, so the constant would be dead code now).

const IDENTITY_FILE: &str = "sync-identity.json";

/// On-disk form of the identity.
#[derive(Serialize, Deserialize)]
struct IdentityFile {
    node: String,
    wall_ms: u64,
    counter: u32,
}

/// Process-wide device identity + HLC source. Held in `AppState` behind `Arc`.
#[derive(Debug)]
pub struct SyncClock {
    node: NodeId,
    gen: Mutex<HlcGenerator>,
    file: PathBuf,
}

impl SyncClock {
    /// Load the persisted identity from `dir/sync-identity.json`, or mint a
    /// fresh one (new ULID node, zero clock) and persist it. All IO is
    /// best-effort: an unreadable file just yields a new node (harmless — it
    /// only costs deterministic tiebreaking for exact-rev ties).
    #[must_use]
    pub fn load_or_init(dir: &Path) -> Self {
        let file = dir.join(IDENTITY_FILE);
        if let Ok(text) = std::fs::read_to_string(&file) {
            if let Ok(f) = serde_json::from_str::<IdentityFile>(&text) {
                return Self {
                    node: NodeId::new(f.node),
                    gen: Mutex::new(HlcGenerator::new(Hlc::new(f.wall_ms, f.counter))),
                    file,
                };
            }
        }
        let clock = Self {
            node: NodeId::new(ulid::Ulid::new().to_string()),
            gen: Mutex::new(HlcGenerator::new(Hlc::ZERO)),
            file,
        };
        clock.persist(Hlc::ZERO);
        clock
    }

    /// This device's stable node id.
    #[must_use]
    pub fn node(&self) -> &NodeId {
        &self.node
    }

    /// Produce the next monotonic stamp and persist the advanced seed.
    pub async fn next(&self) -> Hlc {
        let mut g = self.gen.lock().await;
        let h = g.now();
        self.persist(h);
        h
    }

    /// Next stamp as a storage-neutral [`SyncStamp`] tagged with this node.
    pub async fn next_stamp(&self) -> SyncStamp {
        let h = self.next().await;
        SyncStamp {
            rev_wall: h.wall_ms,
            rev_counter: h.counter,
            origin: self.node.to_string(),
        }
    }

    /// The highest stamp emitted/observed so far (for seeding a detached
    /// generator across a sync, so the clock lock isn't held over network I/O).
    pub async fn last(&self) -> Hlc {
        self.gen.lock().await.last()
    }

    /// Fold an observed (remote/merged) stamp in so future local stamps sort
    /// after it; persist the advanced seed.
    pub async fn observe(&self, remote: Hlc) {
        let mut g = self.gen.lock().await;
        g.observe(remote);
        let last = g.last();
        self.persist(last);
    }

    /// Best-effort write of `node` + `last` to the identity file.
    fn persist(&self, last: Hlc) {
        if let Some(dir) = self.file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let f = IdentityFile {
            node: self.node.to_string(),
            wall_ms: last.wall_ms,
            counter: last.counter,
        };
        if let Ok(text) = serde_json::to_string_pretty(&f) {
            let _ = std::fs::write(&self.file, text);
        }
    }
}
