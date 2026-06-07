import { useEffect, useMemo, useState } from "react";
import {
    ArrowRightLeft,
    ArrowUpRight,
    Copy,
    Download,
    Eye,
    EyeOff,
    FolderOpen,
    HardDrive,
    KeyRound,
    Lock,
    Search,
    Server,
    Share2,
    Terminal,
    Trash2,
} from "lucide-react";

import { useT } from "../../i18n";
import { credentials as credApi, hosts as hostsApi } from "../../lib/ipc";
import type { CredentialDto, CredentialKind, HostDto } from "../../lib/types";
import { useCredentialsStore, useHostsStore, useSessionsStore } from "../../store";
import styles from "./ToolsView.module.css";

type CredLink = { count: number; username: string };

/** Build the real host↔credential linkage from each host's full DTO
 *  (`host_list` omits `credential_ids`). This is the same source the host
 *  editor reads, so the manager reflects exactly what hosts use — no stale
 *  orphans. `ready` flips true once the first aggregation completes. */
function useCredentialLinks(hosts: HostDto[]): {
    links: Map<string, CredLink>;
    ready: boolean;
} {
    const [links, setLinks] = useState<Map<string, CredLink>>(new Map());
    const [ready, setReady] = useState(false);
    useEffect(() => {
        let cancelled = false;
        void (async () => {
            const map = new Map<string, CredLink>();
            const fulls = await Promise.all(
                hosts.map((h) => hostsApi.get(h.id).catch(() => null)),
            );
            for (const f of fulls) {
                if (!f) continue;
                for (const cid of f.credential_ids ?? []) {
                    const cur = map.get(cid) ?? { count: 0, username: "" };
                    cur.count += 1;
                    if (!cur.username && f.username) cur.username = f.username;
                    map.set(cid, cur);
                }
            }
            if (!cancelled) {
                setLinks(map);
                setReady(true);
            }
        })();
        return () => {
            cancelled = true;
        };
    }, [hosts]);
    return { links, ready };
}

type ManageKey = "keys" | "import" | "forwards" | "mounts" | "share";
type KeySeg = "all" | "keys" | "pwd";

const NAV: { id: ManageKey; icon: typeof KeyRound; label: string; soon: boolean }[] = [
    { id: "keys", icon: KeyRound, label: "tools.section.creds", soon: false },
    { id: "import", icon: Download, label: "tools.nav.import", soon: false },
    { id: "forwards", icon: ArrowRightLeft, label: "tools.section.forwards", soon: true },
    { id: "mounts", icon: HardDrive, label: "tools.section.mounts", soon: true },
    { id: "share", icon: Share2, label: "tools.section.share", soon: true },
];

const EMPTY: Record<Exclude<ManageKey, "keys">, { icon: typeof KeyRound; title: string; body: string; soon: boolean }> = {
    import: { icon: Download, title: "tools.nav.import", body: "tools.import.body", soon: false },
    forwards: { icon: ArrowRightLeft, title: "tools.section.forwards", body: "tools.forwards.body", soon: true },
    mounts: { icon: HardDrive, title: "tools.section.mounts", body: "tools.mounts.body", soon: true },
    share: { icon: Share2, title: "tools.section.share", body: "tools.share.body", soon: true },
};

function chipFor(kind: CredentialKind): { key: string; pwd: boolean } {
    if (kind === "password") return { key: "tools.chip.password", pwd: true };
    if (kind === "ssh_key_agent") return { key: "tools.chip.agent", pwd: false };
    return { key: "tools.chip.key", pwd: false };
}

