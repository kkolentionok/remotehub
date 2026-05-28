import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { X } from "lucide-react";

import { useT } from "../../i18n";
import { useSettingsStore } from "../../store";
import { AboutSection } from "./sections/AboutSection";
import { AppearanceSection } from "./sections/AppearanceSection";
import { ConnectionsSection } from "./sections/ConnectionsSection";
import { ImportExportSection } from "./sections/ImportExportSection";
import { ProfileSection } from "./sections/ProfileSection";
import { TerminalSection } from "./sections/TerminalSection";
import styles from "./SettingsDialog.module.css";

/**
 * Tab identifiers. Order here determines order in the sidebar.
 *
 * Several tabs are intentionally empty in Stage 1.6 — they exist so
 * the user can see the roadmap and know what's coming. Each empty
 * section renders a centered hint pointing at the stage that will
 * fill it in.
 */
type Tab =
    | "profile"
    | "appearance"
    | "connections"
    | "terminal"
    | "import-export"
    | "about";

const TABS: Tab[] = [
    "profile",
    "appearance",
    "connections",
    "terminal",
    "import-export",
    "about",
];

interface SettingsDialogProps {
    onClose: () => void;
}

export function SettingsDialog({ onClose }: SettingsDialogProps) {
    const { t } = useT();
    const settings = useSettingsStore((s) => s.settings);
    const load = useSettingsStore((s) => s.load);
    const [tab, setTab] = useState<Tab>("appearance");

    // Lazy-load settings on first mount. The store may already have a
    // value (loaded at app startup); if not, fetch now.
    useEffect(() => {
        if (!settings) {
            void load();
        }
    }, [settings, load]);

    // Esc closes — same convention as the rest of our dialogs.
    useEffect(() => {
        const onKey = (e: KeyboardEvent) => {
            if (e.key === "Escape") onClose();
        };
        document.addEventListener("keydown", onKey);
        return () => document.removeEventListener("keydown", onKey);
    }, [onClose]);

    return createPortal(
        <div className={styles.backdrop} onMouseDown={onClose}>
            <div
                className={styles.panel}
                role="dialog"
                aria-modal="true"
                aria-label={t("settings.title")}
                onMouseDown={(e) => e.stopPropagation()}
            >
                <aside className={styles.nav}>
                    <header className={styles.navHeader}>
                        <h2 className={styles.navTitle}>{t("settings.title")}</h2>
                    </header>
                    <ul className={styles.tabList}>
                        {TABS.map((id) => (
                            <li key={id}>
                                <button
                                    type="button"
                                    className={`${styles.tab} ${tab === id ? styles.tabActive : ""}`}
                                    onClick={() => setTab(id)}
                                >
                                    {t(`settings.tab.${id}`)}
                                </button>
                            </li>
                        ))}
                    </ul>
                </aside>
                <section className={styles.content}>
                    <button
                        type="button"
                        className={styles.close}
                        onClick={onClose}
                        aria-label={t("common.close")}
                    >
                        <X size={16} />
                    </button>
                    {renderSection(tab, settings)}
                </section>
            </div>
        </div>,
        document.body,
    );
}

function renderSection(tab: Tab, settings: ReturnType<typeof useSettingsStore.getState>["settings"]) {
    // Show a quiet loading line on first paint. Loaded settings rarely
    // take more than a few ms from SQLite; users mostly won't see this.
    if (!settings) {
        return <SectionLoading />;
    }
    switch (tab) {
        case "profile":
            return <ProfileSection />;
        case "appearance":
            return <AppearanceSection settings={settings} />;
        case "connections":
            return <ConnectionsSection settings={settings} />;
        case "terminal":
            return <TerminalSection />;
        case "import-export":
            return <ImportExportSection />;
        case "about":
            return <AboutSection />;
    }
}

function SectionLoading() {
    const { t } = useT();
    return <div className={styles.loading}>{t("common.loading")}</div>;
}
