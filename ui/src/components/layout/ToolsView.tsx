import { useEffect, useMemo, useState } from "react";
import {
    ArrowRight,
    ArrowRightLeft,
    ArrowUpRight,
    Check,
    Copy,
    Download,
    Eye,
    EyeOff,
    FolderOpen,
    HardDrive,
    KeyRound,
    Lock,
    Pencil,
    Search,
    Server,
    Share2,
    Terminal,
    Trash2,
    Waypoints,
    X,
    Zap,
} from "lucide-react";

import { useT } from "../../i18n";
import { credentials as credApi, forwards as forwardsApi, hosts as hostsApi } from "../../lib/ipc";
import type { CredentialDto, CredentialKind, ForwardKind, ForwardSaved, HostDto } from "../../lib/types";
import { formatApiError } from "../../lib/types";
import { useCredentialsStore, useHostsStore, useSessionsStore, useUiStore } from "../../store";
import { Combobox, type ComboboxOption } from "../ui/Combobox";
import { SshIdPane } from "./SshIdManager";
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

type ManageKey = "keys" | "ssh_id" | "import" | "forwards" | "mounts" | "share";
type KeySeg = "all" | "keys" | "pwd";

const NAV: { id: ManageKey; icon: typeof KeyRound; label: string; soon: boolean }[] = [
    { id: "keys", icon: KeyRound, label: "tools.section.creds", soon: false },
    { id: "ssh_id", icon: Waypoints, label: "tools.section.sshId", soon: false },
    { id: "import", icon: Download, label: "tools.nav.import", soon: false },
    { id: "forwards", icon: ArrowRightLeft, label: "tools.section.forwards", soon: false },
    { id: "mounts", icon: HardDrive, label: "tools.section.mounts", soon: true },
    { id: "share", icon: Share2, label: "tools.section.share", soon: true },
];

