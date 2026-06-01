# RemoteHub — Project State & Handoff

**Last updated:** **SFTP file explorer (full two-pane commander)** shipped end-to-end, plus the **local terminal (PTY)** and **Tools credential manager** that preceded it. All three live-verified on Windows against the real test host (`89.23.99.57`, password auth). Sections for each below; older RDP/SSH history follows. Detailed SFTP pipeline doc: `docs/specs/sftp.md`.

**Follow-up 2 (region-diff — the real fix):** instrumentation revealed the smoking gun — full-frame JPEG encode was **~130ms** each (the RGBA→RGB copy ran in *unoptimized* rh-rdp; the dev profile only optimized dependencies, not our own crate), capping fps at ~7 and blocking the worker; and full ~130KB base64 frames congested the single webview IPC bridge, so input invokes (clicks) queued behind them and arrived 3-15s late. Fixes: (1) **region-diff** — compute the changed bounding box vs the last frame and JPEG only that rectangle (`FrameJpeg` now carries x,y); a click/keystroke touches a tiny area → tiny encode + tiny payload → no IPC congestion. Frame coalescing was *removed* (each region is a distinct rect; dropping one leaves a stale patch). (2) `[profile.dev.package.rh-rdp] opt-level = 3` so the hot pixel loops are optimized in dev too. Added `rdp frame stats` (fps / avg encode ms / payload KB) + per-click logging for diagnosis.

## Latest — tab-bar scroll, storage scope switcher, SFTP byte-resume

- **Tab-bar horizontal scroll** — session tabs live in a `.scroller` (flex, `overflow-x:auto`, hidden scrollbar); wheel scrolls horizontally; the active tab auto-scrolls into view. Vault/Tools + `+`/gear/window-controls stay pinned.
- **Storage scope switcher** — the Vault chevron opens a Personal/Team dropdown (`storage:scope`). **Personal** is active; **Team** is disabled ("needs sync") — the UI seam for the future sync feature. No backend yet.
- **SFTP byte-offset resume** — transfers now resume from the destination's current size instead of restarting. `download/upload/copy_stream` gained an `offset` param; `SftpConn::size()` stats the remote partial. `SftpTransferRequest.resume` (default false); the dock's ↻ retry sets `resume:true`. Offset=0 path is unchanged (normal transfers unaffected).
  - **Risk (unproven russh-sftp, user compiles):** resume branch uses `File` `AsyncSeek` (remote read seek) + `open_with_flags(WRITE|CREATE|APPEND)` / `russh_sftp::protocol::OpenFlags`. If these names differ, only resume is affected.
- **SSH agent-forward serving — deferred to a spike.** Needs the client-`Handler` forwarded-agent-channel hook + an OS-agent byte bridge (security-sensitive, unproven API). Spike first (like sftp/rdp), then implement.



- **App icons** — real `rhub` icon set generated into `crates/rh-app/icons/` (32 / 128 / 128@2x / `icon.ico` multi-res / `icon.icns` / `icon.png`) from the designer's 1024² PNG. Window + tray now show the brand icon.
- **Favorites** — new `Host.favorite: bool` (rh-core). Migration **v9** (`ALTER TABLE hosts ADD COLUMN favorite … DEFAULT 0`; `CURRENT_SCHEMA_VERSION = 9`; v1.sql fresh schema updated). Runtime SQL in `host_store` (INSERT/UPDATE/SELECT + row map — no sqlx-macro/offline-cache impact). DTOs: `favorite` on full/create/update; handlers set/patch it. Frontend: `favorite` on `HostDto` + create/update; a **star toggle** in the HostDetail header (live-saves immediately, like agent-forwarding). Tray gained a **Favorites** submenu (pinned hosts, by name), rebuilt on `hosts:changed`.



Added a system-tray icon (`rh-app/src/tray.rs`, `tray-icon` feature on the tauri dep). Right-click menu: **Open RemoteHub**, **Recent** (hosts by `last_connected_at`, newest 8), **Groups** (nested submenu per group), separator, **Quit**. Left-click shows/focuses the window. Selecting a host emits `tray:connect <host_id>`; `AppShell` listens and opens it via the normal `sessions.open` flow (connect logic stays in one place). Menu rebuilds on `hosts:changed` / `groups:changed`.

- **Favorites** submenu is NOT wired — `Host` has no favorite flag yet (only `tags`, `group_id`, `last_connected_at`). Needs a `favorite` bool (migration + star toggle in the editor) — small follow-up.
- **App icons** still the Tauri placeholders; tray reuses `default_window_icon()`. Drop a 1024² `rhub.png` and regenerate the `icons/` set (32 / 128 / 128@2x / icon.ico / icon.icns).
- **Risk (Tauri 2.1 menu/tray API, user compiles):** `show_menu_on_left_click` (was `menu_on_left_click` pre-2.1), `SubmenuBuilder`/`MenuItemBuilder` shapes, `tray_by_id`/`set_menu`. Spike-grade surface — report compile errors.



- **Resume/retry interrupted transfers** — failed/cancelled queue rows get a ↻ retry (fresh `transfer_id`, re-enqueued); dock header gains "Retry failed". Cancel now keys on the item's current `transfer_id` (survives retry). (True byte-offset resume is still a follow-up — retry restarts from 0.)
- **Editable path field** — clicking the breadcrumb's empty area turns it into a path input (`navigateTo`): Enter validates by listing — success navigates, failure shows a red border and keeps the current listing (no clobber). Crumb buttons still navigate per-segment.
- **Local-terminal restore-on-reload** — `LocalPtyManager` now mirrors the SSH hub: per-session 256 KiB output ring + swappable sink + `list()`/`reattach()`. New commands `local_session_list` / `local_session_reattach`; `restoreSessions` rebuilds local tabs and replays scrollback after a webview reload (was SSH-only). Local shells no longer vanish on reload.



Closed the SFTP backlog from the roadmap's item 2:
- **Streaming host↔host copy** — `rh_ssh::sftp::copy_stream(src_conn, dst_conn, …)` chunks A→B with real byte-progress + cancel (was buffered 0→full). run_transfer locks by session-id order (A==B → one lock; A≠B → ordered, no deadlock).
- **TOFU host-key pinning** — `SftpConn::connect` now takes `Arc<dyn KnownHostsStore>`; `SftpHostKey` handler does silent trust-on-first-use against the shared `known_hosts` store (matches the SSH path), **rejects a changed key**. Replaces trust-all. `fingerprint_sha256` is now `pub(crate)` in `actor.rs` and reused.
- **SSH-agent auth for SFTP** — `try_auth` handles `RevealedCredential::Agent` (Pageant / OpenSSH pipe), mirroring the shell actor's agent block.
- **chmod** — `SftpConn::chmod(path, mode)` (`set_metadata` w/ `FileAttributes.permissions`), `sftp_chmod` command, context-menu "Permissions…" (host only) → a 3×3 rwx grid dialog with live octal.

**New risk flags (unproven russh-sftp surface):** `set_metadata` + `russh_sftp::protocol::FileAttributes` path (chmod). Agent path mirrors the proven actor code; copy_stream reuses already-compiled open/create/read/write/shutdown.

## Latest — Navy default + Redpanda theme; search-field click target

Theme picker gained **Navy** (deep-blue surfaces) — now the **default** (`Theme::Navy` `#[default]`) — and **Redpanda** (near-black warm surfaces + coral-red accent `#f0552f`, the only theme that shifts the accent). Both are `:root[data-theme=…]` token blocks; applied via `AppShell` `data-theme`. The Storage search hero is now a `<label>` so the whole 54px frame focuses the input (no more narrow hit area).



A complete Termius/commander-style SFTP browser, built incrementally and live-verified. Spec/pipeline reference: **`docs/specs/sftp.md`**.

