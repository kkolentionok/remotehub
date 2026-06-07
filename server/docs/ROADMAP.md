# RemoteHub — Roadmap

Этот документ — централизованное место для **не-в-MVP** пунктов, чтобы они не терялись в open question'ах конкретных стадий. Когда какой-то пункт берётся в работу — переезжает в `docs/specs/plans/NN-<topic>.md` и удаляется отсюда.

Формат записи: краткое описание + почему отложено + когда стоит подумать снова.

## Distribution

### Single-file installer для конечного пользователя

**Что**: `.msi` / `.exe` (Windows), `.dmg` (macOS), `.deb` / `.AppImage` (Linux), которые конечный пользователь скачивает и устанавливает двойным кликом, без Rust / Node / pnpm / Tauri CLI.

**Почему отложено**: MVP-функциональность ещё не готова. Распространять пока нечего; разработчик ставит toolchain один раз сам.

**Когда вернуться**: после Stages 1-6 (полный MVP), перед первым публичным релизом. Намечено как Stage `07-distribution.md`.

**Подскоуп** (когда будем делать):
- Настоящие иконки (`icon.ico`, `icon.icns`, AppIcon).
- NSIS-конфиг: lic-agreement, выбор директории, shortcut, опционально `ssh://`/`rdp://` URL protocol handlers.
- Code signing (Windows EV cert / Apple Developer ID). Без подписи SmartScreen / Gatekeeper покажут предупреждение.
- Tauri updater + GitHub Releases для auto-update.
- Per-user install (`%LOCALAPPDATA%`), без admin-prompt'а.
- CI: build бандлов на каждый release tag.

**Workaround в текущий момент**: `cargo tauri build` уже создаст `.msi` и `.exe` в `crates/rh-app/target/release/bundle/`. Без подписи и иконок, но работающие. Достаточно «дать другу попробовать», не достаточно для public release.

### macOS / Linux таргеты

**Что**: код собирается на этих платформах, бандлится в `.dmg` / `.deb` / `.AppImage`.

**Почему отложено**: Windows-first решение из ранних обсуждений. RDP-frame transport, keychain backend, file paths — всё абстрагировано за trait'ами, но **реально не тестировалось** на других OS.

**Когда вернуться**: после установления Windows как стабильного MVP. Cross-OS тестирование — отдельная итерация, минимум 1-2 недели на каждую платформу (CI matrix + ручная проверка + bug-fix цикл).

## Sync / Cloud

### E2E-encrypted синхронизация между устройствами

**Что**: пользователь устанавливает RemoteHub на двух машинах, hosts/credentials мерджатся через сервер, без того, чтобы сервер видел plaintext-секреты.

**Почему отложено**: требует:
- Backend (Go или Rust HTTP-сервис).
- Account system (email + password / OAuth).
- Криптография: key derivation из master-password, encrypted blobs, conflict resolution.
- Mobile-клиенты (иначе sync без мобилы — наполовину фича).

Это **отдельный проект** масштабом сопоставимый с самим RemoteHub. Не делается ad-hoc.

**Когда вернуться**: когда desktop-MVP стабилен и есть >100 пользователей, которые хотят. Раньше — нет смысла строить инфраструктуру, которую никто не использует.

**Workaround**: export/import JSON-файла хостов вручную. Без секретов; их пользователь докладывает на втором устройстве сам.

## Mobile

### iOS / Android клиенты

**Что**: те же hosts и сессии на телефоне/планшете.

**Почему отложено**: Tauri 2 поддерживает mobile, но RDP на 5"-экране — сомнительный UX. SSH — может быть полезно, но Termius уже это хорошо делает.

**Когда вернуться**: после того, как desktop взлетит. И только если sync (см. выше) реализован — иначе mobile-клиент без серверной синхронизации это автономный продукт, что мало кому нужно.

## Protocol coverage

### Telnet, Mosh, Serial, VNC

**Telnet**: 5 минут работы (`std::net::TcpStream` + xterm.js). Польза в 2026 — околонулевая. Сделаем по запросу.

**Mosh**: SSH-over-UDP с roaming. Реально полезно для нестабильной сети. Требует binary mosh-server на стороне сервера. **Когда вернуться**: после SSH-MVP, если пользователи попросят.

**Serial**: для embedded-разработчиков. Требует доступа к COM-портам / `/dev/ttyUSB*`. Нишево.

