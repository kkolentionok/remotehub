/**
 * Global application state via zustand.
 *
 * Three concerns live here:
 *
 * 1. **Cached data** from storage (hosts, groups, credentials). The
 *    UI reads from this; load actions populate it. Events from Rust
 *    invalidate it by re-running the load.
 *
 * 2. **UI selection** (which host is selected, which dialog is open).
 *    Lives in the same store for simplicity — at this scale a single
 *    store is plenty, and the alternative (per-domain stores) means
 *    components subscribe to multiple stores.
 *
 * 3. **Loading flags** so the UI shows skeletons rather than empty
 *    space during initial fetch.
 *
 * State mutations are intentionally coarse: each loader replaces its
 * collection atomically. We don't optimistically patch — we re-fetch
 * after mutation. The cost (one round-trip per CRUD) is negligible at
 * this scale and the consistency is much easier to reason about.
 */

import { create } from "zustand";
import { writeText as clipboardWriteText, writeImage as clipboardWriteImage } from "@tauri-apps/plugin-clipboard-manager";
import { Image as TauriImage } from "@tauri-apps/api/image";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { listen } from "@tauri-apps/api/event";

import {
    credentials as credentialsApi,
    events,
    groups as groupsApi,
    hosts as hostsApi,
    rdpSession as rdpSessionApi,
    localSession as localSessionApi,
    sessions as sessionsApi,
    settings as settingsApi,
} from "../lib/ipc";
import type {
    CloseReason,
    CredentialDto,
    EnvVar,
    GroupId,
    HostDto,
    HostGroupDto,
    HostId,
    Protocol,
    RdpSessionEvent,
    SessionState,
    Settings,
    SshSessionEvent,
} from "../lib/types";
import { formatApiError } from "../lib/types";
import {
    firstLeafKey,
    hasLeaf,
    leafKeys,
    removeLeaf,
    setRatioAtPath,
    splitLeaf,
    splitLeafWith,
    type PaneNode,
    type SplitDir,
} from "../lib/paneTree";

// =====================================================================
// Hosts
// =====================================================================

interface HostsStore {
    items: HostDto[];
    loading: boolean;
    error: string | null;
    load: () => Promise<void>;
}

export const useHostsStore = create<HostsStore>((set) => ({
    items: [],
    loading: false,
    error: null,
    load: async () => {
        set({ loading: true, error: null });
        try {
            const res = await hostsApi.list({});
            set({ items: res.hosts, loading: false });
        } catch (e) {
            set({ error: stringifyError(e), loading: false });
        }
    },
}));

// =====================================================================
// Groups
// =====================================================================

interface GroupsStore {
    items: HostGroupDto[];
    loading: boolean;
    error: string | null;
    load: () => Promise<void>;
}

export const useGroupsStore = create<GroupsStore>((set) => ({
    items: [],
    loading: false,
    error: null,
    load: async () => {
        set({ loading: true, error: null });
        try {
            const res = await groupsApi.list();
            set({ items: res.groups, loading: false });
        } catch (e) {
            set({ error: stringifyError(e), loading: false });
        }
    },
}));

// =====================================================================
// Credentials
// =====================================================================

interface CredentialsStore {
    items: CredentialDto[];
    loading: boolean;
    error: string | null;
    load: () => Promise<void>;
}

export const useCredentialsStore = create<CredentialsStore>((set) => ({
    items: [],
    loading: false,
    error: null,
    load: async () => {
        set({ loading: true, error: null });
        try {
            const res = await credentialsApi.list();
            set({ items: res.credentials, loading: false });
        } catch (e) {
            set({ error: stringifyError(e), loading: false });
        }
    },
}));

// =====================================================================
// UI selection + dialogs
// =====================================================================

export type DialogKind =
    | { kind: "none" }
    | { kind: "host-delete-confirm"; hostId: HostId }
    | { kind: "group-create"; parentId?: GroupId | null }
    | { kind: "group-rename"; groupId: GroupId }
    | { kind: "group-delete-confirm"; groupId: GroupId }
    | { kind: "credentials-list" }
    | { kind: "credential-create" }
    | { kind: "credential-delete-confirm"; credentialId: string }
    | { kind: "settings"; section?: string }
    /**
     * Discard-changes prompt shown when the user tries to navigate away
     * from a draft host that has at least one filled field but cannot
     * be auto-saved (typically because the required address is empty).
     * `onConfirm` runs the navigation; cancel keeps them on the draft.
     */
    | {
          kind: "discard-changes-confirm";
          onConfirm: () => void;
      };

/**
 * Draft host being created in the right pane. Lives in UI state until
 * either (a) it becomes valid enough to auto-save into the DB, at
 * which point it's promoted to a real host with `selectedHostId`,
 * or (b) the user discards it.
 *
 * `groupId` is null for ungrouped.
 */
export interface HostDraft {
    label: string;
    hostname: string;
    port: string;          // string form; empty means "use default for protocol"
    protocol: "ssh" | "rdp";
    groupId: GroupId | null;
    tags: string[];
    notes: string;
    /** Command run on SSH connect. Empty means none. */
    startupCommand: string;
    /** Environment variables to inject on connect. */
    envVars: EnvVar[];
    /** Inline credential being typed (not yet committed to the credential store). */
    inlineUsername: string;
    inlinePassword: string;
    /** Which auth method the inline credential uses. */
    inlineAuthKind: "password" | "key";
    /** Pasted private key (PEM) when inlineAuthKind === "key". */
    inlinePrivateKey: string;
    /** Optional passphrase for the private key. */
    inlinePassphrase: string;
    /** Imported key file name (names the created credential). */
    inlineKeyName: string;
    /** An existing credential chosen via "use existing" (linked on promotion). */
    pickedCredentialId: string | null;
}

