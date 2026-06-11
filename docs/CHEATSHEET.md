# RemoteHub — Cheat Sheet (general, detailed)

## RDP picture — state after the refresh/resize session (read before touching RDP rendering)

What is SOLVED and must not regress:
- **Disocclusion "ghosts"** (closed/moved/resized remote window leaves stale pixels): fixed by a **client Refresh Rect PDU** sent periodically from the actor loop. `actor.rs`: `RDP_REFRESH=<ms>` env, **default 250 ms**, `0` disables. Uses `active_stage.encode_static(ShareDataPdu::RefreshRectangle(RefreshRectanglePdu{ areas_to_refresh: [full-screen InclusiveRectangle] }))`. This is what mstsc does; it removes the GFX-AVC-444 group-policy requirement (all paths self-heal now). Verified by user. Works on legacy AND GFX.
- **Connect-time sharpness**: `store.createSession` requests the RDP resolution = **window physical px** (`innerWidth/innerHeight × dpr`, cap 3840×2160), NOT the monitor. Previously it used monitor height → the monitor-sized frame was downscaled into the smaller window → aliasing/"shimmer noise" on detailed wallpaper (mstsc renders at window size → clean). Verified: on connect now clean.
- **Encode**: per-band PNG (`PNG_MAX_AREA=200_000`, `BAND_H=64`); pushing PNG higher made aliasing *worse* (crisp aliases more than JPEG on downscale) — leave it.
- **Mode matrix (updated — user testing):**
  - **legacy + remote AVC-444 policy ON = SHARP + no smudge + comma works.** ← user's best config. Even though our GFX is OFF (so IronRDP decodes, no custom-decoder artifacts), enabling the remote "Prioritize H.264/AVC 444" policy puts the *server* into a high-quality graphics mode and it stops sending the coarse first-pass — so there's nothing for the refresh to "re-smudge". Clean and correct. If the user controls the target, this is the daily driver.
  - legacy + policy OFF = smudge (coarse pass never refined; see (A)) and resize handled by CSS-scale (batch 13).
  - GFX + policy = smooth (AVC video) + sharp.
  - GFX without policy = our Progressive/ClearCodec decode artifacts AND no smoothness gain → not recommended.
- **Keyboard (kbd_hook.rs):** modifier PASSTHROUGH to the local OS is **empirically required for RU comma** (Shift+/). Swallowing all modifiers mstsc-style (tried in a build) BROKE comma — reverted. Do NOT remove the passthrough without a live test. Known downside still open: passed-through Ctrl can fire the app's Ctrl+K search, Alt+Shift can steal focus. Candidate fixes for the fast cycle (untested): swallow Ctrl only while keeping Shift+Alt; or disable the webview's Ctrl+K handler while RDP capture is active.

What is STILL OPEN — TWO separate problems:

### (A) "Smudge" noise at rest + the mouse-hover/click clue  ← NEWEST, highest-value lead
- **Symptoms (user, batch 13):** legacy resize artifacts GONE, smoother, but at rest the screen looks faintly "smudged" (esp. detailed wallpaper / small icons). Two interaction clues:
  1. **Hovering** the mouse over a smudged area snaps it SHARP; moving away → smudge returns. Works in the TOP of the screen, not the bottom.
  2. **Pressing a taskbar icon** redraws it SHARP; releasing → reverts to the smudged version.
