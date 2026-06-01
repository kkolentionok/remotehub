import { useEffect, useMemo, useReducer, useRef, useState } from "react";
import {
    AlertTriangle,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    Check,
    ChevronDown,
    ChevronRight,
    Copy,
    CornerLeftUp,
    Eye,
    EyeOff,
    File as FileIcon,
    FileArchive,
    FileCode,
    FileText,
    Folder,
    FolderOpen,
    FolderPlus,
    Home,
    Lock,
    Monitor,
    Pencil,
    RotateCw,
    Search,
    Server,
    Trash2,
    X,
} from "lucide-react";

import { useT } from "../../i18n";
import { localFs, sftp } from "../../lib/ipc";
import { formatApiError } from "../../lib/types";
import type { FsEntry, FsListResponse, HostDto } from "../../lib/types";
import { useHostsStore } from "../../store";
import type { SessionTab } from "../../store";
import styles from "./SftpView.module.css";

/* ── formatting ── */
function fmtSize(b: number, locale: string): string {
    if (b <= 0) return "—";
    const ru = locale === "ru";
    const units = ru ? ["Б", "КБ", "МБ", "ГБ", "ТБ"] : ["B", "KB", "MB", "GB", "TB"];
    if (b < 1024) return `${b} ${units[0]}`;
    let v = b;
    let i = 0;
    while (v >= 1024 && i < units.length - 1) {
        v /= 1024;
        i += 1;
    }
    const s = v.toFixed(i >= 2 ? 2 : 1);
    return `${ru ? s.replace(".", ",") : s} ${units[i]}`;
}

function fmtDate(epoch: number | null, locale: string): string {
    if (!epoch) return "";
    const d = new Date(epoch * 1000);
    const now = new Date();
    if (d.toDateString() === now.toDateString()) {
        return d.toLocaleTimeString(locale, { hour: "2-digit", minute: "2-digit" });
    }
    const sameYear = d.getFullYear() === now.getFullYear();
    return d.toLocaleDateString(
        locale,
        sameYear
            ? { day: "numeric", month: "short" }
            : { day: "numeric", month: "short", year: "2-digit" },
    );
}

function FileGlyph({ name, isDir }: { name: string; isDir: boolean }) {
    if (isDir) return <Folder size={15} />;
    const e = name.split(".").pop()?.toLowerCase() ?? "";
    if (["gz", "zip", "tar", "dump", "exe", "7z", "rar"].includes(e)) return <FileArchive size={15} />;
    if (["sql", "yml", "yaml", "sh", "json", "conf", "html", "js", "ts", "pub", "toml"].includes(e))
        return <FileCode size={15} />;
    if (["txt", "md", "log", "pdf", "docx"].includes(e)) return <FileText size={15} />;
    return <FileIcon size={15} />;
}

function crumbs(path: string, isLocal: boolean): { label: string; full: string }[] {
    if (!path) return [];
    const sep = isLocal ? "\\" : "/";
    const parts = path.split(sep);
    const out: { label: string; full: string }[] = [];
    let acc = "";
    parts.forEach((p, i) => {
        if (i === 0) {
            if (p === "") {
                out.push({ label: "/", full: "/" });
                acc = "";
            } else {
                out.push({ label: p, full: p });
                acc = p;
            }
        } else if (p !== "") {
            acc = acc + sep + p;
            out.push({ label: p, full: acc });
        }
    });
    return out;
}

/* ── per-panel state ── */
type Source = { kind: "local" } | { kind: "host"; host: HostDto };

interface Panel {
    source: Source;
    sessionId: string | null;
    listing: FsListResponse | null;
    busy: boolean;
    err: string | null;
    showHidden: boolean;
    sort: { key: "name" | "size"; dir: "asc" | "desc" };
    sel: Set<string>;
    filter: string;
    filterOpen: boolean;
    renaming: { path: string; value: string } | null;
    creatingFolder: string | null;
    isLocal: boolean;
    selectLocal: () => Promise<void>;
    selectHost: (h: HostDto) => Promise<void>;
    openDir: (path: string) => Promise<void>;
    navigateTo: (path: string) => Promise<string | null>;
    navHome: () => Promise<void>;
    navComputer: () => Promise<void>;
    refresh: () => Promise<void>;
    toggleHidden: () => void;
    setSort: (key: "name" | "size") => void;
    selectRow: (path: string, additive: boolean) => void;
    clearSel: () => void;
    toggleFilter: () => void;
    setFilter: (v: string) => void;
    beginRename: (path: string, name: string) => void;
    setRenameValue: (v: string) => void;
    cancelRename: () => void;
    commitRename: () => Promise<void>;
    remove: (entries: { path: string; is_dir: boolean }[]) => Promise<void>;
    beginCreateFolder: () => void;
    setFolderValue: (v: string) => void;
    cancelCreateFolder: () => void;
    commitCreateFolder: () => Promise<void>;
    chmod: (path: string, mode: number) => Promise<void>;
}

