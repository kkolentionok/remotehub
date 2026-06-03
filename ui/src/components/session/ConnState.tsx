import { useState, type ReactNode } from "react";
import {
    AlertCircle,
    Check,
    ChevronRight,
    Copy,
    KeyRound,
    Loader2,
    Server,
    X,
} from "lucide-react";

import { useT } from "../../i18n";
import type { MessageKey } from "../../i18n/en";
import type { SessionState } from "../../lib/types";
import styles from "./ConnState.module.css";

/** Connection-state taxonomy. `connecting` drives the live handshake; the rest
 *  are terminal presentations of a failed/awaiting connection. */
export type ConnCategory =
    | "connecting"
    | "timeout"
    | "refused"
    | "dns"
    | "network"
    | "generic"
    | "auth"
    | "badpass"
    | "hostkey";

/** Locale-independent failure markers (Winsock codes + io::ErrorKind phrases +
 *  the SSH `auth_failed (method)` message) → a category. The OS message itself
 *  is localized, so we never match on its prose. `hostKey` present means a TOFU
 *  decision is pending → host-key screen. */
export function connCategory(
    _state: SessionState,
    message: string | null,
    hostKeyPending: boolean,
    authMethod?: string | null,
): ConnCategory {
    if (hostKeyPending) return "hostkey";
    // Prefer the explicit method from the `auth_failed` event — reliable even
    // if a later close/error event clobbered the message string.
    if (authMethod) {
        return authMethod.toLowerCase().includes("password") ? "badpass" : "auth";
    }
    const m = (message ?? "").toLowerCase();
    if (
        m.includes("auth failed") ||
        m.includes("authentication failed") ||
        m.includes("permission denied") ||
        m.includes("auth ")
    ) {
        return m.includes("password") ? "badpass" : "auth";
    }
    if (m.includes("10060") || m.includes("timed out") || m.includes("timeout"))
        return "timeout";
    if (m.includes("10061") || m.includes("refused")) return "refused";
    if (
        m.includes("11001") ||
        m.includes("11004") ||
        m.includes("no such host") ||
        m.includes("not known") ||
        m.includes("failed to lookup") ||
        m.includes("resolve") ||
        m.includes("dns")
    )
        return "dns";
    if (
        m.includes("10051") ||
        m.includes("10065") ||
        m.includes("unreachable") ||
        m.includes("network is")
    )
        return "network";
    return "generic";
}

type StepState = "done" | "active" | "pend" | "fail";

function StepIcon({ s }: { s: StepState }) {
    if (s === "done")
        return (
            <span className={styles.hstepIc}>
                <Check size={15} strokeWidth={2.6} />
            </span>
        );
    if (s === "active")
        return (
            <span className={styles.hstepIc}>
                <span className={styles.spin}>
                    <Loader2 size={14} />
                </span>
            </span>
        );
    if (s === "fail")
        return (
            <span className={styles.hstepIc}>
                <X size={15} strokeWidth={2.6} />
            </span>
        );
    return (
        <span className={styles.hstepIc}>
            <span className={styles.pdot} />
        </span>
    );
}

const STEP_CLASS: Record<StepState, string> = {
    done: styles.stepDone ?? "",
    active: styles.stepActive ?? "",
    pend: styles.stepPend ?? "",
    fail: styles.stepFail ?? "",
};

function Step({ s, children }: { s: StepState; children: ReactNode }) {
    return (
        <div className={`${styles.hstep} ${STEP_CLASS[s]}`}>
            <StepIcon s={s} />
            <span className={styles.hstepT}>{children}</span>
        </div>
    );
}

export interface ConnStateProps {
    category: ConnCategory;
    state: SessionState;
    protocol: "ssh" | "rdp";
    hostName: string;
    user: string;
    addr: string;
    port: number | string;
    /** Raw error / detail message (shown verbatim under "technical details"). */
    rawMessage?: string | null;
    /** host-key only: the new key differs from the pinned one (warning). */
    changed?: boolean;
    fingerprint?: string | null;
    keyType?: string | null;
    /** Inline re-auth panel (auth/badpass) — rendered between the diagnosis
     *  and the technical details, per the design. */
    reauthSlot?: ReactNode;
    /** Consecutive attempt count for this host (shown as a badge). */
    attempt?: number;
    /** Action buttons, composed by the caller (reconnect / edit / accept …). */
    children?: ReactNode;
}

