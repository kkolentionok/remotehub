# RemoteHub — Project Context (always-on briefing)

Read this first in any chat in this project. It's the "how to work + what exists" briefing. The blow-by-blow log lives in `docs/STATE.md` (read it when picking up active work); deep feature docs in `docs/specs/*.md`. This file is orientation.

---

## What it is

Cross-platform desktop **SSH + RDP + SFTP** client with a **local terminal**. Windows-first; macOS/Linux follow architecturally. Local-first, dense modern UI, live-save everywhere, OS keychain for secrets, a real two-pane SFTP explorer, a tabbed session shell, and a **system tray** (closing the window hides to tray so sessions survive).

Solo developer on **Windows 11** (user `kolen`), Russian-first, technically strong, direct. Wants concise Russian in chat, English in code/comments/specs. Iterates fast; expects a visible, verified change + a fresh archive each turn.

Environment: PowerShell, **Rust 1.95** (edition 2024), **Node v24**, **pnpm**, Tauri CLI 2.x. Windows Credential Manager service `"RemoteHub"`. DB at `%APPDATA%\RemoteHub\remotehub.db`. Repo: sandbox `/home/claude/remotehub`, machine `C:\remotehub`, GitHub `github.com/kkolentionok/remotehub`.

**Test endpoints** (user-provided): SSH/SFTP `root@89.23.99.57` (password auth). RDP `5.42.106.222:3389` (`Administrator`). Don't commit passwords.

---

## Stack

- **Shell**: Tauri 2. `#[tauri::command]` IPC; `AppHandle`/`Channel<T>` events; `tray-icon` feature for the tray.
- **Backend**: Rust + Tokio + **sqlx** (SQLite, runtime `query()` with `.bind()`/`try_get` for hosts; compile-checked `query!` elsewhere) + **keyring-rs 3.6** + **russh 0.45** (SSH) + **russh-sftp 2.x** (SFTP) + **IronRDP 0.14** (RDP) + **portable-pty** (local terminal). Crypto: **aws-lc-rs**. Errors: **thiserror** in libs, `anyhow` only at the `rh-app` edge. Logs: **tracing** (`#[instrument]`, `skip` secrets).
- **Frontend**: React 18 + TypeScript **strict** + Vite 5 + **Zustand** + **lucide-react** + **CSS Modules** (no Tailwind). Custom i18n (`t(key, vars?)`, EN+RU; the key TYPE is derived from `en.ts` → a used key missing in `en.ts` is a tsc error).

---

## Build, verify, package (the loop every turn)

Sandbox has **no `cargo`** → the user compiles Rust on Windows. I verify the **frontend** in the sandbox and ship the archive; the user reports Rust compile errors and I fix them.

**Frontend verify (mandatory before packaging):**
```bash
cd /home/claude/remotehub/ui
npm install            # ALWAYS first — packaging deletes node_modules; tsc without
                       # it throws phantom "cannot find module react" errors. Not a bug.
npx tsc --noEmit       # zero errors required
npx vite build         # must succeed
```

**Package (every turn):**
```bash
cd /home/claude/remotehub/ui && rm -rf node_modules dist package-lock.json
cd /home/claude
rm -f /mnt/user-data/outputs/remotehub.zip
zip -r -q /mnt/user-data/outputs/remotehub.zip remotehub/ \
  -x '*.DS_Store' '*/target/*' '*/node_modules/*' '*/dist/*' '*/.git/*' \
     '*/Cargo.lock' '*/package-lock.json' '*/pnpm-lock.yaml'
```
Then `present_files`. **`.git` is excluded** — the user commits on their machine.

**User's install/run:** stop `cargo tauri dev`, `Expand-Archive remotehub.zip -DestinationPath C:\ -Force` (clean overwrite — a mid-extraction file-watcher rebuild once caused a half-updated tree / "method takes N arguments" mismatches), then `cd C:\remotehub\ui; pnpm install; cd ..; cargo tauri dev`.

---

## Hard rules / invariants (don't break)

