import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { useT } from "../../i18n";
import {
    openSessionSearch,
    subscribeToBackendEvents,
    useCredentialsStore,
    useGroupsStore,
    useHostsStore,
    useSessionsStore,
    useSettingsStore,
    useUiStore,
} from "../../store";
import { leafKeys } from "../../lib/paneTree";
import { app as appApi, hotkeys, sync as syncApi } from "../../lib/ipc";
import { runUpdateCheck } from "../../lib/updater";
import type { SyncStatus } from "../../lib/types";
import { DialogHost } from "./DialogHost";
import { FocusRail } from "../session/FocusRail";
import { HomeView } from "./HomeView";
import { Launcher } from "./Launcher";
import { PaneGroup } from "./PaneGroup";
import { ShortcutsSheet } from "./ShortcutsSheet";
import { TabBar } from "./TabBar";
import { ToolsView } from "./ToolsView";
import { SnippetsDock } from "./SnippetsDock";
import { UpdateBanner } from "./UpdateBanner";
import styles from "./AppShell.module.css";

/**
 * Root layout: a tab bar on top (pinned Vault + one tab per session),
 * with the active tab's content below. Every tab — the Vault host
 * manager and each session terminal — stays mounted and is toggled via
 * visibility, so scrollback, form drafts, and focus survive switching.
 *
 * On mount: load the four stores and register backend event
 * subscriptions; keep the i18n locale synced with `settings.language`.
 */
