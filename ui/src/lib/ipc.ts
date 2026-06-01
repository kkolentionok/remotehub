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

import { Channel, invoke } from "@tauri-apps/api/core";
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
    FsListResponse,
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
    KnownHostGetResponse,
    KnownHostsListResponse,
    RdpCertsListResponse,
    IdResponse,
    SessionAcceptHostKeyRequest,
    SessionId,
    SessionInputRequest,
    SessionListResponse,
    SessionOpenRequest,
    SessionOpenResponse,
    SessionResizeRequest,
    SettingsChangedPayload,
    SettingsGetAllResponse,
    SettingsUpdateRequest,
    RdpInputRequest,
    RdpSessionEvent,
    SshSessionEvent,
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

    /** The pinned SSH host key (TOFU) for a host, or `{ key: null }`. */
    knownHostKey: (id: HostId): Promise<KnownHostGetResponse> =>
        call("known_host_get", { id }),

    create: (req: HostCreateRequest): Promise<IdResponse<HostId>> =>
        call("host_create", req),

    update: (req: HostUpdateRequest): Promise<void> => call("host_update", req),

    delete: (id: HostId): Promise<void> => call("host_delete", { id }),
};

// =====================================================================
// Known hosts (TOFU pin management)
// =====================================================================

export const knownHosts = {
    list: (): Promise<KnownHostsListResponse> => call("known_hosts_list"),

    forget: (hostname: string, port: number): Promise<void> =>
        call("known_host_forget", { hostname, port }),
};

// =====================================================================
// RDP trusted certificates (TOFU pin management)
// =====================================================================

