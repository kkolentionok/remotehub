# RemoteHub — Project Manifest

## What this is

Кроссплатформенный desktop-клиент для удалённого подключения по SSH и RDP. Один UI, один список хостов, обе сессии в одной системе вкладок. Аналог Termius, но с поддержкой RDP. На старте — Windows; архитектура готова к macOS/Linux.

## Stack

- **Shell / runtime**: Tauri 2 (Rust + WebView, MIT/Apache-2.0)
- **Backend**: Rust 1.80+ (edition 2021), Tokio async runtime
- **Frontend**: React 18 + TypeScript + Vite, shadcn/ui + Tailwind CSS
- **Terminal renderer**: xterm.js + xterm-addon-fit + xterm-addon-webgl
- **SSH**: `russh` ≥ 0.60 (pure Rust, async)
- **RDP**: `ironrdp-*` workspace (pure Rust, Apache-2.0/MIT, Devolutions)
- **Local storage**: SQLite via `sqlx` (compile-time checked queries, async)
- **Secret storage**: `keyring-rs` (Windows Credential Manager / macOS Keychain / Linux Secret Service)
- **Crypto primitives**: `ring` / `aws-lc-rs` (зависит от выбора russh-feature), `argon2` для KDF, `zeroize` для буферов с секретами
- **Logging**: `tracing` + `tracing-subscriber`, JSON в файл + human в stderr (dev)

## Repository layout

```
remotehub/
├── CLAUDE.md                # этот файл
├── Cargo.toml               # workspace
├── rust-toolchain.toml      # pinned MSRV
├── crates/
│   ├── rh-core/             # domain types, errors, traits — без I/O
│   ├── rh-ssh/              # russh-обёртка → SshSession actor
│   ├── rh-rdp/              # ironrdp-обёртка → RdpSession actor
│   ├── rh-storage/          # SQLite + keyring адаптер
│   └── rh-app/              # Tauri commands, главный bin (src-tauri equivalent)
├── ui/                      # React app (Vite)
│   ├── src/
│   ├── package.json
│   └── vite.config.ts
├── docs/
│   ├── specs/               # архитектурные спеки
│   └── design/              # UI/UX дизайн-заметки
└── scripts/                 # helper-скрипты dev/build/release
```

## Conventions

### Rust

- **MSRV**: фиксируется в `rust-toolchain.toml`, поднимается явно (PR).
- **Edition**: 2021.
- **Naming**: snake_case для функций/модулей, CamelCase для типов, SCREAMING_SNAKE_CASE для констант. Без префиксов вида `RhFoo` — модульная иерархия (`rh_core::Host`) уже даёт неймспейс.
- **Error handling**: `thiserror` для domain-error enum'ов, `anyhow` ТОЛЬКО на границе bin'а (`main.rs` / Tauri command handlers). Внутри библиотек — domain errors.
- **Async**: Tokio (`#[tokio::main]` в bin, `tokio::spawn` для acto'ров). Никакого `block_on` внутри async-контекста.
- **Concurrency**: каждая сессия — отдельная Tokio-task с собственным `mpsc`-каналом для команд от UI и `broadcast`-каналом для событий в UI. Drop канала = graceful shutdown.
- **Secrets**: типы, содержащие секреты, не имплементируют `Debug`/`Display` (или имплементируют как `"***"`). Используют `zeroize::Zeroize`/`Zeroizing` для очистки памяти.
- **Logging**: `tracing::instrument` на public методах сессий; `tracing::field::Empty` для полей, которые заполняются динамически. **Никогда** не логируем секреты — даже на TRACE.
- **Tests**: unit-тесты в `#[cfg(test)] mod tests` в том же файле; integration tests в `tests/`. Для асинхронного кода — `#[tokio::test]`.
- **Linting**: `cargo clippy -- -D warnings`, `cargo fmt --check`. В CI обязательно.

### Frontend

- **Language**: TypeScript strict mode, `noUncheckedIndexedAccess: true`.
- **Components**: функциональные, hooks-based. Никаких class components.
- **State**: `zustand` для глобального стейта (список вкладок, активная сессия, настройки), React local state для всего остального. Redux/Mobx не используем.
- **Styling**: Tailwind + CSS variables для темизации. Никаких inline styles за исключением вычисляемых значений (например, размер canvas RDP).
- **Tauri invocation**: только через типизированный wrapper в `ui/src/lib/ipc.ts` — никаких сырых `invoke()` в компонентах.

### Specs

- Спеки — Markdown в `docs/specs/`. Naming: `<domain>-<topic>.md` (например, `session-protocol.md`, `data-model.md`).
- Implementation plans — в `docs/specs/plans/`, с порядковым префиксом `NN-`.
- Диаграммы — Mermaid, инлайн.
- Контракты — как Rust trait/struct definitions с serde-тегами (без тел методов).

## Tauri command surface

Все взаимодействия UI → Rust идут через Tauri `invoke()`. Все Rust → UI события — через Tauri `emit()`/event channels.

Поверхность команд минимальна — UI не должен «думать» в терминах протокола SSH/RDP. Он видит:

- Hosts CRUD: `host_list`, `host_get`, `host_create`, `host_update`, `host_delete`
- Credentials: `credential_save`, `credential_delete` (значения в keychain, не в БД)
- Sessions: `session_open`, `session_close`, `session_send_input`, `session_resize`
- Settings: `settings_get`, `settings_update`

Подробности — в `docs/specs/tauri-api.md`.

## Alpha mode

Проект в альфе. Это значит:
- Ломать схемы БД, контракты команд и протоколы можно свободно — миграции не пишем, **пока пользователь явно не попросит**.
- API не версионируем. После публичного релиза переходим в beta, и тогда вводим версионирование.
- Никаких legacy-shim'ов, deprecation-warnings, обратной совместимости «на всякий случай».

## Out of scope (MVP)

Эти штуки сознательно не включены в MVP. Если они появляются в обсуждении — это новая итерация product-brief, а не «доделать сейчас»:

- Облачная синхронизация между устройствами (требует backend + e2e crypto)
- SFTP-браузер файлов
- Snippets / saved commands / macros
- Telnet, Mosh, Serial console
- Port forwarding UI (под капотом SSH-туннели опционально поддерживаются)
- Mobile (iOS/Android)
- Plugin/extension система
- Session recording / playback

## Agents / collaboration

В проекте используются sub-agent'ы (см. `architect.md`, `frontend-dev.md`, `go-dev.md`, `ui-designer.md` в корне). Поскольку стек — Rust, а не Go: роль `go-dev` исполняется как «rust-dev» с теми же принципами (спеки → реализация, тесты, lint, никаких side-effect в публичном API).

Порядок обычного цикла фичи:
1. `architect` пишет спеку в `docs/specs/`
2. `ui-designer` (если есть UI-составляющая) пишет дизайн-заметку в `docs/design/`
3. `rust-dev` (= `go-dev` для этого проекта) реализует Rust-часть
4. `frontend-dev` реализует UI

Никто не пишет код без спеки. Никто не пишет UI без дизайн-заметки.