interface UiStore {
    /** Currently displayed host in the main pane. Mutually exclusive with `draft`. */
    selectedHostId: HostId | null;
    /** Unsaved new host. Mutually exclusive with `selectedHostId`. */
    draft: HostDraft | null;
    /** Active modal dialog. */
    dialog: DialogKind;
    /** Search query in the sidebar. */
    searchQuery: string;
    /** Collapsed group IDs in the sidebar tree. */
    collapsedGroupIds: Set<GroupId>;
    /** Quick-connect launcher overlay (opened by the tab-bar "+"). */
    launcherOpen: boolean;
    /** Active app section shown when no session tab is active. */
    section: "vault" | "tools";

    selectHost: (id: HostId | null) => void;
    startDraft: (defaultGroupId?: GroupId | null) => void;
    updateDraft: (patch: Partial<HostDraft>) => void;
    clearDraft: () => void;
    setDialog: (dialog: DialogKind) => void;
    closeDialog: () => void;
    setSearchQuery: (q: string) => void;
    toggleGroupCollapsed: (id: GroupId) => void;
    setLauncherOpen: (open: boolean) => void;
    setSection: (section: "vault" | "tools") => void;
}

function emptyDraft(defaultGroupId: GroupId | null = null): HostDraft {
    return {
        label: "",
        hostname: "",
        port: "",
        protocol: "ssh",
        groupId: defaultGroupId,
        tags: [],
        notes: "",
        startupCommand: "",
        envVars: [],
        inlineUsername: "",
        inlinePassword: "",
        inlineAuthKind: "password",
        inlinePrivateKey: "",
        inlinePassphrase: "",
        inlineKeyName: "",
        pickedCredentialId: null,
    };
}

export const useUiStore = create<UiStore>((set) => ({
    selectedHostId: null,
    draft: null,
    dialog: { kind: "none" },
    searchQuery: "",
    collapsedGroupIds: new Set(),
    launcherOpen: false,
    section: "vault",

    selectHost: (id) => set({ selectedHostId: id, draft: null }),
    startDraft: (defaultGroupId = null) =>
        set({ draft: emptyDraft(defaultGroupId), selectedHostId: null }),
    updateDraft: (patch) =>
        set((s) => (s.draft ? { draft: { ...s.draft, ...patch } } : s)),
    clearDraft: () => set({ draft: null }),
    setDialog: (dialog) => set({ dialog }),
    closeDialog: () => set({ dialog: { kind: "none" } }),
    setSearchQuery: (searchQuery) => set({ searchQuery }),
    setLauncherOpen: (launcherOpen) => set({ launcherOpen }),
    setSection: (section) => set({ section }),
    toggleGroupCollapsed: (id) =>
        set((s) => {
            const next = new Set(s.collapsedGroupIds);
            if (next.has(id)) next.delete(id);
            else next.add(id);
            return { collapsedGroupIds: next };
        }),
}));

/**
 * Helper: a draft is "dirty" if any non-default field is filled.
 * Used to decide whether navigating away needs a discard confirmation.
 */
export function isDraftDirty(d: HostDraft): boolean {
    return (
        d.label.trim() !== "" ||
        d.hostname.trim() !== "" ||
        d.port.trim() !== "" ||
        d.tags.length > 0 ||
        d.notes.trim() !== "" ||
        d.startupCommand.trim() !== "" ||
        d.envVars.length > 0 ||
        d.inlineUsername.trim() !== "" ||
        d.inlinePassword !== "" ||
        d.inlinePrivateKey.trim() !== "" ||
        d.inlinePassphrase !== ""
    );
}

/**
 * Helper: a draft can be auto-saved as a real host if it has at least
 * an address (hostname). Label is auto-filled from hostname if empty.
 * If hostname is empty, the draft cannot be promoted — caller should
 * prompt the user to discard.
 */
export function isDraftPromotable(d: HostDraft): boolean {
    return d.hostname.trim() !== "";
}

// =====================================================================
// Wiring: subscribe to Rust events and re-load relevant stores.
// =====================================================================

/**
 * Call once at app startup. Registers event listeners that invalidate
 * stores when Rust emits a change. Returns a cleanup function.
 */
export async function subscribeToBackendEvents(): Promise<() => void> {
    const unsubs = await Promise.all([
        events.onHostsChanged(() => {
            void useHostsStore.getState().load();
        }),
        events.onGroupsChanged(() => {
            void useGroupsStore.getState().load();
        }),
        events.onCredentialsChanged(() => {
            void useCredentialsStore.getState().load();
        }),
        events.onSettingsChanged(() => {
            void useSettingsStore.getState().load();
        }),
    ]);
    return () => unsubs.forEach((u) => u());
}

// =====================================================================
// Settings store
// =====================================================================

interface SettingsStore {
    settings: Settings | null;
    loading: boolean;
    error: string | null;
    /** Initial load — call once at app startup. */
    load: () => Promise<void>;
    /**
     * Patch one or more setting keys. Backend persists, then emits a
     * `settings:changed` event which triggers a re-load. We also
     * optimistically apply the patch to the local copy to avoid a
     * round-trip flicker — the eventual re-load will normalize.
     */
    update: (patches: Record<string, unknown>) => Promise<void>;
}

export const useSettingsStore = create<SettingsStore>((set, get) => ({
    settings: null,
    loading: false,
    error: null,
    load: async () => {
        set({ loading: true, error: null });
        try {
            const res = await settingsApi.getAll();
            set({ settings: res.settings, loading: false });
        } catch (e: unknown) {
            set({ error: stringifyError(e), loading: false });
        }
    },
    update: async (patches) => {
        // Local merge first so the UI reflects the change instantly.
        const current = get().settings;
        if (current) {
            set({ settings: { ...current, ...(patches as Partial<Settings>) } });
        }
        await settingsApi.update({ patches });
        // Backend will emit settings:changed → triggers reload via the
        // subscription in subscribeToBackendEvents. No manual reload here.
    },
}));

