import { useEffect, useState } from "react";
import {
    AlertCircle,
    Check,
    Eye,
    EyeOff,
    KeyRound,
    Loader2,
    LogIn,
    LogOut,
    Server,
    ShieldCheck,
    UserCircle2,
} from "lucide-react";

import { useT } from "../../../i18n";
import { sync } from "../../../lib/ipc";
import { formatApiError, type SyncConfigResponse } from "../../../lib/types";
import { useUiStore } from "../../../store";
import dlg from "../SettingsDialog.module.css";
import s from "./ProfileSection.module.css";

/**
 * Account & Sync. Two states:
 *   - signed out → set the server endpoint + email/password, log in / create an
 *     account, or sign in with Yandex (the bearer token is stored in the OS
 *     keychain by the backend).
 *   - signed in → sync is fully automatic. We only show the live status; the
 *     vault (master) password is entered once via a modal (prompted on sign-in
 *     and each launch until set) and cached in the keychain. No "Sync now".
 *
 * The account password authenticates to the server; the vault password seals
 * the data the server can never read — they are deliberately separate.
 */
export function ProfileSection() {
    const { t } = useT();
    const setDialog = useUiStore((st) => st.setDialog);
    const syncStatus = useUiStore((st) => st.syncStatus);

    const [cfg, setCfg] = useState<SyncConfigResponse | null>(null);
    const [loading, setLoading] = useState(true);

    // signed-out form
    const [endpoint, setEndpoint] = useState("");
    const [email, setEmail] = useState("");
    const [password, setPassword] = useState("");
    const [authBusy, setAuthBusy] = useState<"login" | "register" | "yandex" | null>(null);
    const [authError, setAuthError] = useState<string | null>(null);

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
    const canYandex = !!trimmedEndpoint && !authBusy;

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
            // Prompt for the vault password immediately after sign-in.
            const c = await sync.getConfig();
            if (c.logged_in && !c.has_master) setDialog({ kind: "sync-master", mode: "set" });
        } catch (e: unknown) {
            setAuthError(formatApiError(e));
        } finally {
            setAuthBusy(null);
        }
    }

    async function doYandex() {
        if (!canYandex) return;
        setAuthBusy("yandex");
        setAuthError(null);
        try {
            await sync.setEndpoint(trimmedEndpoint);
            const c = await sync.oauthYandex();
            setCfg(c);
            setEmail(c.email ?? "");
            setPassword("");
            if (c.logged_in && !c.has_master) setDialog({ kind: "sync-master", mode: "set" });
        } catch (e: unknown) {
            setAuthError(formatApiError(e));
        } finally {
            setAuthBusy(null);
        }
    }

    async function doLogout() {
        await sync.logout();
        await refresh();
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
                            <div className={s.idName}>
                                {cfg.email ?? t("settings.sync.signedIn")}
                            </div>
                            <div className={s.idSub}>
                                <Server size={11} /> {cfg.endpoint}
                            </div>
                        </div>
                        <button className={`${s.btn} ${s.btnGhost}`} onClick={() => void doLogout()}>
                            <LogOut size={15} /> {t("settings.sync.logout")}
                        </button>
                    </div>

                    {/* auto-sync status (or a prompt to set the vault password) */}
                    {!cfg.has_master ? (
                        <div className={s.idCard}>
                            <span className={s.idIc}>
                                <KeyRound size={20} />
                            </span>
                            <div className={s.idMeta}>
                                <div className={s.idName}>{t("settings.sync.setMaster")}</div>
                                <div className={s.idSub}>{t("settings.sync.masterNeeded")}</div>
                            </div>
                            <button
                                className={`${s.btn} ${s.btnPrimary}`}
                                onClick={() => setDialog({ kind: "sync-master", mode: "set" })}
                            >
                                {t("settings.sync.setMaster")}
                            </button>
                        </div>
                    ) : (
                        <SyncStatusCard
                            state={syncStatus?.state ?? "idle"}
                            atMs={syncStatus?.at_ms ?? null}
                            message={syncStatus?.message ?? null}
                            onFix={() => setDialog({ kind: "sync-master", mode: "fix" })}
                        />
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
                                    <Loader2 size={16} className={s.spin} />{" "}
                                    {t("settings.sync.loggingIn")}
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
                                    <Loader2 size={16} className={s.spin} />{" "}
                                    {t("settings.sync.registering")}
                                </>
                            ) : (
                                t("settings.sync.register")
                            )}
                        </button>
                    </div>

                    <div className={s.orRow}>
                        <span>{t("settings.sync.or")}</span>
                    </div>
                    <button
                        className={`${s.btn} ${s.btnGhost} ${s.btnWide}`}
                        disabled={!canYandex}
                        onClick={() => void doYandex()}
                    >
                        {authBusy === "yandex" ? (
                            <>
                                <Loader2 size={16} className={s.spin} />{" "}
                                {t("settings.sync.yandexBusy")}
                            </>
                        ) : (
                            <>
                                <LogIn size={15} /> {t("settings.sync.yandex")}
                            </>
                        )}
                    </button>
                </div>
            )}
        </div>
    );
}

function SyncStatusCard({
    state,
    atMs,
    message,
    onFix,
}: {
    state: string;
    atMs: number | null;
    message: string | null;
    onFix: () => void;
}) {
    const { t } = useT();
    const time = atMs ? new Date(atMs).toLocaleTimeString() : null;

    let icon = <ShieldCheck size={20} />;
    let title = t("settings.sync.statusOn");
    let sub: string | null = null;

    if (state === "syncing") {
        icon = <Loader2 size={20} className={s.spin} />;
        title = t("settings.sync.statusSyncing");
    } else if (state === "ok") {
        icon = <Check size={20} />;
        title = t("settings.sync.statusSynced");
        sub = time;
    } else if (state === "error") {
        icon = <AlertCircle size={20} />;
        title = t("settings.sync.statusError");
        sub = message;
    }

    return (
        <div className={s.idCard}>
            <span className={s.idIc}>{icon}</span>
            <div className={s.idMeta}>
                <div className={s.idName}>{title}</div>
                {sub && <div className={s.idSub}>{sub}</div>}
            </div>
            {state === "error" && (
                <button className={`${s.btn} ${s.btnGhost}`} onClick={onFix}>
                    {t("settings.sync.fix")}
                </button>
            )}
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
