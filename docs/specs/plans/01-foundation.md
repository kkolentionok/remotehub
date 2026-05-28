# 01 — Foundation

## Goal

Получить работающий каркас приложения: Tauri 2 + Rust workspace + React UI + SQLite-storage. Без сетевых сессий ещё; цель — чтобы из UI можно было создавать/редактировать хосты и credentials, всё persistent'ит в БД и keychain, и приложение собирается под Windows.

После этой стадии всё готово, чтобы дальше добавлять SSH-actor, RDP-actor — они уже подключатся к существующему Session Manager skeleton'у.

## Scope

### In scope

- Tauri 2 проект, `cargo workspace` со всеми пятью crate'ами (`rh-core`, `rh-ssh`, `rh-rdp`, `rh-storage`, `rh-app`).
- SQLite БД с миграцией v1 (см. `data-model.md`): таблицы `hosts`, `host_groups`, `credentials`, `host_credentials`, `settings`, `schema_meta`.
- Keychain доступ через `keyring-rs`, обёрнутый в `CredentialStore` trait.
- Tauri commands для всех Host/Group/Credential CRUD из `tauri-api.md`. Сессионные команды — stub'ы, возвращающие `Internal{message: "not implemented"}`.
- React/TypeScript UI с базовым layout'ом: sidebar (дерево групп + список хостов), main area (placeholder для табов сессий), modals для создания/редактирования.
- Tracing (logger + JSON-файл).
- `cargo deny` конфиг для license/security audit.
- CI workflow (GitHub Actions) — build + clippy + test на Windows.

### Out of scope (для этой стадии)

- SSH/RDP функциональность — это Stage 2+.
- Импорт/экспорт хостов.
- Темизация (тёмная тема). Только default.
- Drag-n-drop для перетаскивания хостов между группами.
- Иконки/favicon'ы — placeholder.
- Code signing для дистрибутива. Self-signed installer ок.
- macOS/Linux build target'ы. Только Windows.

## Dependencies

- Спеки: `system-overview.md`, `data-model.md`, `tauri-api.md`, `session-protocol.md` (для skeleton сессионных команд).
- Установленные инструменты на dev-машине: Rust 1.80+, Node.js 20+, pnpm/npm, Tauri CLI v2.

## Stages

### Stage 1.1: Repository scaffolding

- **Owner**: rust-dev
- **Files to create**:
  - `Cargo.toml` (workspace)
  - `rust-toolchain.toml`
  - `crates/rh-core/Cargo.toml` + `src/lib.rs` (empty)
  - `crates/rh-ssh/Cargo.toml` + `src/lib.rs` (empty placeholder)
  - `crates/rh-rdp/Cargo.toml` + `src/lib.rs` (empty placeholder)
  - `crates/rh-storage/Cargo.toml` + `src/lib.rs`
  - `crates/rh-app/Cargo.toml` + `src/main.rs` (Tauri builder skeleton)
  - `crates/rh-app/tauri.conf.json`
  - `crates/rh-app/build.rs`
  - `crates/rh-app/icons/` (placeholder PNG / ICO)
  - `ui/package.json` + `vite.config.ts` + `tsconfig.json` + `src/main.tsx` + `index.html`
  - `.gitignore`
  - `deny.toml` (cargo-deny config)
  - `.github/workflows/ci.yml`
  - `scripts/dev.ps1` (запускает tauri dev)
- **Acceptance**:
  - `cargo build --workspace` проходит без ошибок и warnings.
  - `cd ui && pnpm install && pnpm build` проходит.
  - `cargo tauri dev` запускает пустое окно с надписью "RemoteHub".
  - `cargo clippy --workspace --all-targets -- -D warnings` зелёный.
  - `cargo deny check` зелёный (никаких GPL/LGPL/MPL зависимостей).
- **Out of scope**: реальная функциональность; cross-platform build.

### Stage 1.2: rh-core types

- **Owner**: rust-dev
- **Files**:
  - `crates/rh-core/src/types.rs` — все domain types из `system-overview.md` + `data-model.md`.
  - `crates/rh-core/src/error.rs` — `CoreError`, `StorageError`, `SecretError`, `SessionError`.
  - `crates/rh-core/src/id.rs` — ULID-helpers (`generate_host_id`, `generate_credential_id`, etc.). Use `ulid` crate.
  - `crates/rh-core/src/secret.rs` — `SecretValue`, `RevealedSecret`, `RevealedCredential` (re-export для RDP/SSH сторон).
  - `crates/rh-core/src/store.rs` — trait'ы `HostStore`, `CredentialStore`, `GroupStore`, `SettingsStore`.
  - Unit-тесты на сериализацию/десериализацию DTO (snapshot-friendly).
- **Acceptance**:
  - `cargo test -p rh-core` зелёный.
  - Все типы имеют корректные Serde-теги соответственно `tauri-api.md` (snake_case enum tags, etc.).
  - Тип `SecretValue` НЕ имплементирует `Debug` (или имплементирует как `"<redacted>"`).