export const rdpCerts = {
    list: (): Promise<RdpCertsListResponse> => call("rdp_certs_list"),

    forget: (hostname: string, port: number): Promise<void> =>
        call("rdp_cert_forget", { hostname, port }),
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
// Sessions (Stage 2)
//
// `open` creates a Tauri Channel for the high-throughput event stream
// (PTY output, state changes) and passes it alongside the request. The
// Rust actor pushes `SshSessionEvent`s to it. Input/resize/close go back
// through ordinary commands keyed by session id.
// =====================================================================

export const sessions = {
    open: (
        req: SessionOpenRequest,
        onEvent: (e: SshSessionEvent) => void,
    ): Promise<SessionOpenResponse> => {
        const channel = new Channel<SshSessionEvent>();
        channel.onmessage = onEvent;
        return invoke<SessionOpenResponse>("session_open", { req, onEvent: channel });
    },

    close: (sessionId: SessionId): Promise<void> =>
        call("session_close", { session_id: sessionId }),

    sendInput: (req: SessionInputRequest): Promise<void> =>
        call("session_send_input", req),

    resize: (req: SessionResizeRequest): Promise<void> =>
        call("session_resize", req),

    acceptHostKey: (req: SessionAcceptHostKeyRequest): Promise<void> =>
        call("session_accept_host_key", req),

    rejectHostKey: (sessionId: SessionId): Promise<void> =>
        call("session_reject_host_key", { session_id: sessionId }),

    list: (): Promise<SessionListResponse> => call("session_list"),

    /** Re-bind a live session (surviving a webview reload) to a fresh
     *  channel; the backend replays buffered output. Resolves to false if
     *  the session is already gone. */
    reattach: (
        sessionId: SessionId,
        onEvent: (e: SshSessionEvent) => void,
    ): Promise<boolean> => {
        const channel = new Channel<SshSessionEvent>();
        channel.onmessage = onEvent;
        return invoke<boolean>("session_reattach", {
            req: { session_id: sessionId },
            onEvent: channel,
        });
    },
};

// =====================================================================
// RDP sessions (Stage 4) — separate event type, separate registry.
// =====================================================================

export const rdpSession = {
    open: (
        req: SessionOpenRequest,
        onEvent: (e: RdpSessionEvent) => void,
    ): Promise<SessionOpenResponse> => {
        const channel = new Channel<RdpSessionEvent>();
        channel.onmessage = onEvent;
        return invoke<SessionOpenResponse>("rdp_session_open", { req, onEvent: channel });
    },

    close: (sessionId: SessionId): Promise<void> =>
        call("rdp_session_close", { session_id: sessionId }),

    sendInput: (req: RdpInputRequest): Promise<void> =>
        call("rdp_session_input", req),
};

export const localSession = {
    /** Open a local shell PTY. Emits the same SshSessionEvent stream as SSH. */
    open: (
        cols: number,
        rows: number,
        onEvent: (e: SshSessionEvent) => void,
    ): Promise<SessionOpenResponse> => {
        const channel = new Channel<SshSessionEvent>();
        channel.onmessage = onEvent;
        return invoke<SessionOpenResponse>("local_session_open", {
            req: { cols, rows },
            onEvent: channel,
        });
    },

    close: (sessionId: SessionId): Promise<void> =>
        call("local_session_close", { session_id: sessionId }),

    input: (req: SessionInputRequest): Promise<void> =>
        call("local_session_input", req),

    resize: (req: SessionResizeRequest): Promise<void> =>
        call("local_session_resize", req),
};

export const localFs = {
    home: (): Promise<FsListResponse> => call("fs_home"),
    drives: (): Promise<FsListResponse> => call("fs_drives"),
    list: (path: string): Promise<FsListResponse> => call("fs_list", { path }),
    rename: (path: string, newName: string): Promise<void> =>
        call("fs_rename", { path, new_name: newName }),
    remove: (path: string, isDir: boolean): Promise<void> =>
        call("fs_remove", { path, is_dir: isDir }),
    mkdir: (parent: string, name: string): Promise<void> =>
        call("fs_mkdir", { parent, name }),
};

export const sftp = {
    open: (hostId: HostId): Promise<SessionOpenResponse> =>
        call("sftp_open", { host_id: hostId }),
    /** List a remote dir. Empty/"." path resolves to the login directory.
     *  Response shape matches FsListResponse (name/path/is_dir/size). */
    list: (sessionId: SessionId, path: string): Promise<FsListResponse> =>
        call("sftp_list", { session_id: sessionId, path }),
    close: (sessionId: SessionId): Promise<void> =>
        call("sftp_close", { session_id: sessionId }),
    download: (sessionId: SessionId, remotePath: string, localDir: string): Promise<void> =>
        call("sftp_download", {
            session_id: sessionId,
            remote_path: remotePath,
            local_dir: localDir,
        }),
    upload: (sessionId: SessionId, localPath: string, remoteDir: string): Promise<void> =>
        call("sftp_upload", {
            session_id: sessionId,
            local_path: localPath,
            remote_dir: remoteDir,
        }),
    copy: (
        fromSession: SessionId,
        remotePath: string,
        toSession: SessionId,
        remoteDir: string,
    ): Promise<void> =>
        call("sftp_copy", {
            from_session: fromSession,
            remote_path: remotePath,
            to_session: toSession,
            remote_dir: remoteDir,
        }),
    rename: (sessionId: SessionId, path: string, newName: string): Promise<void> =>
        call("sftp_rename", { session_id: sessionId, path, new_name: newName }),
    remove: (sessionId: SessionId, path: string, isDir: boolean): Promise<void> =>
        call("sftp_remove", { session_id: sessionId, path, is_dir: isDir }),
    transfer: (
        req: {
            transfer_id: string;
            kind: "download" | "upload" | "copy";
            session_id: SessionId;
            to_session?: SessionId;
            src_path: string;
            dst_dir: string;
            dst_name?: string;
        },
        onProgress: (bytes: number) => void,
    ): Promise<void> => {
        const channel = new Channel<number>();
        channel.onmessage = onProgress;
        return invoke("sftp_transfer", { req, onProgress: channel });
    },
    transferCancel: (transferId: string): Promise<void> =>
        call("sftp_transfer_cancel", { transfer_id: transferId }),
    mkdir: (sessionId: SessionId, parent: string, name: string): Promise<void> =>
        call("sftp_mkdir", { session_id: sessionId, parent, name }),
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
