import { useState } from "react";
import { Eye, EyeOff, KeyRound, Lock, Trash2 } from "lucide-react";

import { useT } from "../../i18n";
import { credentials as credApi, encodeSecret } from "../../lib/ipc";
import { formatApiError } from "../../lib/types";
import type { CredentialKind } from "../../lib/types";
import { useCredentialsStore } from "../../store";
import { useDebouncedCallback } from "../../lib/useDebouncedCallback";
import { Button } from "../ui/Button";
import styles from "./CredentialInspector.module.css";

type SaveState = "idle" | "saving" | "saved" | "error";

interface Props {
    /** Existing credential id, or null for a new credential. */
    credentialId: string | null;
    onDone: () => void;
}

/**
 * Right-dock editor for a credential — same live-save mechanic as the
 * host inspector. Name/username update live; the secret is rotated
 * explicitly (revealed on demand, never auto-loaded).
 */
export function CredentialInspector({ credentialId, onDone }: Props) {
    const { t } = useT();
    const reload = useCredentialsStore((s) => s.load);
    const existing = useCredentialsStore((s) =>
        credentialId ? s.items.find((c) => c.id === credentialId) ?? null : null,
    );

    const isNew = credentialId === null;

    // ── form state (initialised once; component is remounted on selection) ──
    const [name, setName] = useState(existing?.name ?? "");
    const [username, setUsername] = useState(existing?.username ?? "");
    const [kind, setKind] = useState<CredentialKind>(existing?.kind ?? "password");
    const [secret, setSecret] = useState("");
    const [passphrase, setPassphrase] = useState("");
    const [showSecret, setShowSecret] = useState(false);
    const [revealed, setRevealed] = useState(false);
    const [save, setSave] = useState<SaveState>("idle");
    const [err, setErr] = useState<string | null>(null);

    // ── live-save name/username for existing credentials ──
    const persist = useDebouncedCallback(async (next: { name: string; username: string }) => {
        if (!credentialId) return;
        setSave("saving");
        try {
            await credApi.update({ id: credentialId, name: next.name, username: next.username });
            await reload();
            setSave("saved");
            setErr(null);
            window.setTimeout(() => setSave((s) => (s === "saved" ? "idle" : s)), 1500);
        } catch (e) {
            setErr(formatApiError(e));
            setSave("error");
        }
    }, 400);

    const editName = (v: string) => {
        setName(v);
        if (!isNew) {
            setSave("saving");
            persist.call({ name: v, username });
        }
    };
    const editUsername = (v: string) => {
        setUsername(v);
        if (!isNew) {
            setSave("saving");
            persist.call({ name, username: v });
        }
    };

    // ── reveal current secret (existing only) ──
    const reveal = async () => {
        if (!credentialId) return;
        try {
            const r = await credApi.reveal(credentialId);
            setSecret(r.secret ?? "");
            setRevealed(true);
            setShowSecret(true);
        } catch (e) {
            setErr(formatApiError(e));
        }
    };

    // ── rotate secret (existing) ──
    const rotate = async () => {
        if (!credentialId || secret === "") return;
        setSave("saving");
        try {
            await credApi.rotateSecret({
                id: credentialId,
                secret: encodeSecret(secret),
                ...(passphrase ? { passphrase: encodeSecret(passphrase) } : {}),
            });
            setSave("saved");
            setErr(null);
            window.setTimeout(() => setSave((s) => (s === "saved" ? "idle" : s)), 1500);
        } catch (e) {
            setErr(formatApiError(e));
            setSave("error");
        }
    };

    // ── create (new) ──
    const canCreate =
        name.trim() !== "" && username.trim() !== "" && secret !== "";
    const create = async () => {
        if (!canCreate) return;
        setSave("saving");
        try {
            await credApi.create({
                name: name.trim(),
                kind,
                username: username.trim(),
                secret: encodeSecret(secret),
                ...(kind === "ssh_key" && passphrase
                    ? { passphrase: encodeSecret(passphrase) }
                    : {}),
            });
            await reload();
            onDone();
        } catch (e) {
            setErr(formatApiError(e));
            setSave("error");
        }
    };

    const remove = async () => {
        if (!credentialId) return;
        try {
            await credApi.delete(credentialId);
            await reload();
            onDone();
        } catch (e) {
            setErr(formatApiError(e));
        }
    };

    const KindIcon = kind === "ssh_key" ? KeyRound : Lock;
    const isKey = kind === "ssh_key";

    return (
        <div className={styles.inspector}>
            <div className={styles.header}>
                <span className={styles.headerIcon}>
                    <KindIcon size={16} />
                </span>
                <div className={styles.headerText}>
                    <div className={styles.headerTitle}>
                        {isNew ? t("tools.cred.new") : name || t("tools.cred.untitled")}
                    </div>
                    <div className={styles.headerSub}>
                        {save === "saving"
                            ? t("tools.cred.saving")
                            : save === "saved"
                              ? t("tools.cred.saved")
                              : save === "error"
                                ? (err ?? t("common.error"))
                                : isNew
                                  ? t("tools.cred.newSub")
                                  : t("tools.cred.savedAll")}
                    </div>
                </div>
                <button
                    type="button"
                    className={styles.closeBtn}
                    onClick={onDone}
                    aria-label={t("common.close")}
                    title={t("common.close")}
                >
                    ✕
                </button>
            </div>

            <div className={styles.body}>
                <label className={styles.field}>
                    <span className={styles.label}>{t("tools.cred.name")}</span>
                    <input
                        className={styles.input}
                        value={name}
                        onChange={(e) => editName(e.target.value)}
                        placeholder={t("tools.cred.namePlaceholder")}
                        autoFocus={isNew}
                    />
                </label>

                {isNew && (
                    <div className={styles.field}>
                        <span className={styles.label}>{t("tools.cred.kind")}</span>
                        <div className={styles.segmented}>
                            <button
                                type="button"
                                className={kind === "password" ? styles.segOn : styles.seg}
                                onClick={() => setKind("password")}
                            >
                                <Lock size={13} /> {t("tools.cred.kindPassword")}
                            </button>
                            <button
                                type="button"
                                className={kind === "ssh_key" ? styles.segOn : styles.seg}
                                onClick={() => setKind("ssh_key")}
                            >
                                <KeyRound size={13} /> {t("tools.cred.kindKey")}
                            </button>
                        </div>
                    </div>
                )}

                <label className={styles.field}>
                    <span className={styles.label}>{t("tools.cred.login")}</span>
                    <input
                        className={styles.input}
                        value={username}
                        onChange={(e) => editUsername(e.target.value)}
                        placeholder={t("tools.cred.loginPlaceholder")}
                        autoComplete="off"
                    />
                </label>

                {/* Secret */}
                <div className={styles.field}>
                    <span className={styles.label}>
                        {isKey ? t("tools.cred.key") : t("tools.cred.password")}
                    </span>
                    {!isNew && !revealed ? (
                        <button type="button" className={styles.revealBtn} onClick={reveal}>
                            <Eye size={14} /> {t("tools.cred.reveal")}
                        </button>
                    ) : isKey ? (
                        <textarea
                            className={styles.textarea}
                            value={secret}
                            onChange={(e) => setSecret(e.target.value)}
                            placeholder={t("tools.cred.keyPlaceholder")}
                            rows={5}
                            spellCheck={false}
                        />
                    ) : (
                        <div className={styles.secretRow}>
                            <input
                                className={styles.input}
                                type={showSecret ? "text" : "password"}
                                value={secret}
                                onChange={(e) => setSecret(e.target.value)}
                                placeholder={t("tools.cred.passwordPlaceholder")}
                                autoComplete="off"
                            />
                            <button
                                type="button"
                                className={styles.eyeBtn}
                                onClick={() => setShowSecret((v) => !v)}
                                aria-label={t("tools.cred.toggleShow")}
                            >
                                {showSecret ? <EyeOff size={15} /> : <Eye size={15} />}
                            </button>
                        </div>
                    )}
                    {isKey && (isNew || revealed) && (
                        <input
                            className={styles.input}
                            style={{ marginTop: 8 }}
                            type="password"
                            value={passphrase}
                            onChange={(e) => setPassphrase(e.target.value)}
                            placeholder={t("tools.cred.passphrasePlaceholder")}
                            autoComplete="off"
                        />
                    )}
                    {!isNew && revealed && (
                        <Button
                            variant="secondary"
                            className={styles.rotateBtn}
                            disabled={secret === "" || save === "saving"}
                            onClick={() => void rotate()}
                        >
                            {t("tools.cred.replace")}
                        </Button>
                    )}
                </div>
            </div>

            <div className={styles.footer}>
                {isNew ? (
                    <Button
                        variant="primary"
                        disabled={!canCreate || save === "saving"}
                        onClick={() => void create()}
                    >
                        {t("tools.cred.create")}
                    </Button>
                ) : (
                    <button type="button" className={styles.deleteBtn} onClick={() => void remove()}>
                        <Trash2 size={14} /> {t("common.delete")}
                    </button>
                )}
            </div>
        </div>
    );
}
