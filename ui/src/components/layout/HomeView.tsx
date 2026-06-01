import { type CSSProperties, type ComponentType, type PointerEvent as ReactPointerEvent, useMemo, useRef, useState } from "react";
import {
    FolderPlus,
    Info,
    LayoutGrid,
    List as ListIcon,
    Monitor,
    Pencil,
    Plus,
    Search,
    Server,
    Zap,
} from "lucide-react";

import { useT } from "../../i18n";
import { groupColor } from "../../lib/groupColor";
import type { HostDto } from "../../lib/types";
import {
    useGroupsStore,
    useHostsStore,
    useSessionsStore,
    useUiStore,
} from "../../store";
import { HostDetail } from "../host/HostDetail";
import { osIcon } from "../host/HostIcon";
import { ProtocolBadge } from "../ui/ProtocolBadge";
import styles from "./HomeView.module.css";

type ViewMode = "list" | "grid";

/** Icon for a host: OS brand glyph once detected, else protocol fallback. */
function hostIcon(h: HostDto): ComponentType<{ size?: number | string }> {
    const os = h.detected_os ? osIcon(h.detected_os) : null;
    return os ?? (h.protocol === "rdp" ? Monitor : Server);
}

/**
 * The pinned "Vault" tab, redesigned as a command center: a search hero,
 * quick actions, a group/filter toolbar, and the host list (or grid).
 * Selecting a host docks the editor on the right (the existing live-save
 * HostDetail, kept intact for focus continuity — its visual pass comes
 * next). Sessions open as sibling tabs in the TabBar.
 */
