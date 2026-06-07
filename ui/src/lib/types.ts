/**
 * Types mirroring the DTOs defined in `crates/rh-app/src/api/dto.rs`.
 *
 * These are kept in sync with the Rust side by hand. Any change there
 * needs a matching change here. Future direction (post-MVP): generate
 * from a shared schema. For now, hand-maintenance is cheaper than
 * the codegen infrastructure.
 */

// =====================================================================
// Identifiers (opaque strings; ULIDs server-side)
// =====================================================================

export type HostId = string;
export type CredentialId = string;
export type GroupId = string;
export type SessionId = string;

// =====================================================================
// Enums
// =====================================================================

export type Protocol = "ssh" | "rdp";
export type CredentialKind = "password" | "ssh_key" | "ssh_key_agent";

// =====================================================================
// Entities
// =====================================================================

export interface EnvVar {
    key: string;
    value: string;
}

export interface HostDto {
    id: HostId;
    name: string;
    display_name: string | null;
    group_id: GroupId | null;
    protocol: Protocol;
    hostname: string;
    port: number;
    username: string;
    tags: string[];
    color: string | null;
    detected_os: string | null;
    default_credential_id: CredentialId | null;
    jump_host_id: HostId | null;
    agent_forwarding: boolean;
    favorite: boolean;
    last_connected_at: string | null;
    created_at: string;
    updated_at: string;
}

export interface HostFullDto extends HostDto {
    notes: string | null;
    startup_command: string | null;
    env_vars: EnvVar[];
    /** All credentials linked to this host (default first). */
    credential_ids: CredentialId[];
}

export interface HostGroupDto {
    id: GroupId;
    name: string;
    parent_id: GroupId | null;
    created_at: string;
}

export interface CredentialDto {
    id: CredentialId;
    name: string;
    kind: CredentialKind;
    username: string;
    created_at: string;
    updated_at: string;
}

// =====================================================================
// Sessions (Stage 2)
// =====================================================================

export type SessionState =
    | "resolving"
    | "connecting"
    | "authenticating"
    | "host_key_pending"
    | "ready"
    | "disconnecting"
    | "closed"
    | "failed";

export type CloseReason =
    | { kind: "user_requested" }
    | { kind: "server_disconnected"; message: string | null }
    | { kind: "network_error"; message: string }
    | { kind: "auth_failed" }
    | { kind: "host_key_rejected" }
    | { kind: "crashed"; message: string };

/** Events pushed from the Rust SSH actor over a Tauri Channel. */
export type SshSessionEvent =
    | { kind: "state_changed"; state: SessionState }
    | { kind: "data"; bytes: number[] }
    | { kind: "auth_failed"; method: string }
    | { kind: "host_key_prompt"; fingerprint_sha256: string; key_type: string; changed: boolean }
    | { kind: "error"; message: string }
    | { kind: "closed"; reason: CloseReason };

export type SessionOpenOptions =
    | {
          protocol: "ssh";
          cols: number;
          rows: number;
          term: string;
      }
    | {
          protocol: "rdp";
          width: number;
          height: number;
          color_depth: number;
          keyboard_layout: string;
      };

export interface SessionOpenRequest {
    host_id: HostId;
    credential_id?: CredentialId | null;
    options: SessionOpenOptions;
}

export interface SessionOpenResponse {
    session_id: SessionId;
    event_channel?: string;
}

export interface SessionInputRequest {
    session_id: SessionId;
    data: number[];
}

export interface SessionResizeRequest {
    session_id: SessionId;
    width: number;
    height: number;
}

export interface SessionAcceptHostKeyRequest {
    session_id: SessionId;
    fingerprint: string;
}

/** One live session returned by `session_list` (restore-on-reload). */
export interface SessionSummaryDto {
    session_id: SessionId;
    host_id: HostId;
    hostname: string;
    title: string;
    protocol: Protocol;
    state: SessionState;
    /** RFC 3339 timestamp. */
    opened_at: string;
}

export interface SessionListResponse {
    sessions: SessionSummaryDto[];
}

export interface LocalSessionSummaryDto {
    session_id: SessionId;
    title: string;
}

export interface LocalSessionListResponse {
    sessions: LocalSessionSummaryDto[];
}

export interface SessionReattachRequest {
    session_id: SessionId;
}

export interface KnownHostKeyDto {
    key_type: string;
    fingerprint_sha256: string;
}

export interface KnownHostGetResponse {
    key: KnownHostKeyDto | null;
}

export interface KnownHostEntryDto {
    hostname: string;
    port: number;
    key_type: string;
    fingerprint_sha256: string;
    created_at: string;
}

export interface KnownHostsListResponse {
    entries: KnownHostEntryDto[];
}

export interface RdpCertEntryDto {
    hostname: string;
    port: number;
    fingerprint_sha256: string;
    subject: string;
    trusted_at: string;
}

export interface RdpCertsListResponse {
    entries: RdpCertEntryDto[];
}

// =====================================================================
// Settings
// =====================================================================