- **`RDP_REFRESH=0` test (user):** noise is **noticeably LESS** on both legacy and GFX (but not zero). Ghosts return (expected). → refresh is a **major contributor**, not the whole story.
- **Refined mechanism (best current model):** RDP/RemoteFX sends a coarse frame during activity and a **high-quality "still-image" refinement once a region goes IDLE**. Our aggressive periodic full-screen Refresh Rect (100–250 ms) keeps "touching" the whole screen, so the server treats it as never-idle and **never sends the idle refinement** → stuck on the coarse/smudged pass. `refresh=0` lets the server idle → it sends the crisp refinement → cleaner (but disocclusion ghosts come back, since nothing nudges vacated regions). Any genuine interaction (hover highlight, button press) forces a fresh coarse→refine cycle for that region → momentarily sharp; the next refresh tick (or going idle differently) loses it. Top vs bottom: bottom = taskbar (constantly active → never idle → always coarse); top = static (refinement survives between refresh ticks). The residual noise at `refresh=0` = the connection's base RemoteFX quality / our progressive-upgrade-pass handling.
- **So the real tension:** Refresh Rect removes ghosts but suppresses the server's idle high-quality refinement → smudge. Need to refresh **without** keeping the whole screen "busy".
- **Fix hypothesis (in order of effort / correctness):**
  1. **Region-targeted, on-demand refresh** (correct fix; what mstsc effectively does — it does NOT spam full-screen). Detect a likely disocclusion client-side (a band that changed sharply then went static, plausibly holding stale content) and Refresh-Rect ONLY that band, once. The rest of the screen stays untouched → the server idles → sends crisp refinements everywhere else. Feed a "changed-then-idle" tracker from the diff (`compute_regions*` already lists changed bands per tick). This fixes BOTH ghosts and smudge.
  2. **Gentler / rolling periodic refresh** (stop-gap): a longer interval (≈1500–2000 ms) gives the server idle windows to refine between nudges → less smudge, ghosts clear within ~2 s. Or rolling-band (refresh 1/N of the screen per tick) so the whole screen is never busy at once. NB: the user can tune this RIGHT NOW with `RDP_REFRESH=<ms>` (no rebuild) to find a personal sweet spot — low = ghost-free but smudgy, high = crisp but ghosts linger, 0 = crispest + ghosts.
  3. Check whether a quality cap/connection knob makes the server send a permanently coarse pass (SurfaceCommands flags, RemoteFX quantization, `ConnectionType` — we send `Lan`, good).
- **Likely files:** `rh-rdp/src/actor.rs` (`refresh_interval` loop → region-aware + changed-then-idle tracker), the diff in `compute_regions*`, possibly `gfx.rs` progressive upgrade-pass for the GFX residual.

