/**
 * Typed wrapper around `tauri::invoke()`.
 *
 * Components MUST go through this module — never call `invoke()` directly.
 * Reasons:
 *
 * 1. **Type safety**: each command is typed end-to-end. Renaming a field
 *    surfaces at compile time, not at runtime.
 * 2. **Substitution**: this module can be swapped for a mock in Storybook
 *    or in tests without touching components.
 * 3. **Centralized error normalization**: any cross-cutting concerns
 *    (telemetry, retries, logging) land here.
 *
 * Each function corresponds 1:1 to a Tauri command in `crates/rh-app/src/api/`.
 */

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type {
    AppVersionResponse,
    ChangePayload,
    CredentialCreateRequest,
    CredentialId,
    CredentialLinkRequest,
    CredentialListResponse,
    CredentialRevealResponse,
    CredentialRotateSecretRequest,
    CredentialUpdateRequest,
    GroupCreateRequest,
    GroupId,
    GroupListResponse,
    GroupMoveRequest,
    GroupRenameRequest,
    HostCreateRequest,
    HostFullDto,
    HostId,
    HostListRequest,
    HostListResponse,
    HostUpdateRequest,
    IdResponse,
    SettingsChangedPayload,
    SettingsGetAllResponse,
    SettingsUpdateRequest,
} from "./types";
import {
    EVENT_CREDENTIALS_CHANGED,
    EVENT_GROUPS_CHANGED,
    EVENT_HOSTS_CHANGED,
    EVENT_SETTINGS_CHANGED,
} from "./types";

// =====================================================================
// Internal: wrap each command in a tiny call() to centralize the
// "req" envelope shape that Tauri injects parameters under.
//
// Note: Tauri 2 invoke arg style depends on how the Rust command
// declares its parameters. Our Rust handlers take a single
// `req: SomeRequest` argument, so we wrap as { req: payload }.
// Commands taking no arg get {}.
// =====================================================================

async function call<TResp>(name: string, req?: unknown): Promise<TResp> {
    return invoke<TResp>(name, req !== undefined ? { req } : {});
}

// =====================================================================
// Hosts
// =====================================================================

export const hosts = {
    list: (req: HostListRequest = {}): Promise<HostListResponse> =>
        call("host_list", req),

    get: (id: HostId): Promise<HostFullDto> => call("host_get", { id }),

    create: (req: HostCreateRequest): Promise<IdResponse<HostId>> =>
        call("host_create", req),

    update: (req: HostUpdateRequest): Promise<void> => call("host_update", req),

    delete: (id: HostId): Promise<void> => call("host_delete", { id }),
};

// =====================================================================
// Groups
// =====================================================================

export const groups = {
    list: (): Promise<GroupListResponse> => call("group_list"),

    create: (req: GroupCreateRequest): Promise<IdResponse<GroupId>> =>
        call("group_create", req),

    rename: (req: GroupRenameRequest): Promise<void> => call("group_rename", req),

    move: (req: GroupMoveRequest): Promise<void> => call("group_move", req),

    delete: (id: GroupId): Promise<void> => call("group_delete", { id }),
};

// =====================================================================
// Credentials
// =====================================================================

export const credentials = {
    list: (): Promise<CredentialListResponse> => call("credential_list"),

    create: (req: CredentialCreateRequest): Promise<IdResponse<CredentialId>> =>
        call("credential_create", req),

    update: (req: CredentialUpdateRequest): Promise<void> =>
        call("credential_update", req),

    rotateSecret: (req: CredentialRotateSecretRequest): Promise<void> =>
        call("credential_rotate_secret", req),

    delete: (id: CredentialId): Promise<void> => call("credential_delete", { id }),

    reveal: (id: CredentialId): Promise<CredentialRevealResponse> =>
        call("credential_reveal", { id }),

    linkHost: (req: CredentialLinkRequest): Promise<void> =>
        call("credential_link_host", req),

    unlinkHost: (req: { host_id: HostId; credential_id: CredentialId }): Promise<void> =>
        call("credential_unlink_host", req),
};

// =====================================================================
// Settings
// =====================================================================

export const settings = {
    getAll: (): Promise<SettingsGetAllResponse> => call("settings_get_all"),

    update: (req: SettingsUpdateRequest): Promise<void> =>
        call("settings_update", req),
};

// =====================================================================
// Meta
// =====================================================================

export const meta = {
    appVersion: (): Promise<AppVersionResponse> => call("app_version"),
};

// =====================================================================
// Event subscriptions
// =====================================================================

export const events = {
    onHostsChanged: (handler: (payload: ChangePayload<HostId>) => void): Promise<UnlistenFn> =>
        listen<ChangePayload<HostId>>(EVENT_HOSTS_CHANGED, (e) => handler(e.payload)),

    onGroupsChanged: (handler: (payload: ChangePayload<GroupId>) => void): Promise<UnlistenFn> =>
        listen<ChangePayload<GroupId>>(EVENT_GROUPS_CHANGED, (e) => handler(e.payload)),

    onCredentialsChanged: (
        handler: (payload: ChangePayload<CredentialId>) => void,
    ): Promise<UnlistenFn> =>
        listen<ChangePayload<CredentialId>>(EVENT_CREDENTIALS_CHANGED, (e) =>
            handler(e.payload),
        ),

    onSettingsChanged: (handler: (payload: SettingsChangedPayload) => void): Promise<UnlistenFn> =>
        listen<SettingsChangedPayload>(EVENT_SETTINGS_CHANGED, (e) => handler(e.payload)),
};

// =====================================================================
// Helpers
// =====================================================================

/**
 * Base64-encode a UTF-8 string for use as a secret payload.
 * Used by credential dialogs that take a password from a text input.
 */
export function encodeSecret(plaintext: string): string {
    // btoa expects "binary string" — Latin-1 bytes. For UTF-8 we go
    // via TextEncoder to get the raw bytes first.
    const bytes = new TextEncoder().encode(plaintext);
    let binary = "";
    bytes.forEach((b) => {
        binary += String.fromCharCode(b);
    });
    return btoa(binary);
}
