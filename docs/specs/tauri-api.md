# Tauri API — RemoteHub

## Overview

Контракт взаимодействия между React UI и Rust backend. Все вызовы UI → Rust идут через Tauri `invoke()`. Все вытолкнутые из Rust в UI данные — через event channels или dedicated `Channel<T>` (Tauri 2 feature) для high-throughput потоков.

UI потребляет этот API исключительно через типизированный wrapper `ui/src/lib/ipc.ts`. Сырые `invoke()` в компонентах запрещены — это позволит при необходимости заменить транспорт (например, переехать на mock для Storybook).

## Conventions

### Naming

- Команды — snake_case: `host_list`, `session_open`.
- События — `<entity>:<action>` или `session_event:<session_id>` для per-session streams.
- Все ID — строки (ULID).

### Errors

Tauri командные handler'ы возвращают `Result<T, ApiError>`. На UI-стороне это превращается в reject промиса. `ApiError` — единый serializable enum:

```rust
// rh-app/src/api/error.rs

#[derive(Debug, Serialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ApiError {
    #[error("not found: {entity}")]
    NotFound { entity: String },

    #[error("validation: {field}: {reason}")]
    Validation { field: String, reason: String },

    #[error("storage error: {message}")]
    Storage { message: String },

    #[error("secret store error: {message}")]
    Secret { message: String },

    #[error("session error: {message}")]
    Session { message: String },

    #[error("conflict: {message}")]
    Conflict { message: String },

    #[error("internal error: {message}")]
    Internal { message: String },
}
```

Сериализованная форма (что увидит UI):

```json
{ "kind": "not_found", "entity": "host" }
{ "kind": "validation", "field": "hostname", "reason": "must not be empty" }
```

Любой `CoreError` мапится в `ApiError` в адаптере на границе command-handler'а. Internal-детали (full backtraces, инфо о БД) не пропускаем в UI — они идут в логи.

### Request/response payloads

Все payload-структуры — отдельные DTO в `rh-app/src/api/dto.rs`, не голые domain-types. Это даёт:
- Стабильность UI-контракта даже когда domain эволюционирует.
- Контроль над тем, что именно сериализуется (например, `default_credential_id` есть в DTO; сами credentials — нет, чтобы клиент не получал ссылки на keychain).

## Commands

Группируем по доменам.

### Hosts

#### `host_list`

Список хостов с фильтрацией.

**Request**:
```rust
#[derive(Debug, Deserialize)]
pub struct HostListRequest {
    pub group_id: Option<GroupId>,        // null = все группы
    pub protocol: Option<Protocol>,       // null = оба
    pub search: Option<String>,           // подстрока в name/hostname/tags
    pub limit: Option<u32>,               // default 1000
}
```

**Response**:
```rust
#[derive(Debug, Serialize)]
pub struct HostListResponse {
    pub hosts: Vec<HostDto>,
    pub total: u32,
}

#[derive(Debug, Serialize)]
pub struct HostDto {
    pub id: HostId,
    pub name: String,
    pub group_id: Option<GroupId>,
    pub protocol: Protocol,
    pub hostname: String,
    pub port: u16,
    pub tags: Vec<String>,
    pub color: Option<String>,
    pub default_credential_id: Option<CredentialId>,
    // notes — не возвращается в list, только в host_get
    pub created_at: String,               // ISO 8601
    pub updated_at: String,
}
```

**Errors**: `Storage`.

#### `host_get`

Получить один хост со всеми полями (включая notes).

**Request**: `{ id: HostId }`
**Response**: `HostFullDto` (= `HostDto` + `notes`)
**Errors**: `NotFound`, `Storage`.

#### `host_create`

```rust
#[derive(Debug, Deserialize)]
pub struct HostCreateRequest {
    pub name: String,
    pub group_id: Option<GroupId>,
    pub protocol: Protocol,
    pub hostname: String,
    pub port: Option<u16>,                // default по protocol (22 / 3389)
    pub tags: Option<Vec<String>>,
    pub color: Option<String>,
    pub notes: Option<String>,
    pub default_credential_id: Option<CredentialId>,
}
```

**Response**: `{ id: HostId }`
**Errors**: `Validation` (empty name, bad hostname), `Storage`.

#### `host_update`