export function HomeView() {
    const { t } = useT();
    const hosts = useHostsStore((s) => s.items);
    const groups = useGroupsStore((s) => s.items);
    const search = useUiStore((s) => s.searchQuery);
    const setSearch = useUiStore((s) => s.setSearchQuery);
    const selectedHostId = useUiStore((s) => s.selectedHostId);
    const draft = useUiStore((s) => s.draft);
    const selectHost = useUiStore((s) => s.selectHost);
    const startDraft = useUiStore((s) => s.startDraft);
    const setDialog = useUiStore((s) => s.setDialog);
    const openSession = useSessionsStore((s) => s.open);
    const sessions = useSessionsStore((s) => s.sessions);

    const [view, setView] = useState<ViewMode>("list");
    const [filter, setFilter] = useState<string>("all"); // "all" | groupId

    const docked = selectedHostId !== null || draft !== null;

    // Per-host connection state — drives the left strip:
    // yellow while connecting, green once ready.
    const hostConnState = useMemo(() => {
        const connecting = new Set([
            "resolving",
            "connecting",
            "authenticating",
            "host_key_pending",
        ]);
        const m = new Map<string, "connecting" | "connected">();
        for (const t of sessions) {
            if (t.state === "ready") m.set(t.hostId, "connected");
            else if (connecting.has(t.state) && m.get(t.hostId) !== "connected") {
                m.set(t.hostId, "connecting");
            }
        }
        return m;
    }, [sessions]);

    // Drag-to-scroll the filter chips when they overflow horizontally.
    const filtersRef = useRef<HTMLDivElement>(null);
    const onFiltersPointerDown = (e: ReactPointerEvent<HTMLDivElement>) => {
        const el = filtersRef.current;
        if (!el) return;
        const startX = e.clientX;
        const startScroll = el.scrollLeft;
        let moved = false;
        const move = (ev: PointerEvent) => {
            const dx = ev.clientX - startX;
            if (Math.abs(dx) > 4) moved = true;
            el.scrollLeft = startScroll - dx;
        };
        const up = () => {
            window.removeEventListener("pointermove", move);
            window.removeEventListener("pointerup", up);
            // Swallow the click that ends a drag so we don't toggle a filter.
            if (moved) {
                const block = (ce: Event) => {
                    ce.stopPropagation();
                    ce.preventDefault();
                };
                el.addEventListener("click", block, { capture: true, once: true });
            }
        };
        window.addEventListener("pointermove", move);
        window.addEventListener("pointerup", up);
    };

    const groupName = useMemo(() => {
        const m = new Map(groups.map((g) => [g.id, g.name]));
        return (id: string | null) => (id ? (m.get(id) ?? null) : null);
    }, [groups]);

    const q = search.trim().toLowerCase();
    const filtered = useMemo(
        () =>
            hosts.filter((h) => {
                if (filter !== "all" && h.group_id !== filter) return false;
                if (!q) return true;
                const hay = [
                    h.display_name ?? "",
                    h.name,
                    h.hostname,
                    h.username,
                    ...h.tags,
                ]
                    .join(" ")
                    .toLowerCase();
                return hay.includes(q);
            }),
        [hosts, filter, q],
    );

    const chips = useMemo(
        () => [
            { key: "all", label: t("storage.filter.all"), n: hosts.length, color: null as string | null },
            ...groups.map((g) => ({
                key: g.id,
                label: g.name,
                n: hosts.filter((h) => h.group_id === g.id).length,
                color: groupColor(g.id),
            })),
        ],
        [groups, hosts, t],
    );

    const connect = (h: HostDto) => {
        void openSession(h);
    };
    const newHost = () => startDraft(filter === "all" ? null : filter);

    return (
        <div className={styles.home}>
            <div className={styles.header}>
                {/* command hero — fixed */}
                <div className={styles.hero}>
                        <div className={styles.cmd}>
                            <Search size={20} className={styles.cmdIcon} />
                            <input
                                className={styles.cmdInput}
                                value={search}
                                onChange={(e) => setSearch(e.target.value)}
                                onKeyDown={(e) => {
                                    const first = filtered[0];
                                    if (e.key === "Enter" && first) connect(first);
                                    if (e.key === "Escape") setSearch("");
                                }}
                                placeholder={t("storage.search.placeholder")}
                                spellCheck={false}
                            />
                            <kbd className={styles.kbd}>⌘K</kbd>
                        </div>
                        <div className={styles.hint}>
                            <span>
                                <kbd className={styles.kbd}>↵</kbd> {t("storage.hint.connect")}
                            </span>
                            <span>
                                <kbd className={styles.kbd}>esc</kbd> {t("storage.hint.clear")}
                            </span>
                        </div>
                    </div>

                    {/* toolbar — sticks just below the search bar */}
                    <div className={styles.toolbar}>
                        <div
                            className={styles.filters}
                            ref={filtersRef}
                            onPointerDown={onFiltersPointerDown}
                        >
                            {chips.map((c) => (
                                <div
                                    key={c.key}
                                    className={c.key === "all" ? styles.stickyAll : styles.chipWrap}
                                >
                                    <button
                                        type="button"
                                        className={`${styles.chip} ${filter === c.key ? styles.chipOn : ""}`}
                                        onClick={() => setFilter(c.key)}
                                    >
                                        {c.color && (
                                            <span
                                                className={styles.gdot}
                                                style={{ background: c.color }}
                                            />
                                        )}
                                        {c.label} <span className={styles.chipN}>{c.n}</span>
                                        {c.key !== "all" && filter === c.key && (
                                            <span
                                                className={styles.chipEdit}
                                                role="button"
                                                aria-label={t("storage.editGroup")}
                                                title={t("storage.editGroup")}
                                                onClick={(e) => {
                                                    e.stopPropagation();
                                                    setDialog({
                                                        kind: "group-rename",
                                                        groupId: c.key,
                                                    });
                                                }}
                                            >
                                                <Pencil size={12} />
                                            </span>
                                        )}
                                    </button>
                                </div>
                            ))}
                        </div>
                        <div className={styles.tools}>
                            <div className={styles.viewToggle}>
                                <button
                                    type="button"
                                    className={view === "list" ? styles.viewOn : ""}
                                    title={t("storage.view.list")}
                                    onClick={() => setView("list")}
                                >
                                    <ListIcon size={15} />
                                </button>
                                <button
                                    type="button"
                                    className={view === "grid" ? styles.viewOn : ""}
                                    title={t("storage.view.grid")}
                                    onClick={() => setView("grid")}
                                >
                                    <LayoutGrid size={15} />
                                </button>
                            </div>
                            <button
                                type="button"
                                className={styles.btnGhost}
                                onClick={() => setDialog({ kind: "group-create" })}
                            >
                                <FolderPlus size={14} /> {t("storage.newGroup")}
                            </button>
                            <button type="button" className={styles.btnPrimary} onClick={newHost}>
                                <Plus size={14} /> {t("storage.newHost")}
                            </button>
                        </div>
                    </div>
                </div>

                {/* list / grid + docked editor — only this area scrolls */}
                <div className={styles.listrow}>
                        <div className={styles.liststack}>
                            {filtered.length === 0 ? (
                                <div className={styles.empty}>
                                    <Search size={28} className={styles.emptyIcon} />
                                    <div>
                                        {t("storage.empty.title")}
                                        {q && <> {t("storage.empty.query", { query: search })}</>}.
                                    </div>
                                    <button
                                        type="button"
                                        className={styles.btnPrimary}
                                        style={{ marginTop: 16 }}
                                        onClick={newHost}
                                    >
                                        <Plus size={14} /> {t("storage.empty.createHost")}
                                    </button>
                                </div>
                            ) : view === "list" ? (
                                <div className={`${styles.hl} ${docked ? styles.hlNarrow : ""}`}>
                                    {filtered.map((h) => (
                                        <HostRow
                                            key={h.id}
                                            h={h}
                                            on={selectedHostId === h.id}
                                            connState={hostConnState.get(h.id) ?? null}
                                            editLabel={t("storage.edit")}
                                            infoLabel={t("storage.info")}
                                            dblTitle={t("host.doubleClickConnect")}
                                            onSelect={selectHost}
                                            onConnect={connect}
                                        />
                                    ))}
                                </div>
                            ) : (
                                <div className={styles.hg}>
                                    {filtered.map((h) => (
                                        <HostTile
                                            key={h.id}
                                            h={h}
                                            on={selectedHostId === h.id}
                                            groupLabel={groupName(h.group_id)}
                                            connectLabel={t("host.connect")}
                                            dblTitle={t("host.doubleClickConnect")}
                                            onSelect={selectHost}
                                            onConnect={connect}
                                        />
                                    ))}
                                </div>
                            )}
                        </div>
                        <div
                            className={styles.dock}
                            style={{ flexBasis: docked ? 412 : 0 }}
                        >
                            {docked && (
                                <div className={styles.dockInner}>
                                    <HostDetail />
                                </div>
                            )}
                        </div>
                    </div>
        </div>
    );
}