export type Language = "en" | "ru";
export type Theme = "light" | "dark" | "navy" | "redpanda" | "system";
export type CursorStyle = "block" | "underline" | "bar";
export type TerminalColorScheme =
    | "default"
    | "solarized-dark"
    | "solarized-light"
    | "dracula"
    | "nord"
    | "pro"
    | "light"
    | "kanagawa"
    | "octocat"
    | "material-dark"
    | "homebrew"
    | "redpanda-dark"
    | "redpanda-light";
export type StartupScreen = "home" | "last_hosts";

export type RdpResolution =
    | { kind: "fit" }
    | { kind: "fixed"; width: number; height: number };

/** Summary returned by vault_import after the merge + write-back. */
export interface VaultImportResponse {
    hosts: number;
    groups: number;
    credentials: number;
    deleted: number;
}

/** A vault file read from disk via the native Open dialog (vault_read_file). */
export interface VaultFileResponse {
    body: string;
    name: string;
    size: number;
}

/** Current sync endpoint/account state (sync_get_config). */
export interface SyncConfigResponse {
    endpoint: string;
    email: string | null;
    logged_in: boolean;
}

/** Result of one sync pass (sync_now). */
export interface SyncNowResponse {
    had_remote: boolean;
    pushed_version: string;
    hosts: number;
    groups: number;
    credentials: number;
    deleted: number;
}

export interface Settings {
    language: Language;
    theme: Theme;
    default_ssh_port: number;
    default_rdp_port: number;
    terminal_font_family: string;
    terminal_font_size: number;
    terminal_color_scheme: TerminalColorScheme;
    terminal_cursor_style: CursorStyle;
    terminal_scrollback: number;
    rdp_default_resolution: RdpResolution;
    app_confirm_close_session: boolean;
    app_startup_screen: StartupScreen;
    ssh_keepalive_interval_secs: number;
    ssh_known_hosts_strict: boolean;
    local_shell: string;
    rdp_gfx: boolean;
}

// =====================================================================
// Request payloads (UI → Rust)
// =====================================================================

export interface HostListRequest {
    group_id?: GroupId | null;
    protocol?: Protocol | null;
    search?: string | null;
    limit?: number | null;
}

export interface HostCreateRequest {
    name: string;
    display_name?: string | null;
    group_id?: GroupId | null;
    protocol: Protocol;
    hostname: string;
    port?: number | null;
    username?: string | null;
    tags?: string[] | null;
    color?: string | null;
    notes?: string | null;
    startup_command?: string | null;
    env_vars?: EnvVar[] | null;
    default_credential_id?: CredentialId | null;
    jump_host_id?: HostId | null;
    agent_forwarding?: boolean;
    favorite?: boolean;
}

/**
 * PATCH semantics: each field is one of
 *   - missing (don't touch)
 *   - present with value (set)
 *   - present with null (clear)
 *
 * In TypeScript we encode the last two as `T | null`; "missing" is
 * absence of the key. Caller is expected to omit keys they don't
 * want to change.
 */
export interface HostUpdateRequest {
    id: HostId;
    name?: string;
    display_name?: string | null;
    group_id?: GroupId | null;
    protocol?: Protocol;
    hostname?: string;
    port?: number;
    username?: string;
    tags?: string[];
    color?: string | null;
    notes?: string | null;
    startup_command?: string | null;
    env_vars?: EnvVar[];
    default_credential_id?: CredentialId | null;
    jump_host_id?: HostId | null;
    agent_forwarding?: boolean;
    favorite?: boolean;
}

export interface GroupCreateRequest {
    name: string;
    parent_id?: GroupId | null;
}

export interface GroupRenameRequest {
    id: GroupId;
    name: string;
}

export interface GroupMoveRequest {
    id: GroupId;
    parent_id?: GroupId | null;
}

export interface CredentialCreateRequest {
    name: string;
    kind: CredentialKind;
    username: string;
    /** Base64-encoded bytes. Required for password / ssh_key. */
    secret?: string;
    /** Base64-encoded bytes. Only for ssh_key with encrypted private key. */
    passphrase?: string;
}

export interface CredentialUpdateRequest {
    id: CredentialId;
    name?: string;
    username?: string;
}

export interface CredentialRotateSecretRequest {
    id: CredentialId;
    secret: string;
    passphrase?: string;
}

export interface CredentialLinkRequest {
    host_id: HostId;
    credential_id: CredentialId;
    set_as_default?: boolean;
}

export interface SettingsUpdateRequest {
    patches: Record<string, unknown>;
}

// =====================================================================
// Response payloads (Rust → UI)
// =====================================================================

export interface HostListResponse {
    hosts: HostDto[];
    total: number;
}

export interface GroupListResponse {
    groups: HostGroupDto[];
}

export interface CredentialListResponse {
    credentials: CredentialDto[];
}

/** Response from credential_reveal. `secret` null for ssh_key_agent. */
export interface CredentialRevealResponse {
    kind: CredentialKind;
    username: string;
    secret: string | null;
}

