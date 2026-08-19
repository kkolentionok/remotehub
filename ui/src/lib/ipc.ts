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
    SshIdAddedKey,
    SshIdAvailableKey,
    SshIdCheck,
    SshIdData,
    Snippet,
    HostUpdateRequest,
    KnownHostGetResponse,
    KnownHostsListResponse,
    RdpCertsListResponse,
    IdResponse,
    SessionAcceptHostKeyRequest,
    SessionId,
    SessionInputRequest,
    SessionListResponse,
    LocalSessionListResponse,
    SessionOpenRequest,
    SessionOpenResponse,
    SessionResizeRequest,
    SettingsChangedPayload,
    SettingsGetAllResponse,
    SettingsUpdateRequest,
    RdpInputRequest,
    RdpSessionEvent,
    SshSessionEvent,
    VaultImportResponse,
    VaultFileResponse,
    SyncConfigResponse,
    SyncStatus,
    ForwardEvent,
    ForwardSaveRequest,
    ForwardSaveResponse,
    ForwardListResponse,
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

    /** Re-home a live session's frame stream to this webview (pop-out / re-dock).
     *  Resolves false if the session isn't live. */
    reattach: (
        sessionId: SessionId,
        onEvent: (e: RdpSessionEvent) => void,
    ): Promise<boolean> => {
        const channel = new Channel<RdpSessionEvent>();
        channel.onmessage = onEvent;
        return invoke<boolean>("rdp_session_reattach", {
            req: { session_id: sessionId },
            onEvent: channel,
        });
    },

    sendInput: (req: RdpInputRequest): Promise<void> =>
        call("rdp_session_input", req),

    /** Push the local clipboard text so it can be pasted into the remote. */
    setClipboard: (sessionId: SessionId, text: string): Promise<void> =>
        call("rdp_session_set_clipboard", { session_id: sessionId, text }),
    /** Push a local OS clipboard image (raw RGBA, base64) for remote paste. */
    setClipboardImage: (
        sessionId: SessionId,
        width: number,
        height: number,
        rgbaBase64: string,
    ): Promise<void> =>
        call("rdp_session_set_clipboard_image", {
            session_id: sessionId,
            width,
            height,
            rgba_base64: rgbaBase64,
        }),

    /** Request a DisplayControl dynamic resize (device px). */
    resize: (sessionId: SessionId, width: number, height: number): Promise<void> =>
        call("rdp_session_resize", { session_id: sessionId, width, height }),
    /** Toggle OS-level keyboard capture (fullscreen: route system keys to remote). */
    kbdCapture: (sessionId: SessionId, on: boolean): Promise<void> =>
        call("rdp_session_kbd_capture", { session_id: sessionId, on }),
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

    /** Live local shells (for restore-on-reload). */
    list: (): Promise<LocalSessionListResponse> => call("local_session_list"),

    /** Re-bind a live local shell to a fresh channel after a reload. */
    reattach: (
        sessionId: SessionId,
        onEvent: (e: SshSessionEvent) => void,
    ): Promise<boolean> => {
        const channel = new Channel<SshSessionEvent>();
        channel.onmessage = onEvent;
        return invoke<boolean>("local_session_reattach", {
            req: { session_id: sessionId },
            onEvent: channel,
        });
    },
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
            resume?: boolean;
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
    chmod: (sessionId: SessionId, path: string, mode: number): Promise<void> =>
        call("sftp_chmod", { session_id: sessionId, path, mode }),
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

// =====================================================================
// Vault / Sync (slice 1: export). See docs/specs/sync.md.
// =====================================================================

export const vault = {
    /** Export local state as a portable, E2E-encrypted vault string. */
    export: (master_password: string): Promise<string> =>
        call("vault_export", { master_password }),
    /** Decrypt + reconcile a vault export into local state. */
    import: (
        master_password: string,
        body: string,
        mode: "merge" | "replace" = "merge",
    ): Promise<VaultImportResponse> =>
        call("vault_import", { master_password, body, mode }),
    /** Write a vault export string to a path the user chose via the native
     *  Save dialog. */
    writeFile: (path: string, body: string): Promise<void> =>
        call("vault_write_file", { path, body }),
    /** Read a vault file the user chose via the native Open dialog. */
    readFile: (path: string): Promise<VaultFileResponse> =>
        call("vault_read_file", { path }),
};

// =====================================================================
// Sync (slice 3b: server transport). See docs/specs/sync.md §9.
// =====================================================================

export const sync = {
    /** Current endpoint/account + whether a bearer token is stored. */
    getConfig: (): Promise<SyncConfigResponse> => call("sync_get_config"),
    /** Set the server base URL. */
    setEndpoint: (endpoint: string): Promise<void> =>
        call("sync_set_endpoint", { endpoint }),
    /** Create an account on the configured server. */
    register: (email: string, password: string): Promise<void> =>
        call("sync_register", { email, password }),
    /** Log in; stores the bearer token in the OS keychain. */
    login: (email: string, password: string): Promise<void> =>
        call("sync_login", { email, password }),
    /** Forget the stored token. */
    logout: (): Promise<void> => call("sync_logout"),
    /** Desktop Yandex sign-in (opens the browser; resolves once the loopback
     *  catches the token). May take a while — the user completes consent. */
    oauthYandex: (): Promise<SyncConfigResponse> => call("sync_oauth_yandex"),
    /** Cache the vault (master) password so automatic sync runs unattended.
     *  Validated by a real pass — rejects on a wrong password. `persist` stores
     *  it in the OS keychain; when false it's kept in memory for this session. */
    setMaster: (master_password: string, persist: boolean): Promise<SyncConfigResponse> =>
        call("sync_set_master", { master_password, persist }),
    /** Current background-sync status (for first paint; live updates arrive
     *  via the `sync:status` event). */
    status: (): Promise<SyncStatus> => call("sync_status"),
};

