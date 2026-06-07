import { useEffect, useState } from "react";

import { useT } from "../../i18n";
import { sync as syncApi } from "../../lib/ipc";
import { formatApiError } from "../../lib/types";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/Dialog";
import { Input, TextField } from "../ui/TextField";
import styles from "./HostFormDialog.module.css";

interface Props {
    open: boolean;
    onClose: () => void;
    /** "set" first time, "fix" after a wrong password / sync error. */
    mode?: "set" | "fix";
}

/**
 * Prompts once for the vault (master) password that seals the E2E envelope.
 * On success it's cached in the OS keychain and the background actor takes
 * over — there is no manual "sync now". If the user dismisses this without
 * entering it, it reappears on the next launch (driven by AppShell).
 */
export function SyncMasterDialog({ open, onClose, mode = "set" }: Props) {
    const { t } = useT();
    const [password, setPassword] = useState("");
    const [submitting, setSubmitting] = useState(false);
    const [error, setError] = useState<string | null>(null);

    useEffect(() => {
        if (open) {
            setPassword("");
            setError(null);
        }
    }, [open]);

    async function submit() {
        if (!password) return;
        setSubmitting(true);
        setError(null);
        try {
            await syncApi.setMaster(password);
            onClose();
        } catch (e: unknown) {
            setError(formatApiError(e));
        } finally {
            setSubmitting(false);
        }
    }

    return (
        <Dialog
            open={open}
            onClose={onClose}
            title={t("settings.sync.master.title")}
            size="sm"
            footer={
                <>
                    <Button variant="secondary" onClick={onClose} disabled={submitting}>
                        {t("common.later")}
                    </Button>
                    <Button
                        variant="primary"
                        onClick={submit}
                        disabled={submitting || !password}
                    >
                        {submitting ? t("common.saving") : t("settings.sync.master.save")}
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
                <p style={{ margin: "0 0 var(--space-3)", color: "var(--text-3)" }}>
                    {mode === "fix"
                        ? t("settings.sync.master.descFix")
                        : t("settings.sync.master.desc")}
                </p>
                <TextField label={t("settings.sync.master.label")}>
                    <Input
                        type="password"
                        value={password}
                        onChange={(e) => setPassword(e.target.value)}
                        placeholder={t("settings.sync.master.placeholder")}
                        autoFocus
                    />
                </TextField>
                {error && <div className={styles.errorBox}>{error}</div>}
            </form>
        </Dialog>
    );
}
