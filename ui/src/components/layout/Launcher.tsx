import { useEffect, useMemo, useRef, useState } from "react";
import { Search } from "lucide-react";

import { useT } from "../../i18n";
import type { HostDto } from "../../lib/types";
import { useGroupsStore, useHostsStore, useSessionsStore, useUiStore } from "../../store";
import { ProtocolBadge } from "../ui/ProtocolBadge";
import styles from "./Launcher.module.css";

/**
 * Quick-connect launcher (tab-bar "+"). A command-palette overlay:
 * search hosts, arrow/Enter or click to open a session. SSH only for
 * now — RDP rows are disabled until Stage 4.
 */
export function Launcher() {
    const { t } = useT();
    const hosts = useHostsStore((s) => s.items);
    const groups = useGroupsStore((s) => s.items);
    const open = useSessionsStore((s) => s.open);
    const setLauncherOpen = useUiStore((s) => s.setLauncherOpen);

    const [query, setQuery] = useState("");
    const [sel, setSel] = useState(0);
    const inputRef = useRef<HTMLInputElement>(null);
    const listRef = useRef<HTMLDivElement>(null);

    useEffect(() => {
        inputRef.current?.focus();
    }, []);

    const groupName = useMemo(() => {
        const m = new Map(groups.map((g) => [g.id, g.name]));
        return (id: HostDto["group_id"]) => (id ? (m.get(id) ?? null) : null);
    }, [groups]);

    const filtered = useMemo(() => {
        const q = query.trim().toLowerCase();
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
    }, [hosts, query]);

    useEffect(() => {
        setSel(0);
    }, [query]);

    // Keep the highlighted row visible.
    useEffect(() => {
        listRef.current
            ?.querySelector(`[data-idx="${sel}"]`)
            ?.scrollIntoView({ block: "nearest" });
    }, [sel]);

    const close = () => setLauncherOpen(false);
    const connect = (h: HostDto) => {
        if (h.protocol !== "ssh") return;
        void open(h);
        close();
    };

    const onKeyDown = (e: React.KeyboardEvent) => {
        if (e.key === "Escape") {
            e.preventDefault();
            close();
        } else if (e.key === "ArrowDown") {
            e.preventDefault();
            setSel((s) => Math.min(s + 1, filtered.length - 1));
        } else if (e.key === "ArrowUp") {
            e.preventDefault();
            setSel((s) => Math.max(s - 1, 0));
        } else if (e.key === "Enter") {
            e.preventDefault();
            const h = filtered[sel];
            if (h) connect(h);
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
                        placeholder={t("launcher.placeholder")}
                        spellCheck={false}
                    />
                    <kbd className={styles.kbd}>Esc</kbd>
                </div>

                <div className={styles.list} ref={listRef}>
                    {filtered.length === 0 ? (
                        <div className={styles.empty}>{t("launcher.empty")}</div>
                    ) : (
                        filtered.map((h, i) => {
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
                        })
                    )}
                </div>
            </div>
        </div>
    );
}
