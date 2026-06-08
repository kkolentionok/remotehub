import { useEffect, useMemo, useState } from "react";
import {
    ArrowRightLeft,
    ArrowUpRight,
    ChevronRight,
    Copy,
    Download,
    Eye,
    EyeOff,
    FolderOpen,
    HardDrive,
    KeyRound,
    Lock,
    Play,
    Search,
    Server,
    Share2,
    Square,
    Terminal,
    Trash2,
} from "lucide-react";

import { useT } from "../../i18n";
import { credentials as credApi, forwards as forwardsApi, hosts as hostsApi } from "../../lib/ipc";
import type { CredentialDto, CredentialKind, ForwardKind, ForwardSummary, HostDto } from "../../lib/types";
import { formatApiError } from "../../lib/types";
import { useCredentialsStore, useHostsStore, useSessionsStore, useUiStore } from "../../store";
import { Combobox, type ComboboxOption } from "../ui/Combobox";
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
    { id: "forwards", icon: ArrowRightLeft, label: "tools.section.forwards", soon: false },
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
    const toolsSection = useUiStore((s) => s.toolsSection);
    const setToolsSection = useUiStore((s) => s.setToolsSection);
    // A tray click (or other external nav) requests a sub-section; apply it
    // once, then clear the request so it doesn't re-fire.
    useEffect(() => {
        if (!toolsSection) return;
        const valid: ManageKey[] = ["keys", "import", "forwards", "mounts", "share"];
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
                // Show the address alongside the name, e.g. "richard-tea.com
                // [92.63.193.34]". Skip the brackets when the name *is* the
                // address (host saved by IP) to avoid "IP [IP]".
                const label =
                    name && h.hostname && name !== h.hostname
                        ? `${name} [${h.hostname}]`
                        : name || h.hostname;
                return { value: h.id, label };
            }),
        [sshHosts],
    );

    const [list, setList] = useState<ForwardSummary[]>([]);
    const [hostId, setHostId] = useState("");
    const [kind, setKind] = useState<ForwardKind>("local");
    const [bindHost, setBindHost] = useState("127.0.0.1");
    const [bindPort, setBindPort] = useState("");
    const [targetHost, setTargetHost] = useState("");
    const [targetPort, setTargetPort] = useState("");
    const [busy, setBusy] = useState(false);

    // Poll the live list (cheap; also recovers forwards that survived a
    // webview reload, which have no live event channel anymore).
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

    const bp = Number(bindPort);
    const tp = Number(targetPort);
    const isDyn = kind === "dynamic";
    const portOk = (n: number) => Number.isInteger(n) && n >= 1 && n <= 65535;
    const valid =
        hostId !== "" &&
        portOk(bp) &&
        (isDyn || (targetHost.trim() !== "" && portOk(tp)));

    const start = async () => {
        if (!valid || busy) return;
        setBusy(true);
        try {
            await forwardsApi.open(
                {
                    host_id: hostId,
                    kind,
                    bind_host: bindHost.trim() || undefined,
                    bind_port: bp,
                    target_host: isDyn ? "" : targetHost.trim(),
                    target_port: isDyn ? 0 : tp,
                },
                (e) => {
                    // The actor binds/connects asynchronously, so failures
                    // (e.g. a privileged/blocked local port) arrive here as an
                    // error event — surface them instead of the row just
                    // vanishing on the next refresh.
                    if (e.kind === "error") onToast(e.message);
                    refresh();
                },
            );
            setBindPort("");
            setTargetPort("");
            onToast(t("tools.forwards.started"));
            refresh();
        } catch (e: unknown) {
            onToast(formatApiError(e));
        } finally {
            setBusy(false);
        }
    };

    const stop = async (fid: string) => {
        try {
            await forwardsApi.close(fid);
            onToast(t("tools.forwards.stopped"));
            refresh();
        } catch (e: unknown) {
            onToast(formatApiError(e));
        }
    };

    const stateLabel = (s: ForwardSummary["state"]): string =>
        t(
            (s === "listening"
                ? "tools.forwards.state.listening"
                : s === "connecting"
                  ? "tools.forwards.state.connecting"
                  : s === "error"
                    ? "tools.forwards.state.error"
                    : "tools.forwards.state.closed") as Parameters<typeof t>[0],
        );
    const stateCls = (s: ForwardSummary["state"]): string => {
        const c =
            s === "listening"
                ? styles.fwOk
                : s === "error"
                  ? styles.fwErr
                  : s === "closed"
                    ? styles.fwClosed
                    : styles.fwConn;
        return c ?? "";
    };

    return (
        <>
            <div className={styles.paneHead}>
                <div className={styles.paneTitleRow}>
                    <div className={styles.paneTitle}>
                        <ArrowRightLeft size={18} className={styles.paneTitleIc} />
                        {t("tools.section.forwards")}
                    </div>
                    <div className={styles.paneSp} />
                </div>
                <div className={styles.paneSub}>{t("tools.forwards.sub")}</div>

                <div className={styles.fwForm}>
                    <div className={styles.fwKind}>
                        {(["local", "remote", "dynamic"] as ForwardKind[]).map((k) => (
                            <button
                                key={k}
                                type="button"
                                className={`${styles.fwKindBtn} ${kind === k ? styles.fwKindOn : ""}`}
                                onClick={() => setKind(k)}
                                title={t(`tools.forwards.kind.${k}.hint` as Parameters<typeof t>[0])}
                            >
                                {t(`tools.forwards.kind.${k}` as Parameters<typeof t>[0])}
                            </button>
                        ))}
                    </div>
                    <div className={styles.fwHost}>
                        <Combobox
                            options={hostOptions}
                            value={hostId}
                            onChange={setHostId}
                            placeholder={t("tools.forwards.hostPlaceholder")}
                            clearable
                        />
                    </div>
                    <ChevronRight size={15} className={styles.fwSep} />
                    {!isDyn && (
                        <>
                            <input
                                className={styles.fwTarget}
                                value={targetHost}
                                onChange={(e) => setTargetHost(e.target.value)}
                                placeholder={t(
                                    kind === "remote"
                                        ? "tools.forwards.localHost"
                                        : "tools.forwards.remoteHost",
                                )}
                                spellCheck={false}
                            />
                            <span className={styles.fwColon}>:</span>
                            <input
                                className={styles.fwPort}
                                value={targetPort}
                                onChange={(e) => setTargetPort(e.target.value.replace(/[^0-9]/g, ""))}
                                placeholder={t("tools.forwards.targetPort")}
                                inputMode="numeric"
                                spellCheck={false}
                            />
                            <ArrowRightLeft size={15} className={styles.fwArrow} />
                        </>
                    )}
                    {isDyn && <span className={styles.fwDyn}>{t("tools.forwards.socksTag")}</span>}
                    <input
                        className={styles.fwTarget}
                        value={bindHost}
                        onChange={(e) => setBindHost(e.target.value)}
                        placeholder={t(
                            kind === "remote"
                                ? "tools.forwards.serverHost"
                                : "tools.forwards.localHost",
                        )}
                        spellCheck={false}
                    />
                    <span className={styles.fwColon}>:</span>
                    <input
                        className={styles.fwPort}
                        value={bindPort}
                        onChange={(e) => setBindPort(e.target.value.replace(/[^0-9]/g, ""))}
                        placeholder={t(
                            kind === "remote"
                                ? "tools.forwards.remotePort"
                                : "tools.forwards.localPort",
                        )}
                        inputMode="numeric"
                        spellCheck={false}
                    />
                    <button
                        type="button"
                        className={styles.fwStart}
                        disabled={!valid || busy}
                        onClick={() => void start()}
                    >
                        <Play size={14} />
                        {t("tools.forwards.start")}
                    </button>
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
                    <div className={styles.klist}>
                        {list.map((f) => (
                            <div key={f.forward_id} className={`${styles.krow} ${styles.fwRow}`}>
                                <ArrowRightLeft size={16} className={styles.krowIc} />
                                <div className={styles.krowMain}>
                                    <div className={styles.krowNm}>
                                        {f.host_label}
                                        <span className={styles.fwTag}>
                                            {t(
                                                `tools.forwards.kind.${f.spec.kind}` as Parameters<
                                                    typeof t
                                                >[0],
                                            )}
                                        </span>
                                    </div>
                                    <div className={styles.krowSub}>
                                        {f.spec.kind === "dynamic"
                                            ? `${t("tools.forwards.socksTag")} ${f.spec.bind_host}:${f.spec.bind_port}`
                                            : `${f.spec.target_host}:${f.spec.target_port} ⇄ ${f.spec.bind_host}:${f.spec.bind_port}`}
                                    </div>
                                </div>
                                <span className={`${styles.fwState} ${stateCls(f.state)}`}>
                                    {stateLabel(f.state)}
                                </span>
                                <span className={styles.fwActive}>
                                    {f.active > 0
                                        ? t("tools.forwards.active", { n: String(f.active) })
                                        : ""}
                                </span>
                                <div className={styles.krowAct}>
                                    <button
                                        type="button"
                                        className={styles.actBtnDanger}
                                        title={t("tools.forwards.stop")}
                                        onClick={() => void stop(f.forward_id)}
                                    >
                                        <Square size={15} />
                                    </button>
                                </div>
                            </div>
                        ))}
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