const EMPTY: Record<Exclude<ManageKey, "keys" | "ssh_id">, { icon: typeof KeyRound; title: string; body: string; soon: boolean }> = {
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
    const toolsSection = useUiStore((s) => s.toolsSection);
    const setToolsSection = useUiStore((s) => s.setToolsSection);
    // A tray click (or other external nav) requests a sub-section; apply it
    // once, then clear the request so it doesn't re-fire.
    useEffect(() => {
        if (!toolsSection) return;
        const valid: ManageKey[] = ["keys", "ssh_id", "import", "forwards", "mounts", "share"];
        if ((valid as string[]).includes(toolsSection)) {
            setActive(toolsSection as ManageKey);
        }
        setToolsSection(null);
    }, [toolsSection, setToolsSection]);
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
                ) : active === "ssh_id" ? (
                    <SshIdPane onToast={toast} />
                ) : active === "forwards" ? (
                    <ForwardsPane onToast={toast} />
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
function ForwardsPane({ onToast }: { onToast: (s: string) => void }) {
    const { t } = useT();
    const hosts = useHostsStore((s) => s.items);
    const sshHosts = useMemo(() => hosts.filter((h) => h.protocol === "ssh"), [hosts]);
    const hostOptions = useMemo<ComboboxOption[]>(
        () =>
            sshHosts.map((h) => {
                const name = h.display_name ?? h.name;
                const label =
                    name && h.hostname && name !== h.hostname
                        ? `${name} [${h.hostname}]`
                        : name || h.hostname;
                return { value: h.id, label };
            }),
        [sshHosts],
    );

    const [list, setList] = useState<ForwardSaved[]>([]);
    const [kind, setKind] = useState<ForwardKind>("local");
    const [hostId, setHostId] = useState("");
    // User-facing values. The backend ForwardSpec (bind_*/target_*) is derived
    // per kind on submit so "-R" reads naturally (local port = your service).
    const [localPort, setLocalPort] = useState("");
    const [remotePort, setRemotePort] = useState("");
    const [busy, setBusy] = useState(false);
    const [editId, setEditId] = useState<string | null>(null);
    const [autoStart, setAutoStart] = useState(false);
    const [editWasRunning, setEditWasRunning] = useState(false);

    const refresh = () => {
        void forwardsApi
            .list()
            .then((r) => setList(r.forwards))
            .catch(() => {});
    };
    useEffect(() => {
        refresh();
        const iv = window.setInterval(refresh, 2000);
        return () => window.clearInterval(iv);
    }, []);

    const selHost = useMemo(() => sshHosts.find((h) => h.id === hostId), [sshHosts, hostId]);
    const selHostName = selHost?.hostname ?? "";

    const isDyn = kind === "dynamic";
    const lp = Number(localPort);
    const rp = Number(remotePort);
    const portOk = (n: number) => Number.isInteger(n) && n >= 1 && n <= 65535;
    const valid = hostId !== "" && portOk(lp) && (isDyn || portOk(rp));

    const onlyDigits = (v: string) => v.replace(/[^0-9]/g, "").slice(0, 5);

    // Live preview of what the forward will do, in plain endpoint terms.
    const fwdPreview = useMemo(() => {
        const host = selHostName || t("tools.forwards.previewHost");
        const L = localPort || "·";
        const R = remotePort || "·";
        if (kind === "remote") return `${host}:${R} → localhost:${L}`;
        if (kind === "dynamic") return `localhost:${L} → SOCKS5`;
        return `localhost:${L} → ${host}:${R}`;
    }, [kind, selHostName, localPort, remotePort, t]);

    const resetForm = () => {
        setEditId(null);
        setAutoStart(false);
        setEditWasRunning(false);
        setLocalPort("");
        setRemotePort("");
        // Host stays selected (spec): only the ports clear after a create.
    };

    const run = async (fid: string) => {
        try {
            await forwardsApi.start(fid, (e) => {
                if (e.kind === "error") onToast(e.message);
                refresh();
            });
            refresh();
        } catch (e: unknown) {
            onToast(formatApiError(e));
        }
    };

    // Map the two user-facing ports onto the backend spec for each kind.
    const submit = async () => {
        if (!valid || busy) return;
        setBusy(true);
        try {
            const editing = editId !== null;
            if (editing) await forwardsApi.delete(editId);
            let bind_port: number;
            let target_host: string;
            let target_port: number;
            if (kind === "local") {
                bind_port = lp;
                target_host = selHostName;
                target_port = rp;
            } else if (kind === "remote") {
                bind_port = rp; // port opened ON the server
                target_host = "127.0.0.1";
                target_port = lp; // local-side service it leads back to
            } else {
                bind_port = lp;
                target_host = "";
                target_port = 0;
            }
            const { forward_id } = await forwardsApi.save({
                host_id: hostId,
                kind,
                bind_host: "127.0.0.1",
                bind_port,
                target_host,
                target_port,
                auto_start: autoStart,
            });
            onToast(t(editing ? "tools.forwards.updated" : "tools.forwards.saved"));
            if (!editing || editWasRunning) await run(forward_id);
            else refresh();
            resetForm();
        } catch (e: unknown) {
            onToast(formatApiError(e));
        } finally {
            setBusy(false);
        }
    };

    const startEdit = (f: ForwardSaved) => {
        const s = f.spec;
        setEditId(f.forward_id);
        setAutoStart(f.auto_start);
        setEditWasRunning(f.state === "listening" || f.state === "connecting");
        setKind(s.kind);
        setHostId(f.host_id);
        if (s.kind === "local") {
            setLocalPort(String(s.bind_port));
            setRemotePort(String(s.target_port));
        } else if (s.kind === "remote") {
            setLocalPort(String(s.target_port));
            setRemotePort(String(s.bind_port));
        } else {
            setLocalPort(String(s.bind_port));
            setRemotePort("");
        }
    };

    const stop = async (fid: string) => {
        try {
            await forwardsApi.stop(fid);
            onToast(t("tools.forwards.stopped"));
            refresh();
        } catch (e: unknown) {
            onToast(formatApiError(e));
        }
    };

    const del = async (fid: string) => {
        try {
            await forwardsApi.delete(fid);
            if (editId === fid) resetForm();
            onToast(t("tools.forwards.deleted"));
            refresh();
        } catch (e: unknown) {
            onToast(formatApiError(e));
        }
    };

    const stateLabel = (s: ForwardSaved["state"]): string =>
        t(
            (s === "listening"
                ? "tools.forwards.state.listening"
                : s === "connecting"
                  ? "tools.forwards.state.connecting"
                  : s === "error"
                    ? "tools.forwards.state.error"
                    : s === "closed"
                      ? "tools.forwards.state.closed"
                      : "tools.forwards.state.stopped") as Parameters<typeof t>[0],
        );

    // ssh-command for the copy button.
    const sshCmd = (f: ForwardSaved): string => {
        const s = f.spec;
        const host = hosts.find((h) => h.id === f.host_id)?.hostname || f.host_label;
        if (s.kind === "remote") return `ssh -R ${s.bind_port}:localhost:${s.target_port} ${host}`;
        if (s.kind === "dynamic") return `ssh -D ${s.bind_port} ${host}`;
        return `ssh -L ${s.bind_port}:${s.target_host}:${s.target_port} ${host}`;
    };
    const copyCmd = async (f: ForwardSaved) => {
        try {
            await navigator.clipboard.writeText(sshCmd(f));
            onToast(t("tools.forwards.copied"));
        } catch {
            /* clipboard denied */
        }
    };

    // Route chips for a saved row: [left] via/onServer → [right].
    const route = (f: ForwardSaved) => {
        const s = f.spec;
        if (s.kind === "remote") {
            return {
                left: `${f.host_label}:${s.bind_port}`,
                via: t("tools.forwards.onServer"),
                right: `localhost:${s.target_port}`,
                rightMuted: true,
            };
        }
        if (s.kind === "dynamic") {
            return {
                left: `${t("tools.forwards.socksTag")} :${s.bind_port}`,
                via: t("tools.forwards.via", { host: f.host_label }),
                right: t("tools.forwards.anyAddr"),
                rightMuted: true,
            };
        }
        return {
            left: `localhost:${s.bind_port}`,
            via: t("tools.forwards.via", { host: f.host_label }),
            right: `${s.target_host}:${s.target_port}`,
            rightMuted: false,
        };
    };

    const activeCount = list.filter(
        (f) => f.state === "listening" || f.state === "connecting",
    ).length;

    const Conn = () => (
        <div
            className={`${styles.pfConn} ${kind === "remote" ? styles.pfConnRev : ""}`}
            aria-hidden="true"
        >
            <span className={styles.pfConnLine} />
            <span className={styles.pfDot} />
            <span className={styles.pfDot} />
            <span className={styles.pfDot} />
        </div>
    );

    return (
        <>
            <div className={styles.paneHead}>
                <div className={styles.paneTitleRow}>
                    <div className={styles.paneTitle}>
                        <ArrowRightLeft size={18} className={styles.paneTitleIc} />
                        {t("tools.section.forwards")}
                    </div>
                    {list.length > 0 && (
                        <span className={styles.pfCount}>
                            {t("tools.forwards.count", {
                                a: String(activeCount),
                                n: String(list.length),
                            })}
                        </span>
                    )}
                    <div className={styles.paneSp} />
                    <div className={styles.pfHeadActions}>
                        <button
                            type="button"
                            className={`${styles.pfAuto} ${autoStart ? styles.pfAutoOn : ""}`}
                            aria-pressed={autoStart}
                            title={t("tools.forwards.autoStart")}
                            onClick={() => setAutoStart((v) => !v)}
                        >
                            <Zap size={15} />
                        </button>
                        {editId !== null && (
                            <button
                                type="button"
                                className={styles.pfCancel}
                                title={t("tools.forwards.cancel")}
                                onClick={resetForm}
                            >
                                <X size={15} />
                            </button>
                        )}
                        <button
                            type="button"
                            className={styles.pfCreate}
                            disabled={!valid || busy}
                            onClick={() => void submit()}
                        >
                            <Check size={15} />
                            {t("tools.forwards.save")}
                        </button>
                    </div>
                </div>

                <div className={styles.pfCard}>
                    <div className={styles.pfSegRow}>
                        <div className={styles.pfSeg} role="tablist">
                            {(["local", "remote", "dynamic"] as ForwardKind[]).map((k) => (
                                <button
                                    key={k}
                                    type="button"
                                    role="tab"
                                    aria-selected={kind === k}
                                    className={`${styles.pfSegBtn} ${kind === k ? styles.pfSegOn : ""}`}
                                    onClick={() => setKind(k)}
                                >
                                    <span className={styles.pfSegMain}>
                                        {t(`tools.forwards.kind.${k}` as Parameters<typeof t>[0])}
                                    </span>
                                    <span className={styles.pfSegSub}>
                                        {k === "local" ? "-L" : k === "remote" ? "-R" : "SOCKS5"}
                                    </span>
                                </button>
                            ))}
                        </div>
                        <div className={styles.pfDesc}>
                            <span className={styles.pfPreview}>{fwdPreview}</span>
                        </div>
                    </div>

                    <div className={styles.pfForm}>
                        <div className={styles.pfField}>
                            <div className={styles.pfFieldLbl}>
                                <span className={styles.pfNum}>1</span>
                                {t("tools.forwards.lbl.local")}
                            </div>
                            <label className={styles.pfPortBox}>
                                <span className={styles.pfColon}>:</span>
                                <input
                                    className={styles.pfPortInput}
                                    value={localPort}
                                    onChange={(e) => setLocalPort(onlyDigits(e.target.value))}
                                    placeholder={isDyn ? "1080" : "5432"}
                                    inputMode="numeric"
                                    size={1}
                                    spellCheck={false}
                                />
                            </label>
                        </div>

                        <Conn />

                        <div className={`${styles.pfField} ${styles.pfHostField}`}>
                            <div className={styles.pfFieldLbl}>
                                <span className={styles.pfNum}>2</span>
                                {t("tools.forwards.lbl.host")}
                            </div>
                            <div className={styles.pfHostWrap}>
                                <Server size={15} className={styles.pfHostIcon} />
                                <div className={styles.pfHostCombo}>
                                    <Combobox
                                        options={hostOptions}
                                        value={hostId}
                                        onChange={setHostId}
                                        placeholder={t("tools.forwards.hostPlaceholder")}
                                        clearable
                                    />
                                </div>
                            </div>
                        </div>

                        {!isDyn && <Conn />}

                        {!isDyn && (
                            <div className={styles.pfField}>
                                <div className={styles.pfFieldLbl}>
                                    <span className={styles.pfNum}>3</span>
                                    {t("tools.forwards.lbl.remote")}
                                </div>
                                <label className={styles.pfPortBox}>
                                    <span className={styles.pfColon}>:</span>
                                    <input
                                        className={styles.pfPortInput}
                                        value={remotePort}
                                        onChange={(e) => setRemotePort(onlyDigits(e.target.value))}
                                        placeholder={kind === "remote" ? "8080" : "5432"}
                                        inputMode="numeric"
                                        size={1}
                                        spellCheck={false}
                                    />
                                </label>
                            </div>
                        )}
                    </div>
                </div>
            </div>

            <div className={styles.paneBody}>
                {list.length === 0 ? (
                    <div className={styles.empty}>
                        <ArrowRightLeft size={32} className={styles.emptyIc} />
                        <div className={styles.emptyT}>{t("tools.forwards.emptyT")}</div>
                        <div className={styles.emptyS}>{t("tools.forwards.emptyS")}</div>
                    </div>
                ) : (
                    <div className={styles.pfList}>
                        {list.map((f) => {
                            const r = route(f);
                            const running =
                                f.state === "listening" || f.state === "connecting";
                            const statusCls =
                                f.state === "error"
                                    ? styles.pfErr
                                    : running
                                      ? styles.pfOn
                                      : "";
                            return (
                                <div
                                    key={f.forward_id}
                                    className={`${styles.pfRow} ${editId === f.forward_id ? styles.pfRowEditing : ""}`}
                                >
                                    <div className={styles.pfRowIcon}>
                                        <ArrowRightLeft size={15} />
                                    </div>
                                    <div className={styles.pfMain}>
                                        <div className={styles.pfRoute}>
                                            <span className={styles.pfChip}>{r.left}</span>
                                            <span className={styles.pfVia}>{r.via}</span>
                                            <ArrowRight size={13} className={styles.pfArrow} />
                                            <span
                                                className={`${styles.pfChip} ${r.rightMuted ? styles.pfChipMuted : ""}`}
                                            >
                                                {r.right}
                                            </span>
                                        </div>
                                        <div className={styles.pfMeta}>
                                            <span className={styles.pfTag}>
                                                {t(
                                                    `tools.forwards.kind.${f.spec.kind}` as Parameters<
                                                        typeof t
                                                    >[0],
                                                )}
                                            </span>
                                            {f.auto_start && (
                                                <span
                                                    className={styles.pfAutoIcon}
                                                    title={t("tools.forwards.autoStart")}
                                                >
                                                    <Zap size={12} />
                                                </span>
                                            )}
                                            <span className={styles.pfName}>{f.host_label}</span>
                                            <button
                                                type="button"
                                                className={styles.pfCopy}
                                                title={sshCmd(f)}
                                                onClick={() => void copyCmd(f)}
                                            >
                                                <Copy size={12} />
                                                {t("tools.forwards.copyCmd")}
                                            </button>
                                        </div>
                                    </div>
                                    <span className={`${styles.pfStatus} ${statusCls}`}>
                                        <span className={styles.pfStatusDot} />
                                        {stateLabel(f.state)}
                                    </span>
                                    <span className={styles.pfMetric}>
                                        {f.active > 0
                                            ? t("tools.forwards.conns", { n: String(f.active) })
                                            : "—"}
                                    </span>
                                    <div className={styles.pfActs}>
                                        <button
                                            type="button"
                                            className={styles.actBtn}
                                            title={t("tools.forwards.edit")}
                                            onClick={() => startEdit(f)}
                                        >
                                            <Pencil size={15} />
                                        </button>
                                        <button
                                            type="button"
                                            role="switch"
                                            aria-checked={running}
                                            className={`${styles.pfSwitch} ${running ? styles.pfSwitchOn : ""}`}
                                            title={t(running ? "tools.forwards.stop" : "tools.forwards.run")}
                                            onClick={() =>
                                                void (running ? stop(f.forward_id) : run(f.forward_id))
                                            }
                                        />
                                        <button
                                            type="button"
                                            className={styles.actBtnDanger}
                                            title={t("tools.forwards.delete")}
                                            onClick={() => void del(f.forward_id)}
                                        >
                                            <Trash2 size={15} />
                                        </button>
                                    </div>
                                </div>
                            );
                        })}
                    </div>
                )}
            </div>
        </>
    );
}

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
