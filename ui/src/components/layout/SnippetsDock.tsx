import { useCallback, useEffect, useMemo, useState } from "react";
import { Code2, Pencil, Plus, Search, Trash2, X } from "lucide-react";

import { useT } from "../../i18n";
import { snippets } from "../../lib/ipc";
import type { Snippet } from "../../lib/types";
import { useSessionsStore, useUiStore } from "../../store";
import styles from "./SnippetsDock.module.css";

/**
 * Snippets panel docked on the right of the session area (toggled via the pin
 * in the top-bar snippets menu). Same run/copy behaviour as the popover:
 * clicking a snippet runs it into the focused terminal, or copies if there's
 * no runnable session. Supports add/edit/delete inline; changes propagate to the
 * Tools tab (and vice-versa) via the shared `snippetsRev` signal.
 */
export function SnippetsDock() {
    const { t } = useT();
    const setSnippetsPinned = useUiStore((s) => s.setSnippetsPinned);
    const tabs = useSessionsStore((s) => s.tabs);
    const activeTabId = useSessionsStore((s) => s.activeTabId);
    const sessions = useSessionsStore((s) => s.sessions);
    const bump = useUiStore((s) => s.bumpSnippets);
    const rev = useUiStore((s) => s.snippetsRev);
    const syncAt = useUiStore((s) => s.syncStatus?.at_ms);

    const [items, setItems] = useState<Snippet[] | null>(null);
    const [q, setQ] = useState("");
    const [flash, setFlash] = useState<string | null>(null);
    const [edit, setEdit] = useState<{ id: string | null; name: string; command: string } | null>(null);
    const [busy, setBusy] = useState(false);

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
        // rev = local mutations (Tools/dock); syncAt = a completed sync pass.
    }, [load, rev, syncAt]);

    const activeTab = tabs.find((tb) => tb.id === activeTabId);
    const focusedKey = activeTab ? (activeTab.focusKey ?? activeTab.activePaneKey) : null;
    const focusedSession = focusedKey ? sessions.find((s) => s.key === focusedKey) : undefined;
    const canRun =
        !!focusedSession &&
        focusedSession.protocol !== "rdp" &&
        !focusedSession.sftp &&
        !focusedSession.notes &&
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

    async function save() {
        if (!edit) return;
        const name = edit.name.trim();
        if (!name || busy) return;
        setBusy(true);
        try {
            if (edit.id) await snippets.update(edit.id, name, edit.command);
            else await snippets.create(name, edit.command);
            setEdit(null);
            bump();
        } catch {
            /* surfaced elsewhere; keep the editor open */
        } finally {
            setBusy(false);
        }
    }

    async function del(id: string) {
        try {
            await snippets.delete(id);
            if (edit?.id === id) setEdit(null);
            bump();
        } catch {
            /* ignore */
        }
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
                        <div key={s.id} className={styles.row}>
                            <button
                                type="button"
                                className={styles.rowMain}
                                onClick={() => activate(s)}
                                title={s.command}
                            >
                                <span className={styles.name}>{s.name}</span>
                                <span className={styles.cmd}>
                                    {flash === s.id ? t("snippets.menu.copied") : s.command}
                                </span>
                            </button>
                            <div className={styles.rowActions}>
                                <button
                                    type="button"
                                    className={styles.rowBtn}
                                    title={t("tools.snip.edit")}
                                    onClick={() => setEdit({ id: s.id, name: s.name, command: s.command })}
                                >
                                    <Pencil size={13} />
                                </button>
                                <button
                                    type="button"
                                    className={`${styles.rowBtn} ${styles.del}`}
                                    title={t("tools.snip.delete")}
                                    onClick={() => void del(s.id)}
                                >
                                    <Trash2 size={13} />
                                </button>
                            </div>
                        </div>
                    ))
                )}
            </div>

            {edit ? (
                <div className={styles.editor}>
                    <input
                        className={styles.nameInput}
                        value={edit.name}
                        onChange={(e) => setEdit({ ...edit, name: e.target.value })}
                        placeholder={t("tools.snip.namePlaceholder")}
                        spellCheck={false}
                        autoFocus
                    />
                    <textarea
                        className={styles.cmdInput}
                        value={edit.command}
                        onChange={(e) => setEdit({ ...edit, command: e.target.value })}
                        placeholder={t("tools.snip.cmdPlaceholder")}
                        spellCheck={false}
                    />
                    <div className={styles.editorActions}>
                        <button type="button" className={styles.cancelBtn} onClick={() => setEdit(null)}>
                            {t("tools.snip.cancel")}
                        </button>
                        <button
                            type="button"
                            className={styles.saveBtn}
                            disabled={!edit.name.trim() || busy}
                            onClick={() => void save()}
                        >
                            {t("tools.snip.save")}
                        </button>
                    </div>
                </div>
            ) : (
                <button
                    type="button"
                    className={styles.newBtn}
                    onClick={() => setEdit({ id: null, name: "", command: "" })}
                >
                    <Plus size={14} />
                    {t("snippets.dock.new")}
                </button>
            )}
        </aside>
    );
}
