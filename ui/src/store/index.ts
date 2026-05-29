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
        d.inlinePassword !== ""
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

interface OutputSink {
    buffer: Uint8Array[];
    writer: ((data: Uint8Array) => void) | null;
}

const sessionOutput = new Map<string, OutputSink>();

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
    sessions: SessionTab[];
    activeSessionKey: string | null;
    open: (host: HostDto) => Promise<void>;
    close: (key: string) => Promise<void>;
    setActive: (key: string | null) => void;
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

    return {
        sessions: [],
        activeSessionKey: null,

        open: async (host) => {
            const key =
                typeof crypto !== "undefined" && "randomUUID" in crypto
                    ? crypto.randomUUID()
                    : `s_${Date.now()}_${Math.random().toString(36).slice(2)}`;
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
            set((s) => ({ sessions: [...s.sessions, tab], activeSessionKey: key }));

            try {
                const res = await sessionsApi.open(
                    {
                        host_id: host.id,
                        credential_id: host.default_credential_id ?? null,
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
        },

        close: async (key) => {
            const tab = get().sessions.find((t) => t.key === key);
            if (tab?.sessionId) {
                try {
                    await sessionsApi.close(tab.sessionId);
                } catch {
                    /* actor may already be gone — ignore */
                }
            }
            sessionOutput.delete(key);
            set((s) => {
                const sessions = s.sessions.filter((t) => t.key !== key);
                const activeSessionKey =
                    s.activeSessionKey === key
                        ? (sessions[sessions.length - 1]?.key ?? null)
                        : s.activeSessionKey;
                return { sessions, activeSessionKey };
            });
        },

        setActive: (key) => set({ activeSessionKey: key }),

        sendInput: (key, data) => {
            const tab = get().sessions.find((t) => t.key === key);
            if (!tab?.sessionId) return;
            void sessionsApi
                .sendInput({ session_id: tab.sessionId, data: Array.from(data) })
                .catch(() => {});
        },

        resize: (key, cols, rows) => {
            const tab = get().sessions.find((t) => t.key === key);
            if (!tab?.sessionId) return;
            void sessionsApi
                .resize({ session_id: tab.sessionId, width: cols, height: rows })
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