- **Out of scope**: имплементация trait'ов — она в rh-storage.

### Stage 1.3: rh-storage (SQLite + keychain)

- **Owner**: rust-dev
- **Files**:
  - `crates/rh-storage/src/lib.rs`
  - `crates/rh-storage/src/db.rs` — connection setup, миграции (`init_or_migrate`), pool.
  - `crates/rh-storage/src/migrations/v1.sql` — DDL для всех таблиц из `data-model.md`.
  - `crates/rh-storage/src/host_store.rs` — `SqliteHostStore: HostStore`.
  - `crates/rh-storage/src/group_store.rs` — `SqliteGroupStore: GroupStore`.
  - `crates/rh-storage/src/credential_store.rs` — `KeychainCredentialStore: CredentialStore` (использует `keyring-rs` + `SqliteHostStore` под капотом для метаданных).
  - `crates/rh-storage/src/settings_store.rs` — `SqliteSettingsStore: SettingsStore`.
  - `crates/rh-storage/tests/integration.rs` — integration-тесты с временной БД и mock-keychain (через `keyring::mock::MockKeyring`).
- **Acceptance**:
  - `cargo test -p rh-storage` зелёный. Минимум сценариев:
    - Create + get + update + delete host.
    - Группы: вложенная иерархия, удаление каскадно делает children → root.
    - Credential: create записывает в keychain + БД atomically; delete удаляет оба; reveal возвращает корректное значение.
    - Cycle-detection при group_move.
  - Миграция v1 на пустой БД создаёт все таблицы.
  - При нон-существующем БД-файле — создаётся автоматически.
- **Out of scope**: миграция v2+ (нет ещё).

### Stage 1.4: rh-app — Tauri commands skeleton

- **Owner**: rust-dev
- **Files**:
  - `crates/rh-app/src/main.rs` — Tauri builder.
  - `crates/rh-app/src/state.rs` — AppState (`Arc<HostStore>`, `Arc<CredentialStore>`, etc.).
  - `crates/rh-app/src/api/mod.rs` — re-exports.
  - `crates/rh-app/src/api/error.rs` — `ApiError` enum + `From<CoreError>`.
  - `crates/rh-app/src/api/dto.rs` — DTO для request/response.
  - `crates/rh-app/src/api/hosts.rs` — все `host_*` commands.
  - `crates/rh-app/src/api/groups.rs` — все `group_*` commands.
  - `crates/rh-app/src/api/credentials.rs` — все `credential_*` commands.
  - `crates/rh-app/src/api/settings.rs` — `settings_*` commands.
  - `crates/rh-app/src/api/sessions.rs` — stub'ы `session_open`/`session_close`/etc., возвращают `Internal{message: "session API not yet implemented"}`.
  - `crates/rh-app/src/logging.rs` — tracing setup (JSON to file + console).
  - `crates/rh-app/src/paths.rs` — platform-specific app data paths.
- **Acceptance**:
  - Все команды из `tauri-api.md` секции "Hosts", "Host groups", "Credentials", "Settings" имплементированы.
  - Команда `app_version` возвращает корректный version.
  - Логи пишутся в `%APPDATA%/RemoteHub/logs/app-YYYY-MM-DD.log`.
  - Tauri capability config — минимум прав (нет fs, нет shell, нет http).
  - CSP установлен правильно (см. system-overview).
- **Out of scope**: session commands — stub'ы.

### Stage 1.5: UI — базовый layout

- **Owner**: frontend-dev
- **Files**:
  - `ui/src/main.tsx` — bootstrap.
  - `ui/src/App.tsx` — root layout.
  - `ui/src/lib/ipc.ts` — typed Tauri invoke wrapper.
  - `ui/src/lib/types.ts` — TypeScript types зеркалирующие DTO из `tauri-api.md`.
  - `ui/src/store/hosts.ts` — zustand store для hosts/groups.
  - `ui/src/store/credentials.ts` — zustand для credentials.
  - `ui/src/components/Sidebar.tsx` — дерево групп + список хостов.
  - `ui/src/components/MainArea.tsx` — placeholder для табов сессий ("session not implemented yet").
  - `ui/src/components/HostDialog.tsx` — create/edit host modal.
  - `ui/src/components/CredentialDialog.tsx` — create/edit credential modal.
  - `ui/src/components/GroupDialog.tsx` — create/rename/move group.
  - `ui/src/styles/global.css` — Tailwind base + variables.
  - `ui/tailwind.config.ts`
- **Acceptance**:
  - Пользователь может через UI:
    - Создать группу, переименовать, удалить.
    - Создать host, отредактировать поля, удалить.
    - Создать credential (password / ssh-key), отредактировать metadata, ротировать секрет, удалить.
    - Привязать credential к host'у, сделать default.
    - Видеть актуальный список после CRUD (через events `hosts:changed` etc.).
  - На клик "Connect" в host'е — показывается toast "Sessions not implemented yet" (т.к. session_open вернёт ошибку — UI обрабатывает корректно).
  - Тёмная тема — не реализована (только light), но Tailwind dark: классы расставлены — для будущей стадии.
  - TypeScript strict mode, no `any` (кроме `ipc.ts` boundary с explicit type assertions).
