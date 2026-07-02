import { useCallback, useEffect, useMemo, useState } from "react";
import {
    Check,
    Copy,
    ExternalLink,
    KeyRound,
    Loader2,
    Pencil,
    Plus,
    Trash2,
    Waypoints,
    X,
} from "lucide-react";

import { useT } from "../../i18n";
import { app, sshId } from "../../lib/ipc";
import { formatApiError, type SshIdKey } from "../../lib/types";
import styles from "./SshIdManager.module.css";

const HANDLE_RE = /^[a-z0-9][a-z0-9_-]{0,30}[a-z0-9]$/; // 2–32, first/last alnum

/** OpenSSH-style SHA256 fingerprint, computed in-browser from the key blob. */
async function fingerprint(pubkey: string): Promise<string> {
    try {
        const parts = pubkey.trim().split(/\s+/);
        const b64 = (parts.length > 1 ? parts[1] : parts[0]) ?? "";
        const bin = atob(b64);
        const bytes = new Uint8Array(bin.length);
        for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
        const digest = await crypto.subtle.digest("SHA-256", bytes);
        const d = new Uint8Array(digest);
        let s = "";
        for (const b of d) s += String.fromCharCode(b);
        return "SHA256:" + btoa(s).replace(/=+$/, "");
    } catch {
        return "";
    }
}

function shortFp(fp: string): string {
    if (!fp.startsWith("SHA256:")) return fp;
    const body = fp.slice(7);
    if (body.length <= 14) return fp;
    return `SHA256:${body.slice(0, 6)}…${body.slice(-5)}`;
}

type Stage = "loading" | "need_login" | "A" | "B" | "C";