**VNC**: альтернатива RDP, кроссплатформенно (Linux GUI, например). Если IronRDP работает, добавление VNC через `vnc-rs` — 1-2 недели работы. **Когда вернуться**: после RDP-MVP, если кто-то реально захочет VNC.

## SSH features

### Port forwarding UI

**Что**: пользователь может настроить «локальный порт 8080 → remote 80 на этом сервере» через диалог. Сейчас russh это умеет на уровне API, но UI не выставляет.

**Когда вернуться**: после SSH-MVP. Часто запрашиваемая фича в SSH-клиентах.

### SFTP-браузер

**Что**: drag-n-drop файлов между локалом и удалённым сервером в графическом интерфейсе.

**Почему отложено**: большой UX-проект сам по себе (двухпанельный браузер, прогресс-бары, conflict resolution, разрешения). `russh-sftp` уже есть, но это 30% работы; UI — оставшиеся 70%.

**Когда вернуться**: после SSH-polish (Stage 3). Скорее всего — Stage 8 или 9.

### Snippets / saved commands

**Что**: библиотека сохранённых команд (`docker ps`, `tail -f /var/log/...`), которые можно вставить в активный терминал одним кликом.

**Когда вернуться**: после UX-polish (Stage 6). Полезная фича, не блокирующая.

### Multi-factor auth (keyboard-interactive)

**Что**: поддержка TOTP / PAM-prompt'ов при SSH-логине.

**Когда вернуться**: на Stage 3 (SSH polish). Из всего «после MVP» — это самый близкий кандидат включить в MVP, если простая реализация ляжет.

### SSH agent forwarding

**Что**: проброс `ssh-agent` соединения для chained SSH (логин на bastion → оттуда на target).

**Когда вернуться**: после Stage 3. Требует интеграции с OS-agent'ами (Pageant на Windows, ssh-agent на nix).

### Jump host / ProxyJump

**Что**: SSH-через-SSH без agent forwarding, через настройку прокси-цепочки в UI.

**Когда вернуться**: вместе с port forwarding UI или после.

## RDP features

### RemoteFX / AVC444 codecs

**Что**: продвинутые видео-кодеки RDP для лучшего качества при ограниченной полосе.

**Почему отложено**: декодеры в `ironrdp-graphics` активно разрабатываются и могут быть нестабильными на дату MVP. Базовый RDP (RLE + 16/24bpp bitmap) работает без них.

**Когда вернуться**: после Stage 5 (RDP polish). Реально нужно для удалённой работы на медленных каналах.

### Audio (RDPSND)

**Что**: проигрывание звука с удалённого RDP-сервера локально.

**Когда вернуться**: после Stage 5. `ironrdp-rdpsnd` уже есть, нужна интеграция с локальным аудио-выводом (cpal или подобное).

### Multi-monitor RDP

**Что**: один RDP-сеанс на несколько физических мониторов.

**Когда вернуться**: после Stage 5. Требует пересмотра UI canvas-модели.

### Sticky modifier keys на focus change

**Что**: классический баг — Ctrl/Alt/Shift «залипают» на удалённом столе после Alt+Tab / multi-monitor switch / fullscreen toggle.

**Статус**: **не deferred** — это **обязательно** в Stage 4 (RDP foundation). Иначе UX неприемлемый, MVP неюзабельный. Спека дописана в `docs/specs/rdp-session.md` секция "Focus / modifier-key synchronization".

Эта запись здесь нужна как red flag: при работе над Stage 4, **прежде чем закрывать стадию**, проверить три ручных кейса из секции "Verification" в RDP-спеке.

### Smart-card auth для RDP

**Что**: вход на RDP через PKCS#11 smart-card вместо пароля.

**Когда вернуться**: по запросу. Нишево, но в enterprise-средах востребовано.

## UX

### Drag-n-drop хостов между группами

**Когда**: Stage 6 (UX polish).

### Tag-based filtering

**Что**: пользователь нажимает на тег — видит все хосты с этим тегом.

**Когда**: Stage 6.

### Импорт из других клиентов

**Что**: поддержка импорта hosts из Termius, MobaXterm, PuTTY, SecureCRT.

**Почему отложено**: каждый формат разный, парсинг отдельных файлов. Своих JSON-импортов для bootstrap'а достаточно в MVP.

