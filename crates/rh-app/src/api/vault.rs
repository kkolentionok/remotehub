//! Portable vault — slice 1 (export + import). See `docs/specs/sync.md` §11.
//!
//! Export builds a [`SyncSnapshot`] from local storage (hosts, groups,
//! credentials with their keychain secrets) and seals it under the user's
//! master password into a portable, E2E-encrypted envelope. Import decrypts an
//! envelope, **merges** it with the local snapshot (record-level LWW), and
//! writes the result back to storage + keychain. The plaintext (including
//! secret bytes) only ever exists in memory while sealing/opening.
//!
//! Write-back ordering (FK-safe): groups (two-pass — create flat, then set
//! parents) → credentials → hosts (create with jump/default stripped, then a
//! reconcile pass relinks the default credential and sets `jump_host_id`) →
//! deletions in reverse (hosts, credentials, groups). v1 limitations: only the
//! **default** host↔credential link is restored (the sync model carries
//! `default_credential_id`, not the full `host_credentials` set); settings are
//! not yet replicated; node id/clock are per-call (stable identity arrives with
//! the sync engine, slice 2).
//!
//! The sync engine (server transport, incremental sync) is the next slice.

use rh_core::{
    Credential, CredentialId, GroupId, Host, HostFilter, HostGroup, HostId, Note, NoteId,
    SecretValue, Snippet, SnippetId, SyncStamp,
};
use rh_vault::{
    from_export_string, merge, open_envelope, seal_snapshot, to_export_string, EntityKind, Hlc,
    NodeId, RecordMeta, SyncCredentialPayload, SyncRecord, SyncSnapshot, VaultError,
};
use tauri::State;
use tracing::instrument;

use crate::api::dto::{
    ImportMode, VaultExportRequest, VaultFileResponse, VaultImportRequest, VaultImportResponse,
    VaultReadFileRequest, VaultWriteFileRequest,
};
use crate::api::error::{ApiError, ApiResult};
use crate::state::AppState;
use crate::sync_clock::{KIND_CREDENTIAL, KIND_GROUP, KIND_HOST, KIND_NOTE, KIND_SNIPPET};

/// On-disk form of the per-device sync identity now lives in
/// `crate::sync_clock::SyncClock` (shared, process-wide, in `AppState`).

/// `rh-vault` errors carry no secret material (crate policy), so surfacing
/// the message as an internal error is safe.
fn vault_err(e: VaultError) -> ApiError {
    ApiError::Internal {
        message: e.to_string(),
    }
}

/// The storage-neutral stamp carried by a merged record's metadata. Persisted
/// into `sync_meta` on apply so a rebuilt snapshot keeps the *merged*
/// provenance (not a fresh local stamp), which is what lets the merge stay
/// convergent across repeated syncs.
fn stamp_from(rec: &SyncRecord) -> SyncStamp {
    SyncStamp {
        rev_wall: rec.meta.rev.wall_ms,
        rev_counter: rec.meta.rev.counter,
        origin: rec.meta.origin.to_string(),
    }
}

/// Resolve a record's `(rev, origin)` for the snapshot: the stored `sync_meta`
/// stamp (set on the record's last actual edit), or — for rows that predate
/// slice 2b and were never stamped — a deterministic fallback derived from the
/// entity's own timestamp so it still sorts sensibly on the first sync.
async fn rev_for(
    state: &AppState,
    kind: &str,
    id: &str,
    fallback_ms: i64,
) -> ApiResult<(Hlc, NodeId)> {
    if let Some(s) = state.sync_meta.stamp_of(kind, id).await? {
        Ok((Hlc::new(s.rev_wall, s.rev_counter), NodeId::new(s.origin)))
    } else {
        let ms = u64::try_from(fallback_ms).unwrap_or(0);
        Ok((Hlc::new(ms, 0), state.sync.node().clone()))
    }
}

