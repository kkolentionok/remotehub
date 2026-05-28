# RemoteHub — Project State & Handoff

**Last updated:** Stage 1.5.2 completion (UX-pass: live-save, draft mode, credentials redesign, save status indicator).

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
| 1.8 | Schema extensions: display_name, startup_command, env_vars, detected_os | ⬜ Next |
| 1.9 | Command bar (top, search + user@host:port parser) | ⬜ Future |
| 1.10 | Import .rdp files | ⬜ Future |
| 1.12 | Export/Import JSON (our format) | ⬜ Future |
| 2.x | SSH session actors (russh) | ⬜ Future |
| 2.2 | OS auto-detect after connect → HostIcon switches to Simple Icons SVG | ⬜ Future |
| 4.x | RDP (IronRDP) | ⬜ Future |
| 5.x | Personal/Team Vault via S3 — cloud sync, identity, e2e crypto | ⬜ Future |

109 Rust tests passing. Vite + tsc strict build green.

---

## Stage 1.6 — pickup point

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
