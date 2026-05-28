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
    settings as settingsApi,
} from "../lib/ipc";
import type {
    CredentialDto,
    GroupId,
    HostDto,
    HostGroupDto,
    HostId,
    Settings,
} from "../lib/types";

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
    | { kind: "settings" }
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

    selectHost: (id: HostId | null) => void;
    startDraft: (defaultGroupId?: GroupId | null) => void;
    updateDraft: (patch: Partial<HostDraft>) => void;
    clearDraft: () => void;
    setDialog: (dialog: DialogKind) => void;
    closeDialog: () => void;
    setSearchQuery: (q: string) => void;
    toggleGroupCollapsed: (id: GroupId) => void;
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

    selectHost: (id) => set({ selectedHostId: id, draft: null }),
    startDraft: (defaultGroupId = null) =>
        set({ draft: emptyDraft(defaultGroupId), selectedHostId: null }),
    updateDraft: (patch) =>
        set((s) => (s.draft ? { draft: { ...s.draft, ...patch } } : s)),
    clearDraft: () => set({ draft: null }),
    setDialog: (dialog) => set({ dialog }),
    closeDialog: () => set({ dialog: { kind: "none" } }),
    setSearchQuery: (searchQuery) => set({ searchQuery }),
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