// =====================================================================
// Sessions (Stage 2)
//
// Each open session is a tab. We key tabs by a stable local id (`key`)
// generated up front, so the event channel — set up before the backend
// returns the real `sessionId` — can address the tab without races.
//
// PTY output is high-frequency and must NOT flow through React state.
// A module-level registry maps each tab to its xterm writer; output that
// arrives before the terminal mounts is buffered and flushed on mount.
// =====================================================================

export interface SessionTab {
    key: string;
    sessionId: string | null;
    hostId: HostId;
    title: string;
    protocol: Protocol;
    state: SessionState;
    /** Human-readable error / close detail, if any. */
    message: string | null;
    /** Set while a TOFU host-key decision is pending. `changed` marks a
     *  key that differs from the one previously pinned for this host. */
    hostKey: { fingerprint: string; keyType: string; changed: boolean } | null;
    /** RDP backing resolution (server's negotiated desktop size). The
     *  viewport canvas is sized to this; updated by the `resized` event. */
    rdpWidth?: number;
    rdpHeight?: number;
    /** True for a local shell PTY session (no host; uses local_session_* IPC). */
    local?: boolean;
    /** True for an SFTP file-browser tab (SftpView manages its own backend). */
    sftp?: boolean;
    /** Auth method reported by the last `auth_failed` event ("password" /
     *  "publickey" / "agent" / "jump"). Drives the auth-vs-badpass split and
     *  the inline re-auth default, independent of the (possibly clobbered)
     *  message string. */
    authMethod?: string | null;
}

/**
 * A workspace tab: a layout tree of panes (each leaf hosts one session)
 * plus which pane currently has keyboard focus.
 */
export interface WorkspaceTab {
    id: string;
    root: PaneNode;
    activePaneKey: string;
}

interface OutputSink {
    buffer: Uint8Array[];
    writer: ((data: Uint8Array) => void) | null;
}

const sessionOutput = new Map<string, OutputSink>();

/** Serialized xterm buffers, kept across pane remounts (split/move). */
const sessionSnapshots = new Map<string, string>();
/** Last cols×rows sent per session, to avoid redundant window-change. */
const lastDims = new Map<string, string>();

/** Terminal saves its buffer here on unmount. */
export function saveSessionSnapshot(key: string, data: string) {
    sessionSnapshots.set(key, data);
}

/** Terminal reads (and keeps) the saved buffer on mount. */
export function takeSessionSnapshot(key: string): string | null {
    return sessionSnapshots.get(key) ?? null;
}

function pushOutput(key: string, data: Uint8Array) {
    const sink = sessionOutput.get(key);
    if (!sink) return;
    if (sink.writer) sink.writer(data);
    else sink.buffer.push(data);
}

/** Terminal component calls this on mount; flushes buffered output. */
export function registerSessionTerminal(
    key: string,
    writer: (data: Uint8Array) => void,
): () => void {
    let sink = sessionOutput.get(key);
    if (!sink) {
        sink = { buffer: [], writer: null };
        sessionOutput.set(key, sink);
    }
    sink.writer = writer;
    for (const chunk of sink.buffer) writer(chunk);
    sink.buffer = [];
    return () => {
        const s = sessionOutput.get(key);
        if (s) s.writer = null;
    };
}

// --- RDP frame routing -------------------------------------------------
// RDP frames are a live raster (latest wins), not a replayable byte
// stream — so no ring buffer. The viewport registers a sink; events that
// arrive before it mounts keep only the most recent frame.
type RdpEventSink = (ev: RdpSessionEvent) => void;
const sessionViewports = new Map<string, RdpEventSink>();
const pendingRdpFrame = new Map<string, RdpSessionEvent>();

function pushRdpEvent(key: string, ev: RdpSessionEvent) {
    const sink = sessionViewports.get(key);
    if (sink) {
        sink(ev);
    } else if (ev.kind === "frame" || ev.kind === "frame_batch") {
        pendingRdpFrame.set(key, ev); // keep only the latest
    }
}

/** RdpViewport calls this on mount; replays the last buffered frame. */
export function registerSessionViewport(
    key: string,
    sink: RdpEventSink,
): () => void {
    sessionViewports.set(key, sink);
    const pending = pendingRdpFrame.get(key);
    if (pending) {
        sink(pending);
        pendingRdpFrame.delete(key);
    }
    return () => {
        if (sessionViewports.get(key) === sink) sessionViewports.delete(key);
    };
}

/** Session ids being re-docked: suppresses the "popout window closed → end the
 *  tab" handler for a close we triggered ourselves (re-dock, not the user). */
const redockGuard = new Set<string>();
/** One-time guard so the cross-window popout-closed listener is wired once. */
let popoutListenerWired = false;

function rdpCloseText(reason: { kind: string }): string {
    switch (reason.kind) {
        case "user_requested":
            return "Closed";
        case "server_disconnected":
            return "Server disconnected";
        case "auth_failed":
            return "Authentication failed";
        case "cert_rejected":
            return "Certificate rejected";
        default:
            return "Disconnected";
    }
}

function closeReasonText(reason: CloseReason): string {
    switch (reason.kind) {
        case "user_requested":
            return "Closed";
        case "server_disconnected":
            return reason.message ?? "Server disconnected";
        case "network_error":
            return reason.message;
        case "auth_failed":
            return "Authentication failed";
        case "host_key_rejected":
            return "Host key rejected";
        case "crashed":
            return reason.message;
    }
}

interface SessionsStore {
    /** All live sessions, keyed by `key` (one per pane leaf). */
    sessions: SessionTab[];
    /** Workspace tabs, in bar order. */
    tabs: WorkspaceTab[];
    /** Active tab id, or null for the Vault. */
    activeTabId: string | null;
    /** When set, the next launcher pick splits the active pane instead
     *  of opening a new tab. */
    splitTarget: SplitDir | null;
    /** Session key currently being dragged (tab or pane), for drop zones. */
    draggingSession: string | null;
    /** While dragging a tab, the tab whose workspace is previewed as the
     *  split target (Termius-style: you see where the dragged tab lands). */
    dragPreviewTabId: string | null;
    /** Id of the tab currently being dragged (null when dragging a pane). */
    dragTabId: string | null;

