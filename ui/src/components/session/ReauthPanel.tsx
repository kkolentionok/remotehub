import { useEffect, useMemo, useRef, useState } from "react";
import { Eye, EyeOff, KeyRound, Lock, Zap } from "lucide-react";

import { useT } from "../../i18n";
import { useCredentialsStore, useSessionsStore } from "../../store";
import type { HostId } from "../../lib/types";
import {
    credentials as credApi,
    hosts as hostsApi,
    encodeSecret,
} from "../../lib/ipc";
import { Button } from "../ui/Button";
import styles from "./ConnState.module.css";

type Method = "key" | "password";

/** "Ввести данные заново" — re-enter credentials and reconnect in place,
 *  without leaving the session tab. Shown on `auth` / `badpass` screens
 *  between the diagnosis and the technical details. The default method is
 *  the one that just failed (auth → SSH key, badpass → password). */
export function ReauthPanel({
    hostId,
    sessionKey,
    defaultMethod,
}: {
    hostId: HostId;
    sessionKey: string;
    defaultMethod: Method;
}) {
    const { t } = useT();
    const close = useSessionsStore((s) => s.close);
    const open = useSessionsStore((s) => s.open);
    const creds = useCredentialsStore((s) => s.items);
    const keys = useMemo(
        () => creds.filter((c) => c.kind === "ssh_key" || c.kind === "ssh_key_agent"),
        [creds],
    );

    const [method, setMethod] = useState<Method>(defaultMethod);
    const [pw, setPw] = useState("");
    const [showPw, setShowPw] = useState(false);
    const [keyId, setKeyId] = useState<string>(() => keys[0]?.id ?? "");
    const [busy, setBusy] = useState(false);

    const pwRef = useRef<HTMLInputElement>(null);
    const keyRef = useRef<HTMLSelectElement>(null);

    // Autofocus the active field whenever the method changes (and on mount).
    useEffect(() => {
        if (method === "password") pwRef.current?.focus();
        else keyRef.current?.focus();
    }, [method]);

    // Keep the selected key valid as the credential list loads/changes.
    useEffect(() => {
        if (method === "key" && keys.length > 0 && !keys.some((k) => k.id === keyId)) {
            setKeyId(keys[0]!.id);
        }
    }, [keys, method, keyId]);

    const canSubmit =
        !busy && (method === "password" ? pw !== "" : keyId !== "");

    const submit = async () => {
        if (!canSubmit) return;
        setBusy(true);
        try {
            const host = await hostsApi.get(hostId);
            const ids = host.credential_ids ?? [];
            if (method === "password") {
                const all = useCredentialsStore.getState().items;
                const pwCred = all.find(
                    (c) => ids.includes(c.id) && c.kind === "password",
                );
                if (pwCred) {
                    await credApi.rotateSecret({
                        id: pwCred.id,
                        secret: encodeSecret(pw),
                    });
                } else {
                    const base =
                        host.display_name || host.name || host.hostname || "password";
                    const taken = new Set(all.map((c) => c.name));
                    let name = base;
                    let i = 2;
                    while (taken.has(name)) name = `${base} ${i++}`;
                    const created = await credApi.create({
                        name,
                        kind: "password",
                        username: "",
                        secret: encodeSecret(pw),
                    });
                    await credApi.linkHost({
                        host_id: host.id,
                        credential_id: created.id,
                        set_as_default: true,
                    });
                }
            } else {
                // Link the chosen key and make it the default so the actor
                // tries it first. `link_host` upserts, so re-linking is safe.
                await credApi.linkHost({
                    host_id: host.id,
                    credential_id: keyId,
                    set_as_default: true,
                });
            }
            const fresh = await hostsApi.get(host.id);
            await close(sessionKey);
            void open(fresh);
        } catch {
            setBusy(false);
        }
    };

    return (
        <div className={styles.reauthCard}>
            <div className={styles.reauthHead}>
                <Lock size={14} />
                {t("conn.reauth.title")}
            </div>

            <div className={styles.reauthRow}>
                <span className={styles.reauthLabel}>{t("conn.reauth.access")}</span>
                <div className={styles.seg}>
                    <button
                        type="button"
                        className={`${styles.segBtn} ${method === "key" ? styles.segOn : ""}`}
                        onClick={() => setMethod("key")}
                    >
                        <KeyRound size={13} /> {t("conn.reauth.key")}
                    </button>
                    <button
                        type="button"
                        className={`${styles.segBtn} ${method === "password" ? styles.segOn : ""}`}
                        onClick={() => setMethod("password")}
                    >
                        {t("conn.reauth.password")}
                    </button>
                </div>
            </div>

            {method === "password" ? (
                <div className={styles.reauthRow}>
                    <span className={styles.reauthLabel}>
                        {t("conn.reauth.password")}
                    </span>
                    <div className={styles.pwWrap}>
                        <input
                            ref={pwRef}
                            className={styles.pwInput}
                            type={showPw ? "text" : "password"}
                            value={pw}
                            onChange={(e) => setPw(e.target.value)}
                            placeholder={t("conn.reauth.newPassword")}
                            autoComplete="off"
                            onKeyDown={(e) => {
                                if (e.key === "Enter") void submit();
                            }}
                        />
                        <button
                            type="button"
                            className={styles.eye}
                            onClick={() => setShowPw((v) => !v)}
                            tabIndex={-1}
                            aria-label={showPw ? t("common.hide") : t("common.show")}
                        >
                            {showPw ? <EyeOff size={15} /> : <Eye size={15} />}
                        </button>
                    </div>
                </div>
            ) : (
                <div className={styles.reauthRow}>
                    <span className={styles.reauthLabel}>{t("conn.reauth.keyLabel")}</span>
                    <select
                        ref={keyRef}
                        className={styles.keySelect}
                        value={keyId}
                        onChange={(e) => setKeyId(e.target.value)}
                    >
                        {keys.length === 0 && (
                            <option value="">{t("conn.reauth.noKeys")}</option>
                        )}
                        {keys.map((k) => (
                            <option key={k.id} value={k.id}>
                                {k.name}
                                {k.username ? ` · ${k.username}` : ""}
                            </option>
                        ))}
                    </select>
                </div>
            )}

            <Button
                variant="primary"
                className={styles.saveBtn}
                disabled={!canSubmit}
                onClick={() => void submit()}
            >
                <Zap size={14} /> {t("conn.reauth.save")}
            </Button>
        </div>
    );
}