/// Assemble a full snapshot of the replicated state from storage + keychain.
///
/// Each record carries its **last-edit** stamp read from `sync_meta` (written
/// by every mutation via `AppState::stamp_*`), not a stamp minted now — that is
/// what makes the merge true last-write-wins. Deletions are emitted as
/// tombstones from `sync_meta` so they propagate. `node` and the snapshot's
/// `generated` clock come from the shared `SyncClock`.
pub(crate) async fn build_snapshot(state: &AppState) -> ApiResult<SyncSnapshot> {
    let node = state.sync.node().clone();
    let mut records = Vec::new();

    for host in state.hosts.list(HostFilter::default()).await? {
        let (rev, origin) = rev_for(state, KIND_HOST, host.id.as_str(), host.updated_at.timestamp_millis()).await?;
        records.push(SyncRecord::host(&host, rev, origin).map_err(vault_err)?);
    }

    for group in state.groups.list().await? {
        let (rev, origin) = rev_for(state, KIND_GROUP, group.id.as_str(), group.created_at.timestamp_millis()).await?;
        records.push(SyncRecord::group(&group, rev, origin).map_err(vault_err)?);
    }

    for cred in state.credentials.list().await? {
        // Agent credentials (`SshKeyAgent`) hold no keychain secret; only
        // reveal for kinds that have one.
        let (secret, passphrase) = if cred.kind.requires_keychain_secret() {
            let secret = state.credentials.reveal(&cred.id).await?;
            let passphrase = state.credentials.reveal_passphrase(&cred.id).await?;
            (
                Some(secret.expose().to_vec()),
                passphrase.map(|p| p.expose().to_vec()),
            )
        } else {
            (None, None)
        };
        let (rev, origin) = rev_for(state, KIND_CREDENTIAL, cred.id.as_str(), cred.updated_at.timestamp_millis()).await?;
        let payload = SyncCredentialPayload {
            credential: cred,
            secret,
            passphrase,
        };
        records.push(SyncRecord::credential(&payload, rev, origin).map_err(vault_err)?);
    }

    for snippet in state.snippets.list().await? {
        let (rev, origin) = rev_for(
            state,
            KIND_SNIPPET,
            snippet.id.as_str(),
            snippet.updated_at.timestamp_millis(),
        )
        .await?;
        records.push(SyncRecord::snippet(&snippet, rev, origin).map_err(vault_err)?);
    }

    for note in state.notes.list().await? {
        let (rev, origin) = rev_for(
            state,
            KIND_NOTE,
            note.id.as_str(),
            note.updated_at.timestamp_millis(),
        )
        .await?;
        records.push(SyncRecord::note(&note, rev, origin).map_err(vault_err)?);
    }

    // Tombstones — deletions that must keep propagating until every replica
    // has applied them.
    for (kind, id, stamp) in state.sync_meta.tombstones().await? {
        let entity = match kind.as_str() {
            KIND_HOST => EntityKind::Host,
            KIND_GROUP => EntityKind::Group,
            KIND_CREDENTIAL => EntityKind::Credential,
            KIND_SNIPPET => EntityKind::Snippet,
            KIND_NOTE => EntityKind::Note,
            _ => continue,
        };
        let rev = Hlc::new(stamp.rev_wall, stamp.rev_counter);
        records.push(SyncRecord::tombstone(entity, id, rev, NodeId::new(stamp.origin)));
    }

    // The snapshot's own clock = the next stamp from the shared generator.
    let generated = state.sync.next().await;
    Ok(SyncSnapshot::new(node, generated, records))
}

/// Export local state as a portable, E2E-encrypted vault string.
///
/// `req.master_password` is skipped from the span — it must never be logged.
#[tauri::command]
#[instrument(level = "debug", skip(state, req))]
pub async fn vault_export(
    state: State<'_, AppState>,
    req: VaultExportRequest,
) -> ApiResult<String> {
    let snapshot = build_snapshot(&state).await?;
    let envelope =
        seal_snapshot(&snapshot, req.master_password.as_bytes()).map_err(vault_err)?;
    to_export_string(&envelope).map_err(vault_err)
}

#[derive(Default)]
pub(crate) struct ImportCounts {
    pub(crate) hosts: u32,
    pub(crate) groups: u32,
    pub(crate) credentials: u32,
    pub(crate) snippets: u32,
    pub(crate) notes: u32,
    pub(crate) deleted: u32,
}

/// Whether local provenance for `(kind, id)` is already at least as new as
/// `rec`, in which case the record must NOT be applied.
///
/// Two cases collapse into one rule:
/// * **equal** — we applied this exact revision on an earlier pass; rewriting
///   it would re-touch SQLite and (for credentials) the OS keychain on every
///   single pass for no reason.
/// * **local is newer** — an edit or a delete landed *after* this snapshot was
///   built. That is not hypothetical: a pass reads the local snapshot, then
///   spends network + Argon2 time before applying the merge, and at the notes
///   cadence a delete lands inside that window regularly. Applying anyway
///   re-created the row *and* `bump()` cleared its tombstone, so the deletion
///   was lost permanently and the resurrected note was pushed back to every
///   device. Skipping leaves the tombstone intact; the next pass pushes it.
async fn local_is_current(
    state: &AppState,
    kind: &str,
    id: &str,
    rec: &SyncRecord,
) -> ApiResult<bool> {
    let Some(s) = state.sync_meta.stamp_of(kind, id).await? else {
        return Ok(false);
    };
    let local = RecordMeta::new(Hlc::new(s.rev_wall, s.rev_counter), NodeId::new(s.origin));
    Ok(!rec.meta.wins_over(&local))
}