### (B) Windowed resize/drag corruption (reflow) — currently mitigated, not fixed
- Continuous DisplayControl reflow corrupted on resize (IronRDP #447). **Mitigation shipped (batch 13):** `enableDynamicResize={false}` → window resize just CSS-scales the connect-size frame (mildly soft, readable, NOT corrupt); fullscreen one-shot resize kept. User confirmed batch 13 = legacy artifacts gone, smoother.
- **Proper fix (planned, fast PC/NAS cycle, ~3–6 live iterations):** debounce window-resize (~300 ms after STOP) → ONE clean DisplayControl resize: clear `last_frame` + re-init framebuffer/surface at new size + emit `Resized` + force full repaint. Re-enable `enableDynamicResize` gated on this. Tricky: geometry desync + stale fb.
- **Likely files:** `rh-rdp/src/actor.rs` (resize/ResetGraphics, `last_gfx_size`, clear paths), `gfx.rs` (surface realloc), `RdpViewport.tsx` (ResizeObserver→onResize debounce), `store/index.ts`.

### (B) Windowed resize/drag corruption (reflow) — currently mitigated, not fixed
- Continuous DisplayControl reflow corrupted on resize (IronRDP #447). **Mitigation shipped (batch 13):** `enableDynamicResize={false}` → window resize just CSS-scales the connect-size frame (mildly soft, readable, NOT corrupt); fullscreen one-shot resize kept. User confirmed batch 13 = legacy artifacts gone, smoother.
- **Proper fix (planned, needs the fast PC/NAS build cycle, ~3–6 live iterations):** debounce window-resize (~300 ms after the user STOPS) → ONE clean DisplayControl resize: clear `last_frame` + re-init the framebuffer/surface at the new size + emit `Resized` (canvas matches) + force a full repaint. Re-enable `enableDynamicResize` gated on this clean path. Tricky bits: geometry desync + stale fb. Painful on 20-min blind builds, minutes on the PC cycle — do it there.
- **Likely files:** `rh-rdp/src/actor.rs` (resize/ResetGraphics, `last_gfx_size`, clear paths), `gfx.rs` (surface realloc), `RdpViewport.tsx` (ResizeObserver→onResize debounce), `store/index.ts`.

Keyboard fixes (shipped): the fullscreen `kbd_hook.rs` now **swallows all keys** (mstsc-style) and forwards scancodes; removed the modifier→local-OS passthrough that leaked Ctrl into the webview (Ctrl+K search) and Alt into focus (Alt+Shift focus-steal). Remote layout still switches via forwarded Alt+Shift scancodes; RU comma (Shift+/) is composed server-side. If comma ever regresses, the bug is remote-layout-switch via forwarded scancodes, NOT local layout — do not reintroduce local passthrough.

vSphere/VMware browser console feels smoother than our RDP because it streams the hypervisor framebuffer as video (VNC/H.264 over WebSocket), not RDP into the guest — a fundamentally easier problem. Not a fair comparison; don't chase it.

---

Cross-platform desktop **SSH + RDP + SFTP** client with local terminal. Windows-first; macOS/Linux follow architecturally. "Termius with RDP" (do NOT use the name "Termius" in user-facing copy). Local-first, dense UI, live-save everywhere, OS keychain for secrets, two-pane SFTP, tabbed sessions, system tray (close-to-tray keeps sessions alive).

Solo dev **kolen** on **Windows 11**. Russian in chat, English in code/comments/specs. Concise. Ships a verified change + fresh archive each turn.

## Environment / repo
- PowerShell, **Rust 1.95** (edition 2024, rust-version 1.80), **Node v24**, **pnpm**, Tauri CLI 2.x.
- Windows Credential Manager service `"RemoteHub"`. DB at `%APPDATA%\RemoteHub\remotehub.db`.
- Repo: sandbox `/home/claude/remotehub`, machine `C:\remotehub`, GitHub `github.com/kkolentionok/remotehub`.
- Test endpoints: SSH/SFTP `root@89.23.99.57` (password). RDP `5.42.106.222:3389` and `89.23.99.57` (`Administrator`). **Never commit passwords.**
- Sandbox has **no cargo** → user compiles Rust on Windows and reports errors; I verify the **frontend** only when touched.

## Stack
- **Shell**: Tauri 2 — `#[tauri::command]` IPC; `AppHandle`/`Channel<T>` events; `tray-icon`.
- **Backend (Rust)**: Tokio + **sqlx** (SQLite; hosts use runtime `query()`+`bind`/`try_get`, rest compile-checked `query!`) + **keyring-rs 3.6** + **russh 0.45** + **russh-sftp 2.x** + **IronRDP 0.14** + **portable-pty**. Crypto **aws-lc-rs**. Errors **thiserror** in libs, `anyhow` only at rh-app edge. Logs **tracing** (`#[instrument]`, skip secrets).
- **Frontend**: React 18 + TS strict + Vite 5 + **Zustand** + **lucide-react** + **CSS Modules** (no Tailwind). Custom i18n (`t(key,vars?)`, EN+RU; key type derived from `en.ts` → missing key = tsc error).

## Build / verify / package (every turn)
Frontend verify (only if `ui/` touched):
```bash
cd /home/claude/remotehub/ui && npm install && npx tsc --noEmit && npx vite build
```
Package (zip CONTENTS → entries are `crates/...`, NOT `remotehub/...`):
```bash
cd /home/claude/remotehub && rm -f /home/claude/remotehub.zip /mnt/user-data/outputs/remotehub.zip && \
zip -rq /home/claude/remotehub.zip . \
  -x '*/node_modules/*' -x 'node_modules/*' -x '*/target/*' -x 'target/*' \
  -x '*/.git/*' -x '.git/*' -x 'ui/dist/*' -x 'ui/.vite/*' \
  -x 'Cargo.lock' -x '*/Cargo.lock' -x '*/package-lock.json' -x '*/pnpm-lock.yaml'
```
Then `present_files(["/home/claude/remotehub.zip"])`. **`.git` is excluded — user commits on their machine.**
User install: stop dev, `Expand-Archive remotehub.zip -DestinationPath C:\ -Force`, then `cd C:\remotehub\ui; pnpm install; cd ..; cargo tauri dev`.

## Crate map (`crates/`)
- `rh-core/` — domain types, IDs (HostId/GroupId/CredentialId/SessionId = ULID newtypes), `SecretValue` (zeroize, redacted Debug), `Protocol`, `Settings` (Theme: Light/Dark/Navy(default)/Redpanda/System), `Host` (favorite, agent_forwarding, jump_host_id, last_connected_at), errors, KnownHostsStore. NO tokio/sqlx/tauri.
- `rh-storage/` — sqlx SQLite + keyring. `*Store` traits + Sqlite impls. `db.rs` (inline migration chain, `CURRENT_SCHEMA_VERSION`, latest **v9** `favorite`; fresh DB runs `migrations/v1.sql` = complete current schema, existing DBs run the chain; additive ALTER/CREATE only). host_store (runtime SQL), settings/known_hosts/rdp_cert/credential/group stores.
- `rh-ssh/` — russh client + session actor (`actor.rs`: TOFU known_hosts, agent auth, fingerprint), `ppk.rs`, `sftp.rs` (`SftpConn`: list, download/upload/copy_stream with resume `offset`, size, chmod, rename, remove, mkdir).
- `rh-rdp/` — IronRDP RDP. `actor.rs` (3-thread pipeline), `lib.rs` types, `clearcodec.rs`, `progressive.rs`, `nscodec.rs`, `gfx.rs` (see GFX section).
- `rh-app/` — Tauri binary. `state.rs` (AppState), `main.rs` (generate_handler! + close-to-tray + tray::build), `tray.rs`, `local_pty.rs`, `session.rs` (SSH SessionManager hub), `sftp_session.rs`, `rdp_session.rs`, `paths.rs`, `api/{hosts,groups,credentials,settings,sessions,rdp_sessions,local_sessions,local_fs,sftp_sessions,dto,error,events,meta}.rs`. `icons/` brand (dark variant).

## UI map (`ui/src/`)
- `components/host/HostDetail.tsx` (~1150 lines, live-save + draft promotion; FormHeader favorite star), `sidebar/Sidebar.tsx`, `session/{SessionView,Terminal,RdpViewport}.tsx`, `sftp/SftpView.tsx` (two-pane commander), `layout/{AppShell,TabBar,HomeView,ToolsView,PaneGroup,DialogHost}.tsx`, `settings/sections/{Appearance,Terminal,Connections}.tsx`, `dialog/`, `ui/` primitives.
- `store/index.ts` (zustand stores + SessionTab + DialogKind + restoreSessions), `lib/{ipc.ts,types.ts,useDebouncedCallback.ts}`, `i18n/{en,ru,index}.tsx`, `styles/{tokens.css,fonts.css}`.

## Hard rules / invariants
**Backend:** ULID newtypes (never String/Uuid; serialize via Display/FromStr) · no secrets in SQLite (OS keychain only) · keychain-first create (write secret, then DB row; on fail delete keychain entry) · trait-based storage (`Arc<dyn HostStore + Send + Sync>`) · PATCH = `Option<Option<T>>` + `deserialize_optional_optional`, `deny_unknown_fields` on every DTO · coarse mutations (return nothing/new id; UI refetches) · no anyhow in libs, no unwrap outside tests · migrations are versioned chain · long-lived tokio::spawn needs shutdown (actor: mpsc + select! + cancel/AtomicBool).
**Frontend:** no raw `invoke()` in components (everything via `lib/ipc.ts` + DTO mirror in `lib/types.ts`) · stores own collections, components query via selector · no optimistic updates (refetch) · CSS Modules + tokens.css vars only · i18n every visible string (both en.ts + ru.ts) · never use `host.id` as React key on the host form (remount → focus loss on draft→real promotion) · `tsc --noEmit && vite build` green before packaging.
**Design (Termius/Linear/Raycast feel):** one accent `#4c8eff` (Redpanda theme = coral `#f0552f`); protocol colors only in badge (SSH green, RDP blue); hairline borders; shadows only on popovers; radii ≤8 (dialogs 12); mono for IDs/paths/sizes/perms; no gradients/hero banners/700-weight. Selection IS edit (no modal edit, no Save buttons — live-save + status indicator). Confirm only for destructive irreversibles. Themes via AppShell `data-theme`.

## Established patterns
- **IPC command:** DTO in `api/dto.rs` (deny_unknown_fields) → handler `pub async fn x(state: State<'_,AppState>, req) -> ApiResult<…>` `#[instrument(skip(state))]` → register in `main.rs generate_handler!` → mirror in `lib/types.ts` → wrapper in `lib/ipc.ts`.
- **Storage method:** trait in `store.rs` → Sqlite impl → migration const + bump version + update v1.sql → test with `tempfile::tempdir` + real keychain (`RemoteHubTest_<rand>`).
- **Live-save (HostDetail):** typing → debounced save (400ms / 1000ms notes); `SaveStatusIndicator` idle/pending/saving/saved-1.5s/error-sticky (never a banner). Bool toggles persist immediately + flashSaved().
- **Draft→real focus continuity (don't break):** `+Host` makes UI-only draft; typing hostname auto-promotes WITHOUT losing focus. `HostDetail` holds `editingHost`+`promotedId`, renders edit by `promotedId`; `HostForm useEffect([host.id])` skips reset on `__draft__`→real. Test: `+Host`, type `192.168.0.12` in one burst, cursor must not leave Address.
- **Sessions + restore-on-reload:** SSH `SessionManager` + local `LocalPtyManager` = hubs (output ring 256KiB + swappable sink Channel + list()/reattach()). Rust process survives webview reload; `store.restoreSessions()` rebuilds tabs + replays scrollback. RDP/local share the SSH event/command contract.
- **SFTP transfers:** `sftp_transfer({transfer_id,kind,session_id,to_session?,src_path,dst_dir,dst_name?,resume}, on_progress: Channel<u64>)`; 256KiB chunks, cancel AtomicBool, byte-resume via `offset`. Frontend `useTransfers()`: max 2 parallel, retry/cancel/clearDone.
- **Tray + close-to-tray:** `main.rs on_window_event` intercepts CloseRequested on `main` → prevent_close + hide. Real quit via tray Quit. `tray.rs` menu (Open/Favorites/Recent/Groups/Quit), rebuilds on hosts/groups changed; host click emits `tray:connect`.
- **Events:** CRUD emits `hosts:changed`/`groups:changed`/`credentials:changed`/`settings:changed`. Session output/RDP frames/SFTP progress over Tauri `Channel<T>`; rh-ssh/rh-rdp emit over mpsc, rh-app forwards (libs never depend on tauri).

## Feature status (live-verified on Windows unless noted)
- **Stages 1.x** ✅ SQLite+keychain+IPC+dense UI; host/group/credential CRUD, live-save, draft mode, i18n, settings dialog + language toggle.
- **SSH** ✅ session actors (russh), PTY+scrollback, restore-on-reload, TOFU/known_hosts + UI, agent auth, env passthrough, keepalive (socket2 TcpKeepalive frozen-disconnect fix), OS auto-detect, last-connected, ProxyJump, agent-forwarding request-only (serving side = backlog).
- **SFTP** ✅ two-pane commander: endpoint switcher (local/hosts/"This PC" drives), editable breadcrumb path, sort, multi-select, hidden toggle, RU sizes/dates, perms; transfers via rail/double-click/DnD/context-menu; streaming queue (progress/speed/ETA/cancel, max 2, retry+byte-resume), name-conflict dialog, search-filter, rename, delete, mkdir, chmod; TOFU pinning, agent auth, host↔host stream copy.
- **Local terminal** ✅ portable-pty, shell-choice setting, restore-on-reload.
- **Tools credential manager** ✅ reveal-on-click + copy, only linked creds.
- **Account & Sync** ✅ E2E sync via server **pingie.ru** (crate `server/` = `rh-sync-server`, Docker on VPS). Email/password + **Yandex OAuth** (Google deliberately omitted). Vault sealed with a master password (entered once via modal; kept in keychain or session-mem). **Automatic** sync (`sync_engine` actor: 30s interval + wake-on-edit; no manual "Sync now"). UI = redesigned ProfileSection (login/register w/ pw-strength + inline match, OAuth redirect screen, status card w/ counts). **Logout PURGES the local vault** (hosts/groups/creds + keychain secrets + **`sync_meta` tombstones**) so accounts can't bleed into each other / replay cross-account deletes — data is server-side and returns on re-login. Sync/auth errors localized via `ui/src/lib/syncErrors.ts` (`settings.sync.err.*`).
- **Themes** ✅ System/Light/Dark/Navy(default)/Redpanda. **Tray + close-to-tray** ✅. **Brand icons** ✅. **Favorites** ✅ (v9, header star, tray submenu). **Tab bar** ✅ horizontal scroll. **Storage scope switcher** ✅ UI seam (Personal active / Team locked).
- **RDP** ✅ legacy path: connect/auth/graphics/mouse/keyboard (+modifier-sync), pop-out window, states screen, inline re-auth, server cursor, clipboard (incl. images), aspect-ratio, fullscreen, cert TOFU. **GFX path** — see below.
  - **Connect troubleshooting:** IronRDP requires **Enhanced security** — we set `enable_credssp: true` + drive our own TLS (`build_config`, `actor.rs`). Error `negotiation failure: server only supports Standard RDP Security` ⇒ the *target* offers only legacy RDP Security (RC4), which IronRDP doesn't implement. Fix on the **server/target**, not the client: enable NLA + TLS. Win Pro/host: System Properties → Remote → "allow only NLA"; or registry `HKLM\SYSTEM\CurrentControlSet\Control\Terminal Server\WinStations\RDP-Tcp` → `SecurityLayer=2` (SSL/TLS; 0=Standard,1=Negotiate), `UserAuthentication=1` (NLA), restart `TermService`. **Windows Home has no RDP host** → people use RDP Wrapper, which often ends up Standard-only. (Username/MS-account format is irrelevant here — failure is *before* auth.)

## RDP GFX pipeline (H.264/RemoteFX) — current focus
Opt-in via env `RDP_GFX=1` (read in `actor.rs` ~L222; never set in repo). Default = proven legacy IronRDP path. Custom decode: connect → graphics → input. 3 threads (worker std-thread + off-thread rayon encoder + tokio cmd), self-correcting fb-diff emit (`compute_regions_raw`: band diff of st.fb vs last_frame). Caps advertised V8/8.1/10/10.2/10.3/10.4; server confirms V10_4 (Progressive + bitmap cache + ClearCodec; AVC unused).
Codecs in `crates/rh-rdp/src/`: `progressive.rs` (RemoteFX Progressive FIRST/UPGRADE, DWT, RLGR, SRL), `clearcodec.rs` (glyph/vBar caches, residual, bands, subcodecs RAW/NSCodec/RLEX), `nscodec.rs` (NEW — MS-RDPNSC).
**Fixes this session (chronological):** fb-diff emit · CreateSurface preserve · Progressive `comp.current` baseline ALWAYS stored (diff tiles) — fixed white/gray blocks · removed DelEncCtx `prog.reset()` (caused neutral-gray 128 tiles) · Progressive `comp.sign` from accumulated coeffs — fixed thin edge artifacts · ClearCodec empty full-vBar → band bg color (not black) · **NSCodec decoder** (port of FreeRDP nsc.c) — server uses NSCodec as primary ClearCodec subcodec for all UI; was skipped → black blocks + line artifacts. Now clean.
**Key facts:** RDPGFX_RECT16 is HALF-OPEN (width=right−left; IronRDP misnames it InclusiveRectangle) — applies to ClearCodec rawrect AND S2S source_rectangle · gray (128,128,128) = neutral YCbCr (zero coeffs) · ClearCodec caches VBAR=32768/SHORT=16384/GLYPH=4000 (ring, in lockstep with server); glyph cache reads back the surface so any wrong tile replicates · surface init = black (0,0,0).
**Trace** (`RDP_GFX_TRACE=1`, env, off by default):
```
TRACE S2S src=A->dst=B srcrect=(x,y,w,h) dst_pts=(x,y),...
TRACE Cache2S slot=N surf=S center_rgb=(r,g,b) dst_pts=(x,y),...
TRACE SolidFill surf=S rgb=(r,g,b) rects=(x,y,w,h),...
TRACE Wts1 surf=S codec=ClearCodec|Uncompressed dst=(x,y,w,h)
TRACE Wts2 prog surf=S tiles=N bbox=(x,y,w,h)
```
**REMAINING (next step):** one persistent artifact — a band at the TOP of a window after maximize→restore (disocclusion: vacated region keeps stale content in st.fb; server doesn't recorrect). Predates NSCodec. NEXT: capture one maximize→restore with RDP_GFX_TRACE=1, inspect which op (if any) repaints the vacated top band → fix mapping, or if none covers it the divergence is upstream (our S2S/decode). Then FINALIZE GFX: quiet diagnostic logs (1/s op-counter + startup S2Cache/Cache2S/Wts1) leaving trace under env; update STATE.md + rdp-pipeline.md.

## Anti-patterns (rejected — don't reintroduce)
Modal edit dialogs · card-style read-only credential views (always two editable inputs + eye toggle) · red error banners (status icon + tooltip) · auto-saving partial credentials (need BOTH user+pass) · errors on in-progress invalid input (stay idle) · Save buttons · `host.id` as form key · hero banners/gradients/decorative shadows · auto-generated "explanations" after delivery · stray focus outlines · naming "Termius" in user-facing copy.

## Behaviour
Russian to user, English in code/comments/specs. Concise: gist + what changed + how to run; no "Great question!", no restating, no postamble after archive. Don't ask a wall of questions — assume, state, code, verify, ship, iterate; one question only if genuinely blocked. Flag protocol-API risk (russh/russh-sftp/IronRDP/Tauri) in the summary; spike unproven APIs first. Push back once on invariant violations, comply with a comment if held with new context. Update STATE.md (prepend "Latest —") when a feature lands.

## Backlog (priority)
1. **Finish GFX** (top-of-window disocclusion → finalize). 2. RDP GFX polish: dynamic resize (DisplayControl DVC), higher fps. 3. Profiles/account/sync (needs E2E-encrypted vault + auth model + conflict resolution; "Team" scope is the UI seam). 4. SSH agent-forward serving side (spike). 5. Hotkeys / command palette (Ctrl/Cmd+K, J/K, `?`). 6. **Auto-updater** (Tauri updater, self-host pingie.ru: sign keypair + `createUpdaterArtifacts` + `plugins.updater` endpoint `latest.json` + nginx `/updates/` + `release.ps1`; until then manual reinstall — see `docs/ROADMAP.md`). 7. Import PuTTY/MobaXterm. 8. Micro-polish: distro icon in tab, live-session tray hint, active-transfers badge, confirm-on-Quit if live sessions.

## Glossary
draft = UI-only unsaved new host · promotion = draft→real once valid · live-save = debounced autosave · TOFU = trust-on-first-use key/cert pinning · actor/hub = supervised task owning a session (ring + swappable sink + cmd channel + cancel) · reattach = rebind live session to fresh UI Channel after reload · endpoint/point = SFTP panel source · rail = center column with →/← transfer buttons · This PC = drives view (`fs_drives`) · scope = Personal/Team storage switcher (Team = future sync) · GFX = opt-in RDPGFX decode pipeline (`RDP_GFX=1`).
