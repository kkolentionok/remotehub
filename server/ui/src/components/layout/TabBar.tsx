import { useEffect, useRef, useState } from "react";
import { Check, ChevronDown, Columns2, FolderOpen, HardDrive, Lock, Monitor, Plus, Server, Settings, Terminal, Users, Wrench, X } from "lucide-react";

import { useT } from "../../i18n";
import { leafKeys } from "../../lib/paneTree";
import { useSessionsStore, useUiStore } from "../../store";
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
    const section = useUiStore((s) => s.section);
    const setSection = useUiStore((s) => s.setSection);

    // The shown tab is the preview target while dragging, else the active.
    const shownTabId =
        dragging !== null && dragPreviewTabId !== null
            ? dragPreviewTabId
            : activeTabId;

    // No session tab active → an app section (Vault / Tools) is shown.
    const sectionActive = shownTabId === null;

    // Dragging a *pane* (not a tab) — the bar becomes a pop-out drop region.
    const paneDrag = dragging !== null && dragTabId === null;

    const [dragId, setDragId] = useState<string | null>(null);

    // Horizontal-scrolling strip for session tabs (overflow when many).
    const scrollerRef = useRef<HTMLDivElement>(null);
    useEffect(() => {
        const el = scrollerRef.current;
        if (!el || activeTabId === null) return;
        const active = el.querySelector<HTMLElement>(`[data-tabid="${activeTabId}"]`);
        active?.scrollIntoView({ inline: "nearest", block: "nearest" });
    }, [activeTabId, tabs.length]);

    // Storage scope switcher (Personal / Team). Team is the seam for the
    // future sync feature — disabled until a backend exists.
    const [scopeOpen, setScopeOpen] = useState(false);
    const vaultRef = useRef<HTMLDivElement>(null);
    useEffect(() => {
        if (!scopeOpen) return;
        const onDoc = (e: MouseEvent) => {
            if (vaultRef.current && !vaultRef.current.contains(e.target as Node)) {
                setScopeOpen(false);
            }
        };
        document.addEventListener("mousedown", onDoc);
        return () => document.removeEventListener("mousedown", onDoc);
    }, [scopeOpen]);

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
            {/* App sections — matte, square, neutral, not closable */}
            <div className={styles.vaultWrap} ref={vaultRef}>
                <button
                    type="button"
                    role="tab"
                    aria-selected={sectionActive && section === "vault"}
                    className={`${styles.section} ${sectionActive && section === "vault" ? styles.sectionOn : ""}`}
                    onClick={() => {
                        setActiveTab(null);
                        setSection("vault");
                    }}
                >
                    <Server size={14} className={styles.sectionIcon} />
                    <span className={styles.label}>{t("nav.vault")}</span>
                    <span
                        className={styles.chevBtn}
                        role="button"
                        aria-label={t("storage.scope")}
                        onClick={(e) => {
                            e.stopPropagation();
                            setScopeOpen((v) => !v);
                        }}
                    >
                        <ChevronDown size={13} className={styles.chev} />
                    </span>
                </button>
                {scopeOpen && (
                    <div className={styles.scopeMenu}>
                        <div className={styles.scopeHeader}>{t("storage.scope")}</div>
                        <button
                            type="button"
                            className={`${styles.scopeItem} ${styles.scopeItemOn}`}
                            onClick={() => setScopeOpen(false)}
                        >
                            <HardDrive size={14} />
                            <span>{t("storage.personal")}</span>
                            <Check size={14} className={styles.scopeCheck} />
                        </button>
                        <button type="button" className={styles.scopeItem} disabled title={t("storage.teamLocked")}>
                            <Users size={14} />
                            <span>{t("storage.team")}</span>
                            <Lock size={12} className={styles.scopeLock} />
                        </button>
                        <div className={styles.scopeHint}>{t("storage.teamLocked")}</div>
                    </div>
                )}
            </div>
            <button
                type="button"
                role="tab"
                aria-selected={sectionActive && section === "tools"}
                className={`${styles.section} ${sectionActive && section === "tools" ? styles.sectionOn : ""}`}
                onClick={() => {
                    setActiveTab(null);
                    setSection("tools");
                }}
            >
                <Wrench size={14} className={styles.sectionIcon} />
                <span className={styles.label}>{t("nav.tools")}</span>
            </button>

            <span className={styles.divider} aria-hidden="true" />

            {/* Live session tabs — pill, accent icon, closable. Scrolls
                horizontally when they overflow; wheel scrolls the strip. */}
            <div
                ref={scrollerRef}
                className={styles.scroller}
                onWheel={(e) => {
                    if (e.deltaY !== 0) e.currentTarget.scrollLeft += e.deltaY;
                }}
            >
            {tabs.map((tab) => {
                const keys = leafKeys(tab.root);
                const focused =
                    sessions.find((s) => s.key === tab.activePaneKey) ??
                    sessions.find((s) => s.key === keys[0]);
                const paneCount = keys.length;
                const SessIco = focused?.sftp ? FolderOpen : focused?.local ? Terminal : focused?.protocol === "rdp" ? Monitor : Server;
                const connecting =
                    !!focused &&
                    ["resolving", "connecting", "authenticating", "host_key_pending"].includes(
                        focused.state,
                    );
                return (
                    <button
                        key={tab.id}
                        type="button"
                        role="tab"
                        data-tabid={tab.id}
                        aria-selected={shownTabId === tab.id}
                        draggable
                        className={`${styles.sessionTab} ${shownTabId === tab.id ? styles.active : ""} ${dragging && shownTabId === tab.id ? styles.shown : ""} ${dragging && shownTabId !== tab.id ? styles.dimmed : ""} ${dragId === tab.id ? styles.dragging : ""}`}
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
                            e.preventDefault();
                            e.stopPropagation();
                            endDrag();
                        }}
                        onDragEnd={endDrag}
                    >
                        <SessIco
                            size={13}
                            className={`${styles.sessionIcon} ${connecting ? styles.connecting : ""}`}
                        />
                        <span className={styles.label}>
                            {paneCount > 1
                                ? t("tab.split")
                                : (focused?.title ?? "—")}
                        </span>
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
            </div>

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