// =====================================================================
// App control (tray session count + real quit)
// =====================================================================

export const app = {
    /** Report how many live session tabs are open (drives the tray tooltip
     *  and the quit-confirm check). NOTE: bypasses `call()` on purpose — these
     *  commands take top-level args (`count`), not the `{ req }` envelope. */
    reportSessions: (count: number): Promise<void> =>
        invoke<void>("ui_sessions_report", { count }),
    /** Quit the app for real (after the user confirms with live sessions). */
    quit: (): Promise<void> => invoke<void>("app_quit"),
    /** Open a URL in the system browser (terminal Ctrl/Cmd+click on links). */
    open: (url: string): Promise<void> => invoke<void>("open_external", { url }),
};

// =====================================================================
// Port forwarding (Tools → Forwards): persisted -L / -R / -D
// =====================================================================
// NOTE: `call(name, x)` sends `invoke(name, { req: x })`. So pass the
// payload object itself — NOT `{ req }` (that would double-wrap).

export const forwards = {
    /** Persist a forward definition (does not start it). */
    save: (req: ForwardSaveRequest): Promise<ForwardSaveResponse> =>
        call("forward_save", req),

    /** Start a saved forward; emits a `ForwardEvent` stream over a Channel. */
    start: (forwardId: string, onEvent: (e: ForwardEvent) => void): Promise<void> => {
        const channel = new Channel<ForwardEvent>();
        channel.onmessage = onEvent;
        return invoke<void>("forward_start", {
            req: { forward_id: forwardId },
            onEvent: channel,
        });
    },

    /** Stop a running forward (keeps the saved definition). */
    stop: (forwardId: string): Promise<void> =>
        call("forward_stop", { forward_id: forwardId }),

    /** Stop (if running) and delete the saved definition. */
    delete: (forwardId: string): Promise<void> =>
        call("forward_delete", { forward_id: forwardId }),

    /** Toggle auto-start-on-launch. */
    setAutoStart: (forwardId: string, autoStart: boolean): Promise<void> =>
        call("forward_set_auto_start", { forward_id: forwardId, auto_start: autoStart }),

    list: (): Promise<ForwardListResponse> => call("forward_list"),
};

// =====================================================================
// SSH ID (Tools → SSH ID): public key handle CRUD. These proxy to the
// sync server through rh-app (which holds the bearer token).
// =====================================================================

export const sshId = {
    /** Current handle + published keys for the logged-in account. */
    get: (): Promise<SshIdData> => invoke<SshIdData>("ssh_id_get"),

    /** Claim or rename the handle; returns the canonical handle. */
    setHandle: (handle: string): Promise<string> =>
        invoke<string>("ssh_id_set_handle", { handle }),

    /** Inline availability check for the create form. */
    check: (handle: string): Promise<SshIdCheck> =>
        invoke<SshIdCheck>("ssh_id_check", { handle }),

    /** Publish a public key under the handle. */
    addKey: (public_key: string, label: string | null): Promise<SshIdAddedKey> =>
        invoke<SshIdAddedKey>("ssh_id_add_key", { publicKey: public_key, label }),

    /** Unpublish a key. */
    deleteKey: (id: string): Promise<void> => invoke<void>("ssh_id_delete_key", { id }),

    /** Rename a key's label. */
    updateLabel: (id: string, label: string | null): Promise<void> =>
        invoke<void>("ssh_id_update_label", { id, label }),

    /** SSH-key credentials with their derived public key (for the picker). */
    availableKeys: (): Promise<SshIdAvailableKey[]> =>
        invoke<SshIdAvailableKey[]>("ssh_id_available_keys"),
};

// =====================================================================
// Snippets (Tools → Snippets): reusable commands. Plain CRUD.
// =====================================================================

export const snippets = {
    list: (): Promise<Snippet[]> => invoke<Snippet[]>("snippet_list"),
    create: (name: string, command: string): Promise<string> =>
        invoke<string>("snippet_create", { name, command }),
    update: (id: string, name: string, command: string): Promise<void> =>
        invoke<void>("snippet_update", { id, name, command }),
    delete: (id: string): Promise<void> => invoke<void>("snippet_delete", { id }),
};

// =====================================================================
// Global "unstick modifiers" hotkey (release stuck Ctrl/Alt/Shift/Win).
// =====================================================================

export interface UnstickHotkey {
    ctrl: boolean;
    alt: boolean;
    shift: boolean;
    meta: boolean;
    /** W3C KeyboardEvent.code (e.g. "KeyK"); null/"" disables the hotkey. */
    code: string | null;
}

export const hotkeys = {
    setUnstick: (hk: UnstickHotkey): Promise<void> =>
        invoke<void>("set_unstick_hotkey", {
            ctrl: hk.ctrl,
            alt: hk.alt,
            shift: hk.shift,
            meta: hk.meta,
            code: hk.code,
        }),
};
