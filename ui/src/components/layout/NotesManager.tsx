import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
    Bold,
    Check,
    Copy,
    Italic,
    List,
    ListOrdered,
    Loader2,
    NotebookPen,
    Paperclip,
    Pin,
    Plus,
    Search,
    Strikethrough,
    Trash2,
    Underline,
    X,
} from "lucide-react";

import { useT } from "../../i18n";
import { notes as notesApi } from "../../lib/ipc";
import { formatApiError, type Note } from "../../lib/types";
import { useDebouncedCallback } from "../../lib/useDebouncedCallback";
import styles from "./NotesManager.module.css";

type SaveState = "idle" | "pending" | "saving" | "saved";
type BucketKey = "pinned" | "today" | "yesterday" | "older";

const BUCKETS: { key: BucketKey; label: string }[] = [
    { key: "pinned", label: "tools.notes.group.pinned" },
    { key: "today", label: "tools.notes.group.today" },
    { key: "yesterday", label: "tools.notes.group.yesterday" },
    { key: "older", label: "tools.notes.group.older" },
];

/** Which date group an unpinned note falls into. */
function bucketOf(iso: string): Exclude<BucketKey, "pinned"> {
    const d = new Date(iso);
    if (Number.isNaN(d.getTime())) return "older";
    const startOfToday = new Date();
    startOfToday.setHours(0, 0, 0, 0);
    if (d.getTime() >= startOfToday.getTime()) return "today";
    if (d.getTime() >= startOfToday.getTime() - 86_400_000) return "yesterday";
    return "older";
}

/** Time for today's notes, short date for anything older. */
function stamp(iso: string): string {
    const d = new Date(iso);
    if (Number.isNaN(d.getTime())) return "";
    return bucketOf(iso) === "today"
        ? d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" })
        : d.toLocaleDateString(undefined, { day: "2-digit", month: "2-digit", year: "2-digit" });
}

function preview(note: Note): string {
    const skipTitle = !note.title.trim();
    const lines = note.body.split("\n").filter((l) => l.trim());
    return (skipTitle ? lines.slice(1) : lines)[0]?.trim().slice(0, 46) ?? "";
}

/** Placeholder for rich text — the space is claimed now so the header doesn't
 *  need re-laying out when formatting actually lands. */
function FormatBar({ hint }: { hint: string }) {
    return (
        <div className={styles.fmt}>
            {[Bold, Italic, Underline, Strikethrough].map((Ico, i) => (
                <button key={i} type="button" className={styles.iconBtn} disabled title={hint}>
                    <Ico size={14} />
                </button>
            ))}
            <span className={styles.fmtSep} />
            {[List, ListOrdered, Paperclip].map((Ico, i) => (
                <button key={i} type="button" className={styles.iconBtn} disabled title={hint}>
                    <Ico size={14} />
                </button>
            ))}
        </div>
    );
}