/// Write a merged snapshot back into storage + keychain. See the module doc
/// for the FK-safe ordering and v1 limitations. Deletions and link reconcile
/// are best-effort (a single edge — a cycle, a missing FK target from a
/// concurrent conflict — must not abort the whole import).
pub(crate) async fn apply_snapshot(state: &AppState, snap: &SyncSnapshot) -> ApiResult<ImportCounts> {
    let mut c = ImportCounts::default();

    // Phase 1 — groups, two-pass (create flat, then set parents).
    let mut group_parents: Vec<(GroupId, Option<GroupId>)> = Vec::new();
    for rec in &snap.records {
        if rec.kind != EntityKind::Group || rec.is_deleted() {
            continue;
        }
        if local_is_current(state, KIND_GROUP, &rec.id, rec).await? {
            continue;
        }
        let g: HostGroup = rec.as_group().map_err(vault_err)?;
        match state.groups.get(&g.id).await {
            Ok(_) => state.groups.rename(&g.id, &g.name).await?,
            Err(_) => {
                let flat = HostGroup {
                    parent_id: None,
                    ..g.clone()
                };
                state.groups.create(&flat).await?;
            }
        }
        group_parents.push((g.id.clone(), g.parent_id.clone()));
        state.sync_meta.bump(KIND_GROUP, g.id.as_str(), &stamp_from(rec)).await?;
        c.groups += 1;
    }
    for (id, parent) in &group_parents {
        let _ = state.groups.move_to(id, parent.as_ref()).await;
    }

    // Phase 2 — credentials (+ secrets to keychain).
    for rec in &snap.records {
        if rec.kind != EntityKind::Credential || rec.is_deleted() {
            continue;
        }
        if local_is_current(state, KIND_CREDENTIAL, &rec.id, rec).await? {
            continue;
        }
        let payload = rec.as_credential().map_err(vault_err)?;
        let cred: Credential = payload.credential;
        let secret = SecretValue::new(payload.secret.unwrap_or_default());
        let passphrase = payload.passphrase.map(SecretValue::new);
        match state.credentials.get(&cred.id).await {
            Ok(_) => {
                // Write the secret FIRST: `rotate_secret` stamps `updated_at`
                // to now, so `update` must run after it to restore the record's
                // own `updated_at`. Otherwise the serialized credential drifts
                // on every apply, and the no-op-push fast path (slice 2c) never
                // fires — every sync would re-push and bump the server rev.
                if cred.kind.requires_keychain_secret() {
                    state
                        .credentials
                        .rotate_secret(&cred.id, secret, passphrase)
                        .await?;
                }
                state.credentials.update(&cred).await?;
            }
            Err(_) => state.credentials.create(&cred, secret, passphrase).await?,
        }
        state.sync_meta.bump(KIND_CREDENTIAL, cred.id.as_str(), &stamp_from(rec)).await?;
        c.credentials += 1;
    }

    // Phase 3 — hosts. Strip jump_host_id + default link on first write (the
    // referenced host/credential may be created in this same import), then
    // reconcile once everything exists.
    let mut host_links: Vec<(HostId, Option<CredentialId>, Option<HostId>)> = Vec::new();
    for rec in &snap.records {
        if rec.kind != EntityKind::Host || rec.is_deleted() {
            continue;
        }
        if local_is_current(state, KIND_HOST, &rec.id, rec).await? {
            continue;
        }
        let h: Host = rec.as_host().map_err(vault_err)?;
        let base = Host {
            default_credential_id: None,
            jump_host_id: None,
            ..h.clone()
        };
        match state.hosts.get(&h.id).await {
            Ok(_) => state.hosts.update(&base).await?,
            Err(_) => state.hosts.create(&base).await?,
        }
        host_links.push((h.id.clone(), h.default_credential_id.clone(), h.jump_host_id.clone()));
        state.sync_meta.bump(KIND_HOST, h.id.as_str(), &stamp_from(rec)).await?;
        c.hosts += 1;
    }
    for (id, default_cred, jump) in &host_links {
        if let Some(cid) = default_cred {
            // Recreate the default host↔credential link (also sets the default).
            let _ = state.credentials.link_host(id, cid, true).await;
        }
        if jump.is_some() {
            if let Ok(cur) = state.hosts.get(id).await {
                let updated = Host {
                    jump_host_id: jump.clone(),
                    ..cur
                };
                let _ = state.hosts.update(&updated).await;
            }
        }
    }

    // Phase 3b — snippets (no FK deps; simple upsert). No count surfaced.
    let existing_snips: std::collections::HashSet<String> = state
        .snippets
        .list()
        .await?
        .into_iter()
        .map(|s| s.id.to_string())
        .collect();
    for rec in &snap.records {
        if rec.kind != EntityKind::Snippet || rec.is_deleted() {
            continue;
        }
        if local_is_current(state, KIND_SNIPPET, &rec.id, rec).await? {
            continue;
        }
        let s: Snippet = rec.as_snippet().map_err(vault_err)?;
        if existing_snips.contains(&s.id.to_string()) {
            state.snippets.update(&s).await?;
        } else {
            state.snippets.create(&s).await?;
        }
        state
            .sync_meta
            .bump(KIND_SNIPPET, s.id.as_str(), &stamp_from(rec))
            .await?;
        c.snippets += 1;
    }

    // Phase 3c — notes (no FK deps; simple upsert).
    let existing_notes: std::collections::HashSet<String> = state
        .notes
        .list()
        .await?
        .into_iter()
        .map(|n| n.id.to_string())
        .collect();
    for rec in &snap.records {
        if rec.kind != EntityKind::Note || rec.is_deleted() {
            continue;
        }
        if local_is_current(state, KIND_NOTE, &rec.id, rec).await? {
            continue;
        }
        let n: Note = rec.as_note().map_err(vault_err)?;
        if existing_notes.contains(&n.id.to_string()) {
            state.notes.update(&n).await?;
        } else {
            state.notes.create(&n).await?;
        }
        state
            .sync_meta
            .bump(KIND_NOTE, n.id.as_str(), &stamp_from(rec))
            .await?;
        c.notes += 1;
    }

    // Phase 4 — deletions, reverse FK order (hosts → credentials → groups).
    // Each deletion is recorded as a `sync_meta` tombstone (always, even if the
    // entity row was already gone) so it keeps propagating to other replicas;
    // the entity-row delete itself is best-effort.
    for rec in &snap.records {
        if rec.kind == EntityKind::Host && rec.is_deleted() {
            if local_is_current(state, KIND_HOST, &rec.id, rec).await? {
                continue;
            }
            let id = HostId::from_raw(rec.id.clone());
            let _ = state.hosts.delete(&id).await;
            state.sync_meta.tombstone(KIND_HOST, &rec.id, &stamp_from(rec)).await?;
            c.deleted += 1;
        }
    }
    for rec in &snap.records {
        if rec.kind == EntityKind::Credential && rec.is_deleted() {
            if local_is_current(state, KIND_CREDENTIAL, &rec.id, rec).await? {
                continue;
            }
            let id = CredentialId::from_raw(rec.id.clone());
            let _ = state.credentials.delete(&id).await;
            state.sync_meta.tombstone(KIND_CREDENTIAL, &rec.id, &stamp_from(rec)).await?;
            c.deleted += 1;
        }
    }
    for rec in &snap.records {
        if rec.kind == EntityKind::Snippet && rec.is_deleted() {
            if local_is_current(state, KIND_SNIPPET, &rec.id, rec).await? {
                continue;
            }
            let id = SnippetId::from_raw(rec.id.clone());
            let _ = state.snippets.delete(&id).await;
            state
                .sync_meta
                .tombstone(KIND_SNIPPET, &rec.id, &stamp_from(rec))
                .await?;
            c.deleted += 1;
        }
    }
    for rec in &snap.records {
        if rec.kind == EntityKind::Note && rec.is_deleted() {
            if local_is_current(state, KIND_NOTE, &rec.id, rec).await? {
                continue;
            }
            let id = NoteId::from_raw(rec.id.clone());
            let _ = state.notes.delete(&id).await;
            state
                .sync_meta
                .tombstone(KIND_NOTE, &rec.id, &stamp_from(rec))
                .await?;
            c.deleted += 1;
        }
    }
    for rec in &snap.records {
        if rec.kind == EntityKind::Group && rec.is_deleted() {
            if local_is_current(state, KIND_GROUP, &rec.id, rec).await? {
                continue;
            }
            let id = GroupId::from_raw(rec.id.clone());
            let _ = state.groups.delete(&id).await;
            state.sync_meta.tombstone(KIND_GROUP, &rec.id, &stamp_from(rec)).await?;
            c.deleted += 1;
        }
    }

    // Keep the local clock ahead of everything we just applied so future local
    // stamps sort after the merged content.
    let observed = snap
        .records
        .iter()
        .map(|r| r.meta.rev)
        .fold(snap.generated, Hlc::max);
    state.sync.observe(observed).await;

    Ok(c)
}