```rust
#[derive(Debug, Deserialize)]
pub struct HostUpdateRequest {
    pub id: HostId,
    // Все поля Optional — обновляются только присланные.
    pub name: Option<String>,
    pub group_id: Option<Option<GroupId>>,         // двойной Option: явный null = убрать группу
    pub hostname: Option<String>,
    pub port: Option<u16>,
    pub tags: Option<Vec<String>>,
    pub color: Option<Option<String>>,
    pub notes: Option<Option<String>>,
    pub default_credential_id: Option<Option<CredentialId>>,
    // protocol — НЕ обновляется (нужно delete + create)
}
```

**Response**: `()`
**Errors**: `NotFound`, `Validation`, `Storage`.

#### `host_delete`

**Request**: `{ id: HostId }`
**Response**: `()`
**Errors**: `NotFound`, `Storage`.

Каскадно НЕ удаляет связанные credentials (это shared resource). Только удаляет ассоциации в `host_credentials`.

### Host groups

#### `group_list`

Возвращает все группы плоским списком (UI сам строит дерево по `parent_id`).

**Request**: `()`
**Response**: `{ groups: Vec<HostGroupDto> }`

#### `group_create`

```rust
pub struct GroupCreateRequest {
    pub name: String,
    pub parent_id: Option<GroupId>,
}
```

**Errors**: `Validation`, `Conflict` (duplicate name in parent).

#### `group_rename`, `group_move`, `group_delete`

Self-explanatory. `group_move` валидирует отсутствие циклов.

### Credentials

#### `credential_list`

**Request**: `()`
**Response**:
```rust
#[derive(Debug, Serialize)]
pub struct CredentialDto {
    pub id: CredentialId,
    pub name: String,
    pub kind: CredentialKind,
    pub username: String,
    // НЕТ keychain_ref, НЕТ секрета.
    pub created_at: String,
    pub updated_at: String,
}
```

#### `credential_create`

```rust
pub struct CredentialCreateRequest {
    pub name: String,
    pub kind: CredentialKind,
    pub username: String,
    pub secret: Option<SecretInput>,             // None только для kind=SshKeyAgent
    pub passphrase: Option<SecretInput>,         // только для kind=SshKey с зашифрованным ключом
}

/// Тонкая обёртка над байтами секрета.
/// Существует на границе IPC; внутри Rust сразу превращается в Zeroizing<Vec<u8>>.
#[derive(Deserialize)]
#[serde(transparent)]
pub struct SecretInput(pub String);              // base64-encoded
```

**Поведение**:
1. Декодирует base64.
2. Создаёт credential record в БД (transaction begin).
3. Записывает секрет в keychain под `remotehub.<new_id>`.
4. Если passphrase передан — отдельная запись `remotehub.<new_id>.passphrase`.
5. Commit транзакции БД.
6. На любой ошибке — rollback БД + попытка cleanup keychain.

**Response**: `{ id: CredentialId }`
**Errors**: `Validation`, `Storage`, `Secret`.

#### `credential_update`

Обновление **метаданных** (name, username). **Секрет НЕ меняется** — для смены секрета используется `credential_rotate_secret`. Это сознательная сегрегация: если случайно отправить пустой `secret`, нельзя стереть keychain-запись.

#### `credential_rotate_secret`

```rust
pub struct CredentialRotateSecretRequest {
    pub id: CredentialId,
    pub secret: SecretInput,
    pub passphrase: Option<SecretInput>,
}
```

Перезаписывает keychain-запись. UI вызывает только из специального диалога "Change secret".

#### `credential_delete`

**Request**: `{ id: CredentialId }`
**Response**: `()`

Удаляет запись из БД и keychain. Каскадно удаляет ассоциации в `host_credentials`. `hosts.default_credential_id` для затронутых хостов обнуляется.

#### `credential_link_host`

Создаёт ассоциацию host ↔ credential. **Request**: `{ host_id, credential_id, set_as_default: bool }`.

#### `credential_unlink_host`

Удаляет ассоциацию.

### Sessions

#### `session_open`

```rust
pub struct SessionOpenRequest {
    pub host_id: HostId,
    pub credential_id: Option<CredentialId>,         // None = использовать default
    pub options: SessionOpenOptions,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "protocol", rename_all = "lowercase")]
pub enum SessionOpenOptions {
    Ssh {
        cols: u16,
        rows: u16,
        term: String,                                 // например "xterm-256color"
    },
    Rdp {
        width: u16,
        height: u16,
        color_depth: ColorDepth,                      // 16 | 24 | 32
        keyboard_layout: String,                      // "en-US", "ru-RU", ...
    },
}
```

