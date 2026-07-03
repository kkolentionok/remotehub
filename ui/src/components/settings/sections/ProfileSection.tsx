import { useEffect, useState, type ReactNode } from "react";
import {
    AlertCircle,
    Check,
    Eye,
    EyeOff,
    Code2,
    Folder,
    KeyRound,
    Loader2,
    LogIn,
    LogOut,
    Pencil,
    Server,
    ShieldCheck,
    UserCircle2,
    WifiOff,
    X,
} from "lucide-react";

import { useT } from "../../../i18n";
import { sync } from "../../../lib/ipc";
import { type SyncConfigResponse, type SyncStatus } from "../../../lib/types";
import { localizeSyncError } from "../../../lib/syncErrors";
import { useUiStore } from "../../../store";
import dlg from "../SettingsDialog.module.css";
import s from "./ProfileSection.module.css";

/**
 * Account & Sync. Signed out → email/password (log in or create) or Yandex.
 * Signed in → automatic sync: a live status card (synced / syncing / error)
 * with per-type counts. No manual "sync now" — the only manual touchpoint is
 * (re)entering the vault password via the modal. Everything is E2E-sealed with
 * the vault password before leaving the device.
 */
export function ProfileSection() {
    const { t } = useT();
    const setDialog = useUiStore((st) => st.setDialog);
    const syncStatus = useUiStore((st) => st.syncStatus);

    const [cfg, setCfg] = useState<SyncConfigResponse | null>(null);
    const [loading, setLoading] = useState(true);

    const [authMode, setAuthMode] = useState<"login" | "register">("login");
    const [endpoint, setEndpoint] = useState("");
    const [editingServer, setEditingServer] = useState(false);
    const [email, setEmail] = useState("");
    const [password, setPassword] = useState("");
    const [confirm, setConfirm] = useState("");
    const [authBusy, setAuthBusy] = useState<"login" | "register" | "yandex" | null>(null);
    const [authError, setAuthError] = useState<string | null>(null);
    const [confirmLogout, setConfirmLogout] = useState(false);
    const [logoutBusy, setLogoutBusy] = useState(false);
    const [logoutError, setLogoutError] = useState<string | null>(null);

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
                if (!cancelled) setAuthError(localizeSyncError(t, e));
            } finally {
                if (!cancelled) setLoading(false);
            }
        })();
        return () => {
            cancelled = true;
        };
    }, []);

    const trimmedEndpoint = endpoint.trim();
    const pwScore = scorePw(password);
    const matches = confirm.length > 0 && confirm === password;
    const canLogin = !!trimmedEndpoint && !!email.trim() && !!password && !authBusy;
    const canRegister = canLogin && matches;
    const canYandex = !!trimmedEndpoint && !authBusy;

    async function refresh() {
        const c = await sync.getConfig();
        setCfg(c);
        setEmail(c.email ?? "");
    }

    async function doAuth() {
        const register = authMode === "register";
        if (register ? !canRegister : !canLogin) return;
        setAuthBusy(register ? "register" : "login");
        setAuthError(null);
        try {
            await sync.setEndpoint(trimmedEndpoint);
            if (register) await sync.register(email.trim(), password);
            await sync.login(email.trim(), password);
            setPassword("");
            setConfirm("");
            await refresh();
            const c = await sync.getConfig();
            if (c.logged_in && !c.has_master) setDialog({ kind: "sync-master", mode: "set" });
        } catch (e: unknown) {
            setAuthError(localizeSyncError(t, e));
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
            setAuthError(localizeSyncError(t, e));
        } finally {
            setAuthBusy(null);
        }
    }

    // Session expired (server token TTL elapsed): the fix is to re-authenticate
    // and overwrite the stale bearer token — NOT to re-enter the vault master
    // password (that's the E2E key, unrelated to the server session). Re-runs
    // the Yandex OAuth flow; the sync engine resumes on its next pass.
    // (Password-account re-auth would route to the login form — follow-up.)
    async function reauth() {
        try {
            await sync.oauthYandex();
            await refresh();
        } catch (e: unknown) {
            setAuthError(localizeSyncError(t, e));
        }
    }

    async function doLogout() {
        setLogoutError(null);
        setLogoutBusy(true);
        try {
            await sync.logout();
            setConfirmLogout(false);
            await refresh();
        } catch {
            // The backend refuses to wipe until local data is confirmed on the
            // server; surface that instead of leaving the user in limbo.
            setLogoutError(t("settings.sync.logoutBlocked"));
        } finally {
            setLogoutBusy(false);
        }
    }

    if (loading) {
        return <div className={dlg.loading}>{t("common.loading")}</div>;
    }

    // OAuth redirect screen — shown while the browser dance is in flight.
    if (authBusy === "yandex") {
        return (
            <div className={dlg.section}>
                <div className={s.oauth}>
                    <div className={s.oauthTile}>
                        <span className={s.oauthRing} />Я
                    </div>
                    <div className={s.oauthTitle}>{t("settings.sync.oauthRedirect")}</div>
                    <div className={s.oauthSub}>{t("settings.sync.oauthHint")}</div>
                    <div className={s.dots}>
                        <span /> <span /> <span />
                    </div>
                </div>
            </div>
        );
    }

    if (cfg?.logged_in) {
        return (
            <div className={dlg.section}>
                <div className={dlg.sectionTitle}>{t("settings.sync.title")}</div>
                <p className={dlg.sectionDescription}>{t("settings.sync.lead")}</p>

                <div className={s.flow}>
                    {/* identity */}
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
                        <button
                            className={`${s.btn} ${s.btnGhost}`}
                            onClick={() => setConfirmLogout(true)}
                        >
                            <LogOut size={15} /> {t("settings.sync.logout")}
                        </button>
                    </div>

                    {confirmLogout && (
                        <div className={s.logoutPlate}>
                            <AlertCircle size={14} />
                            <span>{logoutError ?? t("settings.sync.logoutWarn")}</span>
                            <div className={s.logoutActions}>
                                <button
                                    className={`${s.btn} ${s.btnGhost} ${s.btnSm}`}
                                    disabled={logoutBusy}
                                    onClick={() => {
                                        setConfirmLogout(false);
                                        setLogoutError(null);
                                    }}
                                >
                                    {t("common.cancel")}
                                </button>
                                <button
                                    className={`${s.btn} ${s.btnDanger} ${s.btnSm}`}
                                    disabled={logoutBusy}
                                    onClick={() => void doLogout()}
                                >
                                    <LogOut size={15} />{" "}
                                    {logoutBusy
                                        ? t("settings.sync.loggingOut")
                                        : t("settings.sync.logout")}
                                </button>
                            </div>
                        </div>
                    )}

                    {!cfg.has_master ? (
                        <div className={s.idCard}>
                            <span className={s.idIc}>
                                <KeyRound size={20} />
                            </span>
                            <div className={s.idMeta}>
                                <div className={s.idName}>{t("settings.sync.setMaster")}</div>
                                <div className={s.idSub2}>{t("settings.sync.masterNeeded")}</div>
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
                            status={syncStatus}
                            onFix={() => {
                                const msg = (syncStatus?.message ?? "").toLowerCase();
                                if (msg.includes("unauthorized")) void reauth();
                                else setDialog({ kind: "sync-master", mode: "fix" });
                            }}
                        />
                    )}
                </div>
            </div>
        );
    }

    // ---- signed out: login / register ----------------------------------
    const register = authMode === "register";
    return (
        <div className={dlg.section}>
            <div className={s.authHead}>
                <span className={s.authLogo}>
                    <ShieldCheck size={16} />
                </span>
                {register ? t("settings.sync.registerTitle") : t("settings.sync.loginTitle")}
            </div>

            <div className={s.flow}>
                {/* server chip */}
                <div className={s.serverRow}>
                    {editingServer ? (
                        <input
                            className={`${s.input} ${s.mono}`}
                            value={endpoint}
                            onChange={(e) => setEndpoint(e.target.value)}
                            onBlur={() => setEditingServer(false)}
                            placeholder={t("settings.sync.endpointPlaceholder")}
                            spellCheck={false}
                            autoCapitalize="off"
                            autoFocus
                        />
                    ) : (
                        <button className={s.serverChip} onClick={() => setEditingServer(true)}>
                            <Server size={13} className={s.serverChipIc} />
                            <span className={s.serverChipLbl}>{t("settings.sync.server")}</span>
                            <span className={s.serverChipVal}>
                                {trimmedEndpoint.replace(/^https?:\/\//, "") || "pingie.ru"}
                            </span>
                            <Pencil size={13} className={s.serverChipPen} />
                        </button>
                    )}
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
                        placeholder={
                            register
                                ? t("settings.sync.passwordNewPlaceholder")
                                : t("settings.sync.passwordPlaceholder")
                        }
                        onEnter={() => void doAuth()}
                        err={!!authError}
                    />
                    {register && password.length > 0 && (
                        <div className={s.strength}>
                            <div className={s.strengthTrack}>
                                <div
                                    className={s.strengthFill}
                                    data-score={pwScore}
                                    style={{ width: `${(pwScore / 4) * 100}%` }}
                                />
                            </div>
                            <span className={s.strengthLbl} data-score={pwScore}>
                                {t(`settings.sync.pwStrength${pwScore}` as never)}
                            </span>
                        </div>
                    )}
                </div>

                {register && (
                    <div className={s.field}>
                        <div className={s.fieldL}>
                            <KeyRound size={12} /> {t("settings.sync.passwordRepeat")}
                            {confirm.length > 0 &&
                                (matches ? (
                                    <span className={s.matchOk}>
                                        <Check size={11} /> {t("settings.sync.match")}
                                    </span>
                                ) : (
                                    <span className={s.matchBad}>
                                        <X size={11} /> {t("settings.sync.noMatch")}
                                    </span>
                                ))}
                        </div>
                        <Pw
                            value={confirm}
                            onChange={(v) => setConfirm(v)}
                            placeholder={t("settings.sync.passwordRepeatPlaceholder")}
                            onEnter={() => void doAuth()}
                            err={confirm.length > 0 && !matches}
                        />
                    </div>
                )}

                {authError && (
                    <div className={s.errLine}>
                        <AlertCircle size={14} /> {authError}
                    </div>
                )}

                <button
                    className={`${s.btn} ${s.btnPrimary} ${s.btnWide}`}
                    disabled={register ? !canRegister : !canLogin}
                    onClick={() => void doAuth()}
                >
                    {authBusy ? (
                        <>
                            <Loader2 size={16} className={s.spin} />{" "}
                            {register
                                ? t("settings.sync.registering")
                                : t("settings.sync.loggingIn")}
                        </>
                    ) : register ? (
                        <>
                            <ShieldCheck size={15} /> {t("settings.sync.register")}
                        </>
                    ) : (
                        <>
                            <LogIn size={15} /> {t("settings.sync.login")}
                        </>
                    )}
                </button>

                <div className={s.orRow}>
                    <span>{t("settings.sync.or")}</span>
                </div>

                <button
                    className={`${s.btn} ${s.btnGhost} ${s.btnWide}`}
                    disabled={!canYandex}
                    onClick={() => void doYandex()}
                >
                    <span className={s.yIcon}>Я</span> {t("settings.sync.yandex")}
                </button>

                <div className={s.switchRow}>
                    {register ? (
                        <>
                            {t("settings.sync.haveAccount")}{" "}
                            <button className={s.switchLink} onClick={() => setAuthMode("login")}>
                                {t("settings.sync.login")}
                            </button>
                        </>
                    ) : (
                        <>
                            {t("settings.sync.noAccount")}{" "}
                            <button
                                className={s.switchLink}
                                onClick={() => setAuthMode("register")}
                            >
                                {t("settings.sync.create")}
                            </button>
                        </>
                    )}
                </div>

                <div className={s.e2eNote}>
                    <ShieldCheck size={13} /> {t("settings.sync.e2eNote")}
                </div>
            </div>
        </div>
    );
}

function SyncStatusCard({
    status,
    onFix,
}: {
    status: SyncStatus | null;
    onFix: () => void;
}) {
    const { t } = useT();
    const state = status?.state ?? "idle";
    const time = status?.at_ms ? new Date(status.at_ms).toLocaleTimeString() : null;
    const counts = status && (status.hosts || status.groups || status.credentials || status.snippets);

    let badge = s.badgeOk;
    let icon = <Check size={18} />;
    let title = t("settings.sync.statusSynced");
    let sub: string | null = time ? t("settings.sync.lastSync", { time }) : t("settings.sync.statusOn");

    if (state === "syncing") {
        badge = s.badgeSyncing;
        icon = <Loader2 size={18} className={s.spin} />;
        title = t("settings.sync.statusSyncing");
        sub = t("settings.sync.syncingSub");
    } else if (state === "error") {
        badge = s.badgeError;
        icon = <WifiOff size={18} />;
        title = t("settings.sync.statusError");
        sub = time ? t("settings.sync.lastOk", { time }) : null;
    } else if (state === "idle") {
        title = t("settings.sync.statusOn");
        sub = null;
    }

    return (
        <div className={s.result}>
            <div className={s.resultHead}>
                <span className={`${s.resultBadge} ${badge}`}>{icon}</span>
                <div>
                    <div className={s.resultT}>{title}</div>
                    {sub && <div className={s.resultS}>{sub}</div>}
                </div>
            </div>

            {state === "error" && (
                <div className={s.errPlate}>
                    <AlertCircle size={14} />
                    <span>{status?.message ? localizeSyncError(t, status.message) : t("settings.sync.statusError")}</span>
                    <button className={s.fixBtn} onClick={onFix}>
                        {t("settings.sync.fix")}
                    </button>
                </div>
            )}

            {counts ? (
                <div className={s.prev}>
                    <Row icon={<Server size={15} />} label={t("settings.sync.rowHosts")} n={status!.hosts} />
                    <Row icon={<Folder size={15} />} label={t("settings.sync.rowGroups")} n={status!.groups} />
                    <Row icon={<KeyRound size={15} />} label={t("settings.sync.rowCreds")} n={status!.credentials} />
                    <Row icon={<Code2 size={15} />} label={t("settings.sync.rowSnippets")} n={status!.snippets} />
                </div>
            ) : null}
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

function scorePw(pw: string): number {
    if (!pw) return 0;
    let n = 0;
    if (pw.length >= 8) n++;
    if (pw.length >= 12) n++;
    if (/[a-z]/.test(pw) && /[A-Z]/.test(pw)) n++;
    if (/\d/.test(pw)) n++;
    if (/[^A-Za-z0-9]/.test(pw)) n++;
    return Math.min(n, 4);
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
