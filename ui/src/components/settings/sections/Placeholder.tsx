import { type ReactNode } from "react";

import styles from "../SettingsDialog.module.css";

interface PlaceholderProps {
    icon: ReactNode;
    title: string;
    description: string;
    roadmap?: string;
}

/**
 * Empty-state for tabs that aren't built yet (Profile, Terminal,
 * Import/Export). Shows the user what's planned without pretending
 * there are hidden buttons to click.
 */
export function Placeholder({ icon, title, description, roadmap }: PlaceholderProps) {
    return (
        <div className={styles.placeholder}>
            <div className={styles.placeholderIcon}>{icon}</div>
            <div className={styles.placeholderTitle}>{title}</div>
            <div className={styles.placeholderDescription}>{description}</div>
            {roadmap ? <div className={styles.placeholderRoadmap}>{roadmap}</div> : null}
        </div>
    );
}
