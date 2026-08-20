import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Check, Copy, Loader2, NotebookPen, Plus, Trash2 } from "lucide-react";

import { useT } from "../../i18n";
import { notes as notesApi } from "../../lib/ipc";
import { formatApiError, type Note } from "../../lib/types";
import { useUiStore } from "../../store";
import { useDebouncedCallback } from "../../lib/useDebouncedCallback";
import styles from "./NotesManager.module.css";

type SaveState = "idle" | "pending" | "saving" | "saved";

/** Label for the sidebar row: explicit title, else the first non-empty line. */
function label(note: Note, fallback: string): string {
    if (note.title.trim()) return note.title.trim();
    const first = note.body.split("\n").find((l) => l.trim());
    return first ? first.trim().slice(0, 80) : fallback;
}

function preview(note: Note): string {
    const lines = note.body.split("\n").filter((l) => l.trim());
    const skip = note.title.trim() ? 0 : 1;
    return lines.slice(skip, skip + 1).join(" ").slice(0, 90);
}

function stamp(iso: string): string {
    const d = new Date(iso);
    if (Number.isNaN(d.getTime())) return "";
    const today = new Date();
    const sameDay =
        d.getDate() === today.getDate() &&
        d.getMonth() === today.getMonth() &&
        d.getFullYear() === today.getFullYear();
    return sameDay
        ? d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" })
        : d.toLocaleDateString(undefined, { day: "2-digit", month: "2-digit", year: "2-digit" });
}