    /** Open a host in a brand-new tab. */
    open: (host: HostDto) => Promise<void>;
    /** Open a local shell PTY in a new tab. `title` is i18n'd by the caller. */
    openLocalTerminal: (title: string) => void;
    /** Open an SFTP file-browser tab. `title` is i18n'd by the caller. */
    openSftp: (title: string) => void;
    /** Split the active tab's focused pane, opening `host` in the new pane. */
    splitActivePane: (host: HostDto, dir: SplitDir) => void;
    /** Arm a split and open the launcher to choose the host for it. */
    requestSplit: (dir: SplitDir) => void;
    /** Close a single pane/session. Collapses the split; drops the tab if empty. */
    close: (key: string) => Promise<void>;
    /** Close an entire tab and all its sessions. */
    closeTab: (tabId: string) => Promise<void>;
    setActiveTab: (tabId: string | null) => void;
    setActivePane: (tabId: string, key: string) => void;
    setDraggingSession: (key: string | null) => void;
    setDragPreviewTabId: (tabId: string | null) => void;
    setDragTabId: (tabId: string | null) => void;
    /** Reset all drag state (dragging session, preview, dragged tab). */
    endDrag: () => void;
    /**
     * Move `sourceKey` so it splits the pane hosting `targetKey`. `dir`
     * + `newFirst` place it (left/top = newFirst). Collapses/removes the
     * source tab if it empties.
     */
    moveSessionIntoSplit: (
        sourceKey: string,
        targetKey: string,
        dir: SplitDir,
        newFirst: boolean,
    ) => void;
    /** Pop a pane out of its split into its own new tab. */
    popOutSession: (sourceKey: string) => void;
    /** RDP sessions currently rendered in a separate OS window (keyed by tab
     *  key). While set, the tab shows a placeholder instead of the viewport. */
    poppedOut: Record<string, boolean>;
    /** Detach a live RDP session into its own OS window; the tab shows an
     *  "opened in a separate window" placeholder. */
    detachRdpToWindow: (key: string) => Promise<void>;
    /** Re-dock a popped-out RDP session back into its tab (reattach the frame
     *  stream here, then close the separate window). */
    redockRdp: (key: string) => Promise<void>;
    /** Pop-out-window side: bind to an already-live backend RDP session
     *  (reattach + full repaint) and return the local tab key to render. */
    attachExternalRdp: (p: {
        sessionId: string;
        title: string;
        width: number;
        height: number;
    }) => string;
    /**
     * Split the tab that holds `targetKey` with its neighbour tab (the one
     * before it, or after if it is first), merging the neighbour's active
     * session in beside the target. Used when dragging the active tab onto
     * its own pane.
     */
    splitWithPreviousTab: (
        targetKey: string,
        dir: SplitDir,
        newFirst: boolean,
    ) => void;
    /** Persist a divider ratio (path from the tab's root). */
    setSplitRatio: (tabId: string, path: ("a" | "b")[], ratio: number) => void;
    /** Reorder tabs: move tab `from` to the position of tab `to`. */
    reorder: (from: string, to: string) => void;

    sendInput: (key: string, data: Uint8Array) => void;
    /** Per-host consecutive auth-failure count (reset on a successful Ready);
     *  surfaced as the "attempt N" badge on the re-auth screen. */
    authAttempts: Record<string, number>;
    resize: (key: string, cols: number, rows: number) => void;
    acceptHostKey: (key: string) => Promise<void>;
    rejectHostKey: (key: string) => Promise<void>;
    /** After a webview reload, rebuild tabs for sessions the Rust process
     *  kept alive and re-bind each to a fresh event channel. Idempotent;
     *  no-op once any session already exists in the store. */
    restoreSessions: () => Promise<void>;
}