export function NotesPane() {
    const { t } = useT();
    const [items, setItems] = useState<Note[] | null>(null);
    const [selectedId, setSelectedId] = useState<string | null>(null);
    const [query, setQuery] = useState("");
    const [title, setTitle] = useState("");
    const [body, setBody] = useState("");
    const [save, setSave] = useState<SaveState>("idle");
    const [toasts, setToasts] = useState<{ id: number; text: string }[]>([]);
    // True between a keystroke and its save landing: the sync refetch must not
    // overwrite what the user is currently typing.
    const dirty = useRef(false);
    const savedTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

    const toast = useCallback((text: string) => {
        const id = Date.now() + Math.random();
        setToasts((arr) => [...arr, { id, text }]);
        window.setTimeout(() => setToasts((arr) => arr.filter((x) => x.id !== id)), 2200);
    }, []);

    const load = useCallback(async (): Promise<Note[]> => {
        try {
            const list = await notesApi.list();
            setItems(list);
            return list;
        } catch (e) {
            setItems([]);
            toast(formatApiError(e));
            return [];
        }
    }, [toast]);

    // Tight sync cadence only while this screen is mounted.
    useEffect(() => {
        void notesApi.setFastSync(true).catch(() => undefined);
        return () => {
            void notesApi.setFastSync(false).catch(() => undefined);
        };
    }, []);

    useEffect(() => {
        void load();
    }, [load]);

    // Poll the list so edits pulled from another device show up. Cheap: one
    // local SQLite read. The open editor is left alone while it's dirty.
    useEffect(() => {
        const iv = window.setInterval(() => {
            void (async () => {
                const list = await load();
                if (dirty.current || !selectedId) return;
                const fresh = list.find((n) => n.id === selectedId);
                if (!fresh) return;
                setTitle((cur) => (cur === fresh.title ? cur : fresh.title));
                setBody((cur) => (cur === fresh.body ? cur : fresh.body));
            })();
        }, 2000);
        return () => window.clearInterval(iv);
    }, [load, selectedId]);

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
                toast(formatApiError(e));
            }
        },
        [load, toast],
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
            await load();
            dirty.current = false;
            setSelectedId(id);
            setTitle("");
            setBody("");
            setQuery("");
            setSave("idle");
        } catch (e) {
            toast(formatApiError(e));
        }
    }

    async function togglePin(note: Note) {
        try {
            await notesApi.setPinned(note.id, !note.pinned);
            await load();
        } catch (e) {
            toast(formatApiError(e));
        }
    }

    async function remove(id: string) {
        debouncedSave.cancel();
        try {
            await notesApi.delete(id);
            await load();
            if (selectedId === id) {
                setSelectedId(null);
                setTitle("");
                setBody("");
            }
            toast(t("tools.notes.deleted"));
        } catch (e) {
            toast(formatApiError(e));
        }
    }

    const filtered = useMemo(() => {
        const needle = query.trim().toLowerCase();
        if (!needle) return items ?? [];
        return (items ?? []).filter((n) =>
            `${n.title} ${n.body}`.toLowerCase().includes(needle),
        );
    }, [items, query]);

    const groups = useMemo(
        () =>
            BUCKETS.map((b) => ({
                ...b,
                items: filtered.filter((n) =>
                    b.key === "pinned" ? n.pinned : !n.pinned && bucketOf(n.updated_at) === b.key,
                ),
            })).filter((g) => g.items.length > 0),
        [filtered],
    );

    const selected = useMemo(
        () => items?.find((n) => n.id === selectedId) ?? null,
        [items, selectedId],
    );

    return (
        <div className={styles.pane}>
            {/* ── list ── */}
            <div className={styles.list}>
                <div className={styles.listHead}>
                    <span className={styles.listTitle}>
                        <NotebookPen size={15} />
                        {t("tools.section.notes")}
                    </span>
                    <span className={styles.count}>{items?.length ?? 0}</span>
                </div>

                <button type="button" className={styles.newBtn} onClick={() => void create()}>
                    <Plus size={14} />
                    {t("tools.notes.new")}
                </button>

                <div className={styles.search}>
                    <Search size={13} />
                    <input
                        value={query}
                        onChange={(e) => setQuery(e.target.value)}
                        placeholder={t("tools.notes.search")}
                        spellCheck={false}
                    />
                    {query && (
                        <button type="button" className={styles.clear} onClick={() => setQuery("")}>
                            <X size={12} />
                        </button>
                    )}
                </div>

                <div className={styles.scroll}>
                    {groups.length === 0 && (
                        <div className={styles.listEmpty}>
                            {query ? (
                                <>
                                    <Search size={22} />
                                    <b>{t("tools.notes.noMatch")}</b>
                                    <span>{t("tools.notes.noMatchBody", { q: query })}</span>
                                </>
                            ) : (
                                <>
                                    <NotebookPen size={22} />
                                    <b>{t("tools.notes.empty")}</b>
                                    <span>{t("tools.notes.emptyBody")}</span>
                                </>
                            )}
                        </div>
                    )}

                    {groups.map((g) => (
                        <div key={g.key}>
                            <div className={styles.groupHead}>
                                {t(g.label as Parameters<typeof t>[0])}
                            </div>
                            {g.items.map((n) => (
                                <button
                                    type="button"
                                    key={n.id}
                                    className={`${styles.row} ${n.id === selectedId ? styles.rowOn : ""}`}
                                    onClick={() => open(n)}
                                >
                                    <span className={styles.rowTop}>
                                        <span
                                            className={`${styles.rowTitle} ${n.title.trim() ? "" : styles.rowUntitled}`}
                                        >
                                            {n.title.trim() || t("tools.notes.untitled")}
                                        </span>
                                        {n.pinned && (
                                            <span className={styles.rowPin}>
                                                <Pin size={11} />
                                            </span>
                                        )}
                                    </span>
                                    <span className={styles.rowSub}>
                                        <span className={styles.rowTime}>{stamp(n.updated_at)}</span>
                                        <span className={styles.rowPrev}>{preview(n)}</span>
                                    </span>
                                </button>
                            ))}
                        </div>
                    ))}
                </div>
            </div>

            {/* ── editor ── */}
            <div className={styles.editor}>
                {selected ? (
                    <>
                        <div className={styles.editorHead}>
                            <FormatBar hint={t("tools.notes.fmtSoon")} />
                            <span className={styles.spacer} />
                            <span className={styles.save}>
                                {save === "saving" && (
                                    <>
                                        <Loader2 size={12} className={styles.spin} />
                                        {t("tools.notes.savingShort")}
                                    </>
                                )}
                                {save === "saved" && (
                                    <>
                                        <Check size={12} className={styles.ok} />
                                        {t("tools.notes.savedShort")}
                                    </>
                                )}
                                {save === "pending" && <span className={styles.dot} />}
                            </span>
                            <button
                                type="button"
                                className={`${styles.iconBtn} ${selected.pinned ? styles.iconOn : ""}`}
                                title={selected.pinned ? t("tools.notes.unpin") : t("tools.notes.pin")}
                                onClick={() => void togglePin(selected)}
                            >
                                <Pin size={14} />
                            </button>
                            <button
                                type="button"
                                className={styles.iconBtn}
                                title={t("tools.notes.copy")}
                                onClick={() => {
                                    void navigator.clipboard.writeText(body);
                                    toast(t("tools.notes.copied"));
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

                        <div className={styles.editorBody}>
                            <input
                                className={styles.titleInput}
                                value={title}
                                placeholder={t("tools.notes.titlePh")}
                                onChange={(e) => edit(e.target.value, body)}
                            />
                            <textarea
                                className={styles.body}
                                value={body}
                                placeholder={t("tools.notes.bodyPh")}
                                spellCheck={false}
                                onChange={(e) => edit(title, e.target.value)}
                            />
                        </div>
                    </>
                ) : (
                    <div className={styles.blank}>
                        <NotebookPen size={30} />
                        <p>
                            {items && items.length === 0
                                ? t("tools.notes.emptyBody")
                                : t("tools.notes.pickOne")}
                        </p>
                    </div>
                )}
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
