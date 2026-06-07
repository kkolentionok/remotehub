import { useEffect, useState } from "react";
import { Check, Eye, EyeOff, Lock, ShieldCheck } from "lucide-react";

import { useT } from "../../i18n";
import { sync as syncApi } from "../../lib/ipc";
import { localizeSyncError } from "../../lib/syncErrors";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/Dialog";
import form from "./HostFormDialog.module.css";
import s from "./SyncMasterDialog.module.css";

interface Props {
    open: boolean;
    onClose: () => void;
    /** "set" first time, "fix" after a wrong password / sync error. */
    mode?: "set" | "fix";
}

/**
 * Prompts once for the vault (master) password that seals the E2E envelope.
 * "Remember on this device" stores it in the OS keychain so automatic sync runs
 * unattended; unchecked keeps it in memory for the session only (re-prompted
 * next launch). There is no manual "sync now" — the background actor takes over.
 */
export function SyncMasterDialog({ open, onClose, mode = "set" }: Props) {
    const { t } = useT();
    const [password, setPassword] = useState("");
    const [show, setShow] = useState(false);
    const [persist, setPersist] = useState(true);
    const [submitting, setSubmitting] = useState(false);
    const [error, setError] = useState<string | null>(null);

    useEffect(() => {
        if (open) {
            setPassword("");
            setShow(false);
            setPersist(true);
            setError(null);
        }
    }, [open]);

    async function submit() {
        if (!password) return;
        setSubmitting(true);
        setError(null);
        try {
            await syncApi.setMaster(password, persist);
            onClose();
        } catch (e: unknown) {
            setError(localizeSyncError(t, e));
        } finally {
            setSubmitting(false);
        }
    }

    return (
        <Dialog
            open={open}
            onClose={onClose}
            title={t("settings.sync.master.title")}
            subtitle={t("settings.sync.master.subtitle")}
            icon={<Lock size={18} />}
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
                        <ShieldCheck size={15} />
                        {submitting ? t("common.saving") : t("settings.sync.master.save")}
                    </Button>
                </>
            }
        >
            <form
                className={form.form}
                onSubmit={(e) => {
                    e.preventDefault();
                    void submit();
                }}
            >
                <div className={s.e2ePlate}>
                    <ShieldCheck size={16} className={s.e2eIc} />
                    <span>
                        {mode === "fix"
                            ? t("settings.sync.master.descFix")
                            : t("settings.sync.master.desc")}
                    </span>
                </div>

                <div className={s.fieldL}>{t("settings.sync.master.label")}</div>
                <div className={s.pw}>
                    <input
                        className={s.input}
                        type={show ? "text" : "password"}
                        value={password}
                        onChange={(e) => setPassword(e.target.value)}
                        placeholder={t("settings.sync.master.placeholder")}
                        autoFocus
                        spellCheck={false}
                        autoComplete="off"
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

                <label className={s.checkRow}>
                    <span className={`${s.box} ${persist ? s.boxOn : ""}`}>
                        {persist && <Check size={13} />}
                    </span>
                    <input
                        type="checkbox"
                        className={s.checkInput}
                        checked={persist}
                        onChange={(e) => setPersist(e.target.checked)}
                    />
                    <span>{t("settings.sync.master.remember")}</span>
                </label>

                {error && <div className={form.errorBox}>{error}</div>}
            </form>
        </Dialog>
    );
}
