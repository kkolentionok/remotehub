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
 * Save status as an always-visible pill under the inspector title:
 *  - idle / pending → muted check + "All changes saved" (resting state)
 *  - saving         → spinner + "Saving…"
 *  - saved          → green check + "Saved" (parent flips back to idle ~1.5s)
 *  - error          → red icon + label, full message in the tooltip
 */
export function SaveStatusIndicator({ status }: { status: SaveStatus }) {
    const { t } = useT();

    if (status.kind === "saving") {
        return (
            <span className={styles.pill} aria-live="polite">
                <Loader2 size={12} className={styles.spinner} />
                {t("host.save.saving")}…
            </span>
        );
    }
    if (status.kind === "saved") {
        return (
            <span className={`${styles.pill} ${styles.saved}`} aria-live="polite">
                <Check size={12} />
                {t("host.save.saved")}
            </span>
        );
    }
    if (status.kind === "error") {
        return (
            <span
                className={`${styles.pill} ${styles.error}`}
                title={status.message}
                aria-live="polite"
            >
                <XCircle size={12} />
                {t("host.save.error")}
            </span>
        );
    }
    return (
        <span className={styles.pill} aria-live="polite">
            <Check size={12} className={styles.mute} />
            {t("host.save.allSaved")}
        </span>
    );
}