export function AppShell() {
    const { locale, setLocale } = useT();
    const settingsLanguage = useSettingsStore((s) => s.settings?.language);
    const theme = useSettingsStore((s) => s.settings?.theme);
    const tabs = useSessionsStore((s) => s.tabs);
    const activeTabId = useSessionsStore((s) => s.activeTabId);
    const draggingSession = useSessionsStore((s) => s.draggingSession);
    const dragPreviewTabId = useSessionsStore((s) => s.dragPreviewTabId);
    const dragTabId = useSessionsStore((s) => s.dragTabId);
    const setDragPreviewTabId = useSessionsStore((s) => s.setDragPreviewTabId);
    const launcherOpen = useUiStore((s) => s.launcherOpen);
    const shortcutsOpen = useUiStore((s) => s.shortcutsOpen);
    const section = useUiStore((s) => s.section);
    const snippetsPinned = useUiStore((s) => s.snippetsPinned);

    // Borderless window (decorations:false): when maximized on Windows the
    // client area extends past the work area by the resize-border thickness,
    // so the bottom strip slips under the taskbar and clips the last terminal
    // row. Track the maximized state and inset the shell to compensate.
    const [maximized, setMaximized] = useState(false);
    useEffect(() => {
        const win = getCurrentWindow();
        let unlisten: (() => void) | undefined;
        void win.isMaximized().then(setMaximized).catch(() => {});
        void win
            .onResized(() => {
                void win.isMaximized().then(setMaximized).catch(() => {});
            })
            .then((u) => {
                unlisten = u;
            });
        return () => unlisten?.();
    }, []);

    // While dragging a tab, show the split-target tab's workspace (so the
    // green drop zones land on the tab being split into, not the dragged
    // one). Otherwise show the active tab.
    const visibleTabId =
        draggingSession !== null && dragPreviewTabId !== null
            ? dragPreviewTabId
            : activeTabId;

    // When a *tab* is dragged into the content area, preview the split
    // target there: the active tab normally, or — if the active tab is the
    // one being dragged — its neighbour. (Pane drags don't preview; they
    // split within the shown workspace.)
    const onStageDragOver = () => {
        if (dragTabId === null) return;
        let target: string | null;
        if (activeTabId !== null && activeTabId !== dragTabId) {
            target = activeTabId;
        } else {
            const idx = tabs.findIndex((t) => t.id === dragTabId);
            const n = tabs[idx - 1] ?? tabs[idx + 1];
            target = n ? n.id : null;
        }
        if (target !== null && target !== dragPreviewTabId) {
            setDragPreviewTabId(target);
        }
    };

    useEffect(() => {
        void useHostsStore.getState().load();
        void useGroupsStore.getState().load();
        void useCredentialsStore.getState().load();
        void useSettingsStore.getState().load();
        // Rebuild any sessions the Rust process kept alive across a reload.
        void useSessionsStore.getState().restoreSessions();

        // Apply a saved custom unstick hotkey (Rust registers Ctrl+Alt+K by
        // default; a stored override replaces it once the webview boots).
        try {
            const raw = localStorage.getItem("pingie.unstickHotkey");
            if (raw !== null) void hotkeys.setUnstick(JSON.parse(raw));
        } catch {
            /* ignore malformed value */
        }

        let cleanup: (() => void) | undefined;
        subscribeToBackendEvents().then((c) => {
            cleanup = c;
        });
        return () => {
            cleanup?.();
        };
    }, []);

    useEffect(() => {
        let unlisten: (() => void) | undefined;
        void (async () => {
            const { listen } = await import("@tauri-apps/api/event");
            unlisten = await listen<string>("tray:connect", (e) => {
                const host = useHostsStore.getState().items.find((h) => h.id === e.payload);
                if (host) void useSessionsStore.getState().open(host);
            });
        })();
        return () => unlisten?.();
    }, []);

    // Tray "Port forwarding" → show Tools with the Forwards sub-section.
    useEffect(() => {
        let unlisten: (() => void) | undefined;
        void (async () => {
            const { listen } = await import("@tauri-apps/api/event");
            unlisten = await listen("tray:open-forwards", () => {
                const ui = useUiStore.getState();
                ui.setSection("tools");
                ui.setToolsSection("forwards");
            });
        })();
        return () => unlisten?.();
    }, []);

    // Tray Quit with live sessions bounces here: show a confirm before the
    // real exit (which the dialog triggers via the app_quit command).
    useEffect(() => {
        let unlisten: (() => void) | undefined;
        void (async () => {
            const { listen } = await import("@tauri-apps/api/event");
            unlisten = await listen<number>("app:confirm-quit", (e) => {
                useUiStore.getState().setDialog({ kind: "quit-confirm", count: e.payload });
            });
        })();
        return () => unlisten?.();
    }, []);

    // Report the live-session count to the backend (drives the tray tooltip
    // and the quit-confirm threshold). Runs whenever the count changes.
    const sessionCount = useSessionsStore((s) => s.sessions.length);
    useEffect(() => {
        void appApi.reportSessions(sessionCount);
    }, [sessionCount]);

    // Background auto-sync: mirror live status into the store, seed it with the
    // current snapshot, and — if signed in but the vault password isn't set yet
    // — prompt for it. The prompt reappears each launch until they set it.
    useEffect(() => {
        let unlisten: (() => void) | undefined;
        void (async () => {
            try {
                const status = await syncApi.status();
                useUiStore.getState().setSyncStatus(status);
            } catch {
                /* status is best-effort */
            }
            try {
                const cfg = await syncApi.getConfig();
                if (cfg.logged_in && !cfg.has_master) {
                    useUiStore.getState().setDialog({ kind: "sync-master", mode: "set" });
                }
            } catch {
                /* not configured yet */
            }
            const { listen } = await import("@tauri-apps/api/event");
            unlisten = await listen<SyncStatus>("sync:status", (e) => {
                const st = e.payload;
                useUiStore.getState().setSyncStatus(st);
                // A completed pass may have applied remote changes to local
                // storage (apply_snapshot writes directly, without CRUD
                // events) — refresh the collections so the UI reflects them.
                if (st.state === "ok" && st.had_remote) {
                    void useHostsStore.getState().load();
                    void useGroupsStore.getState().load();
                    void useCredentialsStore.getState().load();
                }
            });
        })();
        return () => unlisten?.();
    }, []);

    useEffect(() => {
        if (!settingsLanguage) return;
        if (settingsLanguage !== locale) {
            setLocale(settingsLanguage);
        }
    }, [settingsLanguage, locale, setLocale]);

    // Drive the app color theme from settings (overrides the OS media
    // query). "system" defers to the OS.
    useEffect(() => {
        document.documentElement.setAttribute("data-theme", theme ?? "navy");
    }, [theme]);

    // Split shortcuts: Ctrl+Shift+E splits right, Ctrl+Shift+D splits down.
    // Use e.code (physical key) so it works on any keyboard layout.
    useEffect(() => {
        const onKey = (e: KeyboardEvent) => {
            if (!(e.ctrlKey && e.shiftKey)) return;
            if (e.code === "KeyE" || e.code === "KeyD") {
                e.preventDefault();
                useSessionsStore
                    .getState()
                    .requestSplit(e.code === "KeyE" ? "row" : "col");
            }
        };
        window.addEventListener("keydown", onKey);
        return () => window.removeEventListener("keydown", onKey);
    }, []);

    // Ctrl/Cmd+K opens the command palette (the Launcher in command mode),
    // globally — including while the terminal is focused. We listen in the
    // CAPTURE phase and stopImmediatePropagation so xterm never sees the key
    // and doesn't also fire the shell's kill-line (\x0b). Trade-off: Ctrl+K
    // kill-line in the shell is overridden by the palette; Ctrl+U (kill line)
    // and Ctrl+W (kill word) still reach the shell unaffected.
    useEffect(() => {
        const onKey = (e: KeyboardEvent) => {
            if (!(e.ctrlKey || e.metaKey) || e.altKey || e.shiftKey) return;
            if (e.code !== "KeyK") return; // physical key — layout-independent (RU/EN)
            e.preventDefault();
            e.stopImmediatePropagation();
            // Force command mode (no pending split) and open.
            useSessionsStore.setState({ splitTarget: null });
            useUiStore.getState().setLauncherOpen(true);
        };
        window.addEventListener("keydown", onKey, { capture: true });
        return () => window.removeEventListener("keydown", onKey, { capture: true });
    }, []);

    // Ctrl/Cmd+F opens find-in-output for the active tab's focused terminal.
    // Captured (like Ctrl+K) so it beats xterm's own key handling and the
    // WebView's native find, and works while the terminal is focused. No-op
    // unless a terminal (SSH / local, not RDP / SFTP) pane is focused.
    useEffect(() => {
        const onKey = (e: KeyboardEvent) => {
            if (!(e.ctrlKey || e.metaKey) || e.altKey || e.shiftKey) return;
            if (e.code !== "KeyF") return; // physical key — layout-independent
            const st = useSessionsStore.getState();
            const tab = st.tabs.find((tb) => tb.id === st.activeTabId);
            if (!tab) return;
            const key = tab.focusKey ?? tab.activePaneKey;
            const sess = st.sessions.find((s) => s.key === key);
            if (!sess || sess.protocol === "rdp" || sess.sftp) return;
            e.preventDefault();
            e.stopImmediatePropagation();
            openSessionSearch(key);
        };
        window.addEventListener("keydown", onKey, { capture: true });
        return () => window.removeEventListener("keydown", onKey, { capture: true });
    }, []);

    // Suppress the WebView's native right-click menu (Back / Reload / Inspect…)
    // everywhere — it's meaningless in a desktop app. Components that want a
    // real menu render their own <ContextMenu> from an onContextMenu handler;
    // the terminal keeps its right-click-to-paste (its own listener still runs).
    useEffect(() => {
        const onCtx = (e: MouseEvent) => e.preventDefault();
        document.addEventListener("contextmenu", onCtx);
        // Middle-button (button 1) autoscroll puck is never wanted here, and
        // it swallows the auxclick we use for "close tab" / "open in
        // background". Cancel it app-wide in the capture phase (a React
        // onMouseDown on the element is too late inside a scroll container).
        const onMidDown = (e: MouseEvent) => {
            if (e.button === 1) e.preventDefault();
        };
        document.addEventListener("mousedown", onMidDown, true);
        return () => {
            document.removeEventListener("contextmenu", onCtx);
            document.removeEventListener("mousedown", onMidDown, true);
        };
    }, []);

    // `?` toggles the keyboard-shortcuts cheat sheet — but never while
    // typing in an input/textarea/contenteditable (incl. xterm's helper
    // textarea) or with a modifier held, so it can't hijack a real "?".
    useEffect(() => {
        const onKey = (e: KeyboardEvent) => {
            if (e.key !== "?" || e.ctrlKey || e.metaKey || e.altKey) return;
            const el = document.activeElement as HTMLElement | null;
            if (
                el &&
                (el.tagName === "INPUT" ||
                    el.tagName === "TEXTAREA" ||
                    el.isContentEditable)
            ) {
                return;
            }
            e.preventDefault();
            const ui = useUiStore.getState();
            ui.setShortcutsOpen(!ui.shortcutsOpen);
        };
        window.addEventListener("keydown", onKey);
        return () => window.removeEventListener("keydown", onKey);
    }, []);

    // Silent update check on launch: if a newer version is published it
    // downloads in the background and surfaces the restart banner. Quiet on
    // failure (endpoint may be unreachable / not configured).
    useEffect(() => {
        void runUpdateCheck({ silent: true });
    }, []);

    return (
        <div className={styles.shell} data-maximized={maximized || undefined}>
            <TabBar />
            <UpdateBanner />
            <div className={styles.stage} onDragOver={onStageDragOver}>
                <div
                    className={styles.pane}
                    style={{
                        display:
                            visibleTabId === null && section === "vault"
                                ? "flex"
                                : "none",
                    }}
                >
                    <HomeView />
                </div>
                <div
                    className={styles.pane}
                    style={{
                        display:
                            visibleTabId === null && section === "tools"
                                ? "flex"
                                : "none",
                    }}
                >
                    <ToolsView />
                </div>
                {tabs.map((tab) => {
                    const paneCount = leafKeys(tab.root).length;
                    const focusActive = tab.focusKey != null && paneCount > 1;
                    const ctx = {
                        tabId: tab.id,
                        activePaneKey: tab.activePaneKey,
                        tabVisible: tab.id === visibleTabId,
                        paneCount,
                        focusKey: focusActive ? tab.focusKey ?? null : null,
                    };
                    return (
                        <div
                            key={tab.id}
                            className={styles.pane}
                            style={{ display: tab.id === visibleTabId ? "flex" : "none" }}
                        >
                            {focusActive ? (
                                <div className={styles.focusWrap}>
                                    <FocusRail tabId={tab.id} />
                                    <div className={styles.focusMain}>
                                        <PaneGroup node={tab.root} ctx={ctx} />
                                    </div>
                                </div>
                            ) : paneCount > 1 ? (
                                <div className={styles.splitWrap}>
                                    <PaneGroup node={tab.root} ctx={ctx} />
                                </div>
                            ) : (
                                <PaneGroup node={tab.root} ctx={ctx} />
                            )}
                        </div>
                    );
                })}
                {snippetsPinned && visibleTabId !== null && <SnippetsDock />}
            </div>
            <DialogHost />
            {launcherOpen && <Launcher />}
            {shortcutsOpen && <ShortcutsSheet />}
        </div>
    );
}