**Response**:
```rust
#[derive(Debug, Serialize)]
pub struct SessionOpenResponse {
    pub session_id: SessionId,
    pub event_channel: String,        // имя event-channel'а: "session_event:<session_id>"
}
```

**Errors**: `NotFound` (host/credential), `Validation` (protocol mismatch), `Session` (auth/network failure при initial handshake — но обычно auth-фейл прилетает в event-stream, см. ниже).

**Semantics**: команда возвращается, как только session actor стартовал. Реальное состояние подключения (Connecting → Authenticated → Ready / Failed) — приходит асинхронно в event-channel. UI показывает spinner до первого `Ready` события.

#### `session_close`

**Request**: `{ session_id }`
**Response**: `()`

Graceful shutdown: SSH — close channel, send disconnect; RDP — RDP disconnect PDU. Если actor уже мёртв — no-op.

#### `session_send_input` (SSH)

```rust
pub struct SessionInputRequest {
    pub session_id: SessionId,
    pub data: Vec<u8>,                    // raw bytes от xterm.js onData
}
```

**Response**: `()`

#### `session_send_input` (RDP)

Для RDP используется другой command — `session_rdp_input`:

```rust
pub struct RdpInputRequest {
    pub session_id: SessionId,
    pub event: RdpInputEvent,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RdpInputEvent {
    MouseMove { x: u16, y: u16 },
    MouseButton { button: MouseButton, pressed: bool, x: u16, y: u16 },
    MouseWheel { delta: i16, x: u16, y: u16 },
    Key { scancode: u32, pressed: bool, extended: bool },
    SyncMods { caps_lock: bool, num_lock: bool, scroll_lock: bool },
}
```

Раздельные команды по протоколам — потому что payload-семантика разная (байты vs. структурированные события), и смешивание в общий `session_input` ведёт к слабой типизации на UI.

#### `session_resize`

```rust
pub struct SessionResizeRequest {
    pub session_id: SessionId,
    pub width: u16,                       // символы (SSH) или пиксели (RDP)
    pub height: u16,
}
```

Для SSH — изменяет PTY size (`SIGWINCH`). Для RDP — посылает Server Redirection PDU с новым разрешением (если сервер не поддерживает — no-op + предупреждение).

#### `session_list`

```rust
pub struct SessionListResponse {
    pub sessions: Vec<SessionInfoDto>,
}

pub struct SessionInfoDto {
    pub id: SessionId,
    pub host_id: HostId,
    pub host_name: String,
    pub protocol: Protocol,
    pub state: SessionState,
    pub opened_at: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Connecting,
    Authenticating,
    Ready,
    Disconnecting,
    Closed,
    Failed,
}
```

UI вызывает на startup, чтобы восстановить таб-bar (если приложение пережило WebView-перезагрузку, что у Tauri возможно при dev hot-reload; в проде — на старте сессий нет).

### Settings

#### `settings_get_all`

Возвращает все настройки разом как map.

#### `settings_update`

```rust
pub struct SettingsUpdateRequest {
    pub patches: HashMap<String, serde_json::Value>,
}
```

Partial-update: обновляются только присланные ключи. Невалидный JSON для известного ключа → `Validation`.

### Known hosts (SSH)

#### `known_hosts_list`

Список fingerprint'ов из `known_hosts`.

#### `known_hosts_accept`

Подтвердить TOFU-fingerprint. Используется UI-диалогом при первом подключении к новому хосту.

```rust
pub struct KnownHostsAcceptRequest {
    pub session_id: SessionId,            // ссылка на pending-session
    pub fingerprint_sha256: String,
}
```

После accept — session actor продолжает handshake. Если reject — actor завершается с `Session::HostKeyRejected`.

#### `known_hosts_remove`

Удалить запись (для случая «сменили ключ на сервере, разрешаю»).

### Misc

#### `app_version`

`() -> { version: String, commit: String, target: String }`. Для about-dialog.

#### `import_hosts`

```rust
pub struct ImportRequest {
    pub format: ImportFormat,             // только "remotehub_json" в MVP
    pub data: String,
}
```

Создаёт hosts + groups из JSON-дампа. Credentials НЕ импортируются (нет секретов в экспорте).

#### `export_hosts`