export const useSessionsStore = create<SessionsStore>((set, get) => {
    const patch = (key: string, fields: Partial<SessionTab>) =>
        set((s) => ({
            sessions: s.sessions.map((t) => (t.key === key ? { ...t, ...fields } : t)),
        }));

    const handleEvent = (key: string, ev: SshSessionEvent) => {
        switch (ev.kind) {
            case "state_changed":
                patch(key, { state: ev.state });
                if (ev.state === "ready") {
                    const hostId = get().sessions.find((t) => t.key === key)?.hostId;
                    if (hostId && get().authAttempts[hostId])
                        set((s) => ({
                            authAttempts: { ...s.authAttempts, [hostId]: 0 },
                        }));
                }
                break;
            case "data":
                pushOutput(key, Uint8Array.from(ev.bytes));
                break;
            case "auth_failed": {
                const hostId = get().sessions.find((t) => t.key === key)?.hostId;
                patch(key, {
                    state: "failed",
                    message: `Auth failed (${ev.method})`,
                    authMethod: ev.method,
                });
                if (hostId)
                    set((s) => ({
                        authAttempts: {
                            ...s.authAttempts,
                            [hostId]: (s.authAttempts[hostId] ?? 0) + 1,
                        },
                    }));
                break;
            }
            case "host_key_prompt":
                patch(key, {
                    state: "host_key_pending",
                    hostKey: {
                        fingerprint: ev.fingerprint_sha256,
                        keyType: ev.key_type,
                        changed: ev.changed,
                    },
                });
                break;
            case "error":
                patch(key, { message: ev.message });
                break;
            case "closed": {
                // A drop while still connecting (never reached "ready") is a
                // connection *failure* → show the compact failed card + step
                // log, not a raw "closed" message. Keep the specific message
                // (raw OS error) so the failure category + details work.
                const prev = get().sessions.find((t) => t.key === key);
                const userClosed = ev.reason.kind === "user_requested";
                const wasConnecting = !!prev && prev.state !== "ready" && !userClosed;
                const prevMsg = prev?.message;
                const text = !userClosed && prevMsg ? prevMsg : closeReasonText(ev.reason);
                patch(key, { state: wasConnecting ? "failed" : "closed", message: text });
                break;
            }
        }
    };

    const handleRdpEvent = (key: string, ev: RdpSessionEvent) => {
        switch (ev.kind) {
            case "state_changed":
                patch(key, { state: ev.state });
                break;
            case "error":
                patch(key, { message: ev.message });
                break;
            case "closed": {
                // Keep the specific error (from a preceding `error` event)
                // rather than overwriting it with the generic close text.
                const prev = get().sessions.find((t) => t.key === key);
                const userClosed = ev.reason.kind === "user_requested";
                const wasConnecting = !!prev && prev.state !== "ready" && !userClosed;
                const prevMsg = prev?.message;
                const text =
                    ev.reason.kind === "error" && prevMsg ? prevMsg : rdpCloseText(ev.reason);
                patch(key, { state: wasConnecting ? "failed" : "closed", message: text });
                break;
            }
            case "resized":
                patch(key, { rdpWidth: ev.width, rdpHeight: ev.height });
                break;
            case "frame":
            case "frame_batch":
            case "pointer_position":
            case "pointer_bitmap":
            case "pointer_hidden":
            case "pointer_default":
            case "cert_prompt":
                pushRdpEvent(key, ev);
                break;
            case "clipboard":
                // Remote copied text — mirror to the local OS clipboard.
                if (ev.mime.startsWith("text/") && ev.data) {
                    void clipboardWriteText(ev.data).catch(() => {});
                }
                break;
            case "clipboard_image":
                // Remote copied an image — build it from raw RGBA and write to
                // the OS clipboard. `Image.new` avoids needing tauri's
                // `image-png` feature (which `Image.fromBytes` would require).
                void (async () => {
                    try {
                        const bin = atob(ev.rgba_base64);
                        const bytes = new Uint8Array(bin.length);
                        for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
                        const img = await TauriImage.new(bytes, ev.width, ev.height);
                        await clipboardWriteImage(img);
                    } catch (e) {
                        console.warn("rdp clipboard image write failed:", e);
                    }
                })();
                break;
        }
    };

    const genId = (): string =>
        typeof crypto !== "undefined" && "randomUUID" in crypto
            ? crypto.randomUUID()
            : `s_${Date.now()}_${Math.random().toString(36).slice(2)}`;

    /** Create a session (state + backend connect). Returns its key. */
    const createSession = (host: HostDto): string => {
        const key = genId();
        // Match the *pane* aspect ratio (not the monitor's) so the remote
        // desktop fills the viewport with `object-fit: contain` and no
        // letterbox bars in the common windowed case. Vertical resolution is
        // kept near the monitor's so it stays crisp; a window resize after
        // connect just re-letterboxes cleanly (live reflow is the separate,
        // gated DisplayControl feature). Even dims; capped to 2560×1600.
        const even = (n: number) => Math.max(2, Math.floor(n / 2) * 2);
        let rdpW: number | undefined;
        let rdpH: number | undefined;
        if (host.protocol === "rdp") {
            const TAB_BAR = 44; // approx tab strip height (logical px)
            const availW = Math.max(640, Math.round(window.innerWidth));
            const availH = Math.max(480, Math.round(window.innerHeight - TAB_BAR));
            const aspect = availW / availH;
            const h0 = Math.min(1440, Math.max(720, Math.round(window.screen.height)));
            const w0 = Math.round(h0 * aspect);
            const cap = Math.min(1, 2560 / w0, 1600 / h0);
            rdpW = even(Math.round(w0 * cap));
            rdpH = even(Math.round(h0 * cap));
        }
        const tab: SessionTab = {
            key,
            sessionId: null,
            hostId: host.id,
            title: host.display_name ?? host.name,
            protocol: host.protocol,
            state: "connecting",
            message: null,
            hostKey: null,
            rdpWidth: rdpW,
            rdpHeight: rdpH,
        };
        sessionOutput.set(key, { buffer: [], writer: null });
        set((s) => ({ sessions: [...s.sessions, tab] }));

        void (async () => {
            try {
                if (host.protocol === "rdp") {
                    const res = await rdpSessionApi.open(
                        {
                            host_id: host.id,
                            credential_id: null,
                            options: {
                                protocol: "rdp",
                                width: rdpW ?? 1280,
                                height: rdpH ?? 800,
                                color_depth: 32,
                                keyboard_layout: "0",
                            },
                        },
                        (ev) => handleRdpEvent(key, ev),
                    );
                    patch(key, { sessionId: res.session_id });
                    return;
                }
                const res = await sessionsApi.open(
                    {
                        host_id: host.id,
                        // null = offer every auth method linked to the host
                        // (the backend tries key(s) then password). Passing a
                        // specific id would restrict to that single method.
                        credential_id: null,
                        options: {
                            protocol: "ssh",
                            cols: 80,
                            rows: 24,
                            term: "xterm-256color",
                        },
                    },
                    (ev) => handleEvent(key, ev),
                );
                patch(key, { sessionId: res.session_id });
            } catch (e: unknown) {
                patch(key, { state: "failed", message: formatApiError(e) });
            }
        })();

        return key;
    };

    // Local shell PTY: no host, no connect phase — starts "ready" and
    // streams bytes through the same SshSessionEvent handler as SSH.
    const createLocalSession = (title: string): string => {
        const key = genId();
        const tab: SessionTab = {
            key,
            sessionId: null,
            hostId: "__local__",
            title,
            protocol: "ssh",
            state: "ready",
            message: null,
            hostKey: null,
            local: true,
        };
        sessionOutput.set(key, { buffer: [], writer: null });
        set((s) => ({ sessions: [...s.sessions, tab] }));

        void (async () => {
            try {
                const res = await localSessionApi.open(80, 24, (ev) =>
                    handleEvent(key, ev),
                );
                patch(key, { sessionId: res.session_id });
            } catch (e: unknown) {
                patch(key, { state: "failed", message: formatApiError(e) });
            }
        })();

        return key;
    };

    /** Tear down one session (backend + output buffer). */
    const teardownSession = async (key: string) => {
        const sess = get().sessions.find((t) => t.key === key);
        if (sess?.sessionId) {
            try {
                if (sess.local) {
                    await localSessionApi.close(sess.sessionId);
                } else if (sess.protocol === "rdp") {
                    await rdpSessionApi.close(sess.sessionId);
                } else {
                    await sessionsApi.close(sess.sessionId);
                }
            } catch {
                /* actor may already be gone — ignore */
            }
        }
        sessionOutput.delete(key);
        sessionSnapshots.delete(key);
        sessionViewports.delete(key);
        pendingRdpFrame.delete(key);
        lastDims.delete(key);
    };

    return {
        sessions: [],
        poppedOut: {},
        authAttempts: {},
        tabs: [],
        activeTabId: null,
        splitTarget: null,
        draggingSession: null,
        dragPreviewTabId: null,
        dragTabId: null,

        open: async (host) => {
            const key = createSession(host);
            const id = genId();
            set((s) => ({
                tabs: [
                    ...s.tabs,
                    { id, root: { t: "leaf", key }, activePaneKey: key },
                ],
                activeTabId: id,
            }));
        },

        openLocalTerminal: (title) => {
            const key = createLocalSession(title);
            const id = genId();
            set((s) => ({
                tabs: [
                    ...s.tabs,
                    { id, root: { t: "leaf", key }, activePaneKey: key },
                ],
                activeTabId: id,
            }));
        },

        openSftp: (title) => {
            // SFTP tab: a session-shaped container. SftpView owns the actual
            // local/remote browsing + backend connection; the tab just holds
            // it in the pane system. No backend session opened here.
            const key = genId();
            const tab: SessionTab = {
                key,
                sessionId: null,
                hostId: "__sftp__",
                title,
                protocol: "ssh",
                state: "ready",
                message: null,
                hostKey: null,
                sftp: true,
            };
            sessionOutput.set(key, { buffer: [], writer: null });
            const id = genId();
            set((s) => ({
                sessions: [...s.sessions, tab],
                tabs: [
                    ...s.tabs,
                    { id, root: { t: "leaf", key }, activePaneKey: key },
                ],
                activeTabId: id,
            }));
        },

        splitActivePane: (host, dir) => {
            const st = get();
            const tabId = st.activeTabId;
            const tab = st.tabs.find((tb) => tb.id === tabId);
            if (!tabId || !tab) {
                void get().open(host);
                return;
            }
            const key = createSession(host);
            set((s) => ({
                tabs: s.tabs.map((tb) =>
                    tb.id === tabId
                        ? {
                              ...tb,
                              root: splitLeaf(tb.root, tb.activePaneKey, key, dir),
                              activePaneKey: key,
                          }
                        : tb,
                ),
            }));
        },

        requestSplit: (dir) => {
            if (!get().activeTabId) return;
            set({ splitTarget: dir });
            useUiStore.getState().setLauncherOpen(true);
        },

        close: async (key) => {
            // Remove the tab from the UI immediately so the button is
            // responsive even when the actor is mid-connect (a blocking
            // handshake won't process the close command until it finishes);
            // tear the actor down in the background.
            void teardownSession(key);
            set((s) => {
                const sessions = s.sessions.filter((t) => t.key !== key);
                let activeTabId = s.activeTabId;
                const tabs: WorkspaceTab[] = [];
                for (const tb of s.tabs) {
                    if (!hasLeaf(tb.root, key)) {
                        tabs.push(tb);
                        continue;
                    }
                    const root = removeLeaf(tb.root, key);
                    if (root === null) {
                        // Tab emptied → drop it.
                        if (activeTabId === tb.id) activeTabId = null;
                        continue;
                    }
                    const activePaneKey =
                        tb.activePaneKey === key
                            ? firstLeafKey(root)
                            : tb.activePaneKey;
                    tabs.push({ ...tb, root, activePaneKey });
                }
                if (activeTabId === null && tabs.length > 0) {
                    activeTabId = tabs[tabs.length - 1]?.id ?? null;
                }
                return { sessions, tabs, activeTabId };
            });
        },

        closeTab: async (tabId) => {
            const tab = get().tabs.find((tb) => tb.id === tabId);
            if (!tab) return;
            const keys = leafKeys(tab.root);
            await Promise.all(keys.map((k) => teardownSession(k)));
            set((s) => {
                const sessions = s.sessions.filter((t) => !keys.includes(t.key));
                const tabs = s.tabs.filter((tb) => tb.id !== tabId);
                const activeTabId =
                    s.activeTabId === tabId
                        ? (tabs[tabs.length - 1]?.id ?? null)
                        : s.activeTabId;
                return { sessions, tabs, activeTabId };
            });
        },

        setActiveTab: (tabId) => set({ activeTabId: tabId }),

        setActivePane: (tabId, key) =>
            set((s) => ({
                tabs: s.tabs.map((tb) =>
                    tb.id === tabId ? { ...tb, activePaneKey: key } : tb,
                ),
            })),

        setDraggingSession: (key) => set({ draggingSession: key }),

        setDragPreviewTabId: (tabId) => set({ dragPreviewTabId: tabId }),

        setDragTabId: (tabId) => set({ dragTabId: tabId }),

        endDrag: () =>
            set({
                draggingSession: null,
                dragPreviewTabId: null,
                dragTabId: null,
            }),

        moveSessionIntoSplit: (sourceKey, targetKey, dir, newFirst) =>
            set((s) => {
                if (sourceKey === targetKey) return s;
                const result: WorkspaceTab[] = [];
                let targetTabId: string | null = null;
                for (const tb of s.tabs) {
                    let root: PaneNode | null = tb.root;
                    let activePaneKey = tb.activePaneKey;
                    // Pull the source leaf out of whichever tab holds it.
                    if (hasLeaf(root, sourceKey)) {
                        root = removeLeaf(root, sourceKey);
                        if (root && activePaneKey === sourceKey) {
                            activePaneKey = firstLeafKey(root);
                        }
                    }
                    if (root === null) continue; // source tab emptied → drop
                    // Insert next to the target leaf (same-tab moves land here too).
                    if (hasLeaf(root, targetKey)) {
                        root = splitLeafWith(root, targetKey, sourceKey, dir, newFirst);
                        activePaneKey = sourceKey;
                        targetTabId = tb.id;
                    }
                    result.push({ ...tb, root, activePaneKey });
                }
                if (targetTabId === null) return s; // target vanished — no-op
                let activeTabId: string | null = targetTabId;
                if (!result.some((tb) => tb.id === activeTabId)) {
                    activeTabId = result[result.length - 1]?.id ?? null;
                }
                return {
                    tabs: result,
                    activeTabId,
                    draggingSession: null,
                    dragPreviewTabId: null,
                    dragTabId: null,
                };
            }),

        attachExternalRdp: ({ sessionId, title, width, height }) => {
            const key = genId();
            const tab: SessionTab = {
                key,
                sessionId,
                hostId: "" as HostId,
                title,
                protocol: "rdp" as Protocol,
                state: "ready",
                message: null,
                hostKey: null,
                rdpWidth: width,
                rdpHeight: height,
            };
            sessionOutput.set(key, { buffer: [], writer: null });
            set((s) => ({ sessions: [...s.sessions, tab] }));
            // Bind this fresh webview to the still-live backend session; the
            // reattach forces a full repaint so the canvas paints completely.
            void rdpSessionApi.reattach(sessionId, (ev) => handleRdpEvent(key, ev));
            return key;
        },

        detachRdpToWindow: async (key) => {
            const tab = get().sessions.find((t) => t.key === key);
            if (!tab || !tab.sessionId || tab.protocol !== "rdp") return;
            const sid = tab.sessionId;
            // Wire the "popout window closed by the user" handler once: end the
            // owning tab (unless the close was our own re-dock).
            if (!popoutListenerWired) {
                popoutListenerWired = true;
                void listen<{ sid: string }>("rdp:popout-closed", (e) => {
                    const closedSid = e.payload?.sid;
                    if (!closedSid) return;
                    if (redockGuard.has(closedSid)) {
                        redockGuard.delete(closedSid);
                        return; // our own re-dock close — keep the tab
                    }
                    const owner = get().sessions.find((t) => t.sessionId === closedSid);
                    if (owner) void get().close(owner.key);
                });
            }
            set((s) => ({ poppedOut: { ...s.poppedOut, [key]: true } }));
            const params = new URLSearchParams({
                sid,
                t: tab.title,
                w: String(tab.rdpWidth ?? 1280),
                h: String(tab.rdpHeight ?? 800),
            });
            const winW = Math.min(Math.max(Math.round((tab.rdpWidth ?? 1280) / 1.5), 800), 1920);
            const winH = Math.min(Math.max(Math.round((tab.rdpHeight ?? 800) / 1.5), 600), 1200);
            try {
                // eslint-disable-next-line no-new
                new WebviewWindow(`rdp-${sid}`, {
                    url: `index.html#popout?${params.toString()}`,
                    title: tab.title,
                    width: winW,
                    height: winH,
                    decorations: true,
                });
            } catch {
                // Window creation failed — undo the placeholder so the tab keeps
                // rendering the viewport in place.
                set((s) => {
                    const p = { ...s.poppedOut };
                    delete p[key];
                    return { poppedOut: p };
                });
            }
        },

        redockRdp: async (key) => {
            const tab = get().sessions.find((t) => t.key === key);
            if (!tab || !tab.sessionId) return;
            const sid = tab.sessionId;
            // Reattach the stream back to this (main) webview *first*, so closing
            // the popout window below doesn't tear the session down.
            redockGuard.add(sid);
            await rdpSessionApi.reattach(sid, (ev) => handleRdpEvent(key, ev));
            set((s) => {
                const p = { ...s.poppedOut };
                delete p[key];
                return { poppedOut: p };
            });
            try {
                const w = await WebviewWindow.getByLabel(`rdp-${sid}`);
                await w?.close();
            } catch {
                /* window already gone */
            } finally {
                // Backstop: if the popout's close event never reaches the
                // listener, don't leave a stale guard that would later swallow
                // a genuine user close of the same session.
                window.setTimeout(() => redockGuard.delete(sid), 1500);
            }
        },

        popOutSession: (sourceKey) =>
            set((s) => {
                const owner = s.tabs.find((tb) => hasLeaf(tb.root, sourceKey));
                if (!owner) return { draggingSession: null, dragPreviewTabId: null, dragTabId: null };
                if (owner.root.t === "leaf") return { draggingSession: null, dragPreviewTabId: null, dragTabId: null }; // already standalone
                const newRoot = removeLeaf(owner.root, sourceKey);
                if (newRoot === null) return { draggingSession: null, dragPreviewTabId: null, dragTabId: null };
                const id = genId();
                const tabs = s.tabs.map((tb) =>
                    tb.id === owner.id
                        ? {
                              ...tb,
                              root: newRoot,
                              activePaneKey:
                                  tb.activePaneKey === sourceKey
                                      ? firstLeafKey(newRoot)
                                      : tb.activePaneKey,
                          }
                        : tb,
                );
                tabs.push({
                    id,
                    root: { t: "leaf", key: sourceKey },
                    activePaneKey: sourceKey,
                });
                return { tabs, activeTabId: id, draggingSession: null, dragPreviewTabId: null, dragTabId: null };
            }),

        splitWithPreviousTab: (targetKey, dir, newFirst) => {
            const s = get();
            const idx = s.tabs.findIndex((tb) => hasLeaf(tb.root, targetKey));
            if (idx < 0) return;
            const neighbor = s.tabs[idx - 1] ?? s.tabs[idx + 1];
            if (!neighbor) return;
            get().moveSessionIntoSplit(
                neighbor.activePaneKey,
                targetKey,
                dir,
                newFirst,
            );
        },

        setSplitRatio: (tabId, path, ratio) =>
            set((s) => ({
                tabs: s.tabs.map((tb) =>
                    tb.id === tabId
                        ? { ...tb, root: setRatioAtPath(tb.root, path, ratio) }
                        : tb,
                ),
            })),

        reorder: (from, to) =>
            set((s) => {
                if (from === to) return s;
                const list = [...s.tabs];
                const fromIdx = list.findIndex((tb) => tb.id === from);
                const toIdx = list.findIndex((tb) => tb.id === to);
                if (fromIdx === -1 || toIdx === -1) return s;
                const [moved] = list.splice(fromIdx, 1);
                if (!moved) return s;
                list.splice(toIdx, 0, moved);
                return { tabs: list };
            }),

        sendInput: (key, data) => {
            const tab = get().sessions.find((t) => t.key === key);
            if (!tab?.sessionId) return;
            const payload = { session_id: tab.sessionId, data: Array.from(data) };
            if (tab.local) void localSessionApi.input(payload).catch(() => {});
            else void sessionsApi.sendInput(payload).catch(() => {});
        },

        resize: (key, cols, rows) => {
            const sess = get().sessions.find((t) => t.key === key);
            if (!sess?.sessionId) return;
            const dim = `${cols}x${rows}`;
            if (lastDims.get(key) === dim) return; // unchanged → no SIGWINCH
            lastDims.set(key, dim);
            const payload = { session_id: sess.sessionId, width: cols, height: rows };
            if (sess.local) void localSessionApi.resize(payload).catch(() => {});
            else void sessionsApi.resize(payload).catch(() => {});
        },

        acceptHostKey: async (key) => {
            const tab = get().sessions.find((t) => t.key === key);
            if (!tab?.sessionId || !tab.hostKey) return;
            patch(key, { hostKey: null, state: "authenticating" });
            try {
                await sessionsApi.acceptHostKey({
                    session_id: tab.sessionId,
                    fingerprint: tab.hostKey.fingerprint,
                });
            } catch (e: unknown) {
                patch(key, { state: "failed", message: formatApiError(e) });
            }
        },

        rejectHostKey: async (key) => {
            const tab = get().sessions.find((t) => t.key === key);
            if (!tab?.sessionId) return;
            patch(key, { hostKey: null });
            try {
                await sessionsApi.rejectHostKey(tab.sessionId);
            } catch {
                /* ignore */
            }
        },

        restoreSessions: async () => {
            // The Rust process survives a webview reload, so its actors —
            // and their scrollback — are still live. Rebuild a tab per
            // session and reattach a fresh channel; the backend replays
            // buffered output so the terminal repaints. Guard against
            // double-run (StrictMode / re-mounts) by bailing if we already
            // hold sessions.
            if (get().sessions.length > 0) return;
            let sshList: Awaited<ReturnType<typeof sessionsApi.list>> | null = null;
            try {
                sshList = await sessionsApi.list();
            } catch {
                /* ignore — still attempt local restore below */
            }

            for (const summary of sshList?.sessions ?? []) {
                // Skip sessions the backend reports as already finished.
                if (summary.state === "closed" || summary.state === "failed") {
                    continue;
                }
                const key = genId();
                const tab: SessionTab = {
                    key,
                    sessionId: summary.session_id,
                    hostId: summary.host_id,
                    title: summary.title,
                    protocol: summary.protocol,
                    state: summary.state,
                    message: null,
                    hostKey: null,
                };
                sessionOutput.set(key, { buffer: [], writer: null });
                const id = genId();
                set((s) => ({
                    sessions: [...s.sessions, tab],
                    tabs: [
                        ...s.tabs,
                        { id, root: { t: "leaf", key }, activePaneKey: key },
                    ],
                }));

                void (async () => {
                    try {
                        const ok = await sessionsApi.reattach(
                            summary.session_id,
                            (ev) => handleEvent(key, ev),
                        );
                        if (!ok) {
                            // Died between list and reattach — drop the tab.
                            void get().close(key);
                        }
                    } catch {
                        patch(key, { state: "failed", message: "reattach failed" });
                    }
                })();
            }

            // Local shells survive a reload too: rebuild a tab per live PTY
            // and reattach a fresh channel (backend replays its scrollback).
            let localList: Awaited<ReturnType<typeof localSessionApi.list>> | null = null;
            try {
                localList = await localSessionApi.list();
            } catch {
                /* none live */
            }
            for (const summary of localList?.sessions ?? []) {
                const key = genId();
                const tab: SessionTab = {
                    key,
                    sessionId: summary.session_id,
                    hostId: "__local__",
                    title: summary.title,
                    protocol: "ssh",
                    state: "ready",
                    message: null,
                    hostKey: null,
                    local: true,
                };
                sessionOutput.set(key, { buffer: [], writer: null });
                const id = genId();
                set((s) => ({
                    sessions: [...s.sessions, tab],
                    tabs: [
                        ...s.tabs,
                        { id, root: { t: "leaf", key }, activePaneKey: key },
                    ],
                }));

                void (async () => {
                    try {
                        const ok = await localSessionApi.reattach(
                            summary.session_id,
                            (ev) => handleEvent(key, ev),
                        );
                        if (!ok) void get().close(key);
                    } catch {
                        patch(key, { state: "failed", message: "reattach failed" });
                    }
                })();
            }
        },
    };
});

// =====================================================================
// Helpers
// =====================================================================

function stringifyError(e: unknown): string {
    if (typeof e === "string") return e;
    if (e instanceof Error) return e.message;
    try {
        return JSON.stringify(e);
    } catch {
        return "Unknown error";
    }
}
