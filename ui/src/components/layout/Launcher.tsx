import { useEffect, useMemo, useRef, useState } from "react";
import {
    ArrowRightLeft,
    FolderPlus,
    Keyboard,
    KeyRound,
    type LucideIcon,
    Plus,
    Search,
    Settings,
} from "lucide-react";

import { useT } from "../../i18n";
import type { HostDto } from "../../lib/types";
import { useGroupsStore, useHostsStore, useSessionsStore, useUiStore } from "../../store";
import { ProtocolBadge } from "../ui/ProtocolBadge";
import styles from "./Launcher.module.css";

interface PaletteAction {
    id: string;
    label: string;
    icon: LucideIcon;
    run: () => void;
}

/**
 * Command palette + quick-connect. Opened by Ctrl/Cmd+K, the tab-bar "+",
 * or a pane split.
 *
 * - Ctrl/Cmd+K or "+": shows **Actions** (new host, settings, …) and
 *   **Hosts** (connect), fuzzy-filtered, grouped.
 * - During a pane split (`splitTarget` set): hosts only — the chosen host
 *   drops into the new split pane.
 * SSH-only connect for now (RDP rows disabled until that path is wired
 * through here).
 */
export function Launcher() {
    const { t } = useT();
    const hosts = useHostsStore((s) => s.items);
    const groups = useGroupsStore((s) => s.items);
    const open = useSessionsStore((s) => s.open);
    const splitActivePane = useSessionsStore((s) => s.splitActivePane);
    const splitTarget = useSessionsStore((s) => s.splitTarget);
    const setLauncherOpen = useUiStore((s) => s.setLauncherOpen);
    const startDraft = useUiStore((s) => s.startDraft);
    const setSection = useUiStore((s) => s.setSection);
    const setToolsSection = useUiStore((s) => s.setToolsSection);
    const setDialog = useUiStore((s) => s.setDialog);
    const setShortcutsOpen = useUiStore((s) => s.setShortcutsOpen);

    // No pending split => full command palette (actions + hosts). A pending
    // split => host picker only (the host fills the new pane).
    const commandMode = !splitTarget;

    const [query, setQuery] = useState("");
    const [sel, setSel] = useState(0);
    const inputRef = useRef<HTMLInputElement>(null);
    const listRef = useRef<HTMLDivElement>(null);

    useEffect(() => {
        inputRef.current?.focus();
    }, []);

    const close = () => {
        useSessionsStore.setState({ splitTarget: null });
        setLauncherOpen(false);
    };

    const actions = useMemo<PaletteAction[]>(() => {
        if (!commandMode) return [];
        return [
            {
                id: "new-host",
                label: t("palette.newHost"),
                icon: Plus,
                run: () => {
                    setSection("vault");
                    startDraft();
                },
            },
            {
                id: "new-group",
                label: t("palette.newGroup"),
                icon: FolderPlus,
                run: () => setDialog({ kind: "group-create" }),
            },
            {
                id: "credentials",
                label: t("palette.credentials"),
                icon: KeyRound,
                run: () => setDialog({ kind: "credentials-list" }),
            },
            {
                id: "forwards",
                label: t("palette.forwards"),
                icon: ArrowRightLeft,
                run: () => {
                    setSection("tools");
                    setToolsSection("forwards");
                },
            },
            {
                id: "settings",
                label: t("palette.settings"),
                icon: Settings,
                run: () => setDialog({ kind: "settings" }),
            },
            {
                id: "shortcuts",
                label: t("palette.shortcuts"),
                icon: Keyboard,
                run: () => setShortcutsOpen(true),
            },
        ];
    }, [commandMode, t, setSection, startDraft, setDialog, setToolsSection, setShortcutsOpen]);

    const groupName = useMemo(() => {
        const m = new Map(groups.map((g) => [g.id, g.name]));
        return (id: HostDto["group_id"]) => (id ? (m.get(id) ?? null) : null);
    }, [groups]);

    const q = query.trim().toLowerCase();

    const filteredActions = useMemo(
        () => (q ? actions.filter((a) => a.label.toLowerCase().includes(q)) : actions),
        [actions, q],
    );

    const filteredHosts = useMemo(() => {
        const list = [...hosts].sort((a, b) =>
            (a.display_name ?? a.name).localeCompare(b.display_name ?? b.name),
        );
        if (!q) return list;
        return list.filter((h) =>
            [h.display_name ?? "", h.name, h.hostname, ...h.tags]
                .join(" ")
                .toLowerCase()
                .includes(q),
        );
    }, [hosts, q]);

    const total = filteredActions.length + filteredHosts.length;

    useEffect(() => {
        setSel(0);
    }, [query]);

    // Keep the highlighted row visible.
    useEffect(() => {
        listRef.current
            ?.querySelector(`[data-idx="${sel}"]`)
            ?.scrollIntoView({ block: "nearest" });
    }, [sel]);

    const connect = (h: HostDto) => {
        if (h.protocol !== "ssh") return;
        const splitDir = useSessionsStore.getState().splitTarget;
        if (splitDir) splitActivePane(h, splitDir);
        else void open(h);
        close();
    };
    const runAction = (a: PaletteAction) => {
        a.run();
        close();
    };
    const activate = (i: number) => {
        if (i < filteredActions.length) {
            const a = filteredActions[i];
            if (a) runAction(a);
        } else {
            const h = filteredHosts[i - filteredActions.length];
            if (h) connect(h);
        }
    };

    const onKeyDown = (e: React.KeyboardEvent) => {
        if (e.key === "Escape") {
            e.preventDefault();
            close();
        } else if (e.key === "ArrowDown") {
            e.preventDefault();
            setSel((s) => Math.min(s + 1, total - 1));
        } else if (e.key === "ArrowUp") {
            e.preventDefault();
            setSel((s) => Math.max(s - 1, 0));
        } else if (e.key === "Enter") {
            e.preventDefault();
            activate(sel);
        }
    };

    return (
        <div
            className={styles.backdrop}
            onMouseDown={(e) => {
                if (e.target === e.currentTarget) close();
            }}
        >
            <div className={styles.panel} role="dialog" aria-modal="true">
                <div className={styles.searchRow}>
                    <Search size={16} className={styles.searchIcon} />
                    <input
                        ref={inputRef}
                        className={styles.input}
                        value={query}
                        onChange={(e) => setQuery(e.target.value)}
                        onKeyDown={onKeyDown}
                        placeholder={t(commandMode ? "palette.placeholder" : "launcher.placeholder")}
                        spellCheck={false}
                    />
                    <kbd className={styles.kbd}>Esc</kbd>
                </div>

                <div className={styles.list} ref={listRef}>
                    {total === 0 ? (
                        <div className={styles.empty}>{t("launcher.empty")}</div>
                    ) : (
                        <>
                            {filteredActions.length > 0 && (
                                <div className={styles.section}>{t("palette.actions")}</div>
                            )}
                            {filteredActions.map((a, i) => {
                                const Icon = a.icon;
                                return (
                                    <button
                                        key={a.id}
                                        type="button"
                                        data-idx={i}
                                        className={`${styles.row} ${i === sel ? styles.selected : ""}`}
                                        onMouseMove={() => setSel(i)}
                                        onClick={() => runAction(a)}
                                    >
                                        <Icon size={16} className={styles.rowIcon} />
                                        <span className={styles.rowName}>{a.label}</span>
                                    </button>
                                );
                            })}
                            {commandMode && filteredHosts.length > 0 && (
                                <div className={styles.section}>{t("palette.hosts")}</div>
                            )}
                            {filteredHosts.map((h, j) => {
                                const i = filteredActions.length + j;
                                const g = groupName(h.group_id);
                                const isRdp = h.protocol !== "ssh";
                                return (
                                    <button
                                        key={h.id}
                                        type="button"
                                        data-idx={i}
                                        className={`${styles.row} ${i === sel ? styles.selected : ""}`}
                                        onMouseMove={() => setSel(i)}
                                        onClick={() => connect(h)}
                                        disabled={isRdp}
                                        title={isRdp ? t("host.connectRdpSoon") : undefined}
                                    >
                                        <ProtocolBadge protocol={h.protocol} size="sm" />
                                        <span className={styles.rowName}>
                                            {h.display_name ?? h.name}
                                        </span>
                                        {g && <span className={styles.rowGroup}>{g}</span>}
                                    </button>
                                );
                            })}
                        </>
                    )}
                </div>
            </div>
        </div>
    );
}
