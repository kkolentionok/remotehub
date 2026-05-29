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
    tags: string[];
    color: string | null;
    detected_os: string | null;
    default_credential_id: CredentialId | null;
    created_at: string;
    updated_at: string;
}

export interface HostFullDto extends HostDto {
    notes: string | null;
    startup_command: string | null;
    env_vars: EnvVar[];
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
    | { kind: "host_key_prompt"; fingerprint_sha256: string; key_type: string }
    | { kind: "error"; message: string }
    | { kind: "closed"; reason: CloseReason };

export type SessionOpenOptions = {
    protocol: "ssh";
    cols: number;
    rows: number;
    term: string;
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

// =====================================================================
// Settings
// =====================================================================

export type Language = "en" | "ru";
export type Theme = "light" | "dark" | "system";
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
    tags?: string[] | null;
    color?: string | null;
    notes?: string | null;
    startup_command?: string | null;
    env_vars?: EnvVar[] | null;
    default_credential_id?: CredentialId | null;
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
    tags?: string[];
    color?: string | null;
    notes?: string | null;
    startup_command?: string | null;
    env_vars?: EnvVar[];
    default_credential_id?: CredentialId | null;
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
