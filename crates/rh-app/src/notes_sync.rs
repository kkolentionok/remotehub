//! Notes replication — separate from the vault, and deliberately cheap.
//!
//! A vault pass derives a key with Argon2id, reveals every credential from the
//! OS keychain, and rewrites every record. That is a fine price once every
//! half minute for the whole account; it is an absurd one for a scratchpad the
//! user expects to feel instant. Notes therefore ride their own blob
//! (`/v1/notes`), sealed under a random key, merged with the same HLC rules —
//! but a pass here is a small GET plus one AES-GCM open.
//!
//! **Key distribution.** The notes key is random, kept in the OS keychain, and
//! carried inside the (already sealed) vault snapshot so a second signed-in
//! device picks it up automatically — `api::vault` reads and writes
//! `SyncSnapshot::notes_key_b64`. A device paired by code instead receives the
//! key wrapped under that code, which is what lets it read notes and nothing
//! else.

use std::sync::Arc;

use base64::Engine as _;
use rh_core::{Note, NoteId};
use rh_vault::{
    gen_notes_key, merge, open_notes, seal_notes, NodeId, SyncRecord, SyncRemote, SyncSnapshot,
    VaultKey,
};
use tracing::{debug, warn};

use crate::api::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::sync_clock::KIND_NOTE;
use crate::sync_remote::{self, ServerRemote, SyncConfig};

fn b64() -> base64::engine::general_purpose::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

/// The notes key for this device, or `None` if we have neither stored one nor
/// been given one.
pub fn key_from_keychain() -> Option<VaultKey> {
    let raw = b64().decode(sync_remote::notes_key_get()?).ok()?;
    let arr: [u8; 32] = raw.try_into().ok()?;
    Some(VaultKey::from_bytes(arr))
}

/// Persist a notes key (base64) to the keychain.
pub fn store_key(key: &VaultKey) -> Result<String, ApiError> {
    let encoded = b64().encode(key.as_bytes());
    sync_remote::notes_key_set(&encoded).map_err(|e| ApiError::Internal {
        message: format!("keychain: {e}"),
    })?;
    Ok(encoded)
}

/// Mint a notes key and store it. Called from `build_snapshot` only, so every
/// key reaches other devices through the vault merge that resolves collisions.
pub fn mint_key() -> ApiResult<String> {
    let key = gen_notes_key().map_err(|e| ApiError::Internal {
        message: format!("notes key: {e}"),
    })?;
    store_key(&key)
}

/// Which credential this device syncs notes with: a paired device has only a
/// notes-scoped token; a signed-in one uses its account token.
fn token_for_notes() -> Option<(String, Option<String>)> {
    if let Some(t) = sync_remote::notes_token_get() {
        // Paired device: no refresh token — the notes token is long-lived and
        // revocable server-side.
        return Some((t, None));
    }
    sync_remote::token_get().map(|t| (t, sync_remote::refresh_get()))
}

/// Build the local notes snapshot: every note as a live record, plus the
/// tombstones so deletions keep propagating.
async fn build(state: &AppState) -> ApiResult<SyncSnapshot> {
    let node = state.sync.node().clone();
    let generated = state.sync.next().await;
    let mut records: Vec<SyncRecord> = Vec::new();

    for note in state.notes.list().await? {
        let (rev, origin) = match state.sync_meta.stamp_of(KIND_NOTE, note.id.as_str()).await? {
            Some(s) => (
                rh_vault::Hlc::new(s.rev_wall, s.rev_counter),
                NodeId::new(s.origin),
            ),
            None => (
                rh_vault::Hlc::new(
                    u64::try_from(note.updated_at.timestamp_millis()).unwrap_or(0),
                    0,
                ),
                node.clone(),
            ),
        };
        records.push(
            SyncRecord::note(&note, rev, origin).map_err(|e| ApiError::Internal {
                message: format!("notes snapshot: {e}"),
            })?,
        );
    }

    for (kind, id, stamp) in state.sync_meta.tombstones().await? {
        if kind != KIND_NOTE {
            continue;
        }
        records.push(SyncRecord::tombstone(
            rh_vault::EntityKind::Note,
            id,
            rh_vault::Hlc::new(stamp.rev_wall, stamp.rev_counter),
            NodeId::new(stamp.origin),
        ));
    }

    let mut snap = SyncSnapshot::new(node, generated, records);
    snap.notes_key_b64 = sync_remote::notes_key_get();
    Ok(snap)
}