**Backend**
1. **ULID newtypes** — `HostId/GroupId/CredentialId/SessionId(Ulid)`. Never expose `String`/`Uuid`. Serialize via `Display`/`FromStr`.
2. **No secrets in SQLite** — OS keychain (keyring-rs) only. `SecretValue` zeroizes; `Debug` prints `<redacted N bytes>`; list secret fields under `#[instrument(skip = ...)]`.
3. **Keychain-first create** — write secret to keychain, then DB row; on DB failure delete the keychain entry (no orphans). Tested.
4. **Trait-based storage** — handlers depend on `Arc<dyn HostStore + Send + Sync>`, never a concrete store.
5. **PATCH = `Option<Option<T>>`** with `deserialize_optional_optional`; `#[serde(deny_unknown_fields)]` on every DTO; bool/simple fields use `Option<T>` + `#[serde(default)]`.
6. **Coarse mutations** — a mutation returns nothing / the new id; the UI refetches. No partial/optimistic payloads.
7. **No `anyhow` in libs**; no `unwrap()` outside tests. **Migrations**: `db.rs` holds `CURRENT_SCHEMA_VERSION` + `MIGRATIONS: &[(from, sql)]` inline-const chain; a fresh DB runs `migrations/v1.sql` (kept as the *complete current* schema), existing DBs run the chain. Additive `ALTER/CREATE` only. (Latest schema = **v9**, `favorite` column.)
8. Long-lived `tokio::spawn` needs a shutdown path (actor: mpsc + select! + cancel/AtomicBool).

**Frontend**
1. **No raw `invoke()` in components** — everything through `ui/src/lib/ipc.ts` (+ DTO mirror in `lib/types.ts`).
2. **Stores own collections; components query them** (`useHostsStore.items`, `useGroupsStore`, `useCredentialsStore`; `useUiStore` for view state). Always use a selector.
3. **No optimistic updates** — refetch after a mutation.
4. **CSS Modules + `tokens.css` vars only** — no Tailwind, no hex in component CSS, no inline styles except genuinely dynamic values (e.g. computed popover coords, `fill` on a toggled icon).
5. **i18n every visible string** (`t("key")`); add to both `en.ts` and `ru.ts`.
6. **Never use `host.id` as a React `key` on the host form** — remounts on draft→real promotion and loses focus.
7. **`tsc --noEmit && vite build` green before packaging** — non-negotiable.

**Design (power-tool aesthetic — Termius/Linear/Raycast feel; do NOT name Termius in user-facing copy)**
- One accent `#4c8eff` (Redpanda theme overrides it to coral `#f0552f`). Protocol colors only in the badge (SSH green, RDP blue). Hairline borders; shadows only on popovers (`--shadow-pop`). Radii ≤ 8 (dialogs 12). Mono for IDs/paths/sizes/perms. No gradients/hero banners/700-weight.
- **Selection IS edit** (no modal edit, no Save buttons — live-save + status indicator). Confirm only for destructive irreversibles.
- **Themes**: `:root` = Dark; `[data-theme="navy"]` (**default**), `[data-theme="redpanda"]`, `[data-theme="light"]`, `system` = OS. Applied via `AppShell` `data-theme`. Tokens in `ui/src/styles/tokens.css`.

---

## File map (where things live)

```
crates/
  rh-core/     domain types, IDs, SecretValue, Protocol, Settings (Theme: Light/Dark/Navy(default)/Redpanda/System),
               Host (incl. favorite, agent_forwarding, jump_host_id, last_connected_at), errors, KnownHostsStore. NO tokio/sqlx/tauri.
  rh-storage/  sqlx SQLite + keyring. *Store traits + Sqlite impls. db.rs (migration chain, v9). host_store (runtime SQL),
               settings_store, known_hosts_store, rdp_cert_store, credential_store, group_store. migrations/v1.sql.
  rh-ssh/      SSH client + session actor (actor.rs: TOFU known_hosts handler, agent auth, fingerprint_sha256 pub(crate)),
               ppk.rs, sftp.rs (SftpConn: TOFU handler, agent auth, list, download/upload/copy_stream with resume `offset`,
               size, chmod, rename, remove, mkdir; free fn copy_stream). SshError.
  rh-rdp/      RDP via IronRDP: actor.rs (3-thread pipeline), lib.rs types. Mouse done; KEYBOARD NOT wired. examples/rdp_spike.rs.
  rh-app/      Tauri binary. state.rs (AppState), main.rs (generate_handler! + on_window_event close-to-tray + tray::build),
               tray.rs (tray icon + menu: Open/Favorites/Recent/Groups/Quit; emits `tray:connect`), local_pty.rs (PTY +
               ring/sink/list/reattach restore), sftp_session.rs (SftpManager + cancel registry), rdp_session.rs, session.rs
               (SSH SessionManager hub: ring/sink/list/reattach), paths.rs,
               api/{hosts,groups,credentials,settings,sessions,rdp_sessions,local_sessions,local_fs,sftp_sessions,dto,error,events,meta}.rs
  rh-app/icons/  brand icons (32/128/128@2x/icon.ico/icon.icns/icon.png) — generated from rhub-icon-dark.svg (DARK variant).

ui/src/
  components/
    host/HostDetail.tsx        main pane (~1150+ lines) — live-save + draft promotion; FormHeader has the favorite star toggle
    sidebar/Sidebar.tsx        grouped host tree + draft row
    session/{SessionView,Terminal,RdpViewport}.tsx
    sftp/SftpView.tsx (+ .module.css)  two-pane commander: panels, transfers (retry+byte-resume), chmod dialog, editable path
    layout/{AppShell,TabBar,HomeView,ToolsView,PaneGroup,DialogHost}.tsx  (TabBar: horizontal tab scroll + storage-scope dropdown)
    settings/sections/{AppearanceSection(themes),TerminalSection,ConnectionsSection}.tsx, dialog/, ui/ (primitives)
  store/index.ts               zustand stores + SessionTab + DialogKind + restoreSessions (SSH + local)
  lib/{ipc.ts,types.ts,useDebouncedCallback.ts}
  i18n/{en,ru,index}.tsx       useT() → {t, locale, setLocale}
  styles/{tokens.css,fonts.css}

docs/STATE.md                  progress log (newest on top) — UPDATE when a feature lands
docs/specs/{system-overview,data-model,tauri-api,session-protocol,ssh-session,rdp-session,rdp-pipeline,sftp}.md
```

