import { useEffect, useState } from "react";

import { useT } from "../../../i18n";
import { meta as metaApi } from "../../../lib/ipc";
import styles from "../SettingsDialog.module.css";

export function AboutSection() {
    const { t } = useT();
    const [version, setVersion] = useState<string | null>(null);
    const [target, setTarget] = useState<string | null>(null);

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
                <label className={styles.fieldLabel}>{t("settings.about.links")}</label>
                <span className={styles.fieldHint}>
                    {t("settings.about.linksPlaceholder")}
                </span>
            </div>
        </div>
    );
}