**Backend**
- `rh-ssh/src/sftp.rs` — `SftpConn`: connect (TrustAll host-key — TOFU is a follow-up) via russh `request_subsystem("sftp")` + `russh_sftp::client::SftpSession`; `list` (`SftpEntry{name,path,is_dir,size,modified:Option<i64> from mtime, perms:Option<String> via fmt_perms()}`, dirs-first), `read_file`/`put_in_dir`, `download`/`upload` (buffered), **`download_stream`/`upload_stream`** (256 KiB chunks, cancel flag `&AtomicBool`, `progress: &mut (dyn FnMut(u64)+Send)`), `rename`, `remove` (recursive, boxed async), `mkdir`. `read_dir`/`open`/`read_to_end`/`metadata` are spike-proven; `create`/`write_all`/`shutdown`/`rename`/`remove_file`/`remove_dir`/`create_dir`/`mtime`/`permissions` were unproven russh-sftp surface (compiled clean on Windows).
- `rh-app/src/sftp_session.rs` — `SftpManager`: `HashMap<SessionId, Arc<Mutex<SftpConn>>>` + a per-transfer cancel registry (`cancels: HashMap<String, Arc<AtomicBool>>`, `register/unregister/cancel_transfer`). `rh-app/src/api/sftp_sessions.rs` — commands `sftp_open` (host_id → `revealed_creds_for` → connect), `sftp_list`, `sftp_close`, `sftp_download`/`sftp_upload`/`sftp_copy` (legacy buffered, still registered), **`sftp_transfer`** (`Channel<u64>` byte-progress + `dst_name` override + cancel), `sftp_transfer_cancel`, `sftp_rename`, `sftp_remove`, `sftp_mkdir`.
- `rh-app/src/api/local_fs.rs` — the local side of the explorer: `fs_home`, `fs_drives` ("This PC" — enumerates Windows drive roots), `fs_list` (with `clean()` stripping the `\\?\` verbatim prefix so breadcrumbs are clean), `fs_rename`, `fs_remove` (`remove_dir_all` for dirs), `fs_mkdir`. `FsEntry`/`SftpEntry` share `{name,path,is_dir,size,modified,perms}`.
- tokio gained `io-util` in `rh-ssh` for the streamed copies.

**Frontend** (`ui/src/components/sftp/SftpView.tsx` + `.module.css`, ~1.2k lines)
- Two interchangeable panels ("точка А | точка Б"), each a `usePanel()` hook: source (local | host), session, listing, sort, multi-select, hidden-files toggle (hosts show dotfiles by default, local hides), filter, inline-rename, create-folder. Endpoint switcher with "This machine"/"Hosts" sections; clean breadcrumbs with a PC-icon root → drives.
- **Transfer matrix:** local↔host (download/upload), host↔host (copy through the app). Four ways to move: center-rail →/← (armed on the active pane's selection), double-click a file, drag-and-drop between panels (drop highlight + plate), context menu (ПКМ: send/open/rename F2/copy-path/delete Del).
- **Transfer queue dock** (bottom, collapsible): max 2 parallel (`useTransfers` orchestrator), per-row progress bar / speed / ETA / cancel, total speed, "clear finished"; streamed via `sftp.transfer` + `Channel<u64>`.
- **Name-conflict dialog** on collision: Replace / Keep both (auto `name (1).ext`, via `dst_name`) / Skip.
- File ops: search/filter, rename (inline), delete (confirm dialog, recursive), new folder.
- All `invoke` through `lib/ipc.ts` (`localFs.*`, `sftp.*`); every string i18n'd (`sftp.*`).

**Known follow-ups:** streaming host→host copy (currently buffered, progress 0→100); SFTP TOFU cert pinning (trust-all now); agent-auth for SFTP; transfer queue speed-smoothing; perms-edit (chmod) context item.

## Latest — Local terminal (real PTY)

`portable-pty` in `rh-app`; `rh-app/src/local_pty.rs` `LocalPtyManager` + `spawn_pty` worker (PTY → shell: PowerShell on Windows / `$SHELL`|bash unix, overridable). Reuses `SshSessionEvent`/`SessionCommand` so the existing `Terminal.tsx` works unchanged. Commands `local_session_open/close/input/resize` (`rh-app/src/api/local_sessions.rs`). Shell choice persisted via settings key `local.shell` (rh-core `Settings.local_shell`, `TerminalSection.tsx`). Resize-race fixed (re-send resize once `sessionId` set, else ConPTY sticks at 80×24). Restore-on-reload deferred.

## Latest — Tools credential manager fixes

In the Tools screen, the credential list now: reveals on row click (copy icon appears inline next to a revealed password; no edit-on-click — creds are edited in the host form), shows **only credentials still linked to ≥1 host** (orphans from `unlinkHost` hidden via a `useCredentialLinks` aggregation), and dropped the "+ Add" path (credentials are created in the host editor). Backend orphan-cleanup-on-unlink offered but deferred.



First report of real-world lag (vs native mstsc). Two fixes for the two causes:
- **Input latency** — the worker's idle read timeout was 400ms, so a click sent during an idle read waited up to 400ms before the worker drained + forwarded it. Dropped `READ_POLL` to **16ms** (read_pdu still returns immediately on data; this only bounds the idle wait). This is the dominant click-responsiveness fix.
- **Frame transport** — we were shipping the full 1280×800×4 = 4MB RGBA framebuffer as a serde-JSON number array (~14MB of text) every 100ms (the bottleneck flagged in `rdp-session.md` Open-Q #1). Now: JPEG-compress (quality 72) + base64 → new `RdpSessionEvent::FrameJpeg`, ~40-60KB/frame (~250× smaller), **only sent when the framebuffer actually changed** (raw compare against the last-sent buffer → zero traffic when idle), ~15fps cap. `image` + `base64` moved into `rh-rdp` deps. Frontend decodes the data URL via `Image`/`drawImage`.

Native mstsc will still edge it out (hardware codecs + protocol-level region diffing), but this closes the big gap. Region-diffed frames + an off-thread encoder are the next perf step if needed.

**Follow-up (input backlog + debug-build perf):** first real test showed >10s click latency. Root causes + fixes:
- **Mouse-move flood** — every DOM mousemove was one IPC call; the queue grew faster than the worker drained it, so a click sat behind a flood of moves. Fixed at both ends: frontend throttles `mouse_move` to ~25/s (clicks/wheel/keys still immediate; clicks carry their own coords so dropping intermediate moves is safe), and the worker **coalesces consecutive moves** when draining (only the latest is sent).
- **Debug-build codec cost** — `cargo tauri dev` is unoptimized, making JPEG encode + graphics decode 10-30× slower. Added `[profile.dev.package."*"] opt-level = 3` so dependencies are optimized even in dev (our crates stay unoptimized for fast iteration). First rebuild after this is slow (deps recompile once), then cached.

## Latest — RDP round 2b-2 (mouse input): interactive pointer in the app

Input API **validated by the spike first** (the project's rule for unproven IronRDP surface): extended `rdp_spike` to inject a right-click — it compiled clean (confirming `ironrdp-input 0.5`: `Database::new/apply`, `Operation::MouseMove/MouseButtonPressed/MouseButtonReleased`, `MousePosition`, `ActiveStage::process_fastpath_input` → `ResponseFrame`) and the context menu appeared in the captured PNG. Timing lesson baked in: pointer-move and click must be separated by a beat (in the live app this is natural — real user motion).

Ported the proven mouse path into the app:
- **Actor**: command bridge added — `run` forwards `RdpCommand::Input` to the worker via a std channel; the worker drains it each loop iteration and encodes via `send_input` (mouse move / button / wheel). Keyboard + modifier-sync events are accepted but ignored (next slice — they need the scancode map + `Scancode` API, still unproven).
- **rh-app**: `rdp_session_input` command + `RdpInputRequest` DTO → `RdpSessionManager::send_input` → `RdpCommand::Input`.
- **Frontend** (FE verified): `rdpSession.sendInput` ipc + `RdpInputRequest` type; `SessionView.handleRdpInput` now forwards viewport events to the actor (fire-and-forget). The `ironrdp` meta-crate needs the **`input`** feature (added to `rh-rdp/Cargo.toml`).

You can now **click and scroll** the live desktop. Keyboard is 2b-2b. (Known MVP: mouse-move fires one IPC per event — fine now, throttle/coalesce later with the transport work.)

## Latest — RDP round 2b-1: live desktop in the app (read-only) — spike PROVEN

**Spike (2a) succeeded against the real Windows test server** — `rdp_spike` connected (TLS + NTLM/CredSSP), decoded graphics and saved a full-colour 1280×800 desktop PNG. The IronRDP 0.14 wave + the exact `connector::Config` field set + the blocking connect sequence are confirmed end-to-end on Windows.

**2b-1 ports that proven path into the app as a read-only live viewer.** Because the validated code is *blocking*, the actor runs it on a dedicated OS thread and bridges to async: events out via the Tokio `UnboundedSender` (its `send` is sync), shutdown in via a shared `AtomicBool`. No async-IronRDP API guessing — it's the spike code, verbatim, behind a poll loop (400 ms read timeout so it notices shutdown; full framebuffer pushed at ~10 fps — MVP, region-diff + faster transport deferred per spec Open-Q #1).

- IronRDP moved `rh-rdp` **[dev-dependencies] → [dependencies]** (so the app build pulls it now; the `rdp_spike` example still builds via its remaining dev-deps). First `cargo tauri dev` compile will be slow and *may* surface minor API drift in `actor.rs`, but the connect code is identical to the proven spike, so risk is low.
- `rh-rdp/src/actor.rs` rewritten: `spawn_session` → worker thread (`blocking_session`) doing connect + ActiveStage loop, emitting `StateChanged`(Connecting→Authenticating→Ready)/`Frame`/`Closed`/`Error`. Input/Resize accepted but dropped (2b-2).
- `rh-app`: new `RdpSessionManager` (thin registry, no scrollback — a framebuffer isn't replayable) + `rdp_session_open`/`rdp_session_close` commands (resolve host+password cred, reveal, spawn, forward events→`Channel<RdpSessionEvent>`). Wired into `AppState` + `main.rs`.
- Frontend (FE verified — tsc + vite clean): `SessionOpenOptions` now a ssh|rdp union; `rdpSession` ipc namespace; store routes RDP events (frame-sink keyed by session, latest-frame buffer until the viewport mounts) and branches `createSession`/`teardownSession` on protocol; `RdpViewport` self-registers its frame sink via `registerSessionViewport`. Opening an RDP host now shows the **live remote desktop** (read-only, ~10 fps, no input yet).

Run: `cargo tauri dev`, then open the RDP host in RemoteHub → live desktop. **2b-2 next: input** (mouse/keyboard + modifier-sync) via IronRDP's `input` API — new/unproven, so it's a separate pass.

## Latest — RDP connectivity spike (round 2a): isolated IronRDP connect → PNG

Before wiring IronRDP into the actor/app, validate it in isolation. Added `crates/rh-rdp/examples/rdp_spike.rs` — a near-verbatim port of IronRDP's official blocking `screenshot.rs`: connects (TLS + CredSSP), decodes graphics until idle, saves the desktop to PNG. IronRDP lives in `rh-rdp` **[dev-dependencies]** only (`ironrdp 0.14` + `ironrdp-blocking` + `sspi` + `tokio-rustls` + `x509-cert` + `image`), so the normal app build is unaffected — only `cargo run -p rh-rdp --example rdp_spike` pulls it.

Run: `cargo run -p rh-rdp --example rdp_spike -- --host <IP> -u <USER> -p <PASS> [-d <DOMAIN>] -o shot.png`. If `shot.png` shows the desktop → IronRDP connects to this server and the exact 0.14 API/versions are confirmed. Round 2b then ports the validated connect into the async `rh-rdp` actor + rh-app session path + frontend viewport routing, and adds input. Expect possible version/feature drift on first compile (the spike's purpose is to surface it).

## Latest — RDP actor shell (round 1; compiles, no IronRDP yet)

`rh-rdp` now has the actor shell mirroring rh-ssh: `spawn_session` + `RdpCommand` channel + lifecycle events. No IronRDP deps yet — it reaches `Authenticating` then emits a graceful "not wired" close. Clean compile; round 2 fills `connect_and_pump` (TLS+CredSSP connect, ActiveStage graphics→Frame, input→fastpath) against the real `ironrdp-client` async source, plus the rh-app session wiring + frontend live-routing.

## Latest — RDP trusted-cert store (TOFU for RDP), frontend verified; Rust mirrors known_hosts

Spike-independent prep for RDP. Trusted RDP server certificates get a TOFU store — the exact analog of known_hosts: `RdpCertStore` trait + `TrustedCert`/`RdpCertEntry` (rh-core), `SqliteRdpCertStore` + `rdp_known_certs` table (migration **v7→v8**, additive, `CURRENT_SCHEMA_VERSION=8`), `rdp_certs_list`/`rdp_cert_forget` commands, wired into `AppState`. The actor will use `lookup`/`remember` for cert pinning when it lands. Surfaced as a third tab ("RDP certificates") in the Security dialog (empty until you connect, like Known Hosts was). All a mirror of the SSH known_hosts work — low risk.

## Latest — RDP foundation: contract types + viewport (frontend verified; RUST = types only)

First RDP slice. Deliberately **no IronRDP yet**: the spec mandates a connectivity/transport spike first (run `ironrdp-client` against a real RDP server; benchmark frame transport — Tauri Channel `Vec<u8>`→JSON is too slow at 1080p60, candidates are custom-protocol / localhost WS / SharedArrayBuffer; Open-Qs #1/#5). That spike needs real hardware (yours), so the connect/decode actor is the next slice.

**Contract types** (`crates/rh-rdp/src/lib.rs`, pure types, no IronRDP — compiles as a normal rh-app dep): `RdpInputEvent` (MouseMove/MouseButton/MouseWheel/Key + **SyncModifiers**/**ReleaseAllModifiers** for focus-sync), `RdpSessionEvent` (StateChanged/Frame{region,format,data}/PointerPosition/CertPrompt/Clipboard/Error/Closed), `RdpState`, `PixelFormat`, `FrameRegion`, `RdpCloseReason`, `ColorDepth`, `RdpOpenOptions`, `RevealedRdpCredential`, `RdpSpawnParams`, `ModifierState`, `RdpError(+into_close_reason)`. Layering like rh-ssh (events over mpsc; rh-app bridges to Tauri Channel — NOT tauri in rh-rdp). Mirrored in `ui/src/lib/types.ts`.

**`RdpViewport`** (`ui/src/components/session/RdpViewport.tsx`, verified): `<canvas>` + imperative `applyEvent(frame)` (builds `ImageData`, BGRA→RGBA swap when needed, `putImageData` at region) + full mouse/keyboard capture (display→backing coord mapping) + **focus/modifier sync** — the spec's required-in-foundation fix for stuck modifiers: `blur`→`release_all_modifiers`, `focus`→`sync_modifiers` from last-known physical state (tracked via `getModifierState` on every mouse/key event). Wired into `SessionView` (RDP protocol branch); `onInput` currently routes to a placeholder (`handleRdpInput`) — the backend input channel lands with the actor. RDP sessions still can't be created (session_open → not_implemented) so the branch is dormant but type-complete.

### Next RDP slice (needs your spike): IronRDP connect + decode actor
1. Spike: `ironrdp-client` vs a Windows VM / xrdp — confirm happy-path + pin exact crate versions.
2. Pick frame transport (bench the three options).
3. `rh-rdp` actor: connector + `ActiveStage` loop + cert store (analog of known_hosts) + input mapping (browser code→PS/2 scancode) + frame coalescing. `rh-app`: RDP path in `session_open`, `rdp_session_input` command, bridge events→Channel. Wire `RdpViewport` to live frames + `handleRdpInput` to the channel.



Last SSH item. `ssh -A`: forward the local agent so onward auth on the remote works.

**Data model:** `Host.agent_forwarding: bool`. Migration **v6→v7** `ALTER TABLE hosts ADD COLUMN agent_forwarding INTEGER NOT NULL DEFAULT 0` (`CURRENT_SCHEMA_VERSION=7`, v1.sql bumped + column). Wired through host_store (bool↔INTEGER, `i64::from` / `!= 0`), HostDto, Host{Create,Update}Request (plain `Option<bool>`).

**rh-ssh:** `SshSpawnParams.agent_forwarding`. The actor calls `channel.agent_forward(false)` after channel-open when enabled (confirmed russh API — advertises acceptance). ⚠️ **Serving side deferred:** russh 0.45's client callback is `server_channel_open_agent_forward(&mut self, channel: ChannelId, _)` — it hands a `ChannelId`, not a `Channel`, so back-channel bytes arrive via the `data()` callback and replies go through `session.handle().data(...)`. That stateful relay needs its own tested pass; for now we only advertise (request-only). My first attempt used a `Channel` arg → E0053; removed.

**UI:** "Forward SSH agent (ssh -A)" checkbox in the host form's Advanced section (edit mode only), saved immediately via `host_update` (same pattern as jump host). `.checkboxRow` style added.

### SSH hardening status: ✅ COMPLETE
TOFU/known_hosts + management UI, SSH-agent auth, restore-on-reload, env passthrough, keepalive, OS auto-detect (+ sidebar icon), last_connected, ProxyJump, agent forwarding. Next major: **Stage 4 — RDP via IronRDP** (see `docs/specs/rdp-session.md`, sticky-modifier focus-sync requirement), or Sync/master-password.



A host can now route through a **bastion** — another saved SSH host used as a jump. Agent-forwarding is the next (last) SSH item; kept separate (russh-heavy).

**Data model:** `Host.jump_host_id: Option<HostId>` (plain nullable TEXT, no FK — a deleted bastion is handled at connect time). Migration **v5→v6** `ALTER TABLE hosts ADD COLUMN jump_host_id TEXT` (chained runner; `CURRENT_SCHEMA_VERSION=6`, v1.sql bumped + column added). Wired through host_store INSERT/UPDATE/SELECT/row_to_host, HostDto/HostFullDto, Host{Create,Update}Request (update guards against self-reference).

**Connect flow (actor):** `SshSpawnParams.jump: Option<JumpParams{hostname,port,host_id,credentials}>`. When set, the actor connects the bastion (`russh::client::connect`, auth via `try_all_auth`), opens `channel_open_direct_tcpip(target,…)`, wraps it `into_stream()`, and runs the target transport over it via `connect_stream` — then proceeds identically (auth/PTY/shell/pump). The bastion `Handle` is kept alive (`_bastion_keepalive`) for the session. Refactor: `ClientHandler.auto_accept` (bastion pins its key silently — no double prompt; target keeps normal interactive TOFU); new helpers `ConnectOutcome` + `drive_target_connect` (drives either connect future while forwarding host-key decisions) + `try_all_auth`. ⚠️ russh-version-sensitive calls flagged: `channel_open_direct_tcpip`, `Channel::into_stream`, `connect_stream`.

**rh-app:** `session_open` resolves the jump host (must exist + be SSH), reveals its creds via new self-contained helper `revealed_creds_for` (mirrors target reveal: passwordless fallback, key→agent→password order). One level only (a bastion's own `jump_host_id` is ignored).

**UI:** "Jump host" combobox in the host form's **Advanced** section (edit mode only), listing other SSH hosts (excludes self); empty = direct. Reuses the `Combobox` (clearable). Saved **immediately** on change via `host_update` (not threaded through the debounced text-field autosave — lower risk). Spec appended to `docs/specs/ssh-session.md`.



Two of the four remaining SSH items (jump-host + agent-forwarding are the next, dedicated pass — they restructure the connect path and are the most russh-fragile, so they're kept separate).

**OS auto-detect (Stage 2.2):** the actor runs a best-effort probe on a *separate* exec channel right after Ready (doesn't touch the PTY): `uname -s; ___RH___; cat /etc/os-release`, parsed by `parse_os_slug` (5 unit tests) → "ubuntu"/"debian"/"macos"/"windows"/"linux"/… Emits a new `SshSessionEvent::DetectedOs { os }` which the `SessionManager` pump **consumes** (not forwarded to the UI) and persists via the new `HostStore::mark_detected_os` (targeted UPDATE, like `mark_connected`). The OS chip in the host header (already built) shows it on the next `host_get`. The **sidebar host icon** now switches to the OS logo too: `HostIcon` maps the slug → a Simple Icons glyph via `react-icons/si` (new dep; rendered MONOCHROME via currentColor — no brand colors, per the design language), fallback to the generic `Server`. To refresh the sidebar live after the first connect, the detect path emits `hosts:changed` (AppHandle plumbed into `SessionManager::register`), which the UI already reloads on. ⚠️ The exec probe (`Channel::exec`) is russh-version-sensitive — flagged in `detect_os`.

**Known Hosts management:** `KnownHostsStore::list()` + `KnownHostEntry` (rh-core), `known_hosts_list`/`known_host_forget` commands, `knownHosts` ipc namespace. Surfaced as a **second tab in the key/credentials dialog** (now titled "Security"/"Безопасность"): tab 1 = Credentials (unchanged), tab 2 = Known hosts — list of `hostname:port · key_type · SHA256:… · trusted-date` with a trash button to forget (next connect re-prompts TOFU). Per-host jump/agent-forward will be host-form fields, NOT here (deliberately kept the footer clean — confirmed UX call).



The ⓘ technical-info popover now shows Created / Updated / **Last connection** / Fingerprint. The opaque ULID `ID` row was removed (debug-only; kept the value out of the user's face). Fingerprint is copy-to-clipboard (`SHA256:<fp>` + key-type hint).

- **`last_connected_at: Option<DateTime<Utc>>` on `Host`** (rh-core), machine-set — never written through create/update. New `HostStore::mark_connected(id, when)` does a targeted `UPDATE hosts SET last_connected_at=?`. The `SessionManager` event pump stamps it once, on the first `Ready` event (so it means *connected*, not *attempted*); `Hub.connected_stamped` guards against repeats.
- **Migration v4 → v5**: additive `ALTER TABLE hosts ADD COLUMN last_connected_at TEXT`. `db.rs` `init_or_migrate` was refactored into a **chained runner** (`MIGRATIONS: &[(from, sql)]` + `has_migration_chain`): any DB with a contiguous path (v2→v3→v4→v5) migrates forward with no data loss; only a gap (pre-v2) drop-recreates. `host_store` INSERT/SELECT/`row_to_host` updated for the column; `v1.sql` carries it + version '5'.
- Exposed as `HostDto.last_connected_at` (RFC 3339 string | null). Frontend shows `formatDate(...)` or "Never"/"Ещё не подключались". Info-label column widened to 104px; RU "Подключение".



## Latest — SSH hardening: TOFU, agent, restore-on-reload, env (DONE, live-verified)

Closes the Stage-2 follow-ups before RDP. **Compiled & live-verified on Windows**: TOFU prompt (unknown/changed/reject + silent on known), SSH-agent auth, restore-on-reload (F5 brings sessions back with scrollback), env passthrough, last-connection stamp, copyable fingerprint. The agent block compiled against russh 0.45 as written.

**known_hosts / TOFU (rh-core + rh-storage + rh-ssh + rh-app):**
- New `KnownHostsStore` trait (`rh-core/store.rs`) + `KnownHostKey { key_type, fingerprint_sha256 }` (OpenSSH SHA256, base64 no-pad). `SqliteKnownHostsStore` (`rh-storage/known_hosts_store.rs`, upsert by `(hostname, port)`, 5 tests). New `known_hosts` table.
- **Migration v3→v4 is incremental, data-preserving** (`db.rs` `CURRENT_SCHEMA_VERSION = 4`): `Some(3)` → `CREATE TABLE known_hosts`; plus a chained `Some(2)` → v3 ALTER then v4 CREATE so a two-versions-behind DB isn't wiped. `v1.sql` updated for fresh installs (known_hosts table + version '4'). **Same rule as before: additive change = incremental ALTER/CREATE path, never a bare v1.sql bump.**
- Actor (`rh-ssh/actor.rs`): `check_server_key` now computes the SHA256 fingerprint, looks up the pin, and — on unknown (when `strict`) or **changed** (always) — emits `HostKeyPrompt { fingerprint_sha256, key_type, changed }`, sets state `host_key_pending`, and **blocks** on a decision. The decision arrives as `SessionCommand::HostKeyDecision(bool)`, forwarded into the handler from the command channel while the connect future is in flight (select loop in `connect_and_pump`). Accept → pin + `Ok(true)`; reject → `Ok(false)` → mapped to `CloseReason::HostKeyRejected` via a `rejected` flag + new `SshError::HostKeyRejected`.
- `session_accept_host_key`/`session_reject_host_key` now send `HostKeyDecision(true/false)` (no longer no-ops / hard close). Frontend prompt surface already existed; added a `changed` flag → red-accented warning banner + `session.hostKey.changedPrompt` string (EN/RU).
- `strict_host_key` comes from `Settings.ssh_known_hosts_strict` (default true). Non-strict auto-pins unknown keys silently but **still prompts on a changed key**.
- **Pinned fingerprint shown in the host technical-info panel** (the ⓘ popover): new `known_host_get` command (resolves host_id → hostname/port → `KnownHostsStore::lookup`) + `hosts.knownHostKey` ipc. The popover's ID and fingerprint rows are now copy-to-clipboard (`CopyableValue` in HostDetail.tsx; `SHA256:<fp>` + key-type hint). Shows "not pinned yet" until first trust.

- **SSH-agent auth (rh-ssh + rh-app):**
- `RevealedCredential::Agent { username }`; `CredentialKind::SshKeyAgent` now produces it (was skipped). Actor `try_auth_agent`: connect to agent (unix `$SSH_AUTH_SOCK` via `AgentClient::connect_env`; **windows** `\\.\pipe\openssh-ssh-agent` named pipe — covers OpenSSH agent and modern Pageant), `request_identities`, then `authenticate_future` per identity. **Best-effort & non-fatal** — any agent failure returns `Ok(false)` so other methods still run. ⚠️ **This is the most russh-version-fragile block** — if it doesn't compile, the fix is local to `try_auth_agent` (and possibly a russh `agent` feature flag); TOFU/restore/env are independent of it.
- Auth order is keys → agent → password.
- **Agent UI (HostDetail credential panel):** the "+ SSH-ключ" picker now has a **"Use SSH agent"** footer → creates/reuses an `ssh_key_agent` credential (no secret) and links it; shows as a `Server`-icon chip with ✕ to unlink. Key and agent share the one "method slot" (mutually exclusive in the UI; password is always its own field). This is the only way to create an agent credential — before, `ssh_key_agent` was reachable only via the IPC console.

**Restore-on-reload (rh-app `SessionManager` rewrite + frontend):**
- The Rust process survives a webview reload, so actors stay alive. `SessionManager` now holds a per-session `Hub` { tx_cmd, abort, meta, state, **256 KB output ring**, current `Channel` sink }. `register` absorbs the old event-bridge: it pumps actor events → records into the ring + forwards to the live channel. New `list()` (→ `session_list` returns real `SessionSummaryDto[]`) and `reattach(id, channel)` (→ new `session_reattach` command) which swaps the sink and replays buffered scrollback + current state.
- Frontend: `restoreSessions()` store action (called from `AppShell` mount) calls `session_list`, rebuilds one tab per live session, and `session_reattach`es a fresh channel; dead/closed sessions are skipped, and a reattach miss drops the stale tab. **Split layouts are NOT reconstructed** — each restored session comes back as its own tab (flat). **Edge:** a session reloaded mid host-key-prompt restores without the prompt object (fingerprint isn't buffered) — rare; user reconnects.

**Env + keepalive (rh-ssh + rh-app):**
- `SshSpawnParams.env_vars: Vec<(String,String)>` from `host.env_vars`; actor sends `channel.set_env(false, k, v)` before the shell (servers honor only their `AcceptEnv`; want_reply=false so an unaccepted var can't fail the channel).
- Keepalive interval now from `Settings.ssh_keepalive_interval_secs` (0 = disabled) instead of a hardcoded default.

**Build/run after pulling this:** `cargo tauri dev` (Rust + migration changed). The v3→v4 migration is additive — existing hosts/creds/settings are preserved; only a brand-new `known_hosts` table is added.



UX pass on top of the auth work below. All frontend; verified `tsc --noEmit` + `vite build`.

**Credential panel (HostDetail.tsx — the fragile file):**
- **Saved password is locked by default** (read-only, muted color `.pwMuted`). Eye 👁 reveals the live keychain secret read-only (selectable/copyable, 10s auto-hide). Pencil ✏️ reveals it into an **editable plaintext** field (via `onPasswordRevealed`, which seeds the value as the committed baseline so no save fires).
- **No ✕ on the password.** Removal is deliberate: reveal with the pencil, clear the text, save → `credential_unlink_host`. Guard: an empty field only deletes when it differs from a non-empty committed baseline (so merely opening a host and saving never nukes the password). `saveAction` password block: changed→ empty+pwCred = unlink, non-empty = rotate/create.
- **Re-lock on click-outside** the password row (`pwRowRef` + document mousedown) — and on linked-cred change. So the resting state is always "locked + muted"; clicking Connect, another field, etc. re-locks and hides the reveal.
- **Connect commits the field:** `handleConnect` clears the typed `inlinePassword/privateKey/passphrase` + committed refs after the flush-save. Without this, a locally-typed password lingered and the eye showed the stale typed value (not the live keychain secret) until you navigated away — the "password shows 111 after re-auth changed it to 222" bug. Now the field locks to the saved cred and the eye always reveals live.
- **SSH key add:** "+ SSH-ключ" opens a dropdown of existing `ssh_key` creds + an **"Add new key…"** footer → `AddKeyModal` (paste / import .ppk·PEM / passphrase). Key chip still uses pencil→✕ (keys aren't text, so the 2-step delete stays). `SavedCredentialPicker` and `AddKeyModal` are **exported** from HostDetail.tsx for reuse on the re-auth screen.

**Inline re-auth on auth failure (SessionView.tsx):**
- Detect: `isDead && message.toLowerCase().includes("auth")` — **note auth failure ends the session in state `closed`, not `failed`**, so don't gate on `failed`.
- Layout: password input + a stretched **SSH-key button** (green `--color-ssh` icon) on one row (`.reauthPwRow` is the dropdown anchor → full-width picker), full-width **"Подключиться и сохранить"** below. No hint text, no "Edit" button on the auth screen (Edit stays on the non-auth closed/failed EmptyState).
- Actions all **save to the host** then reconnect: `connectWithPassword` rotates the existing password cred or creates+links a new one; `linkAndReconnect(credId)` links a picked key; `addKeyAndReconnect` creates+links a pasted/imported key. Each fetches the full host (`hosts.get`) for `credential_ids`, mutates via `credApi`, then `close + open(fresh)`.

**Other:**
- **Passwordless fallback (sessions.rs):** if a host has **no linked credential but a username**, try a single empty-password attempt instead of erroring "host has no credential".
- **Split-tab label (TabBar.tsx):** a tab with >1 pane shows `t("tab.split")` ("Сплит") instead of one pane's title; the ⊞ count badge stays.



## Latest — SSH auth, multi-method credentials, per-host username (DONE, live-verified)

Big batch after Stage 2 part 2. All compiled on the user's machine and verified by real connections (key, .ppk, password, passwordless).

**SSH auth (rh-ssh):**
- Public-key auth: `russh::keys::decode_secret_key` for OpenSSH/PEM; `authenticate_publickey` (russh 0.45 `bool` API — note in `actor.rs` for 0.46+ `PrivateKeyWithHashAlg` + `.success()`).
- **Native PuTTY .ppk → OpenSSH** converter: `rh-ssh/src/ppk.rs` (pure Rust, PPK v2 HMAC-SHA1 + v3 Argon2id, aes256-cbc/none, rsa/dss/ecdsa/ed25519). Crypto crates added to `rh-ssh/Cargo.toml` (sha1/sha2/hmac/aes/cbc/cipher/argon2/base64) with a comment justifying the deviation from "aws-lc-rs only". `actor.rs` detects `is_ppk` → converts → decodes with no passphrase.
- Empty/passwordless: missing keychain secret → empty password; and (see sessions) a host with a username but **no** credential tries an empty password.

**Multi-method per host (the actor tries each, keys → password):**
- `SshSpawnParams.credential` → `credentials: Vec<RevealedCredential>`. `actor.rs` loops `try_auth` over them; a bad/undecodable key is skipped (`Ok(false)`) so a working password still gets in; auth fails only if all are rejected.
- `CredentialStore::credentials_for_host(host_id)` (new trait method + JOIN on `host_credentials`, default first).
- `api/sessions.rs`: with no `credential_id` override, gathers **all** linked creds (keys first); if none linked but host has a username → single empty-password attempt; else "host has no credential".
- Frontend `open()` sends `credential_id: null` (was the default id) so the backend offers every method — passing a specific id would restrict to one.

**Per-host username (data-model change, NON-DESTRUCTIVE migration):**
- `username` moved from the **credential** to the **host** (one key shared across hosts with different logins). `Host.username`, `HostFullDto.username`, host create/update DTOs.
- Session resolves `host.username` else falls back to `cred.username` (back-compat for pre-migration hosts).
- **Migration v2→v3 is incremental, data-preserving:** `db.rs` `CURRENT_SCHEMA_VERSION = 3`; `Some(2)` → `ALTER TABLE hosts ADD COLUMN username TEXT NOT NULL DEFAULT ''` + bump `schema_meta` (no drop). Other version mismatches still drop-recreate (alpha mode). `v1.sql` updated for fresh installs (hosts.username + version '3'). New `InitOutcome::Migrated`. **Do not bump the version with a plain edit to v1.sql for an additive change — add an incremental ALTER path or you wipe user data.**
- `HostFullDto.credential_ids: Vec<CredentialId>` (populated by `host_get` via `credentials_for_host`) so the UI can render all linked methods.
- Credential username validation relaxed: **empty username is allowed** for all kinds (login lives on the host now). Inline-created creds pass `username: ""`.

**Credential UX (HostDetail.tsx, the fragile file):**
- Password field is **always visible**; a linked SSH key shows as a chip; each linked method has a ✕ to **unlink** (`credential_unlink_host`). Password/key handled **independently** in save (create+link if absent, rotate if present, change-gated to avoid duplicate creates).
- **New add-key flow:** "+ SSH-ключ" → dropdown of existing keys + **"Add new key…"** footer → **modal** (`AddKeyModal`) to paste or import (.ppk/PEM) + passphrase; on confirm creates the cred in the keychain and applies immediately (edit → linkHost, draft → remembered, linked on promotion). Inline key textarea removed.
- **Connect flushes pending save first** (`handleConnect` cancels debounce, awaits `saveAction`, re-fetches, then opens) so a just-typed password/key is persisted before the session opens — fixes spurious "host has no credential".
- Compact form: Name + Group on one row; Tags / Startup command / Env vars / Notes under an **"Advanced/Дополнительно"** spoiler. Password field full-width with trailing controls (timer/eye/✕) overlaid right.
- Key creds named by imported **file name**; re-import renames. "Use existing" lists **keys only** (passwords stay private to their host).

**Session error screen:** "Edit/Изменить" button next to "Reconnect" → jumps to the Vault tab and selects the host (`setActiveTab(null)` + `selectHost`).



## UX overhaul — tabbed shell (part 1, DONE, verified tsc+vite)

Replaced the permanent left-sidebar shell with a Termius/Windows-Terminal-style tab bar.
- `layout/TabBar.tsx` — top bar: pinned **Vault** tab (`nav.vault`, host manager, not closable, `activeSessionKey === null`) + one tab per session + a "+" button (currently returns to Vault; dedicated launcher is the next step).
- `layout/HomeView.tsx` — Vault content = `CommandBar` + `Sidebar` + `HostDetail` (the former shell body, now the home tab).
- `layout/AppShell.tsx` — `TabBar` over a stage that keeps **every** tab mounted (HomeView + all SessionViews) and toggles visibility via inline `display`, so scrollback, form drafts, and focus all survive tab switches.
- Removed `layout/WorkArea.tsx` and `session/SessionTabs.tsx` (folded into AppShell/TabBar).

**UX overhaul follow-ups (user's list):** (a) "+" launcher — search + recent/host list to start a session (screen ref: Termius new-tab); (b) terminal appearance — bundle a default mono font + make font configurable; (c) terminal theme presets (Dracula/Nord/Solarized/Monokai/Pro…) with a full 16-color ANSI palette + picker + persistence. ANSI output colors already render (server-driven); no client-side keyword highlighting.

## Stage 2 — in progress (SSH sessions)

**Part 1 — frontend + IPC contract (DONE, verified tsc+vite):**
- `ui/src/components/session/{Terminal,SessionView,SessionTabs}.tsx` + `layout/WorkArea.tsx`. The work area now shows a tab strip (Host editor + one tab per session) and swaps between `HostDetail` and a live `SessionView`. AppShell body = Sidebar + WorkArea.
- Terminal is xterm.js (`@xterm/xterm` + `addon-fit`). Output flows via a module-level registry in the sessions store (buffered until the terminal mounts, so switching tabs never loses output); keystrokes → `session_send_input`, resize → `session_resize`.
- `useSessionsStore` (in `store/index.ts`): tabs keyed by a stable local `key` (set up before the backend returns the real `sessionId`, avoiding event races), state machine, host-key prompt, reconnect.
- `lib/ipc.ts` `sessions.*` uses a Tauri **Channel** for `SshSessionEvent` (`state_changed/data/auth_failed/host_key_prompt/error/closed`, `CloseReason`) per `tauri-api.md`. Types in `lib/types.ts`.
- Connect button enabled for **saved SSH hosts** (draft/RDP show a reason tooltip). Backend is still the stub → connecting shows "failed: not implemented" gracefully. No regression to the running app.

**Part 2 — russh backend (DONE — live SSH connection verified against AM-NL):**
- Compiled clean on russh 0.45 after one fix (the `Handler` trait there is `#[async_trait]`, so `ClientHandler` is annotated `#[async_trait]`). The `select!` channel-borrow and PTY/auth signatures worked as written.
- `rh-ssh`: `russh` client actor. `lib.rs` (public types: `SessionState`, `CloseReason`, `SshSessionEvent`, `SessionCommand`, `SshOpenOptions`, `RevealedCredential`, `SshSpawnParams`, `SshSessionHandle`, `spawn_session`), `error.rs` (`SshError` + `into_close_reason`), `actor.rs` (russh connect → password auth → PTY shell → select! pump). Events flow out via `mpsc::UnboundedSender<SshSessionEvent>` (crate stays tauri-free).
- `rh-app`: `session.rs` `SessionManager` (registry + per-session supervisor that evicts on exit); `api/sessions.rs` real handlers (`session_open` reveals credential, bridges mpsc→Tauri `Channel`, spawns actor, registers; close/send_input/resize/accept/reject); DTOs (`SessionOpenResponse`, `SessionInputRequest`, `SessionResizeRequest`, `SessionAcceptHostKeyRequest`); `AppState.sessions`; handlers registered in `main.rs`.
- **v1 simplifications (to land a working connect first):** password auth only (SSH-key/agent → friendly not-implemented); host key auto-accepted TOFU (no `known_hosts` pinning, no interactive prompt blocking inside the russh handler — UI prompt surface stays dormant); no keepalive.
- ✅ **COMPILED & LIVE-VERIFIED.** russh pinned `0.45`. Auth has since grown to multi-method (key/.ppk/password/passwordless) — see the "Latest" section at the top. Keep a known-good zip as rollback when touching the backend.

**Part 2 follow-ups:** ✅ all done — known_hosts pinning + interactive TOFU, SSH-key auth, SSH-agent auth, keepalive, `session_list` restore-on-reload, env passthrough (see the SSH-hardening "Latest" section at top). Next is Stage 4 (RDP).

---

This document is the single source of truth for picking up RemoteHub development. Read it first when starting a new chat or after a long break. When a stage closes, **update this file** before packaging the archive.

---

## What RemoteHub is

A cross-platform desktop client for remote sessions (SSH + RDP). Windows-first; architecturally ready for macOS and Linux. The target is "Termius with RDP" — modern UI, no clutter, live-save everywhere, OS keychain for secrets.

- **Shell**: Tauri 2
- **Backend**: Rust stable + Tokio, russh (Stage 2), IronRDP (Stage 4)
- **Frontend**: React 18 + TypeScript strict + Vite, Zustand state, lucide-react icons
- **Storage**: sqlx + SQLite, keyring-rs 3.6 (apple-native / windows-native / sync-secret-service)
- **Crypto**: aws-lc-rs

User environment: Windows 11 (kolen), PowerShell, Rust 1.95, Node v24.15.0, pnpm v11.4.0, Tauri CLI 2.11.2. Real Windows Credential Manager records with `service="RemoteHub"`. DB at `%APPDATA%\RemoteHub\remotehub.db`.

---

## Workspace layout

```
remotehub/
├── crates/
│   ├── rh-core/        # types, errors, IDs (41 tests)
│   ├── rh-storage/     # SQLite + keychain (37 tests)
│   ├── rh-ssh/         # placeholder for Stage 2
│   ├── rh-rdp/         # placeholder for Stage 4
│   └── rh-app/         # Tauri binary, IPC handlers (31 tests)
├── ui/
│   ├── src/
│   │   ├── components/
│   │   │   ├── host/HostDetail.tsx        # the big one (~1100 lines) — main pane
│   │   │   ├── host/SaveStatusIndicator.tsx
│   │   │   ├── sidebar/Sidebar.tsx
│   │   │   ├── layout/{AppShell,DialogHost}.tsx
│   │   │   ├── dialog/{ConfirmDialog,CredentialFormDialog,CredentialsListDialog,GroupFormDialog}.tsx
│   │   │   └── ui/{Button,Combobox,Dialog,EmptyState,Input,ProtocolBadge,TextField}.tsx
│   │   ├── i18n/{en,ru,index}.tsx         # custom i18n, no react-i18next
│   │   ├── lib/{ipc,types,useDebouncedCallback}.ts
│   │   ├── store/index.ts                 # zustand: hosts, groups, credentials, ui
│   │   └── styles/tokens.css
│   ├── package.json, vite.config.ts, tsconfig.json
│   └── vite-env.d.ts                      # CSS modules types
└── docs/
    ├── ROADMAP.md
    ├── STATE.md                            # this file
    └── specs/
        ├── system-overview.md
        ├── data-model.md
        ├── tauri-api.md
        ├── session-protocol.md
        ├── ssh-session.md
        └── rdp-session.md                  # contains sticky-modifier focus-sync requirement
```

---

## Stage status

| Stage | Title | Status |
|---|---|---|
| 1.1 | Rust workspace skeleton + types | ✅ Done |
| 1.2 | Storage layer (SQLite + keychain) | ✅ Done |
| 1.3 | Tauri IPC handlers | ✅ Done |
| 1.4 | UI scaffolding + IPC client | ✅ Done |
| 1.5 | Initial UI (sidebar + detail + dialogs) | ✅ Done |
| 1.5.1 | UX pass #1: i18n, short-view, reveal, duplicate, group actions | ✅ Done |
| 1.5.2 | UX pass #2: live-save everywhere, draft mode, credentials redesign | ✅ Done |
| 1.7 | Visual pass: Inter + JetBrains Mono fonts, lighter dark theme, rounded sidebar items, HostIcon slot | ✅ Done |
| 1.6 | Settings dialog + language toggle UI | ✅ Done |
| 1.8 | Schema extensions: display_name, startup_command, env_vars, detected_os | ✅ Done |
| 1.9 | Command bar (top, search + user@host:port parser) | ✅ Done |
| 1.10 | Import .rdp files | ⬜ Future |
| 1.12 | Export/Import JSON (our format) | ⬜ Future |
| 2.x | SSH session actors (russh) | ✅ Done |
| 2.2 | OS auto-detect after connect (exec probe → detected_os) | ✅ Done (pending compile) |
| 2.3 | SSH hardening: ProxyJump (jump host) + agent forwarding (request-only; serving side TODO) | ✅ Done (compiles) |
| QA-2 | **Manual end-to-end QA of SSH hardening on real hosts** — agent-forward (`ssh -A` chain), ProxyJump through a bastion, OS-detect icon, known-hosts forget/re-pin, TOFU change warning. None of 2.2/2.3 verified by the user yet; agent-forward client callback name unconfirmed. | ⬜ TODO |
| 2.4 | Agent-forward **serving** side (ChannelId relay via data() + session.handle()) | ⬜ TODO |
| 4.0 | RDP foundation: contract types + viewport (focus/modifier-sync) | ✅ Done (FE verified; rh-rdp = types) |
| 4.0b | RDP trusted-cert store (TOFU) + Security-dialog tab | ✅ Done (FE verified) |
| 4.1a | RDP actor shell (spawn + command channel + lifecycle) | ✅ Done |
| 4.1b | RDP connectivity spike (isolated IronRDP → PNG) | ✅ Done (PROVEN on real server) |
| 4.1c | Read-only live desktop in app (2b-1: actor+rh-app+FE) | ✅ Done (FE verified; Rust pending Win compile) |
| 4.1d | RDP mouse input (2b-2: click/scroll, spike-proven) | ✅ Done (FE verified; Rust pending Win compile) |
| 4.1e | RDP keyboard input (2b-2b: scancode map + modifier-sync) | ⬜ Next |
| 5.x | Personal/Team Vault via S3 — cloud sync, identity, e2e crypto | ⬜ Future |

~119 Rust tests (run `cargo test` on Windows to confirm; +10 in Stage 1.8). Vite + tsc strict build green. **DB schema is v2** as of Stage 1.8 — opening an old v1 DB drops & recreates it (alpha policy, data loss expected).

---

## Stage 1.9 — closeout (command bar)

Frontend-only. New full-width strip at the top of the window (`AppShell` is now a column: CommandBar over a `.body` flex row of Sidebar + HostDetail).

`ui/src/components/layout/CommandBar.{tsx,module.css}` — the **single** search/command surface:
- owns `uiStore.searchQuery`, so typing live-filters the sidebar tree. **The sidebar's own search box was removed** (one search, not two — the redundant double-search looked wrong).
- parses `[ssh|rdp]://[user@]host[:port]`; when the text is an explicit address with no exact host match, a slim "New host" suggestion drops under the bar. Activating it (click or Enter) opens a pre-filled draft (`startDraft` + `updateDraft`).
- Enter with no suggestion opens the sole match if exactly one host matches. Ctrl/Cmd+K focuses; Esc clears. `Ctrl K` hint shown (Windows-first).

No backend changes — real connect is still Stage 2, so the "connect" path is quick host creation for now. i18n: `command.*` keys in both locales (`sidebar.searchPlaceholder` now unused but left in place).

---



Added four columns to `hosts` and threaded them through every layer. Schema bumped **v1 → v2**; alpha drop-recreate policy means existing DBs are wiped on first open with this build.

New fields:
- **`display_name TEXT` (nullable)** — explicit user label. **Retires the `name == hostname` auto-label heuristic** in `HostDetail.buildFormState`. The form's Label input now binds straight to `display_name`; unset = `null` and the input shows the hostname as placeholder. The frontend still sends `name` (= label‖hostname) as the canonical sort/search key *and* `display_name` separately; the backend stores both verbatim (no server-side derivation). Blank labels are normalized to NULL in `host_create`/`host_update`.
- **`startup_command TEXT` (nullable)** — command run on SSH connect. UI field is SSH-only (hidden for RDP and forced to `null` on save when protocol≠ssh). Consumed by the Stage 2 session actor (not yet wired).
- **`env_vars` → `env_vars_json TEXT NOT NULL DEFAULT '[]'`** — JSON array of `{key,value}` (order-preserving), mirroring the `tags_json` pattern. Domain type `EnvVar` in `rh-core`. UI: a `KEY=VALUE`-per-line textarea; `parseEnv`/`formatEnv` in HostDetail. Full-replace list on update (empty array clears).
- **`detected_os TEXT` (nullable)** — machine-set OS slug (e.g. `ubuntu`). **Not accepted on create**; persisted through the normal `host_update` path so the **Stage 2.2** detection routine can set it. Shown read-only as a small chip on the detail header when present; will drive the sidebar HostIcon in 2.2.

DTO placement: `display_name` + `detected_os` live on the lean `HostDto` (sidebar needs them); `startup_command` + `env_vars` only on `HostFullDto` (`host_get`).

Validators (rh-app): display_name ≤256, startup_command ≤4096, env_vars ≤64 entries / key ≤256 / value ≤4096, non-empty + unique keys, no NUL bytes.

Files touched: `rh-core/{types,lib}.rs`, `rh-storage/{db.rs, migrations/v1.sql, host_store.rs, tests/integration.rs}`, `rh-app/api/{dto,hosts}.rs`, `ui/src/lib/types.ts`, `ui/src/store/index.ts` (HostDraft gained `startupCommand`/`envVars` for faithful duplicate + dirty-check), `ui/src/components/host/{HostDetail.tsx,HostDetail.module.css}`, `ui/src/i18n/{en,ru}.ts`.

Side note: fixed three **pre-existing** `tsc` errors in the legacy `dialog/HostFormDialog.tsx` (modal form, not in the live render path) — it referenced i18n keys `credentialUseExisting` / `credentialUseInline` / `credentialSelectNone` that were never added. Added them to both locales so the strict gate is green again. Consider deleting that dead modal in a future cleanup stage.

---

## Stage 1.6 — pickup point (ARCHIVED — 1.6 is ✅ done)

What got done before pausing:

**Rust side — COMPLETE**:
- Added `Language` enum (`en` / `ru`) to `rh-core/src/settings.rs` with `Default = En`.
- Added three fields to `Settings` struct: `language: Language`, `default_ssh_port: u16` (= 22), `default_rdp_port: u16` (= 3389). Defaults wired up.
- Added matching key constants: `keys::LANGUAGE`, `keys::DEFAULT_SSH_PORT`, `keys::DEFAULT_RDP_PORT`.
- Updated `rh-storage/src/settings_store.rs` `load()` to read the three new keys, and `is_known_key()` to accept them.
- Added `Language` to `rh-core/src/lib.rs` re-exports.
- Added `language_serde_lowercase` test, updated `default_settings_match_spec` and `keys_are_unique` tests.

**UI side — PARTIAL**:
- Added `Language = "en" | "ru"` and the three new fields to `Settings` interface in `ui/src/lib/types.ts`.
- Nothing else done on UI yet.

**What remains for Stage 1.6** (pick up here):

1. **Settings dialog component** (~400 lines). Layout: sidebar (200px) + content (rest). Sections:
   - **Профиль** — empty state "Stage 5"
   - **Внешний вид** — language toggle (EN/RU), theme (System/Light/Dark)
   - **Подключения** — Default SSH port (22), Default RDP port (3389)
   - **Терминал** — empty state "Stage 2"
   - **Импорт/Экспорт** — empty state with roadmap pointers
   - **О программе** — version, repo link
2. **Settings store** (Zustand). Methods: `load()`, `update(patch)`, `subscribe()` (for events from Rust). Initial load on app start.
3. **Wire the language switcher**: when user picks RU/EN, also call `setLocale()` on the I18nProvider. They should track each other.
4. **Enable gear button** in sidebar footer: opens settings dialog.
5. **i18n keys** for the dialog text — sections, labels, helpers.
6. **Subscribe to `settings:changed` event** from Rust so settings dialog reflects changes in real-time (rare but cheap).
7. Verify with `tsc --noEmit && vite build`.

**Files to touch when resuming**:
- `ui/src/store/index.ts` — add `useSettingsStore`
- `ui/src/lib/ipc.ts` — already has `settings.getAll()` and `settings.update()`; may need a `subscribe` wrapper
- `ui/src/components/settings/SettingsDialog.tsx` (new) + `.module.css` (new)
- `ui/src/components/settings/sections/*.tsx` (one per tab) — keep sections tiny and isolated so adding new ones later is mechanical
- `ui/src/components/sidebar/Sidebar.tsx` — un-disable the gear button, wire it to `setDialog({ kind: "settings" })`
- `ui/src/store/index.ts` — add `"settings"` to `DialogKind` union
- `ui/src/components/layout/DialogHost.tsx` — handle the new `kind`
- `ui/src/i18n/{en,ru}.ts` — add keys for `settings.*`

**Settings dialog should respect existing patterns**:
- Live save (no Save button)
- Save status indicator in the header (same `SaveStatusIndicator` component)
- Discriminated union for tab selection (`type Tab = "profile" | "appearance" | ...`)
- All visible strings via `t("key")`
- CSS modules using existing tokens

---

## The core UX model (Stage 1.5.2 — important)

This is the Termius-style mental model the user wants. Internalize it before touching anything in HostDetail.

### Selection IS edit

Clicking a host in the sidebar opens it as an **editable form** in the right pane. There is no "Edit" button. Every field is editable in place. Changes auto-save with a debounce.

### Live save with status indicator

- Every keystroke triggers `setSaveStatus({kind: "pending"})`.
- Debounce timer fires (`400ms` for fields, `1000ms` for notes).
- `saveAction` runs: `setSaveStatus({kind: "saving"})`.
- On success: `flashSaved()` shows a green check for 1.5s, then back to `idle`.
- On error: `setSaveStatus({kind: "error", message})`. Sticky until the next successful save.

The indicator is a small icon in the header next to `ⓘ`. No red banner. No popup. Just a spinner / check / X icon.

**Implementation:** `SaveStatusIndicator.tsx` + `setSaveStatus` calls scattered through `saveAction`, `linkCredential`, `createGroup`.

### Draft mode for new hosts

`+ Host` does **not** open a dialog. It calls `startDraft(groupId)` which puts a `HostDraft` object into `UiStore.draft`. HostDetail renders the form for that draft. The sidebar shows a "Черновик / Draft" row in italics above the tree.

Once the user fills `hostname` (and only then), the draft is **promoted**: `hostsApi.create` runs, then the draft is cleared and the new host becomes selected. This must happen **without unmounting HostForm** or the input loses focus — see "Focus continuity" below.

### Discard changes confirm

Triggered only when the user has a dirty draft (some field non-empty) but `hostname` is still empty (so it can't be auto-promoted), and they try to navigate away. Shows a confirm dialog with "Discard changes" / "Cancel" actions.

If `hostname` is filled, navigation is silent — the draft is just auto-promoted and the navigation happens against the new real host.

### Hostname validation (silent)

`isValidHostname(s)` in HostDetail.tsx checks against:
- DNS hostname (RFC 1123 labels)
- IPv4 dotted quad
- IPv6 (loose: ≥2 colons)

If invalid, `saveAction` returns early with `idle` status — **no error shown**. The user is just in the middle of typing. As soon as the address parses, save proceeds.

### Credentials in HostForm

Two always-editable inputs: Username + Password. Below them, a button `+ Использовать имеющиеся (SSH-ключ, сохранённый логин...)` (disabled when there are no saved credentials).

- If the host has no `default_credential_id` and both fields are filled — `saveAction` creates a new credential and links it.
- If the host has a linked credential — username is loaded from it (password stays masked, eye button toggles visibility of what the user typed). Changing username → `credApi.update`. Typing a new password → `credApi.rotateSecret`.
- A small chip below shows "Связано с: <name>" when a credential is linked.

**Key invariants:**
1. Never create a credential with one of {username, password} empty. The OS keychain rejects empty secrets and the user would see a "secret must not be empty" error — which is annoying because they just haven't finished typing.
2. **Don't clear the password input after rotateSecret.** The user wants to see their masked dots as proof the password is saved, and to be able to click the eye to verify it. Diff-checking against `committedPasswordRef` prevents re-rotation on every subsequent keystroke.
3. **`committedUsernameRef` / `committedPasswordRef`** track what we've already written to the keychain. `saveAction` only calls `credApi.update` or `credApi.rotateSecret` when the form values differ. Refs are kept in sync at: initial load via the linkedCred-change effect, after successful save, and after a saved credential is picked. Without these refs, every keystroke would trigger another no-op `rotateSecret` call.

### Focus continuity at draft → host promotion

This is the trickiest part of Stage 1.5.2 and **has been the source of multiple bug reports**. The user types "192" in Address. The promotion happens. The user expects to keep typing "192.168.0.12" without re-clicking the input.

For this to work, **HostForm must stay mounted as the same React node** across the promotion. Implementation:

1. `HostForm.saveAction` calls `props.onDraftPromoted(fresh)` instead of `clearDraft + selectHost`.
2. In `HostDetail`, `onDraftPromoted` does `setEditingHost(fresh)` AND `setPromotedId(fresh.id)`.
3. `HostDetail`'s render priority: if `promotedId && editingHost.id === promotedId` → render edit mode immediately, regardless of UiStore. This is the linchpin — it ensures HostForm sees a continuous `host` prop (id flips from `__draft__` to a real id) without ever rendering a different branch.
4. `HostForm`'s `useEffect([props.host.id])` detects the flip from `__draft__` to a real id and **does not** reset form state (`prevHostIdRef`).
5. A `useEffect` in HostDetail then syncs UiStore (`clearDraft + selectHost`) **after** the render with the real host commits.

If you change anything in this flow, test it: `+ Host`, type "192.168.0.12" in one burst, ensure cursor stays in the input.

### Race condition handling

The user can type faster than the create call returns. `promotingRef` blocks concurrent promotions; `pendingDuringPromote` ref captures the last state during a promotion. After `hostsApi.create` returns, a `while (true)` loop applies any pending state via `hostsApi.update` on the new host id before handing off. This prevents losing trailing keystrokes.

**TS quirk:** TS 5.x narrows the ref-derived local to `never` inside the loop after the reassignment. The code uses `const p = pending as FormState;` as a workaround — there's a comment explaining it.

---

## Live invariants

These hold across the whole app. Don't break them.

1. **No raw `invoke()` calls in components.** Everything goes through `lib/ipc.ts`.
2. **Secrets never logged.** `SecretValue` has `zeroize-on-drop`; `#[instrument]` calls have `skip` for secrets.
3. **Coarse mutations.** After any CRUD, reload the full relevant collection. No optimistic patches in stores.
4. **CSS variables in `tokens.css`.** No hex codes in component CSS. Single accent color `#4c8eff`. SSH = green, RDP = blue.
5. **Hairlines, no shadows.** Borders only via `--color-border`; one shadow allowed (popover dropdowns), and that's the maximum.
6. **Discriminated unions for dialogs.** `DialogKind` in `store/index.ts` is the source of truth for what dialogs exist.
7. **ULID newtypes.** `HostId`, `GroupId`, `CredentialId`, `SessionId` — never `String`.
8. **PATCH semantics.** Rust DTOs use `Option<Option<T>>` with custom `deserialize_optional_optional` to distinguish "not in request" from "set to null".

---

## Build & run

User-side (Windows):
```powershell
cd C:\remotehub\ui
pnpm install              # one-time after distributing a fresh archive
cd ..
cargo tauri dev           # starts vite + rust binary
```

Sandbox (Claude side) build verification:
```bash
cd /home/claude/remotehub/ui
npm install --silent
tsc --noEmit              # strict + noUnusedLocals/Parameters
npx vite build            # production bundle test
```

Both must pass green before packaging.

---

## Packaging the archive

```bash
cd /home/claude/remotehub/ui && rm -rf node_modules dist package-lock.json
cd /home/claude
rm -f /mnt/user-data/outputs/remotehub.zip
zip -r -q /mnt/user-data/outputs/remotehub.zip remotehub/ \
  -x '*.DS_Store' '*/target/*' '*/node_modules/*' '*/dist/*'
```

The user unpacks into `C:\remotehub` (overwriting), then runs the commands above.

---

## What's coming in Stage 1.6 (next)

**Settings dialog.** Opens from the gear icon in the sidebar footer (currently disabled with a "coming soon" tooltip).

Contents (per `tauri-api.md` spec):
- Language toggle (EN / RU). Currently switches via `localStorage` only; should persist to backend `Settings.language`.
- Theme (system / light / dark).
- Possibly Connect timeout default, log level, telemetry opt-in.

Wiring:
- `settings_get_all` / `settings_update` IPC commands already exist (placeholders in rh-app).
- Add real Settings store/persistence in rh-storage.
- UI dialog reads `useSettingsStore`, writes via `settings.update`.
- Subscribe to `settings_changed` event to react if changed from another window.

**Tags combobox.** Reuse the `<Combobox>` primitive from `ui/src/components/ui/Combobox.tsx`. Currently tags are a comma-separated input — replace with a multi-pill input with combobox-style suggestions from existing tags.

**Possible quick win — `display_name` column in DB.** Right now we use a heuristic ("if `name === hostname`, treat label as auto-fill") to decide whether to show the label input as empty with a placeholder. A cleaner solution is a nullable `display_name` column in the hosts table, which would require a migration but no schema rewrite. Hold off unless the heuristic causes real problems.

---

## Anti-patterns to avoid

These have come up in development and been pushed back. Don't reintroduce them.

1. **Modal dialogs for editing.** The user explicitly does not want them. Inline editing only. Confirm dialogs are OK for destructive actions (delete).
2. **Card-style read-only credential view.** The user wanted always-editable inputs, not a "linked credential card" with separate edit affordance.
3. **Red error banner.** Errors go in the status indicator with a tooltip. No banners. No popups for fixable errors.
4. **Auto-creating credentials with one field filled.** Wait for both username AND password before any `credApi.create`.
5. **Showing errors for in-progress input.** If the user types "192" and hostname is currently invalid (too short for an IP), just don't save and show idle — not error.
6. **Save buttons.** Termius doesn't have them. We don't either.
7. **Re-using host id as React `key` on form components.** Causes remount on promotion → focus loss.
8. **TS narrowing through `useRef.current`.** Use `as FormState` cast after explicit null check.

---

## i18n notes

Custom implementation in `ui/src/i18n/`. Two locales: `en` (source of truth) and `ru`. The `t(key, vars?)` helper does template interpolation with `{name}` placeholders.

Adding a new string:
1. Add the key to `en.ts` with the English text.
2. Add the same key to `ru.ts` with Russian. If missing, falls back to English silently.
3. Use `const { t } = useT();` and `t("your.key")` or `t("your.key", { name: "value" })` in the component.

Locale auto-detected from `navigator.language` on first load, persisted to `localStorage["remotehub.locale"]`. Stage 1.6 will route this through Settings/backend.

Dates: `formatDate(rfc3339)` from `useT()` — RU = `27.05.2026, 21:15`, EN = `27 May 2026, 21:15`.

---

## Common pitfalls / known gotchas

- **`pnpm install` from project root fails** — `package.json` is in `ui/`, not root. Use `cd ui && pnpm install`. Or rely on `cargo tauri dev` which has `beforeDevCommand` set but doesn't run install.
- **Rust changes require restart of `cargo tauri dev`**. Vite hot-reloads UI changes.
- **TS strict + noUncheckedIndexedAccess.** Be careful with array indexing; use guards.
- **CSS modules need explicit class composition** — `${styles.foo} ${styles.bar}` not nested.
- **lucide-react icons don't accept `title` prop.** Wrap in `<span title="...">`.
- **Zustand's `s` param has no inferred type without proper store types** — but inside the project this works because stores are well-typed; outside (e.g. sandbox without `node_modules`), tsc reports errors. Don't panic.

---

## Decision log

Architectural choices that aren't obvious from the code. Captured here so they don't get re-litigated.

1. **No react-i18next.** Two locales, no plural/genus complexity worth the dep. ~60 lines custom solves it.
2. **No state-management lib beyond Zustand.** Redux would be overkill. Zustand stores in `store/index.ts` cover hosts, groups, credentials, ui (with draft).
3. **`AppState` uses `Arc<dyn HostStore + Send + Sync>`** in Rust for testability — handlers don't see concrete sqlx types.
4. **`keychain-first` create pattern in storage.** Write secret to OS keychain first; if DB insert fails, clean up the orphaned keychain entry. Tested.
5. **`ON DELETE SET NULL` for `hosts.group_id`.** Deleting a group moves hosts to Ungrouped, not deletes them. (User-requested behavior.)
6. **CSS modules over Tailwind.** Tokens in CSS variables; per-component `.module.css` files. No utility-first; explicit names.
7. **No optimistic updates.** Re-fetch after CRUD. Simpler reasoning model; latency is fine for desktop local SQLite.
8. **Rust storage tests with real SQLite + real Windows Credential Manager.** No mocks at this layer. Tests do real I/O and clean up. CI runs on the user's Windows machine.
9. **Frontend tests deferred.** Stage 1.x prioritized end-to-end smoke tests (open the app, do the thing, see the result) over component unit tests. Re-evaluate after Stage 2.

---

## Glossary

- **Host**: a remote server (SSH or RDP).
- **Group**: collection of hosts. Optional `parent_id` for nesting (currently UI shows flat tree).
- **Credential**: a (username, secret) pair. Secret in OS keychain. Linkable to many hosts; one host has at most one `default_credential_id`.
- **Draft**: a new host being filled in the UI. Lives in `UiStore.draft`. Promoted to a real Host once `hostname` is non-empty.
- **Promotion**: the transition of a draft into a real Host record. Triggered by the first valid save; must preserve input focus.
- **Live save**: pattern where every keystroke triggers a debounced backend write. The user never clicks Save.
- **Reveal**: clicking the eye icon on a linked credential's password to see plaintext for 10s.
- **Inline credential**: username/password typed directly into HostForm fields (vs. picked from saved). Auto-creates a credential entry when promoted/saved.

---

## When the user starts a new chat

If you (Claude) are reading this for the first time in a new conversation:

1. The user will probably say something like "продолжаем" or paste a problem.
2. Open this file, the `docs/specs/` directory, and ROADMAP.md.
3. Verify which stage we're on by checking the table above.
4. The user prefers Russian for conversation, English for code/specs/comments. Mirror their language for prose; keep code identifiers in English.
5. Don't ask a wall of clarifying questions. Make reasonable assumptions, state them, code, ship the archive, iterate.
6. **Always test `tsc --noEmit` and `vite build` before packaging.** Multiple bugs have shipped because I (Claude) didn't verify compilation.
7. **Pack the archive to `/mnt/user-data/outputs/remotehub.zip`** with `*/target/*`, `*/node_modules/*`, `*/dist/*` excluded.
8. **Update this file** before packaging if you closed a stage.

The user is technically capable, productive, and direct. Don't over-explain. If you disagree with a decision, push back once with a reason; if they hold their position, go with it.
