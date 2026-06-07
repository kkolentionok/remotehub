import { useEffect, useState, type ReactNode } from "react";
import {
    AlertCircle,
    Check,
    Eye,
    EyeOff,
    Folder,
    KeyRound,
    Loader2,
    LogOut,
    RefreshCw,
    Server,
    ShieldCheck,
    UserCircle2,
} from "lucide-react";

import { useT } from "../../../i18n";
import { sync } from "../../../lib/ipc";
import {
    formatApiError,
    type SyncConfigResponse,
    type SyncNowResponse,
} from "../../../lib/types";
import {
    useCredentialsStore,
    useGroupsStore,
    useHostsStore,
} from "../../../store";
import dlg from "../SettingsDialog.module.css";
import s from "./ProfileSection.module.css";

/**
 * Account & Sync (the former "Profile" placeholder). Two states:
 *   - signed out → set the server endpoint + email/password, log in or create
 *     an account (the bearer token is stored in the OS keychain by the backend).
 *   - signed in → enter the *vault* password (the E2E key, same one used for
 *     encrypted export — never sent to the server) and "Sync now". Status shows
 *     idle / syncing / merged-or-first-push with per-type counts.
 *
 * Endpoint/email/account-password and the vault password are deliberately
 * separate: the account password authenticates to the server; the vault
 * password seals the data the server can never read.
 */
export function ProfileSection() {
    const { t } = useT();

    const [cfg, setCfg] = useState<SyncConfigResponse | null>(null);
    const [loading, setLoading] = useState(true);

    // signed-out form
    const [endpoint, setEndpoint] = useState("");
    const [email, setEmail] = useState("");
    const [password, setPassword] = useState("");
    const [authBusy, setAuthBusy] = useState<"login" | "register" | null>(null);
    const [authError, setAuthError] = useState<string | null>(null);

    // sync
    const [master, setMaster] = useState("");
    const [phase, setPhase] = useState<"idle" | "working" | "done" | "error">("idle");
    const [result, setResult] = useState<SyncNowResponse | null>(null);
    const [syncError, setSyncError] = useState<string | null>(null);

    useEffect(() => {
        let cancelled = false;
        void (async () => {
            try {
                const c = await sync.getConfig();
                if (cancelled) return;
                setCfg(c);
                setEndpoint(c.endpoint);
                setEmail(c.email ?? "");
            } catch (e: unknown) {
                if (!cancelled) setAuthError(formatApiError(e));
            } finally {
                if (!cancelled) setLoading(false);
            }
        })();
        return () => {
            cancelled = true;
        };
    }, []);

    const trimmedEndpoint = endpoint.trim();
    const canAuth = !!trimmedEndpoint && !!email.trim() && !!password && !authBusy;

    async function refresh() {
        const c = await sync.getConfig();
        setCfg(c);
        setEmail(c.email ?? "");
    }

    async function doAuth(register: boolean) {
        if (!canAuth) return;
        setAuthBusy(register ? "register" : "login");
        setAuthError(null);
        try {
            await sync.setEndpoint(trimmedEndpoint);
            if (register) await sync.register(email.trim(), password);
            await sync.login(email.trim(), password);
            setPassword("");
            await refresh();
        } catch (e: unknown) {
            setAuthError(formatApiError(e));
        } finally {
            setAuthBusy(null);
        }
    }

    async function doLogout() {
        await sync.logout();
        setMaster("");
        setPhase("idle");
        setResult(null);
        setSyncError(null);
        await refresh();
    }

    async function doSync() {
        if (!master || phase === "working") return;
        setPhase("working");
        setSyncError(null);
        try {
            const r = await sync.now(master);
            setResult(r);
            setPhase("done");
            // apply_snapshot may have changed local data — refetch collections.
            await Promise.all([
                useHostsStore.getState().load(),
                useGroupsStore.getState().load(),
                useCredentialsStore.getState().load(),
            ]);
        } catch (e: unknown) {
            setSyncError(formatApiError(e));
            setPhase("error");
        }
    }

    if (loading) {
        return <div className={dlg.loading}>{t("common.loading")}</div>;
    }

    return (
        <div className={dlg.section}>
            <div className={dlg.sectionTitle}>{t("settings.sync.title")}</div>
            <p className={dlg.sectionDescription}>{t("settings.sync.lead")}</p>

            {cfg?.logged_in ? (
                <div className={s.flow}>
                    {/* signed-in identity card */}
                    <div className={s.idCard}>
                        <span className={s.idIc}>
                            <UserCircle2 size={22} />
                        </span>
                        <div className={s.idMeta}>
                            <div className={s.idName}>{cfg.email ?? t("settings.sync.signedIn")}</div>
                            <div className={s.idSub}>
                                <Server size={11} /> {cfg.endpoint}
                            </div>
                        </div>
                        <button className={`${s.btn} ${s.btnGhost}`} onClick={() => void doLogout()}>
                            <LogOut size={15} /> {t("settings.sync.logout")}
                        </button>
                    </div>

                    {/* sync now */}
                    <div className={s.field}>
                        <div className={s.fieldL}>
                            <KeyRound size={12} /> {t("settings.sync.vaultPassword")}
                        </div>
                        <Pw
                            value={master}
                            onChange={(v) => {
                                setMaster(v);
                                if (phase === "error") setPhase("idle");
                            }}
                            placeholder={t("settings.sync.vaultPasswordPlaceholder")}
                            onEnter={() => void doSync()}
                            err={phase === "error"}
                        />
                        <div className={s.hint}>{t("settings.sync.vaultPasswordHint")}</div>
                    </div>

                    <div className={s.actions}>
                        <button
                            className={`${s.btn} ${s.btnPrimary}`}
                            disabled={!master || phase === "working"}
                            onClick={() => void doSync()}
                        >
                            {phase === "working" ? (
                                <>
                                    <Loader2 size={16} className={s.spin} /> {t("settings.sync.syncing")}
                                </>
                            ) : (
                                <>
                                    <RefreshCw size={15} /> {t("settings.sync.syncNow")}
                                </>
                            )}
                        </button>
                    </div>

                    {phase === "error" && syncError && (
                        <div className={s.errLine}>
                            <AlertCircle size={14} /> {syncError}
                        </div>
                    )}

                    {phase === "done" && result && (
                        <div className={s.result}>
                            <div className={s.resultHead}>
                                <span className={s.resultBadge}>
                                    <Check size={16} />
                                </span>
                                <div>
                                    <div className={s.resultT}>{t("settings.sync.syncedTitle")}</div>
                                    <div className={s.resultS}>
                                        {result.had_remote
                                            ? t("settings.sync.syncedMerged")
                                            : t("settings.sync.syncedFirst")}
                                    </div>
                                </div>
                                <span className={s.ver}>v{result.pushed_version}</span>
                            </div>
                            <div className={s.prev}>
                                <Row icon={<Server size={15} />} label={t("settings.sync.rowHosts")} n={result.hosts} />
                                <Row icon={<Folder size={15} />} label={t("settings.sync.rowGroups")} n={result.groups} />
                                <Row
                                    icon={<KeyRound size={15} />}
                                    label={t("settings.sync.rowCreds")}
                                    n={result.credentials}
                                />
                            </div>
                        </div>
                    )}
                </div>
            ) : (
                <div className={s.flow}>
                    <div className={s.field}>
                        <div className={s.fieldL}>
                            <Server size={12} /> {t("settings.sync.endpoint")}
                        </div>
                        <input
                            className={`${s.input} ${s.mono}`}
                            value={endpoint}
                            onChange={(e) => {
                                setEndpoint(e.target.value);
                                setAuthError(null);
                            }}
                            placeholder={t("settings.sync.endpointPlaceholder")}
                            spellCheck={false}
                            autoCapitalize="off"
                        />
                    </div>

                    <div className={s.field}>
                        <div className={s.fieldL}>
                            <UserCircle2 size={12} /> {t("settings.sync.email")}
                        </div>
                        <input
                            className={s.input}
                            type="email"
                            value={email}
                            onChange={(e) => {
                                setEmail(e.target.value);
                                setAuthError(null);
                            }}
                            placeholder={t("settings.sync.emailPlaceholder")}
                            spellCheck={false}
                            autoCapitalize="off"
                            autoComplete="username"
                        />
                    </div>

                    <div className={s.field}>
                        <div className={s.fieldL}>
                            <KeyRound size={12} /> {t("settings.sync.password")}
                        </div>
                        <Pw
                            value={password}
                            onChange={(v) => {
                                setPassword(v);
                                setAuthError(null);
                            }}
                            placeholder={t("settings.sync.passwordPlaceholder")}
                            onEnter={() => void doAuth(false)}
                            err={!!authError}
                        />
                    </div>

                    {authError && (
                        <div className={s.errLine}>
                            <AlertCircle size={14} /> {authError}
                        </div>
                    )}

                    <div className={s.actions}>
                        <button
                            className={`${s.btn} ${s.btnPrimary}`}
                            disabled={!canAuth}
                            onClick={() => void doAuth(false)}
                        >
                            {authBusy === "login" ? (
                                <>
                                    <Loader2 size={16} className={s.spin} /> {t("settings.sync.loggingIn")}
                                </>
                            ) : (
                                <>
                                    <ShieldCheck size={15} /> {t("settings.sync.login")}
                                </>
                            )}
                        </button>
                        <button
                            className={`${s.btn} ${s.btnGhost}`}
                            disabled={!canAuth}
                            onClick={() => void doAuth(true)}
                        >
                            {authBusy === "register" ? (
                                <>
                                    <Loader2 size={16} className={s.spin} /> {t("settings.sync.registering")}
                                </>
                            ) : (
                                t("settings.sync.register")
                            )}
                        </button>
                    </div>
                </div>
            )}
        </div>
    );
}