/// Import a portable vault: decrypt, reconcile with local state, write back.
///
/// `merge` (default) merges the file into the local store (record-level LWW).
/// `replace` wipes the local store and takes the file verbatim — the envelope
/// is decrypted *before* any deletion, so a wrong password fails with nothing
/// destroyed.
///
/// `req.master_password` is skipped from the span — never logged.
#[tauri::command]
#[instrument(level = "debug", skip(state, req))]
pub async fn vault_import(
    state: State<'_, AppState>,
    req: VaultImportRequest,
) -> ApiResult<VaultImportResponse> {
    let envelope = from_export_string(&req.body).map_err(vault_err)?;
    // Wrong password and a corrupt blob are indistinguishable here (by design):
    // both surface as a single decrypt error — and this runs before any wipe.
    let remote = open_envelope(&envelope, req.master_password.as_bytes()).map_err(vault_err)?;

    let to_apply = match req.mode {
        ImportMode::Replace => {
            wipe_local(&state).await?;
            remote
        }
        ImportMode::Merge => {
            let local = build_snapshot(&state).await?;
            merge(&local, &remote, local.node.clone())
        }
    };
    let c = apply_snapshot(&state, &to_apply).await?;
    Ok(VaultImportResponse {
        hosts: c.hosts,
        groups: c.groups,
        credentials: c.credentials,
        deleted: c.deleted,
    })
}

