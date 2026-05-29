import { useState } from "react";
import { Columns2, Plus, Server, Settings, X } from "lucide-react";

import { useT } from "../../i18n";
import { leafKeys } from "../../lib/paneTree";
import { TERMINAL_THEMES } from "../../lib/terminalThemes";
import { useSessionsStore, useSettingsStore, useUiStore } from "../../store";
import { WindowControls } from "./WindowControls";
import styles from "./TabBar.module.css";

/**
 * Application tab bar. The first tab is the pinned "Vault" (host manager,
 * not closable); the rest are workspace tabs, each holding one or more
 * session panes.
 *
 * Drag interactions:
 * - Drag a tab over other tabs → reorder live.
 * - Drag a tab down into the content → split (AppShell previews the target
 *   tab; the pane drop-zones do the merge).
 * - Drag a pane (by its header) over the bar → pop it out into a new tab;
 *   the bar shows a soft gradient to signal the drop region.
 */
export function TabBar() {
    const { t } = useT();
    const sessions = useSessionsStore((s) => s.sessions);
    const tabs = useSessionsStore((s) => s.tabs);
    const activeTabId = useSessionsStore((s) => s.activeTabId);
    const setActiveTab = useSessionsStore((s) => s.setActiveTab);
    const closeTab = useSessionsStore((s) => s.closeTab);
    const reorder = useSessionsStore((s) => s.reorder);
    const popOutSession = useSessionsStore((s) => s.popOutSession);
    const setDraggingSession = useSessionsStore((s) => s.setDraggingSession);
    const setDragPreviewTabId = useSessionsStore((s) => s.setDragPreviewTabId);
    const setDragTabId = useSessionsStore((s) => s.setDragTabId);
    const endDragStore = useSessionsStore((s) => s.endDrag);
    const dragging = useSessionsStore((s) => s.draggingSession);
    const dragPreviewTabId = useSessionsStore((s) => s.dragPreviewTabId);
    const dragTabId = useSessionsStore((s) => s.dragTabId);
    const openLauncher = useUiStore((s) => s.setLauncherOpen);
    const setDialog = useUiStore((s) => s.setDialog);

    // Active session tab borrows the terminal theme's colours so it merges
    // visually with the terminal below (Windows-Terminal "profile colour").
    const scheme = useSettingsStore(
        (s) => s.settings?.terminal_color_scheme ?? "default",
    );
    const termTheme = TERMINAL_THEMES[scheme] ?? TERMINAL_THEMES.default;

    // The shown tab is the preview target while dragging, else the active.
    const shownTabId =
        dragging !== null && dragPreviewTabId !== null
            ? dragPreviewTabId
            : activeTabId;

    // Dragging a *pane* (not a tab) — the bar becomes a pop-out drop region.
    const paneDrag = dragging !== null && dragTabId === null;

    const [dragId, setDragId] = useState<string | null>(null);

    const endDrag = () => {
        setDragId(null);
        endDragStore();
    };

    return (
        <div
            className={`${styles.bar} ${paneDrag ? styles.barDrop : ""}`}
            role="tablist"
            data-tauri-drag-region
            onDragOver={(e) => {
                if (!dragging) return;
                // In the bar (not over content): clear the split preview.
                if (dragPreviewTabId !== null) setDragPreviewTabId(null);
                if (paneDrag) {
                    e.preventDefault();
                    e.dataTransfer.dropEffect = "move";
                }
            }}
            onDrop={(e) => {
                if (paneDrag) {
                    e.preventDefault();
                    popOutSession(dragging);
                    endDrag();
                }
            }}
        >
            <button
                type="button"
                role="tab"
                aria-selected={shownTabId === null}
                className={`${styles.tab} ${styles.vault} ${shownTabId === null ? styles.active : ""} ${dragging && shownTabId !== null ? styles.dimmed : ""}`}
                onClick={() => setActiveTab(null)}
            >
                <Server size={14} />
                <span className={styles.label}>{t("nav.vault")}</span>
            </button>

            {tabs.map((tab) => {
                const keys = leafKeys(tab.root);
                const focused =
                    sessions.find((s) => s.key === tab.activePaneKey) ??
                    sessions.find((s) => s.key === keys[0]);
                const paneCount = keys.length;
                return (
                    <button
                        key={tab.id}
                        type="button"
                        role="tab"
                        aria-selected={shownTabId === tab.id}
                        draggable
                        className={`${styles.tab} ${shownTabId === tab.id ? styles.active : ""} ${dragging && shownTabId === tab.id ? styles.shown : ""} ${dragging && shownTabId !== tab.id ? styles.dimmed : ""} ${dragId === tab.id ? styles.dragging : ""}`}
                        style={
                            shownTabId === tab.id
                                ? {
                                      background: termTheme.background,
                                      color: termTheme.foreground,
                                  }
                                : undefined
                        }
                        onClick={() => setActiveTab(tab.id)}
                        onDragStart={(e) => {
                            setDragId(tab.id);
                            setDragTabId(tab.id);
                            setDraggingSession(tab.activePaneKey);
                            setDragPreviewTabId(null);
                            e.dataTransfer.effectAllowed = "move";
                            e.dataTransfer.setData("text/plain", tab.id);
                        }}
                        onDragOver={(e) => {
                            if (!dragId || dragId === tab.id) {
                                if (dragging) {
                                    e.preventDefault();
                                    e.dataTransfer.dropEffect = "move";
                                    if (dragPreviewTabId !== null)
                                        setDragPreviewTabId(null);
                                }
                                return;
                            }
                            // Live reorder: move the dragged tab toward the
                            // hovered one once the cursor crosses its midpoint
                            // (the midpoint guard prevents oscillation). Stay
                            // in the bar → no split preview.
                            e.preventDefault();
                            e.dataTransfer.dropEffect = "move";
                            if (dragPreviewTabId !== null) setDragPreviewTabId(null);
                            const rect = e.currentTarget.getBoundingClientRect();
                            const after = e.clientX > rect.left + rect.width / 2;
                            const from = tabs.findIndex((x) => x.id === dragId);
                            const over = tabs.findIndex((x) => x.id === tab.id);
                            if (from < over && after) reorder(dragId, tab.id);
                            else if (from > over && !after) reorder(dragId, tab.id);
                        }}
                        onDrop={(e) => {
                            // Dropped in the bar → reorder is already applied.
                            e.preventDefault();
                            e.stopPropagation();
                            endDrag();
                        }}
                        onDragEnd={endDrag}
                    >
                        <span
                            className={`${styles.dot} ${styles[`dot--${focused?.state ?? "connecting"}`] ?? ""}`}
                        />
                        <span className={styles.label}>{focused?.title ?? "—"}</span>
                        {paneCount > 1 && (
                            <span className={styles.count} title={t("tab.panes", { n: paneCount })}>
                                <Columns2 size={11} />
                                {paneCount}
                            </span>
                        )}
                        <span
                            className={styles.close}
                            role="button"
                            aria-label={t("common.close")}
                            onClick={(e) => {
                                e.stopPropagation();
                                void closeTab(tab.id);
                            }}
                        >
                            <X size={12} />
                        </span>
                    </button>
                );
            })}

            <button
                type="button"
                className={styles.add}
                onClick={() => openLauncher(true)}
                title={t("tab.new")}
                aria-label={t("tab.new")}
            >
                <Plus size={15} />
            </button>

            {/* Draggable empty area, then settings + window controls. */}
            <div className={styles.drag} data-tauri-drag-region />
            <button
                type="button"
                className={styles.gear}
                onClick={() =>
                    setDialog({
                        kind: "settings",
                        section: activeTabId !== null ? "terminal" : undefined,
                    })
                }
                title={t("settings.title")}
                aria-label={t("settings.title")}
            >
                <Settings size={16} />
            </button>
            <WindowControls />
        </div>
    );
}
