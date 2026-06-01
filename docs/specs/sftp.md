# SFTP Explorer — Implementation Reference

Status: **complete and live-verified** (local↔host, host↔host, all ops). A
two-pane commander modelled on Termius/Total Commander. This documents how the
whole path fits together so it can be picked back up later.

Engine: **russh 0.45** (SSH transport) + **russh-sftp 2.x** (`SftpSession` over
an SSH `subsystem("sftp")` channel). The local side uses plain `std::fs`.

---

## 1. Component map

```
crates/rh-ssh/
  src/sftp.rs            SftpConn: connect/list/read_file/put_in_dir,
                         download/upload (buffered) + download_stream/
                         upload_stream (chunked, cancel + progress),
                         rename, remove (recursive), mkdir, fmt_perms,
                         join_posix/parent_posix.
  examples/sftp_spike.rs the original connect→list→download spike (reference).

crates/rh-app/
  src/sftp_session.rs    SftpManager: live SftpConn registry +
                         per-transfer cancel-flag registry.
  src/api/sftp_sessions.rs   tauri commands (open/list/close/transfer/
                         cancel/rename/remove/mkdir + legacy copy/dl/ul).
  src/api/local_fs.rs    local filesystem: home/drives/list/rename/remove/
                         mkdir (+ clean() for the \\?\ prefix).
  src/api/dto.rs         request DTOs + SftpTransferKind.

ui/src/
  lib/ipc.ts             localFs.*, sftp.* (transfer takes a Channel<number>).
  lib/types.ts           FsEntry/FsListResponse mirrors.
  components/sftp/SftpView.tsx   the commander (panels, rail, queue, dialogs).
  components/sftp/SftpView.module.css
```

## 2. Connection

`sftp_open(host_id)` → `revealed_creds_for` (reuses the SSH credential-reveal:
key → agent → password order, passwordless fallback) → `SftpConn::connect`:
russh connect, `try_auth` per revealed credential (password + key incl. `.ppk`),
`channel_open_session()` → `request_subsystem(true,"sftp")` →
`SftpSession::new(channel.into_stream())`. Host key currently **TrustAll**
(TOFU pinning is a follow-up — mirror the SSH `known_hosts` store).

A session is kept in `SftpManager` keyed by `SessionId` behind
`Arc<Mutex<SftpConn>>` (the Mutex avoids requiring `SftpConn: Sync` and
serialises access to the one SFTP channel).

## 3. Listing & metadata

`list(path)`: empty/"."/canonicalize → home; `read_dir`; each entry →
`SftpEntry{ name, path: join_posix(dir,name), is_dir, size, modified: mtime,
perms: fmt_perms(permissions) }`; dirs-first. Local `fs_list` mirrors this with
`std::fs`, `perms: None`, and `clean()` strips the Windows `\\?\` verbatim
prefix that `canonicalize` adds (otherwise breadcrumbs show `/ > ? > C:`).
`fs_drives` returns the drive roots for the "This PC" view (Windows: scan A–Z).

Dates/sizes are formatted on the **frontend** (locale-aware; RU `1,15 ГБ`,
today→time, this-year→"D mon", else +year). Perms shown host-only.

## 4. Transfers

Three kinds, all funnelled through one command:

```
sftp_transfer(req: { transfer_id, kind, session_id, to_session?, src_path,
                     dst_dir, dst_name? }, on_progress: Channel<u64>)
```

- **download** — remote `session_id` file → local `dst_dir`.
- **upload** — local file → remote `session_id` `dst_dir`.
- **copy** — remote `session_id` → remote `to_session` (host↔host). SFTP has no
  server-to-server copy, so it streams through the app; currently **buffered**
  (read_file → put_in_dir, progress 0→full). Streaming copy is a follow-up.

Streaming (download/upload) reads/writes in **256 KiB** chunks, emits the
running byte total on the `Channel`, and checks an `AtomicBool` cancel flag each
chunk. `dst_name` overrides the destination filename (used by the conflict
dialog's "keep both"). Cancel: `sftp_transfer_cancel(transfer_id)` flips the
flag in `SftpManager.cancels`; the in-flight `sftp_transfer` then returns an
error and the UI (which knows it requested the cancel) marks it cancelled.

**The conn Mutex is held for the duration of a streamed transfer**, so two
transfers on the *same* session serialise; different sessions run in parallel.
Acceptable for now (one SFTP channel is serial anyway).

## 5. Queue orchestration (frontend)

`useTransfers()` keeps the transfer list in a ref + force-render. `pump()`
starts queued items while fewer than **2** are active; each item awaits
`sftp.transfer` with a progress callback that updates `transferred` + derived
`speed`; on settle it re-pumps and refreshes the target panel. Speed/ETA are
computed from bytes/elapsed and ticked once a second while anything is active.
The dock shows active/queued counts, total speed, per-row bar/speed/ETA/cancel,
and "clear finished".

## 6. The four transfer gestures

All resolve to the same `transferFiles(files, fromPanel, toPanel)` →
conflict-check → enqueue:
1. **Center rail** →/← — armed when the active pane has a file selection.
2. **Double-click** a file → send to the opposite panel.
3. **Drag & drop** a row/selection onto the other panel (drop highlight + plate).
4. **Context menu** → "send to other panel".

Active panel (accent border) is set on mousedown and decides rail direction.

## 7. Conflict resolution

Before enqueue, names are checked against the target listing. On collision a
dialog offers, for the whole batch: **Replace** (overwrite — `create` truncates),
**Keep both** (`uniqueName` → `config (1).yml`, passed as `dst_name`), **Skip**
(only non-conflicting). Non-conflicting files always transfer.

## 8. File operations

- **Rename** — inline input in the row (F2 / pencil button); `fs_rename` /
  `sftp_rename` (same-dir).
- **Delete** — confirm dialog; `fs_remove` (`remove_dir_all`) / `sftp_remove`
  (recursive: walk → `remove_file` children → `rmdir`).
- **New folder** — toolbar button → inline draft row; `fs_mkdir` / `sftp_mkdir`.
- **Search/filter** — header toggle; filters the current listing by name
  (frontend only). Reset on navigation.
- **Copy path** — context menu → clipboard.

## 9. Risk flags (unproven russh-sftp surface, compiled clean on Windows)

`create`, `write_all`/`shutdown`, `rename`, `remove_file`/`remove_dir`,
`create_dir`, and `FileAttributes::{mtime,permissions}`. Proven by the spike:
`read_dir`, `open`, `read_to_end`, `metadata`, `file_name`, `canonicalize`.

## 10. Future work / backlog

- Streaming host→host copy (real progress instead of buffered 0→100).
- SFTP TOFU cert pinning (reuse the SSH known_hosts pattern).
- Agent-auth for SFTP sessions.
- chmod / permissions-edit context action (host).
- Transfer-queue speed smoothing; resume/retry.
- Local↔local copy (currently shows a "not yet" notice).
