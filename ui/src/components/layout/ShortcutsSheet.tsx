import { useEffect } from "react";
import { Keyboard } from "lucide-react";

import { useT } from "../../i18n";
import type { MessageKey } from "../../i18n/en";
import { useUiStore } from "../../store";
import styles from "./ShortcutsSheet.module.css";

/**
 * Keyboard-shortcuts cheat sheet (toggled by `?`). A quiet overlay that
 * lists the app's real, working shortcuts grouped by area. Honest by
 * design: only shortcuts that are actually wired live here — when a new
 * binding lands (e.g. a command palette), add a row.
 *
 * Each shortcut renders its keys as adjacent `<kbd>` chips (chord =
 * chips side by side, Raycast/Linear-style). Esc or a backdrop click
 * closes it; the global `?` toggle lives in AppShell.
 */
interface Shortcut {
    keys: string[];
    label: MessageKey;
}
interface Section {
    title: MessageKey;
    items: Shortcut[];
}

const SECTIONS: Section[] = [
    {
        title: "shortcuts.section.general",
        items: [
            { keys: ["?"], label: "shortcuts.row.help" },
            { keys: ["Esc"], label: "shortcuts.row.close" },
            { keys: ["Ctrl", "Shift", "E"], label: "shortcuts.row.splitRight" },
            { keys: ["Ctrl", "Shift", "D"], label: "shortcuts.row.splitDown" },
            { keys: ["↑", "↓", "Enter"], label: "shortcuts.row.listNav" },
        ],
    },
    {
        title: "shortcuts.section.terminal",
        items: [
            { keys: ["Ctrl", "Scroll"], label: "shortcuts.row.termZoom" },
            { keys: ["Ctrl/Cmd", "Click"], label: "shortcuts.row.termLink" },
        ],
    },
    {
        title: "shortcuts.section.sftp",
        items: [
            { keys: ["Ctrl/Cmd", "Click"], label: "shortcuts.row.sftpMulti" },
            { keys: ["Enter"], label: "shortcuts.row.sftpCommit" },
            { keys: ["Esc"], label: "shortcuts.row.sftpCancel" },
        ],
    },
];

export function ShortcutsSheet() {
    const { t } = useT();
    const setShortcutsOpen = useUiStore((s) => s.setShortcutsOpen);
    const close = () => setShortcutsOpen(false);

    // Esc closes (window-level so it works regardless of focus).
    useEffect(() => {
        const onKey = (e: KeyboardEvent) => {
            if (e.key === "Escape") {
                e.preventDefault();
                close();
            }
        };
        window.addEventListener("keydown", onKey);
        return () => window.removeEventListener("keydown", onKey);
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, []);

    return (
        <div
            className={styles.backdrop}
            onMouseDown={(e) => {
                if (e.target === e.currentTarget) close();
            }}
        >
            <div className={styles.panel} role="dialog" aria-modal="true">
                <div className={styles.header}>
                    <Keyboard size={16} className={styles.headerIcon} />
                    <span className={styles.title}>{t("shortcuts.title")}</span>
                    <kbd className={styles.kbd}>Esc</kbd>
                </div>

                <div className={styles.body}>
                    {SECTIONS.map((sec) => (
                        <div key={sec.title} className={styles.section}>
                            <div className={styles.sectionTitle}>{t(sec.title)}</div>
                            {sec.items.map((sc) => (
                                <div key={sc.label} className={styles.row}>
                                    <span className={styles.rowLabel}>{t(sc.label)}</span>
                                    <span className={styles.keys}>
                                        {sc.keys.map((k, i) => (
                                            <kbd key={i} className={styles.kbd}>
                                                {k}
                                            </kbd>
                                        ))}
                                    </span>
                                </div>
                            ))}
                        </div>
                    ))}
                </div>
            </div>
        </div>
    );
}
