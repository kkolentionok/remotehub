import { useEffect, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { X } from "lucide-react";
import styles from "./Dialog.module.css";

interface DialogProps {
    open: boolean;
    onClose: () => void;
    title: string;
    /** Optional secondary line under the title. */
    subtitle?: string;
    /** Optional leading icon shown left of the title (e.g. a lucide glyph). */
    icon?: ReactNode;
    children: ReactNode;
    footer?: ReactNode;
    /** Width: "sm" 380px, "md" 480px (default), "lg" 640px. */
    size?: "sm" | "md" | "lg";
}

/**
 * Modal dialog rendered into <body>. Closes on Escape and on backdrop click.
 *
 * Focus management is minimal in this version — we don't trap focus inside
 * the dialog. For Stage 1.5 it's acceptable; if/when the app grows, swap
 * for a tested headless library (Radix, react-aria) rather than home-rolling.
 */
export function Dialog({
    open,
    onClose,
    title,
    subtitle,
    icon,
    children,
    footer,
    size = "md",
}: DialogProps) {
    useEffect(() => {
        if (!open) return;
        const onKey = (e: KeyboardEvent) => {
            if (e.key === "Escape") onClose();
        };
        document.addEventListener("keydown", onKey);
        return () => document.removeEventListener("keydown", onKey);
    }, [open, onClose]);

    if (!open) return null;

    return createPortal(
        <div className={styles.backdrop} onMouseDown={onClose}>
            <div
                className={`${styles.panel} ${styles[`panel--${size}`]}`}
                role="dialog"
                aria-modal="true"
                aria-label={title}
                onMouseDown={(e) => e.stopPropagation()}
            >
                <header className={styles.header}>
                    {icon ? <span className={styles.headIcon}>{icon}</span> : null}
                    <div className={styles.headText}>
                        <h2 className={styles.title}>{title}</h2>
                        {subtitle ? <p className={styles.subtitle}>{subtitle}</p> : null}
                    </div>
                    <button
                        className={styles.close}
                        onClick={onClose}
                        aria-label="Close"
                        type="button"
                    >
                        <X size={16} />
                    </button>
                </header>
                <div className={styles.body}>{children}</div>
                {footer ? <footer className={styles.footer}>{footer}</footer> : null}
            </div>
        </div>,
        document.body,
    );
}