**Когда вернуться**: после публичного релиза, по запросу пользователей. Termius-импорт скорее всего первый, поскольку наш product positioning — «Termius + RDP».

### Session recording / playback

**Что**: запись SSH-сессии (как `asciinema`) или RDP-сессии в файл для последующего просмотра.

**Когда**: вряд ли скоро. Compliance/audit-фича для enterprise.

## Security

### SQLite encryption (SQLCipher)

**Что**: шифровать БД-файл целиком ключом, производным от master-password.

**Почему отложено**: БД содержит только метаданные. Секреты — в OS keychain. Encryption на уровне БД даёт защиту от «кто-то скопировал файл», но keychain даёт ту же защиту лучше (он привязан к OS-аккаунту).

**Когда вернуться**: если будет запрос от пользователя с угрозой «недоверенная машина». До тех пор — over-engineering.

### Master password / app lock

**Что**: при запуске приложения — prompt'ит master-password, без которого не подключается к keychain.

**Когда**: после MVP. Не очень нужно (OS-login это уже фактор аутентификации), но appearance security полезна.

### Audit log

**Что**: «когда и к какому хосту подключались» — отдельная таблица в БД.

**Почему отложено**: само по себе security-concern (история доступа — sensitive data). Сделать как opt-in toggle в settings.

**Когда вернуться**: после Stage 6, как опциональная feature.

### Plugin / extension система

**Что**: пользователь может писать свои расширения (например, custom protocol handler, кастомные snippets).

**Почему отложено**: огромный проект сам по себе. Tauri даёт некоторые primitives, но безопасная sandbox-модель — это месяцы работы.

**Когда вернуться**: после первого major-релиза, если пользователи реально просят. Часто это «фича, которая никому не нужна», но добавляет 30% сложности кодовой базы.

## Tooling / DX

### `cargo tauri-cli` как workspace member вместо global install

**Что**: убрать требование `cargo install tauri-cli` — встроить CLI как part of workspace.

**Почему отложено**: незначительное упрощение onboarding'а; пользователь ставит CLI один раз.

**Когда вернуться**: если будем активно onboard'ить контрибьюторов.

### Pin pnpm version

**Что**: добавить `"packageManager": "pnpm@11.x.x"` в `ui/package.json` (Corepack-формат) или явный CI-чек.

**Почему**: pnpm v10 → v11 поменял формат конфигурации (`onlyBuiltDependencies` в `package.json` → `allowBuilds` в `pnpm-workspace.yaml`). Сейчас у нас есть `pnpm-workspace.yaml` для v11, но если кто-то поднимет проект на v10 — там этот файл не читается, и снова всплывёт `ERR_PNPM_IGNORED_BUILDS`. Pin'нуть версию избавит от непредсказуемых сюрпризов.

**Когда вернуться**: вместе с CI workflow (Stage 1.1 их уже описал, но не упоминают pnpm-pin).

### CI: build UI before Rust tests

**Что**: в `.github/workflows/ci.yml` шаг `pnpm --dir ui build` ДОЛЖЕН выполняться до `cargo test` / `cargo clippy`. Tauri `generate_context!()` валидирует существование `frontendDist` в compile-time, и без `dist/` сборка падает.

**Сейчас**: `ui/dist/index.html` — placeholder, который коммитится в репо (см. `.gitignore` исключение). Это работает локально, но **уродливо** в долгосрочной перспективе — на каждом dev-environment'е нужно либо иметь dist (после `pnpm build`), либо placeholder.

**Опции для рефакторинга**:
- (a) `build.rs` в `rh-app` запускает `pnpm build` перед compile (требует pnpm в PATH у разработчика).
- (b) Tauri 2 имеет `beforeBuildCommand`, но он выполняется CLI'ем, не cargo напрямую.
- (c) Условный `frontendDist` — для dev указывать на dev-сервер, для prod на dist.

**Когда вернуться**: вместе с финальной CI настройкой / installer (Stage 7).

### Migration runner (когда выйдем из альфы)

**Что**: вместо текущего `DROP + CREATE` на bump версии — нормальные up-миграции.

**Когда**: когда зафиксируем beta-версию. До тех пор — schema может меняться без оглядки.

### Crash reporter (Sentry или аналог)

**Что**: при панике / краше — отправить anonymized stacktrace на сервер.

**Когда вернуться**: после публичного релиза. Дев-стадия — пользователь (то есть мы) и так видит логи локально.