---

## Established patterns (with examples)

**Adding an IPC command:** DTO in `api/dto.rs` (`deny_unknown_fields`) → handler `pub async fn x(state: State<'_, AppState>, req: XReq) -> ApiResult<…>` `#[instrument(skip(state))]` → register in `main.rs generate_handler!` → mirror type in `lib/types.ts` → wrapper in `lib/ipc.ts`.

**Storage method:** add to the trait in `rh-storage/src/store.rs` → impl on the Sqlite store → migration const + chain entry + bump `CURRENT_SCHEMA_VERSION` + update `v1.sql` if schema changes → test with `tempfile::tempdir` + real keychain.

**Live-save (HostDetail):** typing schedules a debounced save (400ms / 1000ms notes); `SaveStatusIndicator` shows idle/pending/saving/saved-1.5s/error-sticky (never a banner). Boolean toggles (agent-forwarding, favorite) persist immediately via `hostsApi.update({ id, field })` + `flashSaved()`.

**Draft→real focus continuity (don't break):** `+Host` makes a UI-only draft; typing a hostname auto-promotes WITHOUT losing input focus. `HostDetail` holds `editingHost`+`promotedId`; renders edit mode by `promotedId`; `HostForm` `useEffect([host.id])` skips reset on `__draft__`→real.

**Sessions + restore-on-reload:** SSH `SessionManager` (session.rs) and local `LocalPtyManager` (local_pty.rs) are hubs: per-session output ring (256 KiB) + swappable `sink` Channel + `list()`/`reattach()`. The Rust process survives a webview reload; `store.restoreSessions()` lists SSH **and** local sessions, rebuilds tabs, reattaches (replays scrollback). RDP/local share the SSH `SshSessionEvent`/`SessionCommand` contract.

**SFTP transfers:** `sftp_transfer({transfer_id, kind, session_id, to_session?, src_path, dst_dir, dst_name?, resume}, on_progress: Channel<u64>)`. Streamed 256 KiB chunks, cancel `AtomicBool` in `SftpManager`. **Byte-resume**: `resume:true` → backend computes the destination's current size (local `fs::metadata` / remote `SftpConn::size`) and passes `offset` to `download/upload/copy_stream` (seek source, append dest). Frontend `useTransfers()`: max 2 parallel, retry (↻, sets `resume`)/retryAll/cancel/clearDone.

**Tray + close-to-tray:** `main.rs on_window_event` intercepts `CloseRequested` on the `main` window → `prevent_close` + `hide` (sessions/mounts survive). Real quit via tray **Quit**. `tray.rs` builds the menu (Open / Favorites / Recent / Groups / Quit), rebuilds on `hosts:changed`/`groups:changed`, and a host click emits `tray:connect <host_id>`; `AppShell` listens and calls `sessions.open(host)`.

**Events:** CRUD emits `hosts:changed`/`groups:changed`/`credentials:changed`/`settings:changed`. Session output / RDP frames / SFTP progress flow over a Tauri `Channel<T>`; rh-ssh/rh-rdp emit over mpsc and `rh-app` managers forward (libs never depend on tauri).

---

## Feature status (all live-verified on Windows unless noted)

- **Stages 1.x** ✅ — SQLite + keychain + IPC + dense UI: host/group/credential CRUD, live-save, draft mode, i18n, settings dialog + language toggle.
- **SSH (Stage 2) + hardening** ✅ — session actors (russh), PTY + scrollback, restore-on-reload, TOFU/known_hosts + management UI, SSH-agent auth, env passthrough, keepalive, OS auto-detect, last-connected, **ProxyJump**, agent-forwarding **request-only** (serving side = backlog/spike).
- **RDP (Stage 4)** ✅ — IronRDP 3-thread actor, region-diff, PNG/JPEG hybrid, native-res + fullscreen, RDP cert TOFU, **keyboard + modifier-sync done**, clipboard, server cursor, pop-out, inline re-auth. Opt-in **GFX** (H.264/RemoteFX) pipeline behind `RDP_GFX=1`. Doc: `docs/specs/rdp-pipeline.md`.
  - **Connect gotcha:** we negotiate **Enhanced security** only (`enable_credssp: true` + own TLS, `actor.rs build_config`). `negotiation failure: server only supports Standard RDP Security` ⇒ the target offers only legacy RC4 RDP Security (IronRDP won't do it). Fix the **target**: enable NLA + TLS (`SecurityLayer=2`, `UserAuthentication=1` under `…\WinStations\RDP-Tcp`, restart TermService; or System Properties → Remote → "only NLA"). Windows **Home** has no RDP host (RDP Wrapper → often Standard-only). Username format is irrelevant — it fails before auth.
- **Local terminal** ✅ — real PTY (portable-pty), shell-choice setting, **restore-on-reload** (ring + reattach).
- **Tools credential manager** ✅ — reveal-on-click + copy; only linked creds.
- **SFTP explorer** ✅ — two-pane commander: endpoint switcher (local / hosts / "This PC" drives), breadcrumbs (**editable path field**), sort, multi-select, hidden toggle, RU sizes/dates, perms. Transfers via rail/double-click/DnD/context-menu; **streaming queue** (progress/speed/ETA/cancel, max 2 parallel, **retry + byte-offset resume**), name-conflict dialog (Replace/Keep both/Skip), search-filter, rename, delete, new folder, **chmod** (rwx dialog). TOFU key pinning (silent), agent auth, streaming host↔host copy. Doc: `docs/specs/sftp.md`.
- **Themes** ✅ — System / Light / Dark / **Navy (default)** / **Redpanda** (coral accent).
- **System tray + close-to-tray** ✅ (Rust) — Open / Favorites / Recent / Groups / Quit; window-close hides to tray.
- **Brand icons** ✅ — dark variant across the Tauri icon set.
- **Favorites** ✅ — `favorite` flag (migration v9), star toggle in HostDetail header, tray Favorites submenu.
- **Tab bar** ✅ — horizontal scroll on overflow (wheel + auto-scroll-to-active).
- **Storage scope switcher** ✅ (UI seam) — Vault chevron → Personal (active) / Team (locked, "needs sync"). Groundwork for sync.
- **Account & Sync** ✅ — E2E sync via server **pingie.ru** (`server/` crate `rh-sync-server`, Docker on a Timeweb VPS). Email/password + **Yandex OAuth** (no Google). Vault sealed with a master password (entered once; keychain or session-mem). **Automatic** sync only (`sync_engine` actor — 30s interval + wake-on-edit; no manual button). **Logout purges the local vault** (data + keychain secrets + `sync_meta` tombstones) → accounts are isolated and re-login restores from the server. Sync/auth errors localized (`ui/src/lib/syncErrors.ts`). Server already mints an email-verification token on register but only *logs* the link — actual email send + password reset is the next slice.

---

## Outstanding risk flags (unproven APIs — user compiles, may need a fix)

- **SFTP byte-resume** (`rh-ssh/sftp.rs`): the resume branch uses `File` `AsyncSeek` (remote read seek) + `open_with_flags(OpenFlags::WRITE|CREATE|APPEND)` / `russh_sftp::protocol::OpenFlags`. The `offset == 0` path is unchanged, so only resume is at risk if names differ. **(awaiting user's test/compile)**
- **chmod**: `SftpSession::set_metadata(path, FileAttributes { permissions })` / `russh_sftp::protocol::FileAttributes`.
- **Tray/menu (Tauri 2.x)**: `TrayIconBuilder`, `show_menu_on_left_click` (was `menu_on_left_click` pre-2.1), `SubmenuBuilder`/`MenuItemBuilder`, `tray_by_id`/`set_menu`.

When using unproven russh/russh-sftp/IronRDP/Tauri surface, **flag it in the summary** and prefer a small spike first (`sftp_spike`/`rdp_spike` pattern).

---

## Anti-patterns the user has explicitly rejected (don't reintroduce)

Modal dialogs for editing · card-style read-only credential views (always two editable inputs + eye toggle) · red error banners (status icon + tooltip) · auto-saving partial credentials (wait for BOTH username and password) · errors on in-progress invalid input (stay idle) · Save buttons · `host.id` as form `key` · hero banners / gradients / decorative shadows · auto-generated "explanations" after a delivery (terse summary only) · stray focus outlines on inputs · naming "Termius" in user-facing copy.

---

## How to behave in this project

- **Speak Russian to the user; English in code/comments/specs.** Concise — gist + what changed + how to run. No "Great question!", no restating the ask, no postamble after the archive.
- **Don't ask a wall of clarifying questions.** Make reasonable assumptions, state them, code, verify the frontend, ship the archive, iterate. One question at a time, only if genuinely blocked.
- **Verify frontend compilation before packaging** (`npm install` → `tsc --noEmit` → `vite build`).
- **Flag protocol-API risk** in the summary; spike unproven APIs first.
- **Push back once** if a request violates an invariant; comply with a comment if the user holds position with new context.
- **Skills:** backend/IPC/storage/russh/IronRDP/SFTP → `rust-tauri-dev`; React/TS/Zustand/components → `react-frontend-dev`; UX/layout/states/.tsx+CSS → `power-tool-designer`. Cross-cutting: start with designer, then split. (`go-tbot-developer`/`moex-trader` belong to a different project — ignore.)
- **Update `docs/STATE.md`** (prepend a "Latest —" section) when a feature lands, before packaging.
- i18n insertion gotcha: when scripting key inserts, don't split an existing `"key": "value"` line — insert whole `"key": "value",` lines.

---

## Backlog / roadmap (priority order)

1. **RDP keyboard** (headline "dessert") — `Scancode` spike → `KeyboardEvent.code` → PS/2 Set 1 + the anti-sticky modifier-sync (release-all on blur, re-sync on focus). The whole reason for owning the input path.
2. **Profiles / auth / sync** (big) — needs a direction decision first: sync backend **(A) self-hosted server**, **(B) cloud via provider (S3/WebDAV/Git, encrypted blob)**, or **(C) serverless file in a cloud-sync folder**. Requires a portable E2E-encrypted vault (secrets currently in OS keychain), an auth model, conflict resolution. The "Team" storage-scope dropdown is the UI seam.
3. **RDP polish** — clipboard (text), dynamic resize (DisplayControl DVC), server cursor; later GFX/H.264.
4. **SSH agent-forward serving side** (spike) — handle the forwarded `auth-agent@openssh.com` channel + bridge bytes to the OS agent.
5. **Hotkeys / command palette** — Ctrl/Cmd+K, J/K sidebar nav, `?` shortcuts sheet.
6. **Auto-updater** (Tauri updater, self-host on pingie.ru) — sign keypair (`tauri signer generate`), `cargo tauri add updater`, `bundle.createUpdaterArtifacts:true` + `plugins.updater` (`pubkey` + `endpoints:[https://pingie.ru/updates/latest.json]` + `windows.installMode`) + `updater:default` capability; frontend `check()`→`downloadAndInstall()`→`relaunch()`; nginx `location /updates/` serving `latest.json` + signed `setup.exe`; `release.ps1` (bump version → build → read `.sig` → gen `latest.json` → upload over SSH). Until then: manual reinstall of the new `setup.exe`. Details in `docs/ROADMAP.md`.
7. **Import** from PuTTY / Termius / MobaXterm (post-MVP idea).
8. Micro-polish ideas: OS-distro icon in the session tab; "live session" hint in tray tooltip; active-transfers badge on the SFTP tab; confirm-on-Quit if live sessions exist.

---

## Glossary

**draft** UI-only unsaved new host (sidebar italic row) · **promotion** draft→real once valid · **live-save** debounced autosave, no Save button · **TOFU** trust-on-first-use key/cert pinning · **actor/hub** supervised task owning a session (ring buffer + swappable sink + cmd channel + cancel) · **reattach** rebind a live session to a fresh UI Channel after a webview reload · **endpoint/point** an SFTP panel source (local or a host) · **rail** the center column with →/← transfer buttons · **This PC** the drives view (`fs_drives`) · **scope** Personal/Team storage switcher (Team = future sync).