function Row({ icon, label, n }: { icon: ReactNode; label: string; n: number }) {
    return (
        <div className={s.prevRow}>
            <span className={s.prevIc}>{icon}</span>
            <span className={s.prevNm}>{label}</span>
            <span className={s.prevN}>{n}</span>
        </div>
    );
}

function Pw({
    value,
    onChange,
    placeholder,
    onEnter,
    err,
}: {
    value: string;
    onChange: (v: string) => void;
    placeholder: string;
    onEnter?: () => void;
    err?: boolean;
}) {
    const { t } = useT();
    const [show, setShow] = useState(false);
    return (
        <div className={s.pw}>
            <input
                className={`${s.input} ${s.mono} ${err ? s.inputErr : ""}`}
                type={show ? "text" : "password"}
                value={value}
                onChange={(e) => onChange(e.target.value)}
                placeholder={placeholder}
                spellCheck={false}
                autoComplete="new-password"
                onKeyDown={(e) => {
                    if (e.key === "Enter" && onEnter) onEnter();
                }}
            />
            <button
                type="button"
                tabIndex={-1}
                className={s.pwEye}
                onClick={() => setShow((v) => !v)}
                title={show ? t("common.hide") : t("common.show")}
            >
                {show ? <EyeOff size={15} /> : <Eye size={15} />}
            </button>
        </div>
    );
}
