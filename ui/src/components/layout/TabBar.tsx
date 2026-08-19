import { useEffect, useRef, useState } from "react";
import { Check, ChevronDown, Columns2, Copy, FolderOpen, HardDrive, Loader2, Lock, Monitor, PictureInPicture2, Plus, RefreshCw, Search, Server, Settings, ShieldAlert, Terminal, Users, Wrench, X } from "lucide-react";

import { useT } from "../../i18n";
import { sync } from "../../lib/ipc";
import { leafKeys } from "../../lib/paneTree";
import { toggleSessionSearch, useSessionsStore, useTransferBadgeStore, useUiStore } from "../../store";
import { ContextMenu, type MenuItem } from "../ui/ContextMenu";
import { SnippetsMenu } from "./SnippetsMenu";
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
    const transferCounts = useTransferBadgeStore((s) => s.counts);
    const activeTabId = useSessionsStore((s) => s.activeTabId);
    const setActiveTab = useSessionsStore((s) => s.setActiveTab);
    const closeTab = useSessionsStore((s) => s.closeTab);
    const closeOtherTabs = useSessionsStore((s) => s.closeOtherTabs);
    const duplicateTab = useSessionsStore((s) => s.duplicateTab);
    const reconnectTab = useSessionsStore((s) => s.reconnectTab);
    const detachTermToWindow = useSessionsStore((s) => s.detachTermToWindow);
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
    const syncStatus = useUiStore((s) => s.syncStatus);
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
    // Right-click menu on a session tab.
    const [tabMenu, setTabMenu] = useState<{ x: number; y: number; tabId: string } | null>(
        null,
    );

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
    const [scopePos, setScopePos] = useState<{ top: number; left: number }>({ top: 0, left: 0 });
    // Account/sync state, refreshed each time the scope menu opens (cheap).
    const [syncCfg, setSyncCfg] = useState<{ logged_in: boolean; email: string | null } | null>(
        null,
    );
    useEffect(() => {
        if (!scopeOpen) return;
        let cancelled = false;
        void sync
            .getConfig()
            .then((c) => {
                if (!cancelled) setSyncCfg({ logged_in: c.logged_in, email: c.email });
            })
            .catch(() => {
                /* sync server unreachable / not configured — leave as null */
            });
        return () => {
            cancelled = true;
        };
    }, [scopeOpen]);
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

    // The focused pane of the active tab — the magnifier searches it. Only a
    // terminal (SSH / local shell, not RDP and not the SFTP browser) has a
    // find-in-output box.
    const activeTab = tabs.find((tb) => tb.id === activeTabId);
    const focusedKey = activeTab ? (activeTab.focusKey ?? activeTab.activePaneKey) : null;
    const focusedSession = focusedKey
        ? sessions.find((s) => s.key === focusedKey)
        : undefined;
    const canSearch =
        !!focusedSession &&
        focusedSession.protocol !== "rdp" &&
        !focusedSession.sftp;
    // A snippet runs into a live shell (SSH or local terminal). RDP takes no
    // text input this way and SFTP has no shell; those fall back to copy.
    const canRunSnippet =
        !!focusedSession &&
        focusedSession.protocol !== "rdp" &&
        !focusedSession.sftp &&
        focusedSession.state === "ready";

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
                    {syncStatus?.state === "error" && (
                        <ShieldAlert
                            size={13}
                            className={styles.vaultErr}
                            aria-label={t("settings.sync.statusError")}
                        />
                    )}
                    <span
                        className={styles.chevBtn}
                        role="button"
                        aria-label={t("storage.scope")}
                        onClick={(e) => {
                            e.stopPropagation();
                            const r = vaultRef.current?.getBoundingClientRect();
                            if (r) setScopePos({ top: r.bottom + 4, left: r.left });
                            setScopeOpen((v) => !v);
                        }}
                    >
                        <ChevronDown size={13} className={styles.chev} />
                    </span>
                </button>
                {scopeOpen && (
                    <div
                        className={styles.scopeMenu}
                        style={{ top: scopePos.top, left: scopePos.left }}
                    >
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
                        <div className={styles.scopeHint}>
                            {syncCfg?.logged_in
                                ? syncStatus?.state === "error"
                                    ? t("storage.accountAs", { email: syncCfg.email ?? "" })
                                    : t("storage.syncedAs", { email: syncCfg.email ?? "" })
                                : t("storage.notSignedIn")}
                        </div>
                        {syncCfg?.logged_in && syncStatus && (
                            <div className={styles.scopeHint}>
                                {syncStatus.state === "syncing"
                                    ? t("settings.sync.statusSyncing")
                                    : syncStatus.state === "error"
                                      ? t("settings.sync.statusError")
                                      : syncStatus.state === "ok"
                                        ? `${t("settings.sync.statusSynced")}${
                                              syncStatus.at_ms
                                                  ? " · " +
                                                    new Date(syncStatus.at_ms).toLocaleTimeString()
                                                  : ""
                                          }`
                                        : t("settings.sync.statusOn")}
                            </div>
                        )}
                        <button type="button" className={styles.scopeItem} disabled title={t("storage.teamLocked")}>
                            <Users size={14} />
                            <span>{t("storage.team")}</span>
                            <Lock size={12} className={styles.scopeLock} />
                        </button>
                        <div className={styles.scopeHint}>{t("storage.teamLocked")}</div>
                        <button
                            type="button"
                            className={styles.scopeItem}
                            onClick={() => {
                                setScopeOpen(false);
                                setDialog({ kind: "settings", section: "profile" });
                            }}
                        >
                            <Settings size={14} />
                            <span>{t("storage.manageSync")}</span>
                        </button>
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
                const transferCount = focused?.sftp ? (transferCounts[focused.key] ?? 0) : 0;
                const tabTooltip =
                    focused?.detectedOs && !focused.sftp && !focused.local
                        ? `${focused.title} · ${focused.detectedOs}`
                        : focused?.title;
                const connecting =
                    !!focused &&
                    ["resolving", "connecting", "authenticating"].includes(
                        focused.state,
                    );
                // A failed connect / abnormal drop shows a persistent red dot
                // on the tab until the user reopens or closes it.
                const failed = focused?.state === "failed";
                return (
                    <button
                        key={tab.id}
                        type="button"
                        role="tab"
                        data-tabid={tab.id}
                        aria-selected={shownTabId === tab.id}
                        title={tabTooltip}
                        draggable
                        className={`${styles.sessionTab} ${shownTabId === tab.id ? styles.active : ""} ${dragging && shownTabId === tab.id ? styles.shown : ""} ${dragging && shownTabId !== tab.id ? styles.dimmed : ""} ${dragId === tab.id ? styles.dragging : ""}`}
                        onClick={() => setActiveTab(tab.id)}
                        onAuxClick={(e) => {
                            if (e.button === 1) {
                                e.preventDefault();
                                void closeTab(tab.id);
                            }
                        }}
                        onMouseDown={(e) => {
                            // Suppress the middle-click autoscroll puck.
                            if (e.button === 1) e.preventDefault();
                        }}
                        onContextMenu={(e) => {
                            e.preventDefault();
                            e.stopPropagation();
                            setTabMenu({ x: e.clientX, y: e.clientY, tabId: tab.id });
                        }}
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
                        {connecting ? (
                            <Loader2 size={13} className={styles.spin} />
                        ) : (
                            <SessIco size={13} className={styles.sessionIcon} />
                        )}
                        {failed && (
                            <span
                                className={styles.errorDot}
                                aria-hidden="true"
                                title={focused?.message ?? undefined}
                            />
                        )}
                        <span className={styles.label}>
                            {paneCount > 1
                                ? t("tab.split")
                                : (focused?.title ?? "—")}
                        </span>
                        {transferCount > 0 && (
                            <span
                                className={styles.transferBadge}
                                title={t("tab.transfersActive", { n: String(transferCount) })}
                            >
                                {transferCount}
                            </span>
                        )}
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

            {/* Draggable empty area, then search + settings + window controls. */}
            <div className={styles.drag} data-tauri-drag-region />
            <SnippetsMenu focusedKey={focusedKey} canRun={canRunSnippet} />
            {canSearch && (
                <button
                    type="button"
                    className={styles.gear}
                    onClick={() => toggleSessionSearch(focusedKey)}
                    title={t("terminal.search.title")}
                    aria-label={t("terminal.search.title")}
                >
                    <Search size={15} />
                </button>
            )}
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
            {tabMenu && (
                <ContextMenu
                    x={tabMenu.x}
                    y={tabMenu.y}
                    onClose={() => setTabMenu(null)}
                    items={
                        ((): MenuItem[] => {
                            const mt = tabs.find((tb) => tb.id === tabMenu.tabId);
                            const ps = mt
                                ? sessions.find((s) => s.key === mt.activePaneKey)
                                : undefined;
                            const canPopOut =
                                !!ps &&
                                ps.protocol !== "rdp" &&
                                ps.state !== "closed" &&
                                ps.state !== "failed";
                            return [
                                ...(canPopOut
                                    ? ([
                                          {
                                              id: "popout",
                                              label: t("session.popOut"),
                                              icon: PictureInPicture2,
                                              onSelect: () =>
                                                  void detachTermToWindow(ps!.key),
                                          },
                                          { id: "sep0", separator: true },
                                      ] as MenuItem[])
                                    : []),
                                {
                                    id: "reconnect",
                                    label: t("tab.menu.reconnect"),
                                    icon: RefreshCw,
                                    onSelect: () => void reconnectTab(tabMenu.tabId),
                                },
                                {
                                    id: "dup",
                                    label: t("tab.menu.duplicate"),
                                    icon: Copy,
                                    onSelect: () => duplicateTab(tabMenu.tabId),
                                },
                                { id: "sep", separator: true },
                                {
                                    id: "close-others",
                                    label: t("tab.menu.closeOthers"),
                                    icon: Columns2,
                                    disabled: tabs.length < 2,
                                    onSelect: () => void closeOtherTabs(tabMenu.tabId),
                                },
                                {
                                    id: "close",
                                    label: t("tab.menu.close"),
                                    icon: X,
                                    onSelect: () => void closeTab(tabMenu.tabId),
                                },
                            ];
                        })()
                    }
                />
            )}
        </div>
    );
}
