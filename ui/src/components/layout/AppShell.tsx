import { useEffect } from "react";

import { useT } from "../../i18n";
import {
    subscribeToBackendEvents,
    useCredentialsStore,
    useGroupsStore,
    useHostsStore,
    useSettingsStore,
} from "../../store";
import { HostDetail } from "../host/HostDetail";
import { Sidebar } from "../sidebar/Sidebar";
import { DialogHost } from "./DialogHost";
import styles from "./AppShell.module.css";

/**
 * Root layout: sidebar on the left, main pane on the right.
 *
 * On mount: load all four stores (hosts, groups, credentials,
 * settings) and register backend event subscriptions so any change
 * refreshes the relevant store. Also keeps the i18n locale in sync
 * with `settings.language` once settings load — language is the
 * primary source of truth, the in-memory locale just mirrors it.
 */
export function AppShell() {
    const { locale, setLocale } = useT();
    const settingsLanguage = useSettingsStore((s) => s.settings?.language);

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

    // When settings.language is loaded (or changes), reflect it in the
    // I18nProvider's runtime locale. The check prevents a feedback loop:
    // user clicks RU → AppearanceSection sets locale + persists → settings
    // load fires → this effect runs but values match, no-op.
    useEffect(() => {
        if (!settingsLanguage) return;
        if (settingsLanguage !== locale) {
            setLocale(settingsLanguage);
        }
    }, [settingsLanguage, locale, setLocale]);

    return (
        <div className={styles.shell}>
            <Sidebar />
            <HostDetail />
            <DialogHost />
        </div>
    );
}
