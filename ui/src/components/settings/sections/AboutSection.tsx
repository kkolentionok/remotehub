import { useEffect, useState } from "react";

import { useT } from "../../../i18n";
import { meta as metaApi } from "../../../lib/ipc";
import {
    applyUpdateAndRestart,
    runUpdateCheck,
    useUpdateStore,
} from "../../../lib/updater";
import { Button } from "../../ui/Button";
import styles from "../SettingsDialog.module.css";

export function AboutSection() {
    const { t } = useT();
    const [version, setVersion] = useState<string | null>(null);
    const [target, setTarget] = useState<string | null>(null);
    const upd = useUpdateStore((s) => s.state);

    useEffect(() => {
        let cancelled = false;
        metaApi
            .appVersion()
            .then((res) => {
                if (cancelled) return;
                setVersion(res.version);
                setTarget(res.target);
            })
            .catch(() => {
                // Non-fatal; just leaves the version blank.
            });
        return () => {
            cancelled = true;
        };
    }, []);

    const busy = upd.kind === "checking" || upd.kind === "downloading";

    let status: string | null = null;
    if (upd.kind === "checking") status = t("update.checking");
    else if (upd.kind === "downloading")
        status = t("update.downloading", { pct: String(upd.pct) });
    else if (upd.kind === "ready") status = t("update.ready", { version: upd.version });
    else if (upd.kind === "uptodate") status = t("update.uptodate");
    else if (upd.kind === "error") status = t("update.error", { message: upd.message });

    return (
        <div className={styles.section}>
            <h3 className={styles.sectionTitle}>{t("settings.about.title")}</h3>
            <p className={styles.sectionDescription}>
                {t("settings.about.description")}
            </p>

            <div className={styles.field}>
                <label className={styles.fieldLabel}>{t("settings.about.version")}</label>
                <span className={styles.aboutVersion}>
                    {version ? `${version} (${target})` : "—"}
                </span>
            </div>

            <div className={styles.field}>
                <label className={styles.fieldLabel}>{t("settings.about.updates")}</label>
                <div className={styles.updateRow}>
                    <Button
                        variant="secondary"
                        size="sm"
                        disabled={busy}
                        onClick={() => void runUpdateCheck({ silent: false })}
                    >
                        {t("settings.about.checkUpdates")}
                    </Button>
                    {upd.kind === "ready" && (
                        <Button
                            variant="primary"
                            size="sm"
                            onClick={() => void applyUpdateAndRestart()}
                        >
                            {t("update.restart")}
                        </Button>
                    )}
                    {status && (
                        <span className={styles.updateStatus} data-kind={upd.kind}>
                            {status}
                        </span>
                    )}
                </div>
            </div>

            <div className={styles.field}>
                <label className={styles.fieldLabel}>{t("settings.about.links")}</label>
                <span className={styles.fieldHint}>
                    {t("settings.about.linksPlaceholder")}
                </span>
            </div>
        </div>
    );
}