export interface SettingsGetAllResponse {
    settings: Settings;
}

export interface IdResponse<TId = string> {
    id: TId;
}

export interface AppVersionResponse {
    version: string;
    target: string;
}

// =====================================================================
// Errors
// =====================================================================

/**
 * Discriminated union mirroring `ApiError` on the Rust side.
 * Tauri rejects the invoke() promise with one of these.
 */
export type ApiError =
    | { kind: "not_found"; entity: string }
    | { kind: "validation"; field: string; reason: string }
    | { kind: "storage"; message: string }
    | { kind: "secret"; message: string }
    | { kind: "session"; message: string }
    | { kind: "conflict"; message: string }
    | { kind: "internal"; message: string }
    | { kind: "not_implemented"; feature: string };

/** Type guard for ApiError. */
export function isApiError(value: unknown): value is ApiError {
    return (
        typeof value === "object" &&
        value !== null &&
        "kind" in value &&
        typeof (value as { kind: unknown }).kind === "string"
    );
}

/** Human-readable rendering of an ApiError. */
export function formatApiError(err: unknown): string {
    if (!isApiError(err)) {
        return typeof err === "string" ? err : "Unknown error";
    }
    switch (err.kind) {
        case "not_found":
            return `${err.entity} not found`;
        case "validation":
            return `${err.field}: ${err.reason}`;
        case "conflict":
            return err.message;
        case "not_implemented":
            return `Not implemented yet: ${err.feature}`;
        case "storage":
        case "secret":
        case "session":
        case "internal":
            return err.message;
    }
}

// =====================================================================
// Events (Rust → UI via tauri::emit)
// =====================================================================

export type ChangeKind = "created" | "updated" | "deleted";

export interface ChangePayload<TId = string> {
    kind: ChangeKind;
    id: TId;
}

export interface SettingsChangedPayload {
    keys: string[];
}

export const EVENT_HOSTS_CHANGED = "hosts:changed";
export const EVENT_GROUPS_CHANGED = "groups:changed";
export const EVENT_CREDENTIALS_CHANGED = "credentials:changed";
export const EVENT_SETTINGS_CHANGED = "settings:changed";

// =====================================================================
// RDP session contract (Stage 4) — mirrors crates/rh-rdp/src/lib.rs
// =====================================================================

export type RdpMouseButton = "left" | "middle" | "right";

/** Input the UI sends to the RDP actor. `kind`-tagged to match Rust. */
export type RdpInputEvent =
    | { kind: "mouse_move"; x: number; y: number }
    | {
          kind: "mouse_button";
          button: RdpMouseButton;
          pressed: boolean;
          x: number;
          y: number;
      }
    | { kind: "mouse_wheel"; delta: number; x: number; y: number }
    | { kind: "key"; code: string; pressed: boolean; repeat?: boolean }
    | {
          kind: "sync_modifiers";
          ctrl: boolean;
          alt: boolean;
          shift: boolean;
          meta: boolean;
          caps_lock: boolean;
          num_lock: boolean;
          scroll_lock: boolean;
      }
    | { kind: "release_all_modifiers" };

/** Request payload for the `rdp_session_input` command. */
export interface RdpInputRequest {
    session_id: SessionId;
    event: RdpInputEvent;
}

export type RdpPixelFormat = "bgra8" | "rgba8";
export type RdpState = "resolving" | "connecting" | "authenticating" | "ready" | "closed";

export interface RdpFrameRegion {
    x: number;
    y: number;
    width: number;
    height: number;
}

/** Events the RDP actor emits to the UI. `data` is the framebuffer
 *  region's pixels (length === width*height*4). */
export type RdpSessionEvent =
    | { kind: "state_changed"; state: RdpState }
    | {
          kind: "frame";
          region: RdpFrameRegion;
          format: RdpPixelFormat;
          data: number[] | Uint8Array;
      }
    | {
          kind: "frame_batch";
          tiles: {
              x: number;
              y: number;
              width: number;
              height: number;
              format: "png" | "jpeg";
              base64: string;
          }[];
      }
    | { kind: "resized"; width: number; height: number }
    | { kind: "pointer_position"; x: number; y: number }
    | {
          kind: "pointer_bitmap";
          width: number;
          height: number;
          hotspot_x: number;
          hotspot_y: number;
          rgba_base64: string;
      }
    | { kind: "pointer_hidden" }
    | { kind: "pointer_default" }
    | { kind: "cert_prompt"; fingerprint_sha256: string; subject: string }
    | { kind: "clipboard"; mime: string; data: string }
    | { kind: "clipboard_image"; width: number; height: number; rgba_base64: string }
    | { kind: "error"; message: string }
    | { kind: "closed"; reason: { kind: string; code?: number } };

// --- Local filesystem (SFTP left pane) ---
export interface FsEntry {
    name: string;
    path: string;
    is_dir: boolean;
    size: number;
    modified: number | null;
    perms: string | null;
}

export interface FsListResponse {
    path: string;
    parent: string | null;
    entries: FsEntry[];
}