interface RowProps {
    h: HostDto;
    on: boolean;
    connState: "connecting" | "connected" | null;
    editLabel: string;
    infoLabel: string;
    dblTitle: string;
    onSelect: (id: string) => void;
    onConnect: (h: HostDto) => void;
}

function HostRow({ h, on, connState, editLabel, infoLabel, dblTitle, onSelect, onConnect }: RowProps) {
    const Ico = hostIcon(h);
    const gc =
        connState === "connected"
            ? "var(--color-success)"
            : connState === "connecting"
              ? "var(--color-warn)"
              : "transparent";
    return (
        <div
            className={`${styles.lrow} ${on ? styles.lrowOn : ""}`}
            style={{ "--gc": gc } as CSSProperties}
            onClick={() => onSelect(h.id)}
            onDoubleClick={() => onConnect(h)}
            title={dblTitle}
        >
            <div className={styles.lrowDot}>
                <Ico size={14} />
            </div>
            <div className={styles.lrowId}>
                <div className={styles.lrowName}>
                    <span className={styles.nm}>{h.display_name ?? h.name}</span>
                    <ProtocolBadge protocol={h.protocol} size="sm" />
                </div>
                <div className={styles.lrowUser}>{h.username || ""}</div>
            </div>
            <div className={styles.lrowAddr}>
                {h.hostname}
                <span className={styles.dim}>:{h.port}</span>
            </div>
            <div className={styles.lrowAct} onClick={(e) => e.stopPropagation()}>
                <button
                    type="button"
                    className={styles.minibtn}
                    title={infoLabel}
                    onClick={() => onSelect(h.id)}
                >
                    <Info size={14} />
                </button>
                <button
                    type="button"
                    className={styles.minibtn}
                    title={editLabel}
                    onClick={() => onSelect(h.id)}
                >
                    <Pencil size={14} />
                </button>
            </div>
        </div>
    );
}

interface TileProps {
    h: HostDto;
    on: boolean;
    groupLabel: string | null;
    connectLabel: string;
    dblTitle: string;
    onSelect: (id: string) => void;
    onConnect: (h: HostDto) => void;
}

function HostTile({ h, on, groupLabel, connectLabel, dblTitle, onSelect, onConnect }: TileProps) {
    const Ico = hostIcon(h);
    return (
        <div
            className={`${styles.tile} ${on ? styles.tileOn : ""}`}
            onClick={() => onSelect(h.id)}
            onDoubleClick={() => onConnect(h)}
            title={dblTitle}
        >
            <div className={styles.tileTop}>
                <span className={styles.tileIcon}>
                    <Ico size={15} />
                </span>
                <span className={styles.tileName}>{h.display_name ?? h.name}</span>
                <ProtocolBadge protocol={h.protocol} size="sm" />
            </div>
            <div className={styles.tileAddr}>
                {h.username ? `${h.username}@${h.hostname}` : h.hostname}
                <span className={styles.dim}>:{h.port}</span>
            </div>
            <div className={styles.tileFoot}>
                <span className={styles.tileGrp}>
                    {groupLabel && (
                        <>
                            <span className={styles.gdot} style={{ background: groupColor(h.group_id) }} />
                            {groupLabel}
                        </>
                    )}
                </span>
                <span className={styles.tileSp} />
                <button
                    type="button"
                    className={styles.tileGo}
                    title={connectLabel}
                    onClick={(e) => {
                        e.stopPropagation();
                        onConnect(h);
                    }}
                >
                    <Zap size={15} />
                </button>
            </div>
        </div>
    );
}
