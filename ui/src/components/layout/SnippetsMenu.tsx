import { useEffect, useRef, useState } from "react";
import { Code2, Pin, Search } from "lucide-react";

import { useT } from "../../i18n";
import { snippets } from "../../lib/ipc";
import type { Snippet } from "../../lib/types";
import { useSessionsStore, useUiStore } from "../../store";
import styles from "./SnippetsMenu.module.css";

/**
 * Top-bar quick access to snippets. Click a snippet to RUN it into the focused
 * terminal session (types the command + Enter); if no runnable session is
 * focused it copies instead. "Manage" opens Tools → Snippets.
 */
export function SnippetsMenu({
    focusedKey,
    canRun,
}: {
    focusedKey: string | null;
    canRun: boolean;
}) {
    const { t } = useT();
    const [open, setOpen] = useState(false);
    const [items, setItems] = useState<Snippet[] | null>(null);
    const [flash, setFlash] = useState<string | null>(null);
    const [q, setQ] = useState("");
    const [pos, setPos] = useState<{ top: number; right: number }>({ top: 0, right: 0 });
    const wrapRef = useRef<HTMLDivElement>(null);
    const triggerRef = useRef<HTMLButtonElement>(null);

    function toggle() {
        const next = !open;
        if (next) {
            setQ("");
            const r = triggerRef.current?.getBoundingClientRect();
            if (r) setPos({ top: r.bottom + 6, right: Math.max(8, window.innerWidth - r.right) });
        }
        setOpen(next);
    }

    useEffect(() => {
        if (!open) return;
        void snippets
            .list()
            .then(setItems)
            .catch(() => setItems([]));
        const onDoc = (e: MouseEvent) => {
            if (wrapRef.current && !wrapRef.current.contains(e.target as Node)) setOpen(false);
        };
        const onKey = (e: KeyboardEvent) => {
            if (e.key === "Escape") setOpen(false);
        };
        document.addEventListener("mousedown", onDoc);
        document.addEventListener("keydown", onKey);
        return () => {
            document.removeEventListener("mousedown", onDoc);
            document.removeEventListener("keydown", onKey);
        };
    }, [open]);

    function activate(s: Snippet) {
        if (canRun && focusedKey) {
            // Type the command and press Enter (\r matches a real keystroke).
            useSessionsStore
                .getState()
                .sendInput(focusedKey, new TextEncoder().encode(`${s.command}\r`));
            setOpen(false);
        } else {
            void navigator.clipboard.writeText(s.command);
            setFlash(s.id);
            window.setTimeout(() => setFlash((cur) => (cur === s.id ? null : cur)), 1100);
        }
    }

    function manage() {
        // ToolsView only renders when no session tab is active — deselect first.
        useSessionsStore.getState().setActiveTab(null);
        useUiStore.getState().setSection("tools");
        useUiStore.getState().setToolsSection("snippets");
        setOpen(false);
    }

    function pin() {
        useUiStore.getState().setSnippetsPinned(true);
        setOpen(false);
    }

    const filtered = items?.filter((s) => {
        const needle = q.trim().toLowerCase();
        if (!needle) return true;
        return (
            s.name.toLowerCase().includes(needle) || s.command.toLowerCase().includes(needle)
        );
    });

    return (
        <div className={styles.wrap} ref={wrapRef}>
            <button
                ref={triggerRef}
                type="button"
                className={styles.trigger}
                onClick={toggle}
                title={t("snippets.menu.title")}
                aria-label={t("snippets.menu.title")}
            >
                <Code2 size={15} />
            </button>

            {open && (
                <div className={styles.pop} style={{ top: pos.top, right: pos.right }}>
                    <div className={styles.popHead}>
                        <span>{t("tools.section.snippets")}</span>
                        <span className={styles.headRight}>
                            {!canRun && <span className={styles.hint}>{t("snippets.menu.copyHint")}</span>}
                            <button
                                type="button"
                                className={styles.pinBtn}
                                onClick={pin}
                                title={t("snippets.menu.pin")}
                                aria-label={t("snippets.menu.pin")}
                            >
                                <Pin size={13} />
                            </button>
                        </span>
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

                    <div className={styles.list}>
                        {items === null ? (
                            <div className={styles.msg}>{t("common.loading")}</div>
                        ) : filtered && filtered.length === 0 ? (
                            <div className={styles.msg}>
                                {items.length === 0 ? t("snippets.menu.empty") : t("snippets.menu.noMatch")}
                            </div>
                        ) : (
                            filtered?.map((s) => (
                                <button
                                    key={s.id}
                                    type="button"
                                    className={styles.row}
                                    onClick={() => activate(s)}
                                    title={s.command}
                                >
                                    <span className={styles.name}>{s.name}</span>
                                    <span className={styles.cmd}>
                                        {flash === s.id ? t("snippets.menu.copied") : s.command}
                                    </span>
                                </button>
                            ))
                        )}
                    </div>

                    <button type="button" className={styles.manage} onClick={manage}>
                        {t("snippets.menu.manage")}
                    </button>
                </div>
            )}
        </div>
    );
}
