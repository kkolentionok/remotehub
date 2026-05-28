import { Check, Loader2, XCircle } from "lucide-react";

import { useT } from "../../i18n";
import styles from "./SaveStatusIndicator.module.css";

export type SaveStatus =
    | { kind: "idle" }
    | { kind: "pending" }
    | { kind: "saving" }
    | { kind: "saved" }
    | { kind: "error"; message: string };

/**
 * Small indicator displayed alongside other header buttons.
 *
 * - `idle` renders an empty box (preserves layout so neighbouring
 *   icons don't shift when status appears/disappears).
 * - `pending` shows a small muted dot — the user is still typing,
 *   the debounce timer hasn't fired yet.
 * - `saving` shows a spinner.
 * - `saved` shows a check; the caller transitions back to `idle`
 *   after a short delay (handled in the parent's reducer).
 * - `error` shows a red circle with a tooltip containing the message.
 */
export function SaveStatusIndicator({ status }: { status: SaveStatus }) {
    const { t } = useT();
    return (
        <div className={styles.wrap} aria-live="polite">
            {status.kind === "pending" && (
                <span className={styles.pendingDot} title={t("host.save.pending")} />
            )}
            {status.kind === "saving" && (
                <Loader2 size={15} className={styles.spinner} aria-label={t("host.save.saving")} />
            )}
            {status.kind === "saved" && (
                <Check
                    size={15}
                    className={styles.saved}
                    aria-label={t("host.save.saved")}
                />
            )}
            {status.kind === "error" && (
                <span title={status.message} className={styles.errorWrap}>
                    <XCircle
                        size={15}
                        className={styles.error}
                        aria-label={t("host.save.error")}
                    />
                </span>
            )}
        </div>
    );
}