- **Out of scope**: красивая графика; иконки сделаны через lucide-react placeholder'ы; drag-n-drop.

### Stage 1.6: Settings UI

- **Owner**: frontend-dev
- **Files**:
  - `ui/src/components/SettingsDialog.tsx`
- **Acceptance**: пользователь может видеть и менять настройки из `data-model.md` settings таблицы. Изменения немедленно persistent'ятся.

## Verification

End-to-end smoke-сценарий (вручную после Stage 1.6):

1. Запускаем `cargo tauri dev`. Окно открывается за < 3 секунд.
2. Создаём группу "Servers / Prod".
3. Создаём credential "root-key" (тип ssh-key, paste'им dummy PEM).
4. Создаём host "prod-db-01" в "Prod", протокол SSH, hostname `db01.example.com`, port 22, привязываем credential.
5. Закрываем приложение, открываем снова — всё на месте.
6. Открываем Credential Manager / Keychain Access — видим entry `RemoteHub` с одной записью.
7. Удаляем credential — keychain-запись пропадает.
8. Логи в `%APPDATA%/RemoteHub/logs/` непустые, не содержат секретов.

CI должен зелёным проходить:
- `cargo build --workspace --release`
- `cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo deny check`
- `cargo tauri build` (production-сборка)
- `cd ui && pnpm test` (если будут UI-тесты на эту стадию — опционально)

## Open Questions

1. **Tauri CLI как dev-dep или global?** Глобально проще, но reproducibility страдает. **Предложение**: добавить `tauri-cli` как member workspace'а, запускать через `cargo run -p tauri-cli`. Окончательно решается в Stage 1.1.
2. **Версии IronRDP/russh — точные числа.** В спеках `<latest>`; на старте Stage 2 (SSH) и Stage 4 (RDP) — фиксируем точные версии и обновляем спеки. До Stage 1 они не нужны (rh-ssh/rh-rdp — пустые).
3. **Иконки приложения.** Placeholder ок для MVP. Дизайнер (`ui-designer` агент) подключится отдельно для брендинга.

## Next plans (preview)

- `02-ssh-foundation.md` — SSH actor: connect + auth + basic shell. xterm.js integration.
- `03-ssh-polish.md` — known_hosts UI, key-based auth, multiple tabs.
- `04-rdp-foundation.md` — RDP actor: connect + render. Spike по frame-transport.
- `05-rdp-polish.md` — clipboard, resize, NLA edge cases.
- `06-ux-polish.md` — темы, hotkeys, поиск, import/export.
- `07-distribution.md` — see below.

### Distribution / installer (deferred, but tracked)

Когда MVP функционально готов (после Stages 1-6), нужен **бандл-installer для конечного пользователя**, чтобы не требовать у него Rust toolchain / Node / pnpm / Tauri CLI / WebView2-руками. Tauri умеет это из коробки через `cargo tauri build`, который генерирует:

- **Windows**: `.msi` (через WiX) и `.exe` (через NSIS) — single-file installer, включает все runtime-зависимости приложения. WebView2 — bootstrapper подтянет, если у пользователя нет.
- **macOS**: `.dmg` + `.app` bundle (после расширения на macOS).
- **Linux**: `.deb`, `.rpm`, `.AppImage` (после расширения на Linux).

Это работа отдельной стадии **`07-distribution.md`** (пока не написана). Её скоуп:

1. Создать настоящие иконки (Stage 1.1 оставил placeholder PNG; нужны `icon.ico` + `icon.icns` + AppIcon set).
2. Настроить NSIS-installer: лицензионное соглашение, выбор директории, ярлык на рабочем столе, ассоциация протоколов `ssh://` и `rdp://` (опционально).
3. **Code signing**: для Windows-distribution без warning'ов SmartScreen нужен EV-сертификат (~$200-400/год от Sectigo/DigiCert). Без него Windows покажет «Unknown publisher» при запуске. Для пет-режима ок; для public release — обязательно.
4. **Auto-updates**: Tauri 2 имеет `tauri-plugin-updater`. Нужно сервер для манифестов (можно GitHub Releases) и подпись бандлов отдельным private key.
5. CI workflow для сборки installer'ов на каждый release tag.
6. Установка не должна требовать admin-прав (per-user install в `%LOCALAPPDATA%`).

Roadmap-приоритет: **после MVP**, перед первым публичным релизом. До этого момента — dev-режим через `cargo tauri dev`, что требует Rust toolchain. Сам себе ты ставишь это один раз; никому другому раздавать пока не нужно.

> NOTE: если потребуется раньше (например, хочешь дать другу попробовать) — `cargo tauri build` уже сейчас сгенерирует `.msi`/`.exe` в `crates/rh-app/target/release/bundle/`, нужны будут только настоящие иконки. Это hack, не финальное решение, но работает.