/// Delete every local host, credential (incl. keychain secret) and group, in
/// FK-safe order (hosts reference groups + credentials). Best-effort per row so
/// one stuck delete can't abort the wipe. Also drops all `sync_meta` tombstones.
///
/// Used by import `Replace` mode and by **logout** (`api::sync::sync_logout`),
/// which account-scopes the local vault so the next account can't inherit this
/// one's data or its deletion tombstones.
pub(crate) async fn wipe_local(state: &AppState) -> ApiResult<()> {
    for h in state.hosts.list(HostFilter::default()).await? {
        let _ = state.hosts.delete(&h.id).await;
    }
    for c in state.credentials.list().await? {
        let _ = state.credentials.delete(&c.id).await;
    }
    for g in state.groups.list().await? {
        let _ = state.groups.delete(&g.id).await;
    }
    for s in state.snippets.list().await? {
        let _ = state.snippets.delete(&s.id).await;
    }
    for n in state.notes.list().await? {
        let _ = state.notes.delete(&n.id).await;
    }
    // Drop all provenance too, so stale stamps for the wiped entities can't
    // resurface as phantom live records in the next snapshot.
    state.sync_meta.clear_all().await?;
    Ok(())
}

/// Write a vault export to a path chosen by the user via the native Save
/// dialog. The webview cannot write arbitrary files itself; this scoped
/// command does it (no general filesystem plugin needed). `body` is the
/// already-sealed export, so nothing secret is logged (`skip(req)`).
#[tauri::command]
#[instrument(level = "debug", skip(req))]
pub async fn vault_write_file(req: VaultWriteFileRequest) -> ApiResult<()> {
    std::fs::write(&req.path, req.body.as_bytes())
        .map_err(|e| ApiError::Internal { message: format!("write vault file: {e}") })?;
    Ok(())
}

/// Read a vault file the user picked via the native Open dialog. Returns the
/// text body (handed straight back to `vault_import`) plus its basename and
/// byte size for the file card. The body is encrypted, so logging is skipped.
#[tauri::command]
#[instrument(level = "debug")]
pub async fn vault_read_file(req: VaultReadFileRequest) -> ApiResult<VaultFileResponse> {
    let body = std::fs::read_to_string(&req.path)
        .map_err(|e| ApiError::Internal { message: format!("read vault file: {e}") })?;
    let size = body.as_bytes().len() as u64;
    let name = std::path::Path::new(&req.path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| req.path.clone());
    Ok(VaultFileResponse { body, name, size })
}
