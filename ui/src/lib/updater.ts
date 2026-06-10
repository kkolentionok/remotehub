import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { create } from "zustand";

export type UpdateState =
    | { kind: "idle" }
    | { kind: "checking" }
    | { kind: "downloading"; version: string; pct: number }
    | { kind: "ready"; version: string; notes: string }
    | { kind: "uptodate" }
    | { kind: "error"; message: string };

interface UpdateStore {
    state: UpdateState;
    set: (s: UpdateState) => void;
}

/** View-state for the in-app updater (banner + About section). */
export const useUpdateStore = create<UpdateStore>((set) => ({
    state: { kind: "idle" },
    set: (s) => set({ state: s }),
}));

// The downloaded Update object is held here so install() can run later, when
// the user clicks "Restart to apply" — download is silent, install is deferred.
let pending: Update | null = null;

/**
 * Check the configured endpoint for a newer version. If found, download it
 * silently and move to "ready"; the user is then prompted to restart. The
 * install itself is deferred to {@link applyUpdateAndRestart}.
 *
 * - silent=true (on launch): stays quiet when up to date or on error
 *   (e.g. endpoint unreachable / not configured yet).
 * - silent=false (manual button): surfaces "up to date" and errors.
 */
export async function runUpdateCheck(opts: { silent: boolean }): Promise<void> {
    const set = useUpdateStore.getState().set;
    const cur = useUpdateStore.getState().state;
    // Don't start a new check while one is in flight or an update is staged.
    if (cur.kind === "checking" || cur.kind === "downloading" || cur.kind === "ready") {
        return;
    }
    try {
        set({ kind: "checking" });
        const update = await check();
        if (!update) {
            set(opts.silent ? { kind: "idle" } : { kind: "uptodate" });
            return;
        }
        pending = update;
        const version = update.version;
        const notes = update.body ?? "";
        let total = 0;
        let got = 0;
        set({ kind: "downloading", version, pct: 0 });
        await update.download((ev) => {
            if (ev.event === "Started") {
                total = ev.data.contentLength ?? 0;
            } else if (ev.event === "Progress") {
                got += ev.data.chunkLength;
                set({
                    kind: "downloading",
                    version,
                    pct: total > 0 ? Math.min(100, Math.round((got / total) * 100)) : 0,
                });
            }
        });
        set({ kind: "ready", version, notes });
    } catch (e: unknown) {
        const message = e instanceof Error ? e.message : String(e);
        // On a silent launch check, a missing/unreachable endpoint is normal —
        // don't bother the user; just go idle.
        set(opts.silent ? { kind: "idle" } : { kind: "error", message });
    }
}

/** Install the already-downloaded update and relaunch into the new version. */
export async function applyUpdateAndRestart(): Promise<void> {
    if (!pending) return;
    try {
        await pending.install();
        await relaunch();
    } catch (e: unknown) {
        useUpdateStore.getState().set({
            kind: "error",
            message: e instanceof Error ? e.message : String(e),
        });
    }
}

/** Dismiss the "ready"/"uptodate"/"error" state back to idle. */
export function dismissUpdate(): void {
    useUpdateStore.getState().set({ kind: "idle" });
}
