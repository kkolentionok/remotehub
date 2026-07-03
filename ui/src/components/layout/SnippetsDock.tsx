import { useCallback, useEffect, useMemo, useState } from "react";
import { Code2, CornerDownLeft, Plus, Search, X } from "lucide-react";

import { useT } from "../../i18n";
import { snippets } from "../../lib/ipc";
import type { Snippet } from "../../lib/types";
import { useSessionsStore, useUiStore } from "../../store";
import styles from "./SnippetsDock.module.css";

/**
 * Snippets panel docked on the right of the session area (toggled via the pin
 * in the top-bar snippets menu). Same run/copy behaviour as the popover:
 * clicking a snippet runs it into the focused terminal, or copies if there's
 * no runnable session. Read/run only — CRUD lives in Tools → Snippets.
 */
export function SnippetsDock() {
    const { t } = useT();
    const setSnippetsPinned = useUiStore((s) => s.setSnippetsPinned);
    const tabs = useSessionsStore((s) => s.tabs);
    const activeTabId = useSessionsStore((s) => s.activeTabId);
    const sessions = useSessionsStore((s) => s.sessions);

    const [items, setItems] = useState<Snippet[] | null>(null);
    const [q, setQ] = useState("");
    const [flash, setFlash] = useState<string | null>(null);

    const load = useCallback(() => {
        void snippets
            .list()
            .then(setItems)
            .catch(() => setItems([]));
    }, []);

    useEffect(() => {
        load();
        // Reflect edits made in Tools → Snippets when the window regains focus.
        const onFocus = () => load();
        window.addEventListener("focus", onFocus);
        return () => window.removeEventListener("focus", onFocus);
    }, [load]);

    const activeTab = tabs.find((tb) => tb.id === activeTabId);
    const focusedKey = activeTab ? (activeTab.focusKey ?? activeTab.activePaneKey) : null;
    const focusedSession = focusedKey ? sessions.find((s) => s.key === focusedKey) : undefined;
    const canRun =
        !!focusedSession &&
        focusedSession.protocol !== "rdp" &&
        !focusedSession.sftp &&
        focusedSession.state === "ready";

    const filtered = useMemo(() => {
        const needle = q.trim().toLowerCase();
        return (items ?? []).filter(
            (s) =>
                !needle ||
                s.name.toLowerCase().includes(needle) ||
                s.command.toLowerCase().includes(needle),
        );
    }, [items, q]);

    function activate(s: Snippet) {
        if (canRun && focusedKey) {
            useSessionsStore
                .getState()
                .sendInput(focusedKey, new TextEncoder().encode(`${s.command}\r`));
        } else {
            void navigator.clipboard.writeText(s.command);
            setFlash(s.id);
            window.setTimeout(() => setFlash((c) => (c === s.id ? null : c)), 1100);
        }
    }

    function newSnippet() {
        useSessionsStore.getState().setActiveTab(null);
        useUiStore.getState().setSection("tools");
        useUiStore.getState().setToolsSection("snippets");
    }

    return (
        <aside className={styles.dock}>
            <div className={styles.head}>
                <Code2 size={14} className={styles.headIcon} />
                <span className={styles.title}>{t("tools.section.snippets")}</span>
                {items && <span className={styles.count}>· {items.length}</span>}
                <span className={styles.spacer} />
                <button
                    type="button"
                    className={styles.iconBtn}
                    onClick={() => setSnippetsPinned(false)}
                    title={t("snippets.dock.unpin")}
                    aria-label={t("snippets.dock.unpin")}
                >
                    <X size={15} />
                </button>
            </div>

            <div className={styles.filterRow}>
                <Search size={13} className={styles.filterIcon} />
                <input
                    className={styles.filter}
                    value={q}
                    onChange={(e) => setQ(e.target.value)}
                    placeholder={t("snippets.menu.filter")}
                    spellCheck={false}
                />
            </div>

            {!canRun && <div className={styles.hintBar}>{t("snippets.menu.copyHint")}</div>}

            <div className={styles.list}>
                {items === null ? (
                    <div className={styles.msg}>{t("common.loading")}</div>
                ) : filtered.length === 0 ? (
                    <div className={styles.msg}>
                        {items.length === 0 ? t("snippets.menu.empty") : t("snippets.menu.noMatch")}
                    </div>
                ) : (
                    filtered.map((s) => (
                        <button
                            key={s.id}
                            type="button"
                            className={styles.row}
                            onClick={() => activate(s)}
                            title={s.command}
                        >
                            <div className={styles.rowMain}>
                                <span className={styles.name}>{s.name}</span>
                                <span className={styles.cmd}>
                                    {flash === s.id ? t("snippets.menu.copied") : s.command}
                                </span>
                            </div>
                            {canRun && <CornerDownLeft size={14} className={styles.runIcon} />}
                        </button>
                    ))
                )}
            </div>

            <button type="button" className={styles.newBtn} onClick={newSnippet}>
                <Plus size={14} />
                {t("snippets.dock.new")}
            </button>
        </aside>
    );
}
