import { useEffect } from "react";

import { useT } from "../../i18n";
import {
    subscribeToBackendEvents,
    useCredentialsStore,
    useGroupsStore,
    useHostsStore,
    useSessionsStore,
    useSettingsStore,
    useUiStore,
} from "../../store";
import { leafKeys } from "../../lib/paneTree";
import { DialogHost } from "./DialogHost";
import { HomeView } from "./HomeView";
import { Launcher } from "./Launcher";
import { PaneGroup } from "./PaneGroup";
import { TabBar } from "./TabBar";
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

        let cleanup: (() => void) | undefined;
        subscribeToBackendEvents().then((c) => {
            cleanup = c;
        });
        return () => {
            cleanup?.();
        };
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
        document.documentElement.setAttribute("data-theme", theme ?? "system");
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

    return (
        <div className={styles.shell}>
            <TabBar />
            <div className={styles.stage} onDragOver={onStageDragOver}>
                <div
                    className={styles.pane}
                    style={{ display: visibleTabId === null ? "flex" : "none" }}
                >
                    <HomeView />
                </div>
                {tabs.map((tab) => (
                    <div
                        key={tab.id}
                        className={styles.pane}
                        style={{ display: tab.id === visibleTabId ? "flex" : "none" }}
                    >
                        <PaneGroup
                            node={tab.root}
                            ctx={{
                                tabId: tab.id,
                                activePaneKey: tab.activePaneKey,
                                tabVisible: tab.id === visibleTabId,
                                paneCount: leafKeys(tab.root).length,
                            }}
                        />
                    </div>
                ))}
            </div>
            <DialogHost />
            {launcherOpen && <Launcher />}
        </div>
    );
}
