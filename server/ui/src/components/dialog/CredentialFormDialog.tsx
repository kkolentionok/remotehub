import { useEffect, useState } from "react";
import { Eye, EyeOff } from "lucide-react";

import { useT } from "../../i18n";
import { credentials as credApi, encodeSecret } from "../../lib/ipc";
import { formatApiError } from "../../lib/types";
import type { CredentialKind } from "../../lib/types";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/Dialog";
import { Input, Select, TextField, Textarea } from "../ui/TextField";
import styles from "./HostFormDialog.module.css";

interface Props {
    open: boolean;
    onClose: () => void;
}

interface FormState {
    name: string;
    kind: CredentialKind;
    username: string;
    secret: string;
    passphrase: string;
}

const empty: FormState = {
    name: "",
    kind: "password",
    username: "",
    secret: "",
    passphrase: "",
};

export function CredentialFormDialog({ open, onClose }: Props) {
    const { t } = useT();
    const [form, setForm] = useState<FormState>(empty);
    const [submitting, setSubmitting] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const [showSecret, setShowSecret] = useState(false);

    useEffect(() => {
        if (open) {
            setForm(empty);
            setError(null);
            setShowSecret(false);
        }
    }, [open]);

    const update = <K extends keyof FormState>(key: K, value: FormState[K]) =>
        setForm((s) => ({ ...s, [key]: value }));

    async function submit() {
        setSubmitting(true);
        setError(null);
        try {
            // Only the private key is mandatory. A password may be empty
            // (passwordless / empty-password hosts); the agent has no secret.
            if (form.kind === "ssh_key" && form.secret.length === 0) {
                throw {
                    kind: "validation",
                    field: t("dialog.credential.privateKey"),
                    reason: "required",
                };
            }
            await credApi.create({
                name: form.name.trim(),
                kind: form.kind,
                username: form.username.trim(),
                secret:
                    form.kind !== "ssh_key_agent"
                        ? encodeSecret(form.secret)
                        : undefined,
                passphrase:
                    form.kind === "ssh_key" && form.passphrase.length > 0
                        ? encodeSecret(form.passphrase)
                        : undefined,
            });
            onClose();
        } catch (e: unknown) {
            setError(formatApiError(e));
        } finally {
            setSubmitting(false);
        }
    }

    const isSshKey = form.kind === "ssh_key";
    const isAgent = form.kind === "ssh_key_agent";

    return (
        <Dialog
            open={open}
            onClose={onClose}
            title={t("dialog.credentials.new")}
            size="md"
            footer={
                <>
                    <Button variant="secondary" onClick={onClose} disabled={submitting}>
                        {t("common.cancel")}
                    </Button>
                    <Button variant="primary" onClick={submit} disabled={submitting}>
                        {submitting ? t("common.saving") : t("common.create")}
                    </Button>
                </>
            }
        >
            <form
                className={styles.form}
                onSubmit={(e) => {
                    e.preventDefault();
                    void submit();
                }}
            >
                <TextField
                    label={t("dialog.credential.name")}
                    hint={t("dialog.credential.nameHint")}
                >
                    <Input
                        value={form.name}
                        onChange={(e) => update("name", e.target.value)}
                        placeholder={t("dialog.credential.namePlaceholder")}
                        required
                        autoFocus
                    />
                </TextField>

                <div className={styles.row} style={{ gridTemplateColumns: "1fr 1fr" }}>
                    <TextField label={t("dialog.credential.type")}>
                        <Select
                            value={form.kind}
                            onChange={(e) => update("kind", e.target.value as CredentialKind)}
                        >
                            <option value="password">
                                {t("dialog.credential.kind.password")}
                            </option>
                            <option value="ssh_key">
                                {t("dialog.credential.kind.ssh_key")}
                            </option>
                            <option value="ssh_key_agent" disabled>
                                {t("dialog.credential.kind.ssh_key_agent")}
                            </option>
                        </Select>
                    </TextField>

                    <TextField label={t("dialog.credential.username")}>
                        <Input
                            value={form.username}
                            onChange={(e) => update("username", e.target.value)}
                            placeholder={t("dialog.credential.usernamePlaceholder")}
                            required={!isAgent}
                        />
                    </TextField>
                </div>

                {form.kind === "password" && (
                    <TextField label={t("dialog.credential.password")}>
                        <div className={styles.secretRow}>
                            <Input
                                type={showSecret ? "text" : "password"}
                                value={form.secret}
                                onChange={(e) => update("secret", e.target.value)}
                                placeholder={t("dialog.credential.passwordPlaceholder")}
                            />
                            <button
                                type="button"
                                className={styles.eyeButton}
                                onClick={() => setShowSecret((v) => !v)}
                                aria-label={
                                    showSecret ? t("common.hide") : t("common.show")
                                }
                            >
                                {showSecret ? <EyeOff size={14} /> : <Eye size={14} />}
                            </button>
                        </div>
                    </TextField>
                )}

                {isSshKey && (
                    <>
                        <TextField
                            label={t("dialog.credential.privateKey")}
                            hint={t("dialog.credential.privateKeyHint")}
                        >
                            <Textarea
                                value={form.secret}
                                onChange={(e) => update("secret", e.target.value)}
                                placeholder="-----BEGIN OPENSSH PRIVATE KEY-----&#10;...&#10;-----END OPENSSH PRIVATE KEY-----"
                                rows={6}
                                required
                                style={{
                                    fontFamily: "var(--font-mono)",
                                    fontSize: "var(--text-sm)",
                                }}
                            />
                        </TextField>

                        <TextField
                            label={t("dialog.credential.passphrase")}
                            hint={t("dialog.credential.passphraseHint")}
                        >
                            <Input
                                type="password"
                                value={form.passphrase}
                                onChange={(e) => update("passphrase", e.target.value)}
                                placeholder={t("dialog.credential.passphrasePlaceholder")}
                            />
                        </TextField>
                    </>
                )}

                {error && <div className={styles.errorBox}>{error}</div>}
            </form>
        </Dialog>
    );
}