export function ConnState(props: ConnStateProps) {
    const { t } = useT();
    const [detOpen, setDetOpen] = useState(false);
    const [copied, setCopied] = useState(false);

    const { category: cat, state, protocol, hostName, user, addr, port } = props;
    const isHostkey = cat === "hostkey";
    const isErr = cat !== "connecting";
    const kindClass = !isErr
        ? ""
        : isHostkey
          ? styles.warn
          : styles.fail;

    const I = { addr, port: String(port), user };

    // ─── handshake steps ───
    const sResolve = t("conn.step.resolve");
    const sConnect = t("conn.step.connect");
    const sAuth = t("conn.step.auth");
    const sSession = t("conn.step.session");

    function authBase(): string {
        if (cat === "badpass") return `${sAuth} · ${t("conn.byPassword")}`;
        return sAuth;
    }

    let steps: ReactNode;
    if (cat === "connecting") {
        const r: StepState = "done";
        const c: StepState =
            state === "resolving" ? "pend" : state === "connecting" ? "active" : "done";
        const a: StepState = state === "authenticating" ? "active" : "pend";
        const resolveState: StepState = state === "resolving" ? "active" : r;
        steps = (
            <>
                <Step s={resolveState}>{`${sResolve} · ${addr}`}</Step>
                <Step s={c}>{`${sConnect} · :${port}`}</Step>
                <Step s={a}>{authBase()}</Step>
                <Step s="pend">{sSession}</Step>
            </>
        );
    } else if (cat === "dns") {
        steps = (
            <>
                <Step s="fail">{`${sResolve} · ${addr} — ${t("conn.tail.dns")}`}</Step>
                <Step s="pend">{`${sConnect} · :${port}`}</Step>
                <Step s="pend">{sAuth}</Step>
                <Step s="pend">{sSession}</Step>
            </>
        );
    } else if (cat === "auth" || cat === "badpass" || isHostkey) {
        const tail = isHostkey
            ? props.changed
                ? t("conn.tail.hostkeyChanged")
                : t("conn.tail.hostkeyNew")
            : cat === "badpass"
              ? t("conn.tail.badpass")
              : t("conn.tail.authKey");
        steps = (
            <>
                <Step s="done">{`${sResolve} · ${addr}`}</Step>
                <Step s="done">{`${sConnect} · :${port}`}</Step>
                <Step s="fail">{`${authBase()} — ${tail}`}</Step>
                <Step s="pend">{sSession}</Step>
            </>
        );
    } else {
        // timeout / refused / network / generic — fail at TCP connect
        const tail =
            cat === "timeout"
                ? t("conn.tail.timeout")
                : cat === "refused"
                  ? t("conn.tail.refused")
                  : cat === "network"
                    ? t("conn.tail.network")
                    : t("conn.tail.generic");
        steps = (
            <>
                <Step s="done">{`${sResolve} · ${addr}`}</Step>
                <Step s="fail">{`${sConnect} · :${port} — ${tail}`}</Step>
                <Step s="pend">{sAuth}</Step>
                <Step s="pend">{sSession}</Step>
            </>
        );
    }

    // ─── headline ───
    const headline = !isErr
        ? null
        : isHostkey
          ? props.changed
              ? t("conn.head.hostkey")
              : t("conn.head.hostkeyNew")
          : t(`conn.head.${cat}` as MessageKey);

    // ─── diagnosis ───
    const diagKey = isHostkey
        ? props.changed
            ? "conn.diag.hostkey"
            : "conn.diag.hostkeyNew"
        : `conn.diag.${cat}`;
    const diagnosis = isErr ? t(diagKey as MessageKey, I) : null;

    // ─── fixes checklist (not for hostkey / generic) ───
    const fixes: ReactNode[] = [];
    if (cat === "timeout") {
        fixes.push(
            t("conn.fix.timeout.vpn"),
            <>
                {t("conn.fix.timeout.port")} <code>nc -vz {addr} {port}</code>
            </>,
            t("conn.fix.timeout.power"),
        );
    } else if (cat === "refused") {
        fixes.push(
            <>
                {t("conn.fix.refused.svc")} <code>systemctl status sshd</code>
            </>,
            t("conn.fix.refused.port"),
        );
    } else if (cat === "dns") {
        fixes.push(t("conn.fix.dns.name"), t("conn.fix.dns.dns"));
    } else if (cat === "network") {
        fixes.push(t("conn.fix.network.route"), t("conn.fix.network.iface"));
    }

    // ─── raw technical detail ───
    const raw = isHostkey
        ? `HostKeyError {\n  algo: ${props.keyType ?? "?"},\n  fingerprint: SHA256:${props.fingerprint ?? "?"},\n  changed: ${props.changed ? "true" : "false"} }`
        : cat === "auth" || cat === "badpass"
          ? `AuthError {\n  method: ${cat === "badpass" ? "password" : "publickey"},\n  user: ${user || "?"},\n  message: ${JSON.stringify(props.rawMessage ?? "")} }`
          : (props.rawMessage ?? "");

    const copyReport = () => {
        setCopied(true);
        setTimeout(() => setCopied(false), 1600);
        try {
            void navigator.clipboard?.writeText(
                `${hostName} ${user}@${addr}:${port}\n${raw}`,
            );
        } catch {
            /* clipboard unavailable — non-fatal */
        }
    };

    return (
        <div className={styles.stage}>
            <div className={`${styles.card} ${kindClass}`}>
                {/* identity */}
                <div className={styles.id}>
                    <div className={styles.tile}>
                        <Server size={22} />
                    </div>
                    <div style={{ minWidth: 0 }}>
                        <div className={styles.name}>{hostName}</div>
                        <div className={styles.addr}>
                            <span
                                className={`${styles.proto} ${protocol === "ssh" ? styles.protoSsh : styles.protoRdp}`}
                            >
                                {protocol.toUpperCase()}
                            </span>
                            {user ? `${user}@` : ""}
                            {addr}:{port}
                        </div>
                    </div>
                </div>

                {/* connecting: indeterminate bar */}
                {cat === "connecting" && <div className={styles.ind} />}

                {/* error / warning headline */}
                {headline && (
                    <div className={styles.head}>
                        <span className={styles.sdot} />
                        {headline}
                    </div>
                )}

                {/* handshake log */}
                <div className={styles.hs}>{steps}</div>

                {/* diagnosis */}
                {diagnosis && (
                    <div className={styles.diag}>
                        <span className={styles.diagIc}>
                            {isHostkey ? (
                                <KeyRound size={18} />
                            ) : (
                                <AlertCircle size={18} />
                            )}
                        </span>
                        <div>
                            <p>{diagnosis}</p>
                        </div>
                    </div>
                )}

                {/* fixes */}
                {fixes.length > 0 && (
                    <div className={styles.fixes}>
                        <div className={styles.fixesH}>{t("conn.fixesTitle")}</div>
                        {fixes.map((f, i) => (
                            <div className={styles.fix} key={i}>
                                <span className={styles.fixN}>{i + 1}</span>
                                <span>{f}</span>
                            </div>
                        ))}
                    </div>
                )}

                {/* inline re-auth panel (auth / badpass) */}
                {props.reauthSlot}

                {/* technical details */}
                {isErr && raw && (
                    <div className={`${styles.det} ${detOpen ? styles.detOpen : ""}`}>
                        <button
                            type="button"
                            className={styles.detH}
                            onClick={() => setDetOpen((o) => !o)}
                        >
                            <span className={styles.chev}>
                                <ChevronRight size={13} />
                            </span>
                            {t("conn.details")}
                            <span className={styles.detSp} />
                            <span
                                className={styles.detCp}
                                onClick={(e) => {
                                    e.stopPropagation();
                                    copyReport();
                                }}
                            >
                                {copied ? <Check size={12} /> : <Copy size={12} />}
                                {copied ? t("conn.copied") : t("conn.copy")}
                            </span>
                        </button>
                        <div className={styles.detB}>
                            <div className={styles.detCode}>{raw}</div>
                        </div>
                    </div>
                )}

                {/* actions (composed by caller) */}
                <div className={styles.act}>
                    {props.children}
                    {props.attempt != null && props.attempt > 1 && (
                        <span className={styles.retry}>
                            {t("conn.attempt", { n: props.attempt })}
                        </span>
                    )}
                </div>
            </div>
        </div>
    );
}
