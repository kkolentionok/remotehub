import { useT } from "../../../i18n";
import { useSettingsStore } from "../../../store";
import type { Language, Settings, Theme } from "../../../lib/types";
import styles from "../SettingsDialog.module.css";

interface Props {
    settings: Settings;
}

/**
 * Language + theme. Both live-save: the moment the user clicks a
 * segment, the value is sent to the backend and the local copy
 * updated. The `settings:changed` event then re-loads the store
 * to confirm.
 *
 * Language ALSO updates the I18nProvider's runtime locale so the
 * UI re-renders in the new language immediately. The provider
 * reads `settings.language` via a hook in App.tsx (wired up
 * separately in this stage's bootstrap step).
 */
export function AppearanceSection({ settings }: Props) {
    const { t, locale, setLocale } = useT();
    const update = useSettingsStore((s) => s.update);

    const setLanguage = async (lang: Language) => {
        // Update the in-memory locale first so the UI flips
        // language without waiting for the IPC round-trip.
        if (lang !== locale) setLocale(lang);
        await update({ language: lang });
    };

    const setTheme = async (theme: Theme) => {
        await update({ theme });
    };

    return (
        <div className={styles.section}>
            <h3 className={styles.sectionTitle}>{t("settings.appearance.title")}</h3>
            <p className={styles.sectionDescription}>
                {t("settings.appearance.description")}
            </p>

            <div className={styles.field}>
                <label className={styles.fieldLabel}>
                    {t("settings.appearance.language")}
                </label>
                <div className={styles.segmented}>
                    <button
                        type="button"
                        className={`${styles.segment} ${settings.language === "en" ? styles.segmentActive : ""}`}
                        onClick={() => void setLanguage("en")}
                    >
                        English
                    </button>
                    <button
                        type="button"
                        className={`${styles.segment} ${settings.language === "ru" ? styles.segmentActive : ""}`}
                        onClick={() => void setLanguage("ru")}
                    >
                        Русский
                    </button>
                </div>
            </div>

            <div className={styles.field}>
                <label className={styles.fieldLabel}>
                    {t("settings.appearance.theme")}
                </label>
                <div className={styles.segmented}>
                    <button
                        type="button"
                        className={`${styles.segment} ${settings.theme === "system" ? styles.segmentActive : ""}`}
                        onClick={() => void setTheme("system")}
                    >
                        {t("settings.appearance.themeSystem")}
                    </button>
                    <button
                        type="button"
                        className={`${styles.segment} ${settings.theme === "light" ? styles.segmentActive : ""}`}
                        onClick={() => void setTheme("light")}
                    >
                        {t("settings.appearance.themeLight")}
                    </button>
                    <button
                        type="button"
                        className={`${styles.segment} ${settings.theme === "dark" ? styles.segmentActive : ""}`}
                        onClick={() => void setTheme("dark")}
                    >
                        {t("settings.appearance.themeDark")}
                    </button>
                </div>
                <span className={styles.fieldHint}>
                    {t("settings.appearance.themeHint")}
                </span>
            </div>
        </div>
    );
}
