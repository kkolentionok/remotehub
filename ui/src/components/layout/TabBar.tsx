import { Plus, Server, Settings, X } from "lucide-react";

import { useT } from "../../i18n";
import { useSessionsStore, useUiStore } from "../../store";
import { WindowControls } from "./WindowControls";
import styles from "./TabBar.module.css";

/**
 * Application tab bar. The first tab is the pinned "Vault" (host
 * manager, not closable); the rest are live sessions. The active tab's
 * content is shown by AppShell. "+" returns to the Vault to pick a host
 * (a dedicated launcher lands next).
 */
export function TabBar() {
    const { t } = useT();
    const sessions = useSessionsStore((s) => s.sessions);
    const activeKey = useSessionsStore((s) => s.activeSessionKey);
    const setActive = useSessionsStore((s) => s.setActive);
    const close = useSessionsStore((s) => s.close);
    const openLauncher = useUiStore((s) => s.setLauncherOpen);
    const setDialog = useUiStore((s) => s.setDialog);

    return (
        <div className={styles.bar} role="tablist" data-tauri-drag-region>
            <button
                type="button"
                role="tab"
                aria-selected={activeKey === null}
                className={`${styles.tab} ${styles.vault} ${activeKey === null ? styles.active : ""}`}
                onClick={() => setActive(null)}
            >
                <Server size={14} />
                <span className={styles.label}>{t("nav.vault")}</span>
            </button>

            {sessions.map((s) => (
                <button
                    key={s.key}
                    type="button"
                    role="tab"
                    aria-selected={activeKey === s.key}
                    className={`${styles.tab} ${activeKey === s.key ? styles.active : ""}`}
                    onClick={() => setActive(s.key)}
                >
                    <span className={`${styles.dot} ${styles[`dot--${s.state}`] ?? ""}`} />
                    <span className={styles.label}>{s.title}</span>
                    <span
                        className={styles.close}
                        role="button"
                        aria-label={t("common.close")}
                        onClick={(e) => {
                            e.stopPropagation();
                            void close(s.key);
                        }}
                    >
                        <X size={12} />
                    </span>
                </button>
            ))}

            <button
                type="button"
                className={styles.add}
                onClick={() => openLauncher(true)}
                title={t("tab.new")}
                aria-label={t("tab.new")}
            >
                <Plus size={15} />
            </button>

            {/* Draggable empty area, then settings + window controls. */}
            <div className={styles.drag} data-tauri-drag-region />
            <button
                type="button"
                className={styles.gear}
                onClick={() =>
                    setDialog({
                        kind: "settings",
                        section: activeKey !== null ? "terminal" : undefined,
                    })
                }
                title={t("settings.title")}
                aria-label={t("settings.title")}
            >
                <Settings size={16} />
            </button>
            <WindowControls />
        </div>
    );
}
