import { RotateCw, X } from "lucide-react";

import { useT } from "../../i18n";
import { applyUpdateAndRestart, dismissUpdate, useUpdateStore } from "../../lib/updater";
import styles from "./UpdateBanner.module.css";

/**
 * Slim, quiet bar shown only when an update has been downloaded and is ready
 * to apply. The download happens silently in the background; applying it
 * requires a restart (NSIS replaces the running app), so we ask rather than
 * force it.
 */
export function UpdateBanner() {
    const { t } = useT();
    const state = useUpdateStore((s) => s.state);

    if (state.kind !== "ready") return null;

    return (
        <div className={styles.bar} role="status">
            <RotateCw size={14} className={styles.icon} />
            <span className={styles.text}>
                {t("update.bannerReady", { version: state.version })}
            </span>
            <button
                type="button"
                className={styles.restart}
                onClick={() => void applyUpdateAndRestart()}
            >
                {t("update.restart")}
            </button>
            <button
                type="button"
                className={styles.dismiss}
                title={t("update.later")}
                onClick={dismissUpdate}
            >
                <X size={14} />
            </button>
        </div>
    );
}
