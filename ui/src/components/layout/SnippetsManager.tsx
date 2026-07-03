import { useCallback, useEffect, useState } from "react";
import { Code2, Copy, Pencil, Plus, Trash2 } from "lucide-react";

import { useT } from "../../i18n";
import { snippets } from "../../lib/ipc";
import { formatApiError, type Snippet } from "../../lib/types";
import styles from "./SnippetsManager.module.css";

type Draft = { id: string | null; name: string; command: string };

export function SnippetsPane({ onToast }: { onToast: (s: string) => void }) {
    const { t } = useT();
    const [items, setItems] = useState<Snippet[] | null>(null);
    const [editing, setEditing] = useState<Draft | null>(null);
    const [busy, setBusy] = useState(false);

    const load = useCallback(async () => {
        try {
            setItems(await snippets.list());
        } catch (e) {
            setItems([]);
            onToast(formatApiError(e));
        }
    }, [onToast]);

    useEffect(() => {
        void load();
    }, [load]);

    async function save() {
        if (!editing) return;
        const name = editing.name.trim();
        if (!name || busy) return;
        setBusy(true);
        try {
            if (editing.id) await snippets.update(editing.id, name, editing.command);
            else await snippets.create(name, editing.command);
            setEditing(null);
            await load();
            onToast(t("tools.snip.saved"));
        } catch (e) {
            onToast(formatApiError(e));
        } finally {
            setBusy(false);
        }
    }

    async function del(id: string) {
        try {
            await snippets.delete(id);
            await load();
            onToast(t("tools.snip.deleted"));
        } catch (e) {
            onToast(formatApiError(e));
        }
    }

    function copy(cmd: string) {
        void navigator.clipboard.writeText(cmd);
        onToast(t("tools.snip.copied"));
    }

    return (
        <div className={styles.pane}>
            <div className={styles.head}>
                <div className={styles.headRow}>
                    <Code2 size={17} className={styles.headIcon} />
                    <h2>{t("tools.section.snippets")}</h2>
                </div>
                <p className={styles.desc}>{t("tools.snip.desc")}</p>
            </div>

            <div className={styles.body}>
                <div className={styles.sectRow}>
                    <div className={styles.sect}>
                        {t("tools.snip.list")}
                        {items ? <span className={styles.count}> · {items.length}</span> : null}
                    </div>
                    {!editing && (
                        <button type="button" className={styles.addBtn} onClick={() => setEditing({ id: null, name: "", command: "" })}>
                            <Plus size={13} />
                            {t("tools.snip.add")}
                        </button>
                    )}
                </div>

                {editing && (
                    <div className={styles.editor}>
                        <input
                            className={styles.nameInput}
                            value={editing.name}
                            onChange={(e) => setEditing({ ...editing, name: e.target.value })}
                            placeholder={t("tools.snip.namePlaceholder")}
                            spellCheck={false}
                            autoFocus
                        />
                        <textarea
                            className={styles.cmdInput}
                            value={editing.command}
                            onChange={(e) => setEditing({ ...editing, command: e.target.value })}
                            placeholder={t("tools.snip.cmdPlaceholder")}
                            spellCheck={false}
                        />
                        <div className={styles.editorActions}>
                            <button type="button" className={styles.cancel} onClick={() => setEditing(null)}>
                                {t("tools.snip.cancel")}
                            </button>
                            <button
                                type="button"
                                className={styles.confirm}
                                disabled={!editing.name.trim() || busy}
                                onClick={() => void save()}
                            >
                                {t("tools.snip.save")}
                            </button>
                        </div>
                    </div>
                )}

                {items && items.length === 0 && !editing ? (
                    <div className={styles.empty}>{t("tools.snip.empty")}</div>
                ) : items && items.length > 0 ? (
                    <div className={styles.list}>
                        {items.map((s) => (
                            <div key={s.id} className={styles.row}>
                                <div className={styles.rowMain}>
                                    <span className={styles.name}>{s.name}</span>
                                    <span className={styles.cmd}>{s.command}</span>
                                </div>
                                <button type="button" className={styles.iconBtn} title={t("tools.snip.copy")} onClick={() => copy(s.command)}>
                                    <Copy size={14} />
                                </button>
                                <button type="button" className={styles.iconBtn} title={t("tools.snip.edit")} onClick={() => setEditing({ id: s.id, name: s.name, command: s.command })}>
                                    <Pencil size={14} />
                                </button>
                                <button type="button" className={`${styles.iconBtn} ${styles.del}`} title={t("tools.snip.delete")} onClick={() => void del(s.id)}>
                                    <Trash2 size={14} />
                                </button>
                            </div>
                        ))}
                    </div>
                ) : null}
            </div>
        </div>
    );
}
