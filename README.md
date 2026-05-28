# RemoteHub

Cross-platform desktop client for SSH and RDP. One UI, one host list, both
session types side-by-side. Windows first; macOS/Linux later.

This is **Stage 1.1** scaffolding — the workspace builds and `cargo tauri dev`
opens an empty window. Real functionality lands in subsequent stages of
`docs/specs/plans/01-foundation.md`.

## Prerequisites

- **Rust** 1.80+ — install via [rustup](https://rustup.rs/). The toolchain
  is pinned in `rust-toolchain.toml`.
- **Node.js** 20+ and **pnpm** 9+ — for the UI build.
- **Tauri 2 system deps** — on Windows, install [WebView2](https://developer.microsoft.com/microsoft-edge/webview2/)
  (it ships with Windows 11; Windows 10 may need a manual install).
- **Tauri CLI** — `cargo install tauri-cli --version "^2.1" --locked`.
- **cargo-deny** (optional, for license/audit check) — `cargo install cargo-deny --locked`.

## First-time setup

```powershell
# From repo root
pnpm --dir ui install
```

## Run in dev mode

```powershell
pwsh -NoProfile -File scripts/dev.ps1 -Mode dev
# or directly:
cargo tauri dev
```

A window titled "RemoteHub" should open within a couple of seconds. Hot reload
works for both the React UI (Vite HMR) and the Rust backend (Tauri restarts
the binary).

## Other dev commands

```powershell
pwsh -File scripts/dev.ps1 -Mode test    # cargo test --workspace
pwsh -File scripts/dev.ps1 -Mode lint    # fmt + clippy + UI typecheck
pwsh -File scripts/dev.ps1 -Mode build   # production bundle
pwsh -File scripts/dev.ps1 -Mode icons   # regenerate icon set
pwsh -File scripts/dev.ps1 -Mode clean   # nuke build artifacts
```

## Repository layout

```
remotehub/
├── CLAUDE.md            project manifest — conventions, stack, agent notes
├── Cargo.toml           workspace
├── rust-toolchain.toml  pinned MSRV
├── deny.toml            cargo-deny license/audit config
├── crates/
│   ├── rh-core/         domain types, traits, errors — no I/O
│   ├── rh-storage/      SQLite + keychain
│   ├── rh-ssh/          SSH actor (placeholder until Stage 2)
│   ├── rh-rdp/          RDP actor (placeholder until Stage 4)
│   └── rh-app/          Tauri binary — commands, wiring, logging
├── ui/                  React + TypeScript + Vite
└── docs/specs/          architectural specs and implementation plans
```

## Project conventions

See [CLAUDE.md](CLAUDE.md). Highlights:

- Rust 2021 edition, MSRV 1.80. `cargo clippy -- -D warnings` is enforced in CI.
- TypeScript in strict mode, `noUncheckedIndexedAccess: true`.
- All inter-component contracts live in `docs/specs/`. No undocumented APIs.
- Secrets in OS keychain only. The SQLite database stores metadata and a
  reference, never the secret itself.
- Alpha mode: schemas and contracts may break without migrations until we ship
  a public beta.

## Architecture

Start with `docs/specs/system-overview.md`. Then domain-specific specs:
`data-model.md`, `tauri-api.md`, `session-protocol.md`, and the per-protocol
files `ssh-session.md` and `rdp-session.md`.

Deferred / post-MVP items are tracked in [`docs/ROADMAP.md`](docs/ROADMAP.md) —
including the **single-file installer** for end users (Stages 1-6 use
`cargo tauri dev`; a proper `.msi` / `.exe` installer comes in Stage 7).

## Status

- [x] **Stage 1.1** — repository scaffolding
- [x] **Stage 1.2** — rh-core types
- [x] **Stage 1.3** — rh-storage (SQLite + keychain)
- [x] **Stage 1.4** — rh-app Tauri commands (stubs for sessions)
- [x] **Stage 1.5** — UI: hosts / credentials CRUD
- [ ] Stage 1.6 — UI: settings dialog

After Stage 1 — `docs/specs/plans/02-ssh-foundation.md` (not yet written).

## License

MIT OR Apache-2.0 at your option.