export function NotesPane() {
    const { t } = useT();
    // The pane owns a tab, so it carries its own transient toasts rather than
    // borrowing the Tools screen's.
    const [toasts, setToasts] = useState<{ id: number; text: string }[]>([]);
    const onToast = useCallback((text: string) => {
        const id = Date.now() + Math.random();
        setToasts((arr) => [...arr, { id, text }]);
        window.setTimeout(() => setToasts((arr) => arr.filter((x) => x.id !== id)), 2200);
    }, []);
    const syncAt = useUiStore((st) => st.syncStatus?.at_ms);
    const [items, setItems] = useState<Note[] | null>(null);
    const [selectedId, setSelectedId] = useState<string | null>(null);
    const [title, setTitle] = useState("");
    const [body, setBody] = useState("");
    const [save, setSave] = useState<SaveState>("idle");
    // Ignore the refetch that our own save triggers, and never clobber the
    // textarea while the user is mid-edit.
    const dirty = useRef(false);
    const savedTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

    const load = useCallback(
        async (keep = true) => {
            try {
                const list = await notesApi.list();
                setItems(list);
                return list;
            } catch (e) {
                if (keep) onToast(formatApiError(e));
                setItems([]);
                return [];
            }
        },
        [onToast],
    );

    // Tell the backend to run the tight sync cadence while this screen is up.
    useEffect(() => {
        void notesApi.setFastSync(true).catch(() => undefined);
        return () => {
            void notesApi.setFastSync(false).catch(() => undefined);
        };
    }, []);

    useEffect(() => {
        void load();
    }, [load]);

    // A completed sync pass may have pulled remote edits. Refetch, but leave
    // the open editor alone while the user is typing (our own text wins until
    // the debounce fires; the remote copy lands on the next pass).
    useEffect(() => {
        if (syncAt === undefined) return;
        void (async () => {
            const list = await load();
            if (dirty.current || !selectedId) return;
            const fresh = list.find((n) => n.id === selectedId);
            if (!fresh) return;
            setTitle((cur) => (cur === fresh.title ? cur : fresh.title));
            setBody((cur) => (cur === fresh.body ? cur : fresh.body));
        })();
    }, [syncAt, load, selectedId]);

    const persist = useCallback(
        async (id: string, nextTitle: string, nextBody: string) => {
            setSave("saving");
            try {
                await notesApi.update(id, nextTitle, nextBody);
                dirty.current = false;
                setSave("saved");
                if (savedTimer.current) clearTimeout(savedTimer.current);
                savedTimer.current = setTimeout(() => setSave("idle"), 1500);
                await load();
            } catch (e) {
                setSave("idle");
                onToast(formatApiError(e));
            }
        },
        [load, onToast],
    );

    const debouncedSave = useDebouncedCallback(
        (id: string, nextTitle: string, nextBody: string) => {
            void persist(id, nextTitle, nextBody);
        },
        400,
    );

    function edit(nextTitle: string, nextBody: string) {
        setTitle(nextTitle);
        setBody(nextBody);
        if (!selectedId) return;
        dirty.current = true;
        setSave("pending");
        debouncedSave.call(selectedId, nextTitle, nextBody);
    }

    function open(note: Note) {
        if (note.id === selectedId) return;
        debouncedSave.flush();
        dirty.current = false;
        setSelectedId(note.id);
        setTitle(note.title);
        setBody(note.body);
        setSave("idle");
    }

    async function create() {
        debouncedSave.flush();
        try {
            const id = await notesApi.create("", "");
            const list = await load();
            const fresh = list.find((n) => n.id === id);
            dirty.current = false;
            setSelectedId(id);
            setTitle(fresh?.title ?? "");
            setBody(fresh?.body ?? "");
            setSave("idle");
        } catch (e) {
            onToast(formatApiError(e));
        }
    }

    async function remove(id: string) {
        debouncedSave.cancel();
        try {
            await notesApi.delete(id);
            const list = await load();
            if (selectedId === id) {
                setSelectedId(null);
                setTitle("");
                setBody("");
            }
            void list;
            onToast(t("tools.notes.deleted"));
        } catch (e) {
            onToast(formatApiError(e));
        }
    }

    const selected = useMemo(
        () => items?.find((n) => n.id === selectedId) ?? null,
        [items, selectedId],
    );

    return (
        <div className={styles.pane}>
            <div className={styles.head}>
                <div className={styles.headRow}>
                    <NotebookPen size={17} className={styles.headIcon} />
                    <h2>{t("tools.section.notes")}</h2>
                </div>
                <p className={styles.desc}>{t("tools.notes.desc")}</p>
            </div>

            <div className={styles.split}>
                <div className={styles.list}>
                    <button type="button" className={styles.newBtn} onClick={() => void create()}>
                        <Plus size={14} />
                        {t("tools.notes.new")}
                    </button>

                    {items && items.length === 0 && (
                        <div className={styles.listEmpty}>{t("tools.notes.empty")}</div>
                    )}

                    {items?.map((n) => (
                        <button
                            type="button"
                            key={n.id}
                            className={n.id === selectedId ? `${styles.row} ${styles.rowOn}` : styles.row}
                            onClick={() => open(n)}
                        >
                            <span className={styles.rowTitle}>
                                {label(n, t("tools.notes.untitled"))}
                            </span>
                            <span className={styles.rowMeta}>
                                <span className={styles.rowDate}>{stamp(n.updated_at)}</span>
                                {preview(n) && <span className={styles.rowPrev}>{preview(n)}</span>}
                            </span>
                        </button>
                    ))}
                </div>

                <div className={styles.editor}>
                    {selected ? (
                        <>
                            <div className={styles.editorBar}>
                                <input
                                    className={styles.titleInput}
                                    value={title}
                                    placeholder={t("tools.notes.titlePh")}
                                    onChange={(e) => edit(e.target.value, body)}
                                />
                                <span className={styles.status}>
                                    {save === "saving" && <Loader2 size={13} className={styles.spin} />}
                                    {save === "saved" && <Check size={13} className={styles.ok} />}
                                    {save === "pending" && <span className={styles.dot} />}
                                </span>
                                <button
                                    type="button"
                                    className={styles.iconBtn}
                                    title={t("tools.notes.copy")}
                                    onClick={() => {
                                        void navigator.clipboard.writeText(body);
                                        onToast(t("tools.notes.copied"));
                                    }}
                                >
                                    <Copy size={14} />
                                </button>
                                <button
                                    type="button"
                                    className={`${styles.iconBtn} ${styles.danger}`}
                                    title={t("tools.notes.delete")}
                                    onClick={() => void remove(selected.id)}
                                >
                                    <Trash2 size={14} />
                                </button>
                            </div>
                            <textarea
                                className={styles.body}
                                value={body}
                                placeholder={t("tools.notes.bodyPh")}
                                spellCheck={false}
                                onChange={(e) => edit(title, e.target.value)}
                            />
                        </>
                    ) : (
                        <div className={styles.blank}>
                            <NotebookPen size={30} />
                            <p>{items && items.length === 0 ? t("tools.notes.emptyBody") : t("tools.notes.pickOne")}</p>
                        </div>
                    )}
                </div>
            </div>

            <div className={styles.toasts}>
                {toasts.map((x) => (
                    <div key={x.id} className={styles.toast}>
                        {x.text}
                    </div>
                ))}
            </div>
        </div>
    );
}
