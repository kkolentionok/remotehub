import { useEffect } from "react";

import { useT } from "../../i18n";
import {
    subscribeToBackendEvents,
    useCredentialsStore,
    useGroupsStore,
    useHostsStore,
    useSessionsStore,
    useSettingsStore,
    useUiStore,
} from "../../store";
import { SessionView } from "../session/SessionView";
import { DialogHost } from "./DialogHost";
import { HomeView } from "./HomeView";
import { Launcher } from "./Launcher";
import { TabBar } from "./TabBar";
import styles from "./AppShell.module.css";

/**
 * Root layout: a tab bar on top (pinned Vault + one tab per session),
 * with the active tab's content below. Every tab — the Vault host
 * manager and each session terminal — stays mounted and is toggled via
 * visibility, so scrollback, form drafts, and focus survive switching.
 *
 * On mount: load the four stores and register backend event
 * subscriptions; keep the i18n locale synced with `settings.language`.
 */
export function AppShell() {
    const { locale, setLocale } = useT();
    const settingsLanguage = useSettingsStore((s) => s.settings?.language);
    const theme = useSettingsStore((s) => s.settings?.theme);
    const sessions = useSessionsStore((s) => s.sessions);
    const activeKey = useSessionsStore((s) => s.activeSessionKey);
    const launcherOpen = useUiStore((s) => s.launcherOpen);

    useEffect(() => {
        void useHostsStore.getState().load();
        void useGroupsStore.getState().load();
        void useCredentialsStore.getState().load();
        void useSettingsStore.getState().load();

        let cleanup: (() => void) | undefined;
        subscribeToBackendEvents().then((c) => {
            cleanup = c;
        });
        return () => {
            cleanup?.();
        };
    }, []);

    useEffect(() => {
        if (!settingsLanguage) return;
        if (settingsLanguage !== locale) {
            setLocale(settingsLanguage);
        }
    }, [settingsLanguage, locale, setLocale]);

    // Drive the app color theme from settings (overrides the OS media
    // query). "system" defers to the OS.
    useEffect(() => {
        document.documentElement.setAttribute("data-theme", theme ?? "system");
    }, [theme]);

    return (
        <div className={styles.shell}>
            <TabBar />
            <div className={styles.stage}>
                <div
                    className={styles.pane}
                    style={{ display: activeKey === null ? "flex" : "none" }}
                >
                    <HomeView />
                </div>
                {sessions.map((s) => (
                    <div
                        key={s.key}
                        className={styles.pane}
                        style={{ display: s.key === activeKey ? "flex" : "none" }}
                    >
                        <SessionView session={s} active={s.key === activeKey} />
                    </div>
                ))}
            </div>
            <DialogHost />
            {launcherOpen && <Launcher />}
        </div>
    );
}