/// Apply a merged notes snapshot locally, honouring local provenance exactly
/// as the vault path does (a delete that landed mid-pass must not be undone).
async fn apply(state: &AppState, snap: &SyncSnapshot) -> ApiResult<u32> {
    let existing: std::collections::HashSet<String> = state
        .notes
        .list()
        .await?
        .into_iter()
        .map(|n| n.id.to_string())
        .collect();
    let mut changed = 0u32;

    for rec in &snap.records {
        if rec.kind != rh_vault::EntityKind::Note {
            continue;
        }
        if crate::api::vault::local_is_current(state, KIND_NOTE, &rec.id, rec).await? {
            continue;
        }
        let stamp = crate::api::vault::stamp_from(rec);

        if rec.is_deleted() {
            let _ = state.notes.delete(&NoteId::from_raw(rec.id.clone())).await;
            state.sync_meta.tombstone(KIND_NOTE, &rec.id, &stamp).await?;
            changed += 1;
            continue;
        }

        let n: Note = rec.as_note().map_err(|e| ApiError::Internal {
            message: format!("notes decode: {e}"),
        })?;
        if existing.contains(&n.id.to_string()) {
            state.notes.update(&n).await?;
        } else {
            state.notes.create(&n).await?;
        }
        state.sync_meta.bump(KIND_NOTE, n.id.as_str(), &stamp).await?;
        changed += 1;
    }

    Ok(changed)
}

/// One notes pass: pull, merge, push if changed, apply. Returns how many
/// records changed locally.
pub async fn run_pass(state: &AppState) -> ApiResult<u32> {
    let cfg = SyncConfig::load();
    if cfg.endpoint.is_empty() {
        return Ok(0);
    }
    let Some((token, refresh)) = token_for_notes() else {
        return Ok(0);
    };
    // No key yet means the vault pass that mints and publishes one hasn't run
    // here. Do nothing rather than seal a blob under a key nobody else has.
    let Some(key) = key_from_keychain() else {
        debug!("notes pass skipped: no notes key yet");
        return Ok(0);
    };
    debug!("notes pass: key {}", b64().encode(&key.as_bytes()[..4]));
    let remote: Arc<dyn SyncRemote> = Arc::new(ServerRemote::new_notes(
        cfg.endpoint.clone(),
        token,
        refresh,
    ));

    let local = build(state).await?;

    let pulled = remote.pull().await.map_err(|e| ApiError::Internal {
        message: format!("notes pull: {e}"),
    })?;

    // Open the remote blob if we can. If we can't, we are holding a different
    // key than whoever sealed it — recover by republishing our own notes under
    // the current key instead of failing forever. Refusing to write here is
    // what turned a first-run key race into a permanent dead end: the device
    // could neither read nor push, so nothing ever reconciled.
    let remote_snap = match &pulled {
        Some(blob) => {
            let text = String::from_utf8_lossy(&blob.bytes).to_string();
            match open_notes(&text, &key) {
                Ok(snap) => Some(snap),
                Err(e) => {
                    warn!("notes blob unreadable with the current key ({e}) — republishing");
                    None
                }
            }
        }
        None => None,
    };

    let expected = pulled.as_ref().map(|b| b.version.clone());
    let merged = match &remote_snap {
        Some(remote_snap) => {
            let merged = merge(&local, remote_snap, state.sync.node().clone());
            state.sync.observe(merged.generated).await;
            merged
        }
        None => local.clone(),
    };

    // Push only when the merge actually differs from what the server holds —
    // an idle device must not churn the blob's rev, or every other device
    // would see a change and pull for nothing.
    let remote_matches = remote_snap
        .as_ref()
        .map(|r| records_equal(r, &merged))
        .unwrap_or(false);

    if !remote_matches {
        let sealed = seal_notes(&merged, &key).map_err(|e| ApiError::Internal {
            message: format!("notes seal: {e}"),
        })?;
        match remote.push(sealed.as_bytes(), expected.as_deref()).await
        {
            Ok(_) => {}
            Err(e) => {
                // A conflict just means another device pushed first; the next
                // pass re-pulls and re-merges. Anything else is worth a line.
                debug!("notes push deferred: {e}");
                return Ok(0);
            }
        }
    }

    apply(state, &merged).await
}

fn records_equal(a: &SyncSnapshot, b: &SyncSnapshot) -> bool {
    if a.records.len() != b.records.len() {
        return false;
    }
    let mut x: Vec<String> = a
        .records
        .iter()
        .filter_map(|r| serde_json::to_string(r).ok())
        .collect();
    let mut y: Vec<String> = b
        .records
        .iter()
        .filter_map(|r| serde_json::to_string(r).ok())
        .collect();
    x.sort_unstable();
    y.sort_unstable();
    x == y
}

/// Best-effort pass used by the background loop; failures are logged, never
/// surfaced, because a scratchpad must not nag about a flaky network.
pub async fn run_pass_quiet(state: &AppState) {
    if let Err(e) = run_pass(state).await {
        warn!("notes sync: {e}");
    }
}
