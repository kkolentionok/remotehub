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

import {
    credentials as credentialsApi,
    events,
    groups as groupsApi,
    hosts as hostsApi,
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

    selectHost: (id: HostId | null) => void;
    startDraft: (defaultGroupId?: GroupId | null) => void;
    updateDraft: (patch: Partial<HostDraft>) => void;
    clearDraft: () => void;
    setDialog: (dialog: DialogKind) => void;
    closeDialog: () => void;
    setSearchQuery: (q: string) => void;
    toggleGroupCollapsed: (id: GroupId) => void;
    setLauncherOpen: (open: boolean) => void;
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
    /** Set while a TOFU host-key decision is pending. */
    hostKey: { fingerprint: string; keyType: string } | null;
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
    resize: (key: string, cols: number, rows: number) => void;
    acceptHostKey: (key: string) => Promise<void>;
    rejectHostKey: (key: string) => Promise<void>;
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
                break;
            case "data":
                pushOutput(key, Uint8Array.from(ev.bytes));
                break;
            case "auth_failed":
                patch(key, { state: "failed", message: `Auth failed (${ev.method})` });
                break;
            case "host_key_prompt":
                patch(key, {
                    state: "host_key_pending",
                    hostKey: {
                        fingerprint: ev.fingerprint_sha256,
                        keyType: ev.key_type,
                    },
                });
                break;
            case "error":
                patch(key, { message: ev.message });
                break;
            case "closed":
                patch(key, { state: "closed", message: closeReasonText(ev.reason) });
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
        const tab: SessionTab = {
            key,
            sessionId: null,
            hostId: host.id,
            title: host.display_name ?? host.name,
            protocol: host.protocol,
            state: "connecting",
            message: null,
            hostKey: null,
        };
        sessionOutput.set(key, { buffer: [], writer: null });
        set((s) => ({ sessions: [...s.sessions, tab] }));

        void (async () => {
            try {
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

    /** Tear down one session (backend + output buffer). */
    const teardownSession = async (key: string) => {
        const sess = get().sessions.find((t) => t.key === key);
        if (sess?.sessionId) {
            try {
                await sessionsApi.close(sess.sessionId);
            } catch {
                /* actor may already be gone — ignore */
            }
        }
        sessionOutput.delete(key);
        sessionSnapshots.delete(key);
        lastDims.delete(key);
    };

    return {
        sessions: [],
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
            await teardownSession(key);
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
            void sessionsApi
                .sendInput({ session_id: tab.sessionId, data: Array.from(data) })
                .catch(() => {});
        },

        resize: (key, cols, rows) => {
            const sess = get().sessions.find((t) => t.key === key);
            if (!sess?.sessionId) return;
            const dim = `${cols}x${rows}`;
            if (lastDims.get(key) === dim) return; // unchanged → no SIGWINCH
            lastDims.set(key, dim);
            void sessionsApi
                .resize({ session_id: sess.sessionId, width: cols, height: rows })
                .catch(() => {});
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
