import type { ReactNode } from "react";
import styles from "./EmptyState.module.css";

interface Props {
    icon?: ReactNode;
    title: string;
    description?: string;
    action?: ReactNode;
}

export function EmptyState({ icon, title, description, action }: Props) {
    return (
        <div className={styles.root}>
            {icon ? <div className={styles.icon}>{icon}</div> : null}
            <h3 className={styles.title}>{title}</h3>
            {description ? (
                <p className={styles.description}>{description}</p>
            ) : null}
            {action ? <div className={styles.action}>{action}</div> : null}
        </div>
    );
}