export function ToolsView() {
    const { t } = useT();
    const creds = useCredentialsStore((s) => s.items);
    const hosts = useHostsStore((s) => s.items);
    const openLocalTerminal = useSessionsStore((s) => s.openLocalTerminal);
    const openSftp = useSessionsStore((s) => s.openSftp);
    const { links, ready } = useCredentialLinks(hosts);
    // Only credentials actually attached to a host. Orphans left behind when
    // a host's password is cleared / auth method switched never show here.
    const inUse = useMemo(
        () => creds.filter((c) => links.has(c.id)),
        [creds, links],
    );
    const [active, setActive] = useState<ManageKey>("keys");
    const [toasts, setToasts] = useState<{ id: number; text: string }[]>([]);

    const toast = (text: string) => {
        const id = Date.now() + Math.random();
        setToasts((arr) => [...arr, { id, text }]);
        window.setTimeout(() => setToasts((arr) => arr.filter((x) => x.id !== id)), 2200);
    };

    const openTerminal = () => {
        openLocalTerminal(t("storage.quick.terminal"));
        toast(t("tools.toast.opened"));
    };
    const openSftpTab = () => {
        openSftp(t("storage.quick.sftp"));
        toast(t("tools.toast.opened"));
    };

    return (
        <div className={styles.split}>
            {/* ── left rail ── */}
            <nav className={styles.rail}>
                <div className={styles.group}>
                    <div className={styles.groupH}>{t("tools.launch")}</div>
                    <button type="button" className={styles.launch} onClick={openTerminal}>
                        <span className={styles.launchIc}><Terminal size={16} /></span>
                        <span className={styles.launchTx}>
                            <span className={styles.launchT}>{t("storage.quick.terminal")}</span>
                            <span className={styles.launchS}>{t("storage.quick.terminalSub")}</span>
                        </span>
                        <ArrowUpRight size={14} className={styles.launchGo} />
                    </button>
                    <button type="button" className={styles.launch} onClick={openSftpTab}>
                        <span className={styles.launchIc}><FolderOpen size={16} /></span>
                        <span className={styles.launchTx}>
                            <span className={styles.launchT}>{t("storage.quick.sftp")}</span>
                            <span className={styles.launchS}>{t("storage.quick.sftpSub")}</span>
                        </span>
                        <ArrowUpRight size={14} className={styles.launchGo} />
                    </button>
                </div>

                <div className={styles.group}>
                    <div className={styles.groupH}>{t("tools.manage")}</div>
                    {NAV.map((n) => {
                        const Ico = n.icon;
                        const on = active === n.id;
                        return (
                            <button
                                key={n.id}
                                type="button"
                                className={`${styles.nav} ${on ? styles.navOn : ""}`}
                                onClick={() => setActive(n.id)}
                            >
                                <Ico size={16} className={styles.navIc} />
                                <span className={styles.navT}>{t(n.label as Parameters<typeof t>[0])}</span>
                                {n.id === "keys" && (
                                    <span className={styles.navCount}>
                                        {ready ? inUse.length : creds.length}
                                    </span>
                                )}
                                {n.soon && <span className={styles.navSoon}>{t("tools.soon")}</span>}
                            </button>
                        );
                    })}
                </div>
            </nav>

            {/* ── right pane ── */}
            <div className={styles.pane}>
                {active === "keys" ? (
                    <KeysPane creds={inUse} links={links} ready={ready} onToast={toast} />
                ) : (
                    <EmptyPane cfg={EMPTY[active]} />
                )}
            </div>

            <div className={styles.toasts}>
                {toasts.map((x) => (
                    <div key={x.id} className={styles.toast}>
                        <ArrowUpRight size={15} /> {x.text}
                    </div>
                ))}
            </div>
        </div>
    );
}