export function SshIdPane({ onToast }: { onToast: (s: string) => void }) {
    const { t } = useT();

    const [stage, setStage] = useState<Stage>("loading");
    const [handle, setHandleVal] = useState<string | null>(null);
    const [keys, setKeys] = useState<SshIdKey[]>([]);

    // State B
    const [input, setInput] = useState("");
    const [avail, setAvail] = useState<{ available: boolean; reason: string | null } | null>(null);
    const [checking, setChecking] = useState(false);
    const [creating, setCreating] = useState(false);
    const [editing, setEditing] = useState(false);

    // State C
    const [addOpen, setAddOpen] = useState(false);
    const [paste, setPaste] = useState("");
    const [addLabel, setAddLabel] = useState("");
    const [adding, setAdding] = useState(false);
    const [linkCopied, setLinkCopied] = useState(false);
    const [cmdCopied, setCmdCopied] = useState(false);
    const [fps, setFps] = useState<Record<string, string>>({});
    const [cmdType, setCmdType] = useState<"all" | "ed25519" | "rsa" | "ecdsa">("all");

    const load = useCallback(async () => {
        try {
            const data = await sshId.get();
            setHandleVal(data.handle);
            setKeys(data.keys);
            setStage(data.handle ? "C" : "A");
        } catch (e) {
            const msg = formatApiError(e).toLowerCase();
            setStage(/logged in|unauthorized|401/.test(msg) ? "need_login" : "A");
        }
    }, []);

    useEffect(() => {
        void load();
    }, [load]);

    // compute fingerprints when the key list changes
    useEffect(() => {
        let cancelled = false;
        void (async () => {
            const m: Record<string, string> = {};
            for (const k of keys) m[k.id] = await fingerprint(k.public_key);
            if (!cancelled) setFps(m);
        })();
        return () => {
            cancelled = true;
        };
    }, [keys]);

    // ── State B validation ──
    const localOk = HANDLE_RE.test(input);
    const hasUpper = /[A-Z]/.test(input);
    const isFree = localOk && avail?.available === true;
    const isTaken = localOk && avail?.available === false;

    useEffect(() => {
        if (!localOk) {
            setAvail(null);
            setChecking(false);
            return;
        }
        setChecking(true);
        const id = setTimeout(async () => {
            try {
                const r = await sshId.check(input);
                setAvail(r);
            } catch {
                setAvail(null);
            } finally {
                setChecking(false);
            }
        }, 350);
        return () => clearTimeout(id);
    }, [input, localOk]);

    const hint = useMemo(() => {
        if (input.length === 0) return { text: t("tools.sshId.hintDefault"), cls: styles.hintMuted };
        if (hasUpper) return { text: t("tools.sshId.hintUpper"), cls: styles.hintWarn };
        if (!localOk) return { text: t("tools.sshId.hintPattern"), cls: styles.hintWarn };
        if (checking) return { text: t("tools.sshId.hintChecking"), cls: styles.hintMuted };
        if (isTaken) return { text: t("tools.sshId.hintTaken"), cls: styles.hintWarn };
        if (isFree) return { text: t("tools.sshId.hintFree"), cls: styles.hintFree };
        return { text: t("tools.sshId.hintDefault"), cls: styles.hintMuted };
    }, [input, hasUpper, localOk, checking, isTaken, isFree, t]);

    async function create() {
        if (!isFree || creating) return;
        setCreating(true);
        try {
            await sshId.setHandle(input);
            await load();
        } catch (e) {
            onToast(formatApiError(e));
        } finally {
            setCreating(false);
        }
    }

    function startEdit() {
        setInput(handle ?? "");
        setAvail(null);
        setEditing(true);
    }
    function cancelEdit() {
        setEditing(false);
        setInput("");
    }
    async function saveEdit() {
        if (!isFree || creating || input === handle) return;
        setCreating(true);
        try {
            await sshId.setHandle(input);
            await load();
            setEditing(false);
            setInput("");
            onToast(t("tools.sshId.handleSaved"));
        } catch (e) {
            onToast(formatApiError(e));
        } finally {
            setCreating(false);
        }
    }

    async function reloadKeys() {
        try {
            const d = await sshId.get();
            setKeys(d.keys);
        } catch {
            /* ignore */
        }
    }

    async function confirmAdd() {
        const pk = paste.trim();
        if (pk.length < 8 || adding) return;
        setAdding(true);
        try {
            await sshId.addKey(pk, addLabel.trim() || null);
            setAddOpen(false);
            setPaste("");
            setAddLabel("");
            await reloadKeys();
            onToast(t("tools.sshId.keyAdded"));
        } catch (e) {
            onToast(formatApiError(e));
        } finally {
            setAdding(false);
        }
    }

    async function del(id: string) {
        try {
            await sshId.deleteKey(id);
            await reloadKeys();
            onToast(t("tools.sshId.keyDeleted"));
        } catch (e) {
            onToast(formatApiError(e));
        }
    }

    async function saveLabel(id: string, current: string | null, next: string) {
        const v = next.trim();
        if ((current ?? "") === v) return; // unchanged
        try {
            await sshId.updateLabel(id, v || null);
            await reloadKeys();
        } catch (e) {
            onToast(formatApiError(e));
        }
    }

    function copyLink() {
        void navigator.clipboard.writeText(`pingie.ru/${handle}`);
        setLinkCopied(true);
        setTimeout(() => setLinkCopied(false), 1600);
    }

    const cmd = `curl -fs https://pingie.ru/${handle ?? ""}${cmdType !== "all" ? "/" + cmdType : ""} >> ~/.ssh/authorized_keys`;
    function copyCmd() {
        void navigator.clipboard.writeText(cmd);
        setCmdCopied(true);
        setTimeout(() => setCmdCopied(false), 1600);
    }

    // ── render ──
    const header = (
        <div className={styles.head}>
            <div className={styles.headRow}>
                <Waypoints size={17} className={styles.headIcon} />
                <h2>{t("tools.sshId.title")}</h2>
            </div>
            <p className={styles.desc}>{t("tools.sshId.desc")}</p>
        </div>
    );

    return (
        <div className={styles.pane}>
            {header}
            <div className={styles.body}>
                {stage === "loading" && <div className={styles.loading}>{t("common.loading")}</div>}

                {stage === "need_login" && (
                    <div className={styles.notice}>{t("tools.sshId.needLogin")}</div>
                )}

                {stage === "A" && (
                    <div className={styles.aWrap}>
                        <div className={styles.aIcon}>
                            <KeyRound size={19} />
                        </div>
                        <div className={styles.aText}>{t("tools.sshId.empty")}</div>
                        <button
                            type="button"
                            className={styles.primary}
                            onClick={() => setStage("B")}
                        >
                            <Plus size={15} />
                            {t("tools.sshId.create")}
                        </button>
                        <div className={styles.aHint}>pingie.ru/&lt;{t("tools.sshId.yourHandle")}&gt;</div>
                    </div>
                )}

                {stage === "B" && (
                    <div className={styles.bWrap}>
                        <div className={styles.sect}>{t("tools.sshId.chooseHandle")}</div>
                        <div className={`${styles.inputRow} ${isTaken ? styles.taken : ""}`}>
                            <span className={styles.inputPrefix}>pingie.ru/</span>
                            <input
                                value={input}
                                onChange={(e) => setInput(e.target.value)}
                                placeholder={t("tools.sshId.handlePlaceholder")}
                                spellCheck={false}
                                autoComplete="off"
                                autoFocus
                                onKeyDown={(e) => {
                                    if (e.key === "Enter") void create();
                                }}
                            />
                            <span className={styles.status}>
                                {checking ? (
                                    <Loader2 size={14} className={`${styles.headIcon} ${styles.spin}`} />
                                ) : isFree ? (
                                    <Check size={15} color="var(--color-success)" strokeWidth={2.3} />
                                ) : isTaken ? (
                                    <X size={14} color="var(--color-danger)" strokeWidth={2.3} />
                                ) : null}
                            </span>
                        </div>
                        <div className={`${styles.hint} ${hint.cls}`}>{hint.text}</div>
                        <button
                            type="button"
                            className={`${styles.primary} ${styles.bCreate}`}
                            disabled={!isFree || creating}
                            onClick={() => void create()}
                        >
                            {t("tools.sshId.createBtn")}
                        </button>
                    </div>
                )}

                {stage === "C" && (
                    <>
                        <div className={styles.sect}>{t("tools.sshId.publicLink")}</div>
                        {editing ? (
                            <>
                                <div className={`${styles.inputRow} ${isTaken ? styles.taken : ""}`}>
                                    <span className={styles.inputPrefix}>pingie.ru/</span>
                                    <input
                                        value={input}
                                        onChange={(e) => setInput(e.target.value)}
                                        spellCheck={false}
                                        autoComplete="off"
                                        autoFocus
                                        onKeyDown={(e) => {
                                            if (e.key === "Enter") void saveEdit();
                                            if (e.key === "Escape") cancelEdit();
                                        }}
                                    />
                                    <span className={styles.status}>
                                        {checking ? (
                                            <Loader2 size={14} className={`${styles.headIcon} ${styles.spin}`} />
                                        ) : input === handle ? null : isFree ? (
                                            <Check size={15} color="var(--color-success)" strokeWidth={2.3} />
                                        ) : isTaken ? (
                                            <X size={14} color="var(--color-danger)" strokeWidth={2.3} />
                                        ) : null}
                                    </span>
                                    <button
                                        type="button"
                                        className={styles.ghost}
                                        disabled={!isFree || creating || input === handle}
                                        onClick={() => void saveEdit()}
                                    >
                                        {t("tools.sshId.save")}
                                    </button>
                                    <button
                                        type="button"
                                        className={`${styles.ghost} ${styles.ghostIcon}`}
                                        title={t("tools.sshId.cancel")}
                                        onClick={cancelEdit}
                                    >
                                        <X size={14} />
                                    </button>
                                </div>
                                <div className={`${styles.hint} ${hint.cls}`} style={{ marginBottom: 22 }}>
                                    {input === handle ? t("tools.sshId.hintSameHandle") : hint.text}
                                </div>
                            </>
                        ) : (
                            <div className={styles.linkRow}>
                                <span className={styles.linkText}>
                                    pingie.ru/<span className={styles.at}>{handle}</span>
                                </span>
                                <button
                                    type="button"
                                    className={`${styles.ghost} ${styles.ghostIcon}`}
                                    title={t("tools.sshId.editHandle")}
                                    onClick={startEdit}
                                >
                                    <Pencil size={14} />
                                </button>
                                <button
                                    type="button"
                                    className={styles.ghost}
                                    title={t("tools.sshId.copy")}
                                    onClick={copyLink}
                                >
                                    {linkCopied ? (
                                        <span className={styles.ghostOk}>
                                            <Check size={14} strokeWidth={2.2} /> {t("tools.sshId.copied")}
                                        </span>
                                    ) : (
                                        <>
                                            <Copy size={14} /> {t("tools.sshId.copy")}
                                        </>
                                    )}
                                </button>
                                <button
                                    type="button"
                                    className={`${styles.ghost} ${styles.ghostIcon}`}
                                    title={t("tools.sshId.openBrowser")}
                                    onClick={() => void app.open(`https://pingie.ru/${handle}`)}
                                >
                                    <ExternalLink size={14} />
                                </button>
                            </div>
                        )}

                        <div className={styles.sectRow}>
                            <div className={styles.sect} style={{ marginBottom: 0 }}>
                                {t("tools.sshId.keys")} <span className={styles.count}>· {keys.length}</span>
                            </div>
                            <button
                                type="button"
                                className={styles.addBtn}
                                onClick={() => {
                                    setAddOpen((v) => !v);
                                    setPaste("");
                                    setAddLabel("");
                                }}
                            >
                                <Plus size={13} />
                                {t("tools.sshId.addKey")}
                            </button>
                        </div>

                        {keys.length > 0 ? (
                            <div className={styles.list}>
                                {keys.map((k) => {
                                    const type = k.key_type.toUpperCase();
                                    return (
                                        <div key={k.id} className={styles.key}>
                                            <span
                                                className={`${styles.badge} ${k.key_type === "ed25519" ? styles.badgeAccent : styles.badgeNeutral}`}
                                            >
                                                {type}
                                            </span>
                                            <span className={styles.keyLabel}>
                                                <span className={styles.hash}>#</span>
                                                <input
                                                    key={k.id}
                                                    className={styles.labelEdit}
                                                    defaultValue={k.label ?? ""}
                                                    placeholder={t("tools.sshId.noLabel")}
                                                    spellCheck={false}
                                                    autoComplete="off"
                                                    onBlur={(e) => void saveLabel(k.id, k.label, e.target.value)}
                                                    onKeyDown={(e) => {
                                                        if (e.key === "Enter") (e.target as HTMLInputElement).blur();
                                                    }}
                                                />
                                            </span>
                                            <span className={styles.keyFp} title={fps[k.id]}>
                                                {shortFp(fps[k.id] ?? "")}
                                            </span>
                                            <button
                                                type="button"
                                                className={styles.del}
                                                title={t("tools.sshId.delete")}
                                                onClick={() => void del(k.id)}
                                            >
                                                <Trash2 size={14} />
                                            </button>
                                        </div>
                                    );
                                })}
                            </div>
                        ) : (
                            <div className={styles.empty}>{t("tools.sshId.noKeys")}</div>
                        )}

                        {addOpen && (
                            <div className={styles.dialog}>
                                <div className={styles.dialogBody}>
                                    <textarea
                                        className={styles.textarea}
                                        value={paste}
                                        onChange={(e) => setPaste(e.target.value)}
                                        placeholder={t("tools.sshId.pastePlaceholder")}
                                        spellCheck={false}
                                    />
                                    <input
                                        className={styles.labelInput}
                                        value={addLabel}
                                        onChange={(e) => setAddLabel(e.target.value)}
                                        placeholder={t("tools.sshId.labelPlaceholder")}
                                        spellCheck={false}
                                    />
                                    <div className={styles.dialogActions}>
                                        <button
                                            type="button"
                                            className={styles.cancel}
                                            onClick={() => setAddOpen(false)}
                                        >
                                            {t("tools.sshId.cancel")}
                                        </button>
                                        <button
                                            type="button"
                                            className={styles.confirm}
                                            disabled={paste.trim().length < 8 || adding}
                                            onClick={() => void confirmAdd()}
                                        >
                                            {t("tools.sshId.add")}
                                        </button>
                                    </div>
                                </div>
                            </div>
                        )}

                        <div className={`${styles.sect} ${styles.cmdHead}`}>{t("tools.sshId.command")}</div>
                        <div className={styles.cmd}>
                            <code className={styles.cmdCode}>
                                <span className={styles.kw}>curl</span> -fs https://pingie.ru/{handle}
                                {cmdType !== "all" ? "/" + cmdType : ""} &gt;&gt; ~/.ssh/authorized_keys
                            </code>
                            <select
                                className={styles.typeSel}
                                value={cmdType}
                                onChange={(e) => setCmdType(e.target.value as typeof cmdType)}
                            >
                                <option value="all">All</option>
                                <option value="ed25519">ED25519</option>
                                <option value="rsa">RSA</option>
                                <option value="ecdsa">ECDSA</option>
                            </select>
                            <button
                                type="button"
                                className={styles.ghost}
                                title={t("tools.sshId.copy")}
                                onClick={copyCmd}
                            >
                                {cmdCopied ? (
                                    <span className={styles.ghostOk}>
                                        <Check size={14} strokeWidth={2.2} /> {t("tools.sshId.copied")}
                                    </span>
                                ) : (
                                    <>
                                        <Copy size={14} /> {t("tools.sshId.copy")}
                                    </>
                                )}
                            </button>
                        </div>
                    </>
                )}
            </div>
        </div>
    );
}
