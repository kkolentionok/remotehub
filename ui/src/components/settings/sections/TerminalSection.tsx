import { useT } from "../../../i18n";
import { useSettingsStore } from "../../../store";
import type { CursorStyle, Settings } from "../../../lib/types";
import {
    TERMINAL_SCHEME_NAMES,
    TERMINAL_SCHEME_ORDER,
    TERMINAL_THEMES,
} from "../../../lib/terminalThemes";
import styles from "../SettingsDialog.module.css";
import term from "./TerminalSection.module.css";

const SIZES = [12, 13, 14, 16];
const CURSORS: CursorStyle[] = ["block", "underline", "bar"];

/**
 * Terminal appearance: font family + size, color scheme, cursor style.
 * All live-saved to settings; the Terminal component reads them and
 * applies to every xterm instance.
 */
export function TerminalSection({ settings }: { settings: Settings }) {
    const { t } = useT();
    const update = useSettingsStore((s) => s.update);

    const fonts = [
        { value: "JetBrains Mono", label: "JetBrains Mono" },
        { value: "Cascadia Mono", label: "Cascadia Mono" },
        { value: "Consolas", label: "Consolas" },
        { value: "monospace", label: t("settings.terminal.fontSystem") },
    ];

    return (
        <div className={styles.section}>
            <h3 className={styles.sectionTitle}>{t("settings.terminal.title")}</h3>
            <p className={styles.sectionDescription}>
                {t("settings.terminal.description")}
            </p>

            <div className={styles.field}>
                <label className={styles.fieldLabel}>
                    {t("settings.terminal.colorScheme")}
                </label>
                <div className={term.grid}>
                    {TERMINAL_SCHEME_ORDER.map((scheme) => {
                        const th = TERMINAL_THEMES[scheme];
                        const selected = settings.terminal_color_scheme === scheme;
                        const name =
                            scheme === "default"
                                ? t("settings.terminal.schemeDefault")
                                : TERMINAL_SCHEME_NAMES[scheme];
                        return (
                            <button
                                key={scheme}
                                type="button"
                                className={`${term.card} ${selected ? term.active : ""}`}
                                onClick={() =>
                                    void update({ terminal_color_scheme: scheme })
                                }
                            >
                                <div
                                    className={term.preview}
                                    style={{ background: th.background }}
                                >
                                    <div className={term.bars}>
                                        <span style={{ background: th.red }} />
                                        <span style={{ background: th.green }} />
                                        <span style={{ background: th.yellow }} />
                                        <span style={{ background: th.blue }} />
                                        <span style={{ background: th.magenta }} />
                                    </div>
                                    <div
                                        className={term.sample}
                                        style={{ color: th.foreground }}
                                    >
                                        $ ls
                                    </div>
                                </div>
                                <span className={term.name}>{name}</span>
                            </button>
                        );
                    })}
                </div>
            </div>

            <div className={styles.field}>
                <label className={styles.fieldLabel}>
                    {t("settings.terminal.font")}
                </label>
                <div className={styles.segmented}>
                    {fonts.map((f) => (
                        <button
                            key={f.value}
                            type="button"
                            className={`${styles.segment} ${settings.terminal_font_family === f.value ? styles.segmentActive : ""}`}
                            style={{ fontFamily: `'${f.value}', monospace` }}
                            onClick={() => void update({ terminal_font_family: f.value })}
                        >
                            {f.label}
                        </button>
                    ))}
                </div>
            </div>

            <div className={styles.field}>
                <label className={styles.fieldLabel}>
                    {t("settings.terminal.fontSize")}
                </label>
                <div className={styles.segmented}>
                    {SIZES.map((sz) => (
                        <button
                            key={sz}
                            type="button"
                            className={`${styles.segment} ${settings.terminal_font_size === sz ? styles.segmentActive : ""}`}
                            onClick={() => void update({ terminal_font_size: sz })}
                        >
                            {sz}px
                        </button>
                    ))}
                </div>
            </div>

            <div className={styles.field}>
                <label className={styles.fieldLabel}>
                    {t("settings.terminal.cursor")}
                </label>
                <div className={styles.segmented}>
                    {CURSORS.map((c) => (
                        <button
                            key={c}
                            type="button"
                            className={`${styles.segment} ${settings.terminal_cursor_style === c ? styles.segmentActive : ""}`}
                            onClick={() => void update({ terminal_cursor_style: c })}
                        >
                            {t(`settings.terminal.cursor.${c}`)}
                        </button>
                    ))}
                </div>
            </div>
        </div>
    );
}