// ── Keys & passwords pane (real) ──
function KeysPane({
    creds,
    links,
    ready,
    onToast,
}: {
    creds: CredentialDto[];
    links: Map<string, CredLink>;
    ready: boolean;
    onToast: (s: string) => void;
}) {
    const { t, locale } = useT();
    const reload = useCredentialsStore((s) => s.load);
    const [q, setQ] = useState("");
    const [seg, setSeg] = useState<KeySeg>("all");

    // Revealed secrets cache + which password rows are currently unmasked.
    const [secrets, setSecrets] = useState<Record<string, string>>({});
    const [shown, setShown] = useState<Set<string>>(new Set());
    const [keyModal, setKeyModal] = useState<{ name: string; secret: string } | null>(null);

    const ensureSecret = async (id: string): Promise<string> => {
        if (secrets[id] !== undefined) return secrets[id] ?? "";
        const r = await credApi.reveal(id);
        const s = r.secret ?? "";
        setSecrets((prev) => ({ ...prev, [id]: s }));
        return s;
    };

    const onEye = async (c: CredentialDto) => {
        try {
            const s = await ensureSecret(c.id);
            if (c.kind === "password") {
                setShown((prev) => {
                    const n = new Set(prev);
                    if (n.has(c.id)) n.delete(c.id);
                    else n.add(c.id);
                    return n;
                });
            } else {
                setKeyModal({ name: c.name, secret: s });
            }
        } catch {
            onToast(t("common.error"));
        }
    };

    const onCopy = async (c: CredentialDto) => {
        try {
            const s = await ensureSecret(c.id);
            await navigator.clipboard.writeText(s);
            onToast(t("tools.keys.copied"));
        } catch {
            onToast(t("common.error"));
        }
    };

    const hostsLabel = (n: number): string => {
        if (locale === "ru") {
            const m10 = n % 10;
            const m100 = n % 100;
            const word =
                m10 === 1 && m100 !== 11
                    ? "хост"
                    : m10 >= 2 && m10 <= 4 && (m100 < 12 || m100 > 14)
                      ? "хоста"
                      : "хостов";
            return `${n} ${word}`;
        }
        return `${n} ${n === 1 ? "host" : "hosts"}`;
    };

    const filtered = creds.filter((c) => {
        if (seg === "keys" && c.kind === "password") return false;
        if (seg === "pwd" && c.kind !== "password") return false;
        const needle = q.trim().toLowerCase();
        if (!needle) return true;
        const login = c.username || links.get(c.id)?.username || "";
        return (
            c.name.toLowerCase().includes(needle) ||
            login.toLowerCase().includes(needle)
        );
    });

    const del = async (id: string) => {
        try {
            await credApi.delete(id);
            await reload();
            onToast(t("tools.keys.deleted"));
        } catch {
            onToast(t("common.error"));
        }
    };

    return (
        <>
            <div className={styles.paneHead}>
                <div className={styles.paneTitleRow}>
                    <div className={styles.paneTitle}>
                        <KeyRound size={18} className={styles.paneTitleIc} />
                        {t("tools.section.creds")}
                    </div>
                    <div className={styles.paneSp} />
                </div>
                <div className={styles.paneSub}>{t("tools.keys.sub")}</div>
                <div className={styles.paneTools}>
                    <div className={styles.search}>
                        <Search size={15} />
                        <input
                            value={q}
                            onChange={(e) => setQ(e.target.value)}
                            placeholder={t("tools.keys.searchPlaceholder")}
                            spellCheck={false}
                        />
                    </div>
                    <div className={styles.seg}>
                        {(["all", "keys", "pwd"] as KeySeg[]).map((s) => (
                            <button
                                key={s}
                                type="button"
                                className={seg === s ? styles.segOn : ""}
                                onClick={() => setSeg(s)}
                            >
                                {t(
                                    (s === "all"
                                        ? "tools.keys.segAll"
                                        : s === "keys"
                                          ? "tools.keys.segKeys"
                                          : "tools.keys.segPwd") as Parameters<typeof t>[0],
                                )}
                            </button>
                        ))}
                    </div>
                </div>
            </div>

            <div className={styles.paneBody}>
                {!ready ? (
                    <div className={styles.placeholder}>{t("tools.keys.loading")}</div>
                ) : filtered.length === 0 ? (
                    <div className={styles.placeholder}>{t("tools.creds.empty")}</div>
                ) : (
                    <div className={styles.klist}>
                        {filtered.map((c) => {
                            const isKey = c.kind !== "password";
                            const chip = chipFor(c.kind);
                            const info = links.get(c.id);
                            const n = info?.count ?? 0;
                            const login = c.username || info?.username || "";
                            const date = new Date(c.created_at).toLocaleDateString();
                            const open = shown.has(c.id);
                            const secretText =
                                !isKey && open && secrets[c.id] !== undefined
                                    ? secrets[c.id]
                                    : "••••••";
                            return (
                                <div
                                    key={c.id}
                                    className={`${styles.krow} ${isKey ? styles.krowKey : ""}`}
                                    onClick={() => void onEye(c)}
                                >
                                    <span className={styles.krowIc}>
                                        {isKey ? <KeyRound size={16} /> : <Lock size={16} />}
                                    </span>
                                    <div className={styles.krowMain}>
                                        <div className={styles.krowNm}>{c.name}</div>
                                        <div className={styles.krowSub}>
                                            {date}
                                            {login ? ` · ${login}` : ""} ·{" "}
                                            <span className={styles.krowSecret}>{secretText}</span>
                                            {!isKey && open && (
                                                <button
                                                    type="button"
                                                    className={styles.inlineCopy}
                                                    title={t("tools.keys.copy")}
                                                    onClick={(e) => {
                                                        e.stopPropagation();
                                                        void onCopy(c);
                                                    }}
                                                >
                                                    <Copy size={12} />
                                                </button>
                                            )}
                                        </div>
                                    </div>
                                    <span className={`${styles.kchip} ${chip.pwd ? styles.kchipPwd : ""}`}>
                                        {t(chip.key as Parameters<typeof t>[0])}
                                    </span>
                                    <span className={styles.krowUsed}>
                                        {n > 0 ? (
                                            <>
                                                <Server size={13} /> {hostsLabel(n)}
                                            </>
                                        ) : (
                                            <span className={styles.krowUnused}>
                                                {t("tools.keys.unused")}
                                            </span>
                                        )}
                                    </span>
                                    <span className={styles.krowAct}>
                                        <button
                                            type="button"
                                            className={styles.actBtn}
                                            title={isKey ? t("tools.keys.viewKey") : t("tools.keys.reveal")}
                                            onClick={(e) => {
                                                e.stopPropagation();
                                                void onEye(c);
                                            }}
                                        >
                                            {!isKey && open ? <EyeOff size={14} /> : <Eye size={14} />}
                                        </button>
                                        <button
                                            type="button"
                                            className={styles.actBtnDanger}
                                            title={t("common.delete")}
                                            onClick={(e) => {
                                                e.stopPropagation();
                                                void del(c.id);
                                            }}
                                        >
                                            <Trash2 size={14} />
                                        </button>
                                    </span>
                                </div>
                            );
                        })}
                    </div>
                )}
            </div>

            {keyModal && (
                <div className={styles.modalOverlay} onClick={() => setKeyModal(null)}>
                    <div className={styles.modal} onClick={(e) => e.stopPropagation()}>
                        <div className={styles.modalHead}>
                            <KeyRound size={15} /> {keyModal.name}
                        </div>
                        <textarea
                            className={styles.modalKey}
                            readOnly
                            value={keyModal.secret}
                            rows={8}
                            spellCheck={false}
                            onFocus={(e) => e.currentTarget.select()}
                        />
                        <div className={styles.modalFoot}>
                            <button
                                type="button"
                                className={styles.modalCopy}
                                onClick={async () => {
                                    try {
                                        await navigator.clipboard.writeText(keyModal.secret);
                                        onToast(t("tools.keys.copied"));
                                    } catch {
                                        onToast(t("common.error"));
                                    }
                                }}
                            >
                                <Copy size={14} /> {t("tools.keys.copy")}
                            </button>
                            <button
                                type="button"
                                className={styles.modalClose}
                                onClick={() => setKeyModal(null)}
                            >
                                {t("common.close")}
                            </button>
                        </div>
                    </div>
                </div>
            )}
        </>
    );
}

// ── coming-soon / empty pane ──
function EmptyPane({ cfg }: { cfg: { icon: typeof KeyRound; title: string; body: string; soon: boolean } }) {
    const { t } = useT();
    const Ico = cfg.icon;
    return (
        <div className={styles.empty}>
            <div className={styles.emptyIc}>
                <Ico size={26} />
            </div>
            <div className={styles.emptyT}>{t(cfg.title as Parameters<typeof t>[0])}</div>
            <div className={styles.emptyS}>{t(cfg.body as Parameters<typeof t>[0])}</div>
            {cfg.soon && <div className={styles.emptySoon}>{t("tools.soon")}</div>}
        </div>
    );
}