function usePanel(): Panel {
    const [source, setSource] = useState<Source>({ kind: "local" });
    const [sessionId, setSessionId] = useState<string | null>(null);
    const [listing, setListing] = useState<FsListResponse | null>(null);
    const [busy, setBusy] = useState(false);
    const [err, setErr] = useState<string | null>(null);
    const [showHidden, setShowHidden] = useState(false);
    const [sort, setSortState] = useState<{ key: "name" | "size"; dir: "asc" | "desc" }>({
        key: "name",
        dir: "asc",
    });
    const [sel, setSel] = useState<Set<string>>(new Set());
    const [filter, setFilter] = useState("");
    const [filterOpen, setFilterOpen] = useState(false);
    const [renaming, setRenaming] = useState<{ path: string; value: string } | null>(null);
    const [creatingFolder, setCreatingFolder] = useState<string | null>(null);

    useEffect(() => {
        return () => {
            if (sessionId) void sftp.close(sessionId);
        };
    }, [sessionId]);

    const loadLocal = async (path: string | null) => {
        setBusy(true);
        setSel(new Set());
        setFilter("");
        setCreatingFolder(null);
        try {
            const r = path ? await localFs.list(path) : await localFs.home();
            setListing(r);
            setErr(null);
        } catch (e: unknown) {
            setErr(formatApiError(e));
        } finally {
            setBusy(false);
        }
    };

    useEffect(() => {
        void loadLocal(null);
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, []);

    const selectLocal = async () => {
        setSource({ kind: "local" });
        setSessionId(null);
        setListing(null);
        setShowHidden(false);
        await loadLocal(null);
    };

    const selectHost = async (host: HostDto) => {
        setSource({ kind: "host", host });
        setSessionId(null);
        setListing(null);
        setShowHidden(true);
        setSel(new Set());
        setErr(null);
        setBusy(true);
        try {
            const res = await sftp.open(host.id);
            setSessionId(res.session_id);
            const r = await sftp.list(res.session_id, ".");
            setListing(r);
        } catch (e: unknown) {
            setErr(formatApiError(e));
        } finally {
            setBusy(false);
        }
    };

    const openDir = async (path: string) => {
        setSel(new Set());
        setFilter("");
        setCreatingFolder(null);
        if (source.kind === "local") {
            await loadLocal(path);
            return;
        }
        if (!sessionId) return;
        setBusy(true);
        try {
            const r = await sftp.list(sessionId, path);
            setListing(r);
            setErr(null);
        } catch (e: unknown) {
            setErr(formatApiError(e));
        } finally {
            setBusy(false);
        }
    };

    // Navigate to a typed path. Returns an error message on failure WITHOUT
    // clobbering the current listing; returns null on success.
    const navigateTo = async (path: string): Promise<string | null> => {
        setBusy(true);
        try {
            const r =
                source.kind === "local"
                    ? await localFs.list(path)
                    : sessionId
                      ? await sftp.list(sessionId, path)
                      : null;
            if (r) {
                setListing(r);
                setSel(new Set());
                setFilter("");
                setCreatingFolder(null);
                setErr(null);
            }
            return null;
        } catch (e: unknown) {
            return formatApiError(e);
        } finally {
            setBusy(false);
        }
    };

    const navHome = async () => {
        if (source.kind === "local") {
            await loadLocal(null);
        } else if (sessionId) {
            await openDir(".");
        }
    };

    const navComputer = async () => {
        setSel(new Set());
        setFilter("");
        setBusy(true);
        try {
            const r = await localFs.drives();
            setListing(r);
            setErr(null);
        } catch (e: unknown) {
            setErr(formatApiError(e));
        } finally {
            setBusy(false);
        }
    };

    const refresh = async () => {
        if (listing) await openDir(listing.path);
    };

    const selectRow = (path: string, additive: boolean) => {
        setSel((prev) => {
            const next = new Set(additive ? prev : []);
            if (additive && prev.has(path)) next.delete(path);
            else next.add(path);
            return next;
        });
    };

    const commitRename = async () => {
        if (!renaming) return;
        const { path, value } = renaming;
        const name = value.trim();
        if (!name) {
            setRenaming(null);
            return;
        }
        if (source.kind === "local") await localFs.rename(path, name);
        else if (sessionId) await sftp.rename(sessionId, path, name);
        setRenaming(null);
        await refresh();
    };

    const remove = async (items: { path: string; is_dir: boolean }[]) => {
        for (const it of items) {
            if (source.kind === "local") await localFs.remove(it.path, it.is_dir);
            else if (sessionId) await sftp.remove(sessionId, it.path, it.is_dir);
        }
        setSel(new Set());
        await refresh();
    };

    const commitCreateFolder = async () => {
        const name = (creatingFolder ?? "").trim();
        const dir = listing?.path ?? "";
        if (!name || (source.kind === "host" && !sessionId)) {
            setCreatingFolder(null);
            return;
        }
        if (source.kind === "local") await localFs.mkdir(dir, name);
        else if (sessionId) await sftp.mkdir(sessionId, dir, name);
        setCreatingFolder(null);
        await refresh();
    };

    return {
        source,
        sessionId,
        listing,
        busy,
        err,
        showHidden,
        sort,
        sel,
        filter,
        filterOpen,
        renaming,
        creatingFolder,
        isLocal: source.kind === "local",
        selectLocal,
        selectHost,
        openDir,
        navigateTo,
        navHome,
        navComputer,
        refresh,
        toggleHidden: () => setShowHidden((h) => !h),
        setSort: (key) =>
            setSortState((s) => (s.key === key ? { key, dir: s.dir === "asc" ? "desc" : "asc" } : { key, dir: "asc" })),
        selectRow,
        clearSel: () => setSel(new Set()),
        toggleFilter: () => setFilterOpen((o) => { if (o) setFilter(""); return !o; }),
        setFilter,
        beginRename: (path, name) => setRenaming({ path, value: name }),
        setRenameValue: (v) => setRenaming((r) => (r ? { ...r, value: v } : r)),
        cancelRename: () => setRenaming(null),
        commitRename,
        remove,
        beginCreateFolder: () => setCreatingFolder(""),
        setFolderValue: (v) => setCreatingFolder(v),
        cancelCreateFolder: () => setCreatingFolder(null),
        commitCreateFolder,
        chmod: async (path, mode) => {
            if (sessionId) await sftp.chmod(sessionId, path, mode);
            await refresh();
        },
    };
}

/* ── endpoint switcher ── */
function EndpointPicker({ panel, hosts }: { panel: Panel; hosts: HostDto[] }) {
    const { t } = useT();
    const [open, setOpen] = useState(false);
    const src = panel.source;
    const isLocal = src.kind === "local";
    const name = src.kind === "host" ? src.host.display_name ?? src.host.name : t("sftp.local");
    return (
        <div className={styles.epick} onClick={() => setOpen((o) => !o)}>
            <span className={`${styles.epickIc} ${isLocal ? "" : styles.epickIcHost}`}>
                {isLocal ? <Monitor size={15} /> : <Server size={15} />}
            </span>
            <span className={styles.epickNm}>{name}</span>
            {!isLocal && <span className={`${styles.sdot} ${styles.sdotOnline}`} />}
            <span className={styles.epickChev}>
                <ChevronDown size={13} />
            </span>
            {open && (
                <>
                    <div className={styles.menuBackdrop} onClick={(e) => { e.stopPropagation(); setOpen(false); }} />
                    <div className={styles.emenu} onClick={(e) => e.stopPropagation()}>
                        <div className={styles.emenuH}>{t("sftp.thisMachine")}</div>
                        <button
                            type="button"
                            className={`${styles.emenuItem} ${isLocal ? styles.emenuItemOn : ""}`}
                            onClick={() => { setOpen(false); void panel.selectLocal(); }}
                        >
                            <Monitor size={16} />
                            <span className={styles.emenuNm}>{t("sftp.local")}</span>
                            {isLocal && <Check size={15} />}
                        </button>
                        <div className={styles.emenuH}>{t("sftp.hosts")}</div>
                        {hosts.length === 0 && <div className={styles.emenuAd} style={{ padding: "6px 9px" }}>{t("sftp.noHosts")}</div>}
                        {hosts.map((h) => {
                            const on = panel.source.kind === "host" && panel.source.host.id === h.id;
                            return (
                                <button
                                    key={h.id}
                                    type="button"
                                    className={`${styles.emenuItem} ${on ? styles.emenuItemOn : ""}`}
                                    onClick={() => { setOpen(false); void panel.selectHost(h); }}
                                >
                                    <Server size={16} />
                                    <span className={styles.emenuNm}>{h.display_name ?? h.name}</span>
                                    <span className={styles.emenuAd}>{h.hostname}</span>
                                    {on ? <Check size={15} /> : <span className={`${styles.sdot} ${styles.sdotOnline}`} />}
                                </button>
                            );
                        })}
                    </div>
                </>
            )}
        </div>
    );
}

/* ── one pane ── */
function Pane({
    panel,
    hosts,
    active,
    onActivate,
    onTransferFiles,
    onNotify,
    onRequestDelete,
    onDragStartFiles,
    onDropToPane,
    onRequestChmod,
}: {
    panel: Panel;
    hosts: HostDto[];
    active: boolean;
    onActivate: () => void;
    onTransferFiles: (entries: FsEntry[]) => void;
    onNotify: (s: Status) => void;
    onRequestDelete: (panel: Panel, entries: { path: string; is_dir: boolean }[]) => void;
    onDragStartFiles: (files: FsEntry[]) => void;
    onDropToPane: () => void;
    onRequestChmod: (panel: Panel, entry: FsEntry) => void;
}) {
    const { t, locale } = useT();
    const { listing, sort, showHidden, sel, isLocal } = panel;
    const [dropping, setDropping] = useState(false);
    const [menu, setMenu] = useState<{ x: number; y: number; entry: FsEntry } | null>(null);
    const [editingPath, setEditingPath] = useState(false);
    const [pathDraft, setPathDraft] = useState("");
    const [pathErr, setPathErr] = useState(false);

    const entries = useMemo(() => {
        if (!listing) return [];
        let arr = listing.entries;
        if (!showHidden) arr = arr.filter((e) => !e.name.startsWith("."));
        const f = panel.filter.trim().toLowerCase();
        if (f) arr = arr.filter((e) => e.name.toLowerCase().includes(f));
        const sorted = [...arr].sort((a, b) => {
            if (a.is_dir !== b.is_dir) return a.is_dir ? -1 : 1;
            let r = 0;
            if (sort.key === "name") r = a.name.localeCompare(b.name, locale);
            else r = a.size - b.size;
            return sort.dir === "asc" ? r : -r;
        });
        return sorted;
    }, [listing, showHidden, sort, locale, panel.filter]);

    const selEntries = entries.filter((e) => sel.has(e.path));
    const selFiles = selEntries.filter((e) => !e.is_dir);
    const selSize = selFiles.reduce((s, e) => s + e.size, 0);
    const grid = isLocal ? "1fr 92px 104px" : "1fr 84px 96px 104px";
    const SortArrow = sort.dir === "asc" ? ArrowUp : ArrowDown;
    const pathCrumbs = crumbs(listing?.path ?? "", isLocal);
    const atComputer = isLocal && (listing?.path ?? "") === "";
    const canUp = !!listing && (!!listing.parent || (isLocal && !atComputer));
    const onUp = () => {
        if (listing?.parent) void panel.openDir(listing.parent);
        else if (isLocal && !atComputer) void panel.navComputer();
    };

    const renameSelected = () => {
        if (sel.size !== 1) return;
        const only = entries.find((e) => sel.has(e.path));
        if (only) panel.beginRename(only.path, only.name);
    };
    const deleteSelected = () => {
        if (selEntries.length === 0) return;
        onRequestDelete(
            panel,
            selEntries.map((e) => ({ path: e.path, is_dir: e.is_dir })),
        );
    };

    // Files carried by a drag from a row: the whole selection if the dragged
    // row is part of it, otherwise just that row.
    const dragFilesFor = (e: FsEntry): FsEntry[] => {
        const sf = sel.has(e.path) ? selFiles : e.is_dir ? [] : [e];
        return sf;
    };

    const closeMenu = () => setMenu(null);
    const menuEntry = menu?.entry;
    const copyPath = (p: string) => void navigator.clipboard?.writeText(p);

    const beginEditPath = () => {
        if (atComputer) return;
        setPathDraft(listing?.path ?? "");
        setPathErr(false);
        setEditingPath(true);
    };
    const commitPath = async () => {
        const err = await panel.navigateTo(pathDraft.trim());
        if (err) setPathErr(true);
        else setEditingPath(false);
    };

    return (
        <div
            className={`${styles.pane} ${active ? styles.paneActive : ""} ${dropping ? styles.paneDrop : ""}`}
            onMouseDown={onActivate}
            onDragOver={(e) => {
                e.preventDefault();
                if (!dropping) setDropping(true);
            }}
            onDragLeave={(e) => {
                if (!e.currentTarget.contains(e.relatedTarget as Node)) setDropping(false);
            }}
            onDrop={(e) => {
                e.preventDefault();
                setDropping(false);
                onDropToPane();
            }}
        >
            <div className={styles.phead}>
                <EndpointPicker panel={panel} hosts={hosts} />
                <span className={styles.pheadSp} />
                <button
                    className={`${styles.pheadBtn} ${panel.filterOpen ? styles.pheadBtnOn : ""}`}
                    title={t("sftp.search")}
                    onClick={panel.toggleFilter}
                >
                    <Search size={15} />
                </button>
                <button
                    className={styles.pheadBtn}
                    title={t("sftp.rename")}
                    disabled={sel.size !== 1}
                    onClick={renameSelected}
                >
                    <Pencil size={15} />
                </button>
                <button
                    className={`${styles.pheadBtn} ${styles.pheadBtnDanger}`}
                    title={t("sftp.delete")}
                    disabled={selEntries.length === 0}
                    onClick={deleteSelected}
                >
                    <Trash2 size={15} />
                </button>
                <button className={styles.pheadBtn} title={t("sftp.hidden")} onClick={panel.toggleHidden}>
                    {showHidden ? <Eye size={16} /> : <EyeOff size={16} />}
                </button>
                <button className={styles.pheadBtn} title={t("sftp.refresh")} onClick={() => void panel.refresh()}>
                    <RotateCw size={15} className={panel.busy ? styles.spin : ""} />
                </button>
            </div>

            {panel.filterOpen && (
                <div className={styles.filterRow}>
                    <Search size={14} className={styles.filterIc} />
                    <input
                        className={styles.filterInput}
                        autoFocus
                        value={panel.filter}
                        placeholder={t("sftp.searchPlaceholder")}
                        onChange={(e) => panel.setFilter(e.target.value)}
                        onKeyDown={(e) => e.key === "Escape" && panel.toggleFilter()}
                    />
                    <button className={styles.filterClear} title={t("sftp.clear")} onClick={panel.toggleFilter}>
                        <X size={14} />
                    </button>
                </div>
            )}

            <div className={styles.pbar}>
                <button className={styles.pbarBtn} title={t("sftp.home")} onClick={() => void panel.navHome()}>
                    <Home size={15} />
                </button>
                <button
                    className={styles.pbarBtn}
                    title={t("sftp.up")}
                    disabled={!canUp}
                    onClick={onUp}
                >
                    <CornerLeftUp size={16} />
                </button>
                {editingPath ? (
                    <input
                        className={`${styles.pathInput} ${pathErr ? styles.pathInputErr : ""}`}
                        autoFocus
                        value={pathDraft}
                        spellCheck={false}
                        onChange={(e) => {
                            setPathDraft(e.target.value);
                            setPathErr(false);
                        }}
                        onKeyDown={(e) => {
                            if (e.key === "Enter") void commitPath();
                            else if (e.key === "Escape") setEditingPath(false);
                        }}
                        onBlur={() => setEditingPath(false)}
                    />
                ) : (
                    <div className={styles.crumbs} onClick={beginEditPath} title={t("sftp.editPath")}>
                        {isLocal && (
                            <button
                                className={`${styles.crumb} ${atComputer ? styles.crumbLast : ""}`}
                                title={t("sftp.thisMachine")}
                                onClick={(e) => {
                                    e.stopPropagation();
                                    void panel.navComputer();
                                }}
                            >
                                <Monitor size={13} />
                            </button>
                        )}
                        {pathCrumbs.map((c, i) => (
                            <div key={c.full + i} style={{ display: "flex", alignItems: "center" }}>
                                {(isLocal || i > 0) && (
                                    <span className={styles.crumbSep}>
                                        <ChevronRight size={12} />
                                    </span>
                                )}
                                <button
                                    className={`${styles.crumb} ${i === pathCrumbs.length - 1 ? styles.crumbLast : ""}`}
                                    onClick={(e) => {
                                        e.stopPropagation();
                                        void panel.openDir(c.full);
                                    }}
                                >
                                    {c.label}
                                </button>
                            </div>
                        ))}
                    </div>
                )}
                <button
                    className={styles.pbarBtn}
                    title={t("sftp.newFolder")}
                    disabled={atComputer || !listing}
                    onClick={() => panel.beginCreateFolder()}
                >
                    <FolderPlus size={15} />
                </button>
            </div>

            <div className={styles.cols} style={{ gridTemplateColumns: grid }}>
                <div
                    className={`${styles.col} ${sort.key === "name" ? styles.colSorted : ""}`}
                    onClick={() => panel.setSort("name")}
                >
                    {t("sftp.colName")} <span className={styles.colSort}><SortArrow size={11} /></span>
                </div>
                <div
                    className={`${styles.col} ${styles.colNum} ${sort.key === "size" ? styles.colSorted : ""}`}
                    onClick={() => panel.setSort("size")}
                >
                    {t("sftp.colSize")} <span className={styles.colSort}><SortArrow size={11} /></span>
                </div>
                <div className={`${styles.col} ${styles.colNum}`}>{t("sftp.colModified")}</div>
                {!isLocal && <div className={`${styles.col} ${styles.colNum}`}>{t("sftp.colPerms")}</div>}
            </div>

            <div className={styles.flist}>
                {panel.err ? (
                    <div className={styles.empty}>{panel.err}</div>
                ) : panel.busy && !listing ? (
                    <div className={styles.empty}>
                        {isLocal ? t("sftp.loading") : t("sftp.connecting")}
                    </div>
                ) : (
                    <>
                        {panel.creatingFolder !== null && (
                            <div className={styles.frowf} style={{ gridTemplateColumns: grid }}>
                                <div className={styles.fname}>
                                    <span className={`${styles.fnameIc} ${styles.fnameIcDir}`}>
                                        <Folder size={15} />
                                    </span>
                                    <input
                                        className={styles.renameInput}
                                        autoFocus
                                        value={panel.creatingFolder}
                                        placeholder={t("sftp.newFolderName")}
                                        onChange={(ev) => panel.setFolderValue(ev.target.value)}
                                        onBlur={() => panel.cancelCreateFolder()}
                                        onKeyDown={(ev) => {
                                            if (ev.key === "Enter")
                                                void panel
                                                    .commitCreateFolder()
                                                    .catch((err: unknown) =>
                                                        onNotify({ kind: "error", text: formatApiError(err) }),
                                                    );
                                            else if (ev.key === "Escape") panel.cancelCreateFolder();
                                        }}
                                    />
                                </div>
                                <div /><div />
                                {!isLocal && <div />}
                            </div>
                        )}
                        {listing?.parent && (
                            <div
                                className={`${styles.frowf} ${styles.frowfUp}`}
                                style={{ gridTemplateColumns: grid }}
                                onDoubleClick={() => listing.parent && void panel.openDir(listing.parent)}
                            >
                                <div className={styles.fname}>
                                    <span className={styles.fnameIc}><CornerLeftUp size={15} /></span>
                                    <span className={styles.fnameT}>..</span>
                                </div>
                                <div /><div />
                                {!isLocal && <div />}
                            </div>
                        )}
                        {entries.map((e) => (
                            <div
                                key={e.path}
                                className={`${styles.frowf} ${sel.has(e.path) ? styles.frowfSel : ""}`}
                                style={{ gridTemplateColumns: grid }}
                                draggable={panel.renaming?.path !== e.path}
                                onDragStart={(ev) => {
                                    const files = dragFilesFor(e);
                                    if (files.length === 0) {
                                        ev.preventDefault();
                                        return;
                                    }
                                    ev.dataTransfer.effectAllowed = "copy";
                                    ev.dataTransfer.setData("text/plain", files.map((f) => f.name).join(", "));
                                    onDragStartFiles(files);
                                }}
                                onClick={(ev) => panel.selectRow(e.path, ev.ctrlKey || ev.metaKey)}
                                onDoubleClick={() => (e.is_dir ? void panel.openDir(e.path) : onTransferFiles([e]))}
                                onContextMenu={(ev) => {
                                    ev.preventDefault();
                                    if (!sel.has(e.path)) panel.selectRow(e.path, false);
                                    setMenu({ x: ev.clientX, y: ev.clientY, entry: e });
                                }}
                            >
                                <div className={styles.fname}>
                                    <span className={`${styles.fnameIc} ${e.is_dir ? styles.fnameIcDir : ""}`}>
                                        <FileGlyph name={e.name} isDir={e.is_dir} />
                                    </span>
                                    {panel.renaming?.path === e.path ? (
                                        <input
                                            className={styles.renameInput}
                                            autoFocus
                                            value={panel.renaming.value}
                                            onClick={(ev) => ev.stopPropagation()}
                                            onChange={(ev) => panel.setRenameValue(ev.target.value)}
                                            onBlur={() => panel.cancelRename()}
                                            onKeyDown={(ev) => {
                                                if (ev.key === "Enter")
                                                    void panel
                                                        .commitRename()
                                                        .catch((err: unknown) =>
                                                            onNotify({ kind: "error", text: formatApiError(err) }),
                                                        );
                                                else if (ev.key === "Escape") panel.cancelRename();
                                            }}
                                        />
                                    ) : (
                                        <span className={styles.fnameT}>{e.name}</span>
                                    )}
                                </div>
                                <div className={styles.fsize}>{e.is_dir ? "—" : fmtSize(e.size, locale)}</div>
                                <div className={styles.fdate}>{fmtDate(e.modified, locale)}</div>
                                {!isLocal && <div className={styles.fperm}>{e.perms ?? ""}</div>}
                            </div>
                        ))}
                    </>
                )}
            </div>

            <div className={styles.pstat}>
                {sel.size > 0 ? (
                    <>
                        <span className={styles.pstatSel}>{t("sftp.selected", { n: String(sel.size) })}</span>
                        {selSize > 0 && <span>· {fmtSize(selSize, locale)}</span>}
                    </>
                ) : (
                    <span>{t("sftp.items", { n: String(entries.length) })}</span>
                )}
                <span className={styles.pstatSp} />
                <span className={styles.mono}>
                    {panel.source.kind === "host"
                        ? `${panel.source.host.username || "?"}@${panel.source.host.hostname}`
                        : listing?.path ?? ""}
                </span>
            </div>

            {dropping && (
                <div className={styles.dropPlate}>
                    <div className={styles.dropPlateCard}>
                        <ArrowDown size={16} /> {t("sftp.dropInto", { name: panel.source.kind === "local" ? t("sftp.local") : panel.source.host.display_name ?? panel.source.host.name })}
                    </div>
                </div>
            )}

            {menu && menuEntry && (
                <>
                    <div className={styles.menuBackdrop} onClick={closeMenu} onContextMenu={(e) => { e.preventDefault(); closeMenu(); }} />
                    <div
                        className={styles.ctx}
                        style={{ left: Math.min(menu.x, window.innerWidth - 224), top: Math.min(menu.y, window.innerHeight - 260) }}
                    >
                        <button
                            className={styles.ctxItem}
                            onClick={() => { closeMenu(); onTransferFiles(selFiles.length > 0 ? selFiles : (menuEntry.is_dir ? [] : [menuEntry])); }}
                        >
                            <ArrowRight size={15} /> <span>{t("sftp.ctxTransfer")}</span><span className={styles.ctxKbd}>F5</span>
                        </button>
                        {menuEntry.is_dir && (
                            <button className={styles.ctxItem} onClick={() => { closeMenu(); void panel.openDir(menuEntry.path); }}>
                                <FolderOpen size={15} /> <span>{t("sftp.ctxOpen")}</span>
                            </button>
                        )}
                        <div className={styles.ctxSep} />
                        <button
                            className={styles.ctxItem}
                            onClick={() => { closeMenu(); panel.beginRename(menuEntry.path, menuEntry.name); }}
                        >
                            <Pencil size={15} /> <span>{t("sftp.rename")}</span><span className={styles.ctxKbd}>F2</span>
                        </button>
                        <button className={styles.ctxItem} onClick={() => { closeMenu(); copyPath(menuEntry.path); }}>
                            <Copy size={15} /> <span>{t("sftp.ctxCopyPath")}</span>
                        </button>
                        {!isLocal && (
                            <button className={styles.ctxItem} onClick={() => { closeMenu(); onRequestChmod(panel, menuEntry); }}>
                                <Lock size={15} /> <span>{t("sftp.ctxPerms")}</span>
                            </button>
                        )}
                        <div className={styles.ctxSep} />
                        <button
                            className={`${styles.ctxItem} ${styles.ctxItemDanger}`}
                            onClick={() => { closeMenu(); onRequestDelete(panel, (selEntries.length > 0 ? selEntries : [menuEntry]).map((e) => ({ path: e.path, is_dir: e.is_dir }))); }}
                        >
                            <Trash2 size={15} /> <span>{t("sftp.delete")}</span><span className={styles.ctxKbd}>Del</span>
                        </button>
                    </div>
                </>
            )}
        </div>
    );
}

function permsToBits(perms: string | null): boolean[] {
    const p = (perms ?? "").slice(-9).padEnd(9, "-");
    return p.split("").map((c) => c !== "-");
}
function bitsToMode(bits: boolean[]): number {
    let mode = 0;
    bits.forEach((b, i) => {
        if (b) mode |= 1 << (8 - i);
    });
    return mode;
}

function ChmodDialog({
    entry,
    onApply,
    onClose,
}: {
    entry: FsEntry;
    onApply: (mode: number) => void;
    onClose: () => void;
}) {
    const { t } = useT();
    const [bits, setBits] = useState(() => permsToBits(entry.perms));
    const mode = bitsToMode(bits);
    const rows = [t("sftp.permsOwner"), t("sftp.permsGroup"), t("sftp.permsOther")];
    const cols = ["r", "w", "x"];
    return (
        <div className={styles.scrim} onClick={onClose}>
            <div className={styles.confirm} onClick={(e) => e.stopPropagation()}>
                <div className={styles.confirmTitle}>{t("sftp.permsTitle")}</div>
                <div className={styles.permsName}>{entry.name}</div>
                <div className={styles.permsGrid}>
                    <div />
                    {cols.map((c) => (
                        <div key={c} className={styles.permsCol}>{c}</div>
                    ))}
                    {rows.map((label, r) => (
                        <div key={label} style={{ display: "contents" }}>
                            <div className={styles.permsRow}>{label}</div>
                            {cols.map((_, c) => {
                                const i = r * 3 + c;
                                return (
                                    <button
                                        key={c}
                                        type="button"
                                        className={`${styles.permsCell} ${bits[i] ? styles.permsCellOn : ""}`}
                                        onClick={() => setBits((b) => b.map((v, idx) => (idx === i ? !v : v)))}
                                    >
                                        {bits[i] ? cols[c] : "–"}
                                    </button>
                                );
                            })}
                        </div>
                    ))}
                </div>
                <div className={styles.permsOctal}>
                    {t("sftp.permsMode")}: <span className={styles.mono}>{mode.toString(8).padStart(3, "0")}</span>
                </div>
                <div className={styles.confirmActions}>
                    <button className={styles.btnGhost} onClick={onClose}>{t("common.cancel")}</button>
                    <button className={styles.btnDanger} style={{ background: "var(--color-accent)" }} onClick={() => onApply(mode)}>
                        {t("sftp.permsApply")}
                    </button>
                </div>
            </div>
        </div>
    );
}

function uniqueName(name: string, taken: Set<string>): string {
    const dot = name.lastIndexOf(".");
    const base = dot > 0 ? name.slice(0, dot) : name;
    const ext = dot > 0 ? name.slice(dot) : "";
    let n = 1;
    let cand = `${base} (${n})${ext}`;
    while (taken.has(cand)) {
        n += 1;
        cand = `${base} (${n})${ext}`;
    }
    return cand;
}

type Status = { kind: "idle" | "busy" | "done" | "error"; text: string };

/* ── transfer queue ── */
type TKind = "up" | "down";
type TState = "queued" | "active" | "done" | "error" | "cancelled";
interface TItem {
    id: string;
    name: string;
    dir: TKind;
    routeFrom: string;
    routeTo: string;
    total: number;
    transferred: number;
    state: TState;
    error?: string;
    startedAt: number;
    speed: number;
    onComplete: () => void;
    req: {
        transfer_id: string;
        kind: "download" | "upload" | "copy";
        session_id: string;
        to_session?: string;
        src_path: string;
        dst_dir: string;
        dst_name?: string;
        resume?: boolean;
    };
}

function useTransfers() {
    const ref = useRef<TItem[]>([]);
    const [, force] = useReducer((x: number) => x + 1, 0);
    const cancelledIds = useRef<Set<string>>(new Set());

    const patch = (id: string, p: Partial<TItem>) => {
        ref.current = ref.current.map((it) => (it.id === id ? { ...it, ...p } : it));
        force();
    };

    function pump() {
        let slots = 2 - ref.current.filter((i) => i.state === "active").length;
        for (const it of ref.current) {
            if (slots <= 0) break;
            if (it.state === "queued") {
                slots -= 1;
                start(it);
            }
        }
    }

    function start(item: TItem) {
        patch(item.id, { state: "active", startedAt: Date.now(), transferred: 0 });
        void sftp
            .transfer(item.req, (bytes) => {
                const cur = ref.current.find((i) => i.id === item.id);
                const elapsed = cur ? (Date.now() - cur.startedAt) / 1000 : 0;
                patch(item.id, { transferred: bytes, speed: elapsed > 0 ? bytes / elapsed : 0 });
            })
            .then(() => {
                const cur = ref.current.find((i) => i.id === item.id);
                patch(item.id, { state: "done", transferred: cur?.total ?? 0 });
                item.onComplete();
            })
            .catch((e: unknown) => {
                if (cancelledIds.current.has(item.id)) patch(item.id, { state: "cancelled" });
                else patch(item.id, { state: "error", error: formatApiError(e) });
            })
            .finally(() => pump());
    }

    const enqueue = (items: TItem[]) => {
        ref.current = [...ref.current, ...items];
        force();
        pump();
    };

    const cancel = (id: string) => {
        const it = ref.current.find((i) => i.id === id);
        if (!it) return;
        if (it.state === "queued") {
            patch(id, { state: "cancelled" });
            return;
        }
        cancelledIds.current.add(id);
        void sftp.transferCancel(it.req.transfer_id);
    };

    const retry = (id: string) => {
        const it = ref.current.find((i) => i.id === id);
        if (!it || (it.state !== "error" && it.state !== "cancelled")) return;
        cancelledIds.current.delete(id);
        const newTid = `${Date.now()}-${Math.random().toString(36).slice(2, 9)}`;
        patch(id, {
            state: "queued",
            transferred: 0,
            speed: 0,
            error: undefined,
            startedAt: 0,
            req: { ...it.req, transfer_id: newTid, resume: true },
        });
        pump();
    };

    const retryAll = () => {
        for (const it of ref.current) {
            if (it.state === "error" || it.state === "cancelled") retry(it.id);
        }
    };

    const clearDone = () => {
        ref.current = ref.current.filter((i) => i.state === "active" || i.state === "queued");
        force();
    };

    const anyActive = ref.current.some((i) => i.state === "active");
    useEffect(() => {
        if (!anyActive) return;
        const id = window.setInterval(force, 1000);
        return () => window.clearInterval(id);
    }, [anyActive]);

    return { items: ref.current, enqueue, cancel, retry, retryAll, clearDone };
}

function fmtSpeed(bps: number, locale: string): string {
    if (bps <= 0) return "";
    return `${fmtSize(bps, locale)}/${locale === "ru" ? "с" : "s"}`;
}
function fmtEta(sec: number): string {
    if (!isFinite(sec) || sec <= 0) return "";
    const m = Math.floor(sec / 60);
    const s = Math.floor(sec % 60);
    return `${m}:${s.toString().padStart(2, "0")}`;
}

function TransferQueue({
    items,
    onCancel,
    onRetry,
    onRetryAll,
    onClear,
}: {
    items: TItem[];
    onCancel: (id: string) => void;
    onRetry: (id: string) => void;
    onRetryAll: () => void;
    onClear: () => void;
}) {
    const { t, locale } = useT();
    const [collapsed, setCollapsed] = useState(false);
    const active = items.filter((i) => i.state === "active");
    const queued = items.filter((i) => i.state === "queued");
    const totalSpeed = active.reduce((s, i) => s + i.speed, 0);
    const hasDone = items.some((i) => i.state === "done" || i.state === "error" || i.state === "cancelled");
    const hasFailed = items.some((i) => i.state === "error" || i.state === "cancelled");

    return (
        <div className={styles.queue}>
            <div className={styles.queueHead} onClick={() => setCollapsed((c) => !c)}>
                <span className={styles.queueTitle}>
                    <ArrowDown size={15} />
                    {t("sftp.transfers")}
                </span>
                <span className={styles.queueBadge}>
                    {t("sftp.qActive", { n: String(active.length) })}
                    {queued.length > 0 && ` · ${t("sftp.qQueued", { n: String(queued.length) })}`}
                </span>
                <span className={styles.queueSp} />
                {totalSpeed > 0 && <span className={styles.queueSpeed}>{fmtSpeed(totalSpeed, locale)}</span>}
                {hasFailed && (
                    <button
                        className={styles.queueRetry}
                        onClick={(e) => {
                            e.stopPropagation();
                            onRetryAll();
                        }}
                    >
                        {t("sftp.qRetryAll")}
                    </button>
                )}
                {hasDone && (
                    <button
                        className={styles.queueClear}
                        onClick={(e) => {
                            e.stopPropagation();
                            onClear();
                        }}
                    >
                        {t("sftp.qClear")}
                    </button>
                )}
                <span className={styles.queueChev} style={{ transform: collapsed ? "rotate(-90deg)" : "none" }}>
                    <ChevronDown size={16} />
                </span>
            </div>
            {!collapsed && (
                <div className={styles.queueBody}>
                    {items.length === 0 ? (
                        <div className={styles.queueEmpty}>{t("sftp.qEmpty")}</div>
                    ) : (
                        items.map((it) => {
                            const pct = it.total > 0 ? Math.min(100, (it.transferred / it.total) * 100) : it.state === "done" ? 100 : 0;
                            const remain = it.speed > 0 ? (it.total - it.transferred) / it.speed : Infinity;
                            return (
                                <div key={it.id} className={`${styles.qrow} ${it.state === "done" ? styles.qrowDone : ""}`}>
                                    <span className={`${styles.qdir} ${it.dir === "up" ? styles.qdirUp : styles.qdirDown}`}>
                                        {it.dir === "up" ? <ArrowUp size={15} /> : <ArrowDown size={15} />}
                                    </span>
                                    <span className={styles.qname} title={it.name}>{it.name}</span>
                                    <span className={styles.qroute} title={`${it.routeFrom} → ${it.routeTo}`}>
                                        {it.routeFrom} → <span className={styles.qrouteH}>{it.routeTo}</span>
                                    </span>
                                    {it.state === "active" && (
                                        <>
                                            <span className={styles.qbar}><span className={styles.qbarFill} style={{ width: `${pct}%` }} /></span>
                                            <span className={styles.qmeta}>{fmtSpeed(it.speed, locale)}</span>
                                            <span className={styles.qmeta}>{fmtEta(remain)}</span>
                                        </>
                                    )}
                                    {it.state === "queued" && <span className={styles.qqueued}>{t("sftp.qWaiting")}</span>}
                                    {it.state === "done" && (
                                        <span className={styles.qdoneLbl}><Check size={13} /> {t("sftp.qDone")}</span>
                                    )}
                                    {it.state === "error" && <span className={styles.qerr} title={it.error}>{t("sftp.qError")}</span>}
                                    {it.state === "cancelled" && <span className={styles.qqueued}>{t("sftp.qCancelled")}</span>}
                                    {(it.state === "active" || it.state === "queued") ? (
                                        <button className={styles.qx} title={t("common.cancel")} onClick={() => onCancel(it.id)}>
                                            <X size={14} />
                                        </button>
                                    ) : it.state === "error" || it.state === "cancelled" ? (
                                        <button className={styles.qretry} title={t("sftp.qRetry")} onClick={() => onRetry(it.id)}>
                                            <RotateCw size={13} />
                                        </button>
                                    ) : (
                                        <span className={styles.qx} />
                                    )}
                                </div>
                            );
                        })
                    )}
                </div>
            )}
        </div>
    );
}

/* ── the explorer ── */
export function SftpView({
    session: _session,
    visible: _visible,
}: {
    session: SessionTab;
    visible: boolean;
}) {
    const { t } = useT();
    const hosts = useHostsStore((s) => s.items).filter((h) => h.protocol === "ssh");
    const left = usePanel();
    const right = usePanel();
    const [activeSide, setActiveSide] = useState<"left" | "right">("left");
    const dragRef = useRef<{ from: "left" | "right"; files: FsEntry[] } | null>(null);
    const [status, setStatus] = useState<Status>({ kind: "idle", text: "" });
    const transfers = useTransfers();
    const epName = (p: Panel) =>
        p.source.kind === "local" ? t("sftp.local") : p.source.host.display_name ?? p.source.host.name;
    const [confirmDelete, setConfirmDelete] = useState<{
        panel: Panel;
        entries: { path: string; is_dir: boolean }[];
    } | null>(null);
    const [conflict, setConflict] = useState<{
        files: FsEntry[];
        from: Panel;
        to: Panel;
        existing: Set<string>;
        names: string[];
    } | null>(null);
    const [chmodTarget, setChmodTarget] = useState<{ panel: Panel; entry: FsEntry } | null>(null);

    const resolveConflict = (mode: "replace" | "keep" | "skip") => {
        if (!conflict) return;
        const { files, from, to, existing } = conflict;
        setConflict(null);
        if (mode === "replace") {
            enqueueItems(files, from, to, () => undefined);
        } else if (mode === "skip") {
            enqueueItems(files.filter((f) => !existing.has(f.name)), from, to, () => undefined);
        } else {
            const taken = new Set(existing);
            enqueueItems(files, from, to, (f) => {
                if (!existing.has(f.name)) return undefined;
                const u = uniqueName(f.name, taken);
                taken.add(u);
                return u;
            });
        }
    };

    const runDelete = async () => {
        if (!confirmDelete) return;
        const { panel, entries } = confirmDelete;
        setConfirmDelete(null);
        setStatus({ kind: "busy", text: t("sftp.deleting", { n: String(entries.length) }) });
        try {
            await panel.remove(entries);
            setStatus({ kind: "done", text: t("sftp.deleted", { n: String(entries.length) }) });
        } catch (e: unknown) {
            setStatus({ kind: "error", text: formatApiError(e) });
        }
    };

    const enqueueItems = (
        files: FsEntry[],
        from: Panel,
        to: Panel,
        nameFor: (f: FsEntry) => string | undefined,
    ) => {
        if (!to.listing) return;
        const fromHost = from.source.kind === "host";
        const toHost = to.source.kind === "host";
        const dstDir = to.listing.path;
        const items: TItem[] = files.map((f) => {
            const id = `${Date.now()}-${Math.random().toString(36).slice(2, 9)}`;
            const dstName = nameFor(f);
            let dir: TKind;
            let req: TItem["req"];
            if (fromHost && !toHost) {
                dir = "down";
                req = { transfer_id: id, kind: "download", session_id: from.sessionId as string, src_path: f.path, dst_dir: dstDir, dst_name: dstName };
            } else if (!fromHost && toHost) {
                dir = "up";
                req = { transfer_id: id, kind: "upload", session_id: to.sessionId as string, src_path: f.path, dst_dir: dstDir, dst_name: dstName };
            } else {
                dir = "up";
                req = { transfer_id: id, kind: "copy", session_id: from.sessionId as string, to_session: to.sessionId as string, src_path: f.path, dst_dir: dstDir, dst_name: dstName };
            }
            return {
                id,
                name: dstName ?? f.name,
                dir,
                routeFrom: epName(from),
                routeTo: `${epName(to)}:${dstDir}`,
                total: f.size,
                transferred: 0,
                state: "queued" as TState,
                startedAt: 0,
                speed: 0,
                onComplete: () => void to.refresh(),
                req,
            };
        });
        from.clearSel();
        transfers.enqueue(items);
    };

    const transferFiles = (files: FsEntry[], from: Panel, to: Panel) => {
        const real = files.filter((f) => !f.is_dir);
        if (real.length === 0 || !to.listing) return;
        const fromHost = from.source.kind === "host";
        const toHost = to.source.kind === "host";
        if (!fromHost && !toHost) {
            setStatus({ kind: "error", text: t("sftp.localToLocalSoon") });
            return;
        }
        const existing = new Set((to.listing.entries ?? []).map((e) => e.name));
        const conflicts = real.filter((f) => existing.has(f.name));
        if (conflicts.length === 0) {
            enqueueItems(real, from, to, () => undefined);
        } else {
            setConflict({ files: real, from, to, existing, names: conflicts.map((f) => f.name) });
        }
    };

    useEffect(() => {
        if (status.kind !== "done") return;
        const id = window.setTimeout(() => setStatus({ kind: "idle", text: "" }), 2500);
        return () => window.clearTimeout(id);
    }, [status]);

    const leftSelFiles = (left.listing?.entries ?? []).filter((e) => left.sel.has(e.path) && !e.is_dir);
    const rightSelFiles = (right.listing?.entries ?? []).filter((e) => right.sel.has(e.path) && !e.is_dir);
    const canRight = activeSide === "left" && leftSelFiles.length > 0;
    const canLeft = activeSide === "right" && rightSelFiles.length > 0;

    return (
        <div className={styles.sftp}>
            <div className={styles.panes}>
                <Pane
                    panel={left}
                    hosts={hosts}
                    active={activeSide === "left"}
                    onActivate={() => setActiveSide("left")}
                    onTransferFiles={(e) => void transferFiles(e, left, right)}
                    onNotify={setStatus}
                    onRequestDelete={(panel, entries) => setConfirmDelete({ panel, entries })}
                    onDragStartFiles={(files) => (dragRef.current = { from: "left", files })}
                    onDropToPane={() => {
                        const d = dragRef.current;
                        dragRef.current = null;
                        if (d && d.from === "right") transferFiles(d.files, right, left);
                    }}
                    onRequestChmod={(panel, entry) => setChmodTarget({ panel, entry })}
                />
                <div className={styles.prail}>
                    <span className={styles.prailLbl}>{epName(right)}</span>
                    <button
                        type="button"
                        className={`${styles.prailBtn} ${canRight ? styles.prailBtnArmed : ""}`}
                        disabled={!canRight}
                        title={t("sftp.sendOver")}
                        onClick={() => void transferFiles(leftSelFiles, left, right)}
                    >
                        <ArrowRight size={18} />
                    </button>
                    <button
                        type="button"
                        className={`${styles.prailBtn} ${canLeft ? styles.prailBtnArmed : ""}`}
                        disabled={!canLeft}
                        title={t("sftp.sendOver")}
                        onClick={() => void transferFiles(rightSelFiles, right, left)}
                    >
                        <ArrowLeft size={18} />
                    </button>
                    <span className={styles.prailLbl}>{epName(left)}</span>
                </div>
                <Pane
                    panel={right}
                    hosts={hosts}
                    active={activeSide === "right"}
                    onActivate={() => setActiveSide("right")}
                    onTransferFiles={(e) => void transferFiles(e, right, left)}
                    onNotify={setStatus}
                    onRequestDelete={(panel, entries) => setConfirmDelete({ panel, entries })}
                    onDragStartFiles={(files) => (dragRef.current = { from: "right", files })}
                    onDropToPane={() => {
                        const d = dragRef.current;
                        dragRef.current = null;
                        if (d && d.from === "left") transferFiles(d.files, left, right);
                    }}
                    onRequestChmod={(panel, entry) => setChmodTarget({ panel, entry })}
                />
            </div>
            {transfers.items.length > 0 && (
                <TransferQueue items={transfers.items} onCancel={transfers.cancel} onRetry={transfers.retry} onRetryAll={transfers.retryAll} onClear={transfers.clearDone} />
            )}
            {status.kind !== "idle" && (
                <div className={`${styles.status} ${styles[`status${status.kind === "busy" ? "Busy" : status.kind === "done" ? "Done" : "Error"}`]}`}>
                    {status.text}
                </div>
            )}
            {confirmDelete && (
                <div className={styles.scrim} onClick={() => setConfirmDelete(null)}>
                    <div className={styles.confirm} onClick={(e) => e.stopPropagation()}>
                        <div className={styles.confirmTitle}>
                            {t("sftp.deleteTitle", { n: String(confirmDelete.entries.length) })}
                        </div>
                        <div className={styles.confirmList}>
                            {confirmDelete.entries.slice(0, 8).map((e) => (
                                <div key={e.path} className={styles.confirmItem}>
                                    {e.path.split(/[\\/]/).pop()}
                                </div>
                            ))}
                            {confirmDelete.entries.length > 8 && (
                                <div className={styles.confirmItem}>
                                    … +{confirmDelete.entries.length - 8}
                                </div>
                            )}
                        </div>
                        <div className={styles.confirmActions}>
                            <button className={styles.btnGhost} onClick={() => setConfirmDelete(null)}>
                                {t("common.cancel")}
                            </button>
                            <button className={styles.btnDanger} onClick={() => void runDelete()}>
                                {t("sftp.delete")}
                            </button>
                        </div>
                    </div>
                </div>
            )}
            {conflict && (
                <div className={styles.scrim} onClick={() => setConflict(null)}>
                    <div className={styles.confirm} onClick={(e) => e.stopPropagation()}>
                        <div className={styles.cfHead}>
                            <AlertTriangle size={18} className={styles.cfWarn} />
                            <div>
                                <div className={styles.confirmTitle} style={{ padding: 0 }}>
                                    {t("sftp.conflictTitle", { n: String(conflict.names.length) })}
                                </div>
                            </div>
                        </div>
                        <div className={styles.confirmList}>
                            {conflict.names.slice(0, 8).map((n) => (
                                <div key={n} className={styles.confirmItem}>{n}</div>
                            ))}
                            {conflict.names.length > 8 && (
                                <div className={styles.confirmItem}>… +{conflict.names.length - 8}</div>
                            )}
                        </div>
                        <div className={styles.cfOpts}>
                            <button className={styles.cfOpt} onClick={() => resolveConflict("replace")}>
                                <span className={styles.cfOptT}>{t("sftp.cfReplace")}</span>
                                <span className={styles.cfOptS}>{t("sftp.cfReplaceHint")}</span>
                            </button>
                            <button className={styles.cfOpt} onClick={() => resolveConflict("keep")}>
                                <span className={styles.cfOptT}>{t("sftp.cfKeep")}</span>
                                <span className={styles.cfOptS}>{t("sftp.cfKeepHint")}</span>
                            </button>
                            <button className={styles.cfOpt} onClick={() => resolveConflict("skip")}>
                                <span className={styles.cfOptT}>{t("sftp.cfSkip")}</span>
                                <span className={styles.cfOptS}>{t("sftp.cfSkipHint")}</span>
                            </button>
                        </div>
                        <div className={styles.confirmActions}>
                            <button className={styles.btnGhost} onClick={() => setConflict(null)}>
                                {t("common.cancel")}
                            </button>
                        </div>
                    </div>
                </div>
            )}
            {chmodTarget && (
                <ChmodDialog
                    entry={chmodTarget.entry}
                    onClose={() => setChmodTarget(null)}
                    onApply={(mode) => {
                        const { panel, entry } = chmodTarget;
                        setChmodTarget(null);
                        panel
                            .chmod(entry.path, mode)
                            .catch((e: unknown) => setStatus({ kind: "error", text: formatApiError(e) }));
                    }}
                />
            )}
        </div>
    );
}