`() -> { format: "remotehub_json", data: String }`. JSON-объект со списком hosts + groups; credentials присутствуют как **stub'ы** (id + name + kind), без секретов. UI на импорте показывает: "восстановить ассоциации credentials → нужно создать вручную".

## Events

События идут одним из двух способов:

1. **Глобальные** (через `emit_all`) — например, `hosts:changed` после CRUD. Подписка: `listen("hosts:changed", ...)`.
2. **Per-session** (через `Channel<T>`, см. ниже) — high-throughput data flow.

### Global events

| Event | Payload | Когда |
|---|---|---|
| `hosts:changed` | `{ kind: "created" \| "updated" \| "deleted", id: HostId }` | После любого host CRUD |
| `groups:changed` | `{ kind: "...", id: GroupId }` | После group CRUD |
| `credentials:changed` | `{ kind: "...", id: CredentialId }` | После credential CRUD |
| `settings:changed` | `{ keys: Vec<String> }` | После `settings_update` |
| `app:notice` | `{ severity: "info" \| "warn" \| "error", message: String }` | Backend-инициированные нотификации (например, "keychain unavailable") |

UI подписывается один раз на старте, инвалидирует local cache и/или рефетчит данные.

### Session events

Per-session высокочастотный поток. Используем Tauri 2 **Channel API** для нулевого overhead'а (без JSON-сериализации на каждое сообщение, бинарные frames возможны).

```rust
// Schema for SSH session events:
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SshSessionEvent {
    StateChanged { state: SessionState },
    Data { bytes: Vec<u8> },                          // PTY output
    AuthFailed { method: String },
    HostKeyPrompt { fingerprint_sha256: String, key_type: String },
    Error { message: String },                        // terminal error
    Closed { reason: CloseReason },
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RdpSessionEvent {
    StateChanged { state: SessionState },
    Frame { region: FrameRegion, data: Vec<u8> },     // RGBA / BGRA blob, см. session-protocol.md
    PointerShape { hotspot_x: u16, hotspot_y: u16, width: u16, height: u16, data: Vec<u8> },
    Clipboard { mime: String, data: Vec<u8> },
    CertPrompt { fingerprint_sha256: String, subject: String },
    Error { message: String },
    Closed { reason: CloseReason },
}

#[derive(Debug, Serialize)]
pub struct FrameRegion {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CloseReason {
    UserRequested,
    ServerDisconnected { message: Option<String> },
    NetworkError { message: String },
    AuthFailed,
    HostKeyRejected,
    Crashed { message: String },
}
```

Channel создаётся при `session_open` и возвращается UI как handle. UI делает:

```typescript
const { sessionId, channel } = await invoke<SessionOpenResponse>("session_open", { ... });
channel.onmessage = (evt: SshSessionEvent) => { ... };
```

## Versioning

В альфе не версионируем. Поле `app_version` возвращает текущую версию приложения; UI и backend всегда поставляются вместе (это desktop-приложение, не клиент-сервер). Schema-mismatch невозможен.

## Open Questions

1. **`session_send_input` для SSH принимает `Vec<u8>`** — это в IPC превращается в JSON-array из чисел, что неэффективно для крупных paste'ов. Альтернатива — base64-encoded String. **Предложение**: в MVP используем Channel API в обратную сторону (UI → Rust также через Channel), но это требует проверки на Tauri 2 — поддерживает ли он bi-directional Channels. Если нет — base64 для input. Уточнить на этапе реализации SSH.
2. **Прогресс-событие при долгом RDP-handshake** (TLS + NLA может занять 1-2 секунды на медленной сети). Сейчас только `StateChanged`. **Предложение**: добавить `StateChanged { state, detail: Option<String> }` — `detail` несёт human-readable ("Negotiating TLS", "Authenticating"). Не критично для MVP, но дешёво добавить.
3. **Batch API для CRUD?** Например, `host_create_many`. **Предложение**: НЕТ в MVP, добавим при первой жалобе на UI lag.

## Assumptions

- UI и backend всегда одной версии (моно-репо, один installer).
- Все секреты приходят в IPC как base64-encoded строки. Channel API для секретов не используется (не нужно — это редкие, не high-throughput события).
- Tauri command-handler'ы выполняются в `tokio` runtime'е (Tauri 2 это даёт из коробки).

## Related specs

- `system-overview.md` — общий контекст.
- `data-model.md` — domain-сущности, на которые мапятся DTO.
- `session-protocol.md` — детали actor-модели сессий, lifecycle.
