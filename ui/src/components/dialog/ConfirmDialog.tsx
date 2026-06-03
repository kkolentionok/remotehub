import { useRef, useState } from "react";

import { useT } from "../../i18n";
import { formatApiError } from "../../lib/types";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/Dialog";
import styles from "./HostFormDialog.module.css";

interface Props {
    open: boolean;
    onClose: () => void;
    title: string;
    description: string;
    confirmLabel?: string;
    onConfirm: () => Promise<void>;
}

/**
 * Generic destructive-action confirmation. Surfaces backend errors
 * inline; keeps the dialog open on failure so the user can retry.
 */
export function ConfirmDialog({
    open,
    onClose,
    title,
    description,
    confirmLabel,
    onConfirm,
}: Props) {
    const { t } = useT();
    const [submitting, setSubmitting] = useState(false);
    const [error, setError] = useState<string | null>(null);
    // Synchronous guard: `disabled` only takes effect after a re-render, so a
    // fast double-click could fire `onConfirm` twice (e.g. delete → the second
    // call races the first).
    const busy = useRef(false);

    async function run() {
        if (busy.current) return;
        busy.current = true;
        setSubmitting(true);
        setError(null);
        try {
            await onConfirm();
            onClose();
        } catch (e: unknown) {
            setError(formatApiError(e));
        } finally {
            setSubmitting(false);
            busy.current = false;
        }
    }

    return (
        <Dialog
            open={open}
            onClose={onClose}
            title={title}
            size="sm"
            footer={
                <>
                    <Button variant="secondary" onClick={onClose} disabled={submitting}>
                        {t("common.cancel")}
                    </Button>
                    <Button variant="danger" onClick={run} disabled={submitting}>
                        {submitting ? t("common.working") : confirmLabel ?? t("common.delete")}
                    </Button>
                </>
            }
        >
            <p style={{ margin: 0, color: "var(--color-fg-muted)" }}>{description}</p>
            {error && (
                <div
                    className={styles.errorBox}
                    style={{ marginTop: "var(--space-3)" }}
                >
                    {error}
                </div>
            )}
        </Dialog>
    );
}
