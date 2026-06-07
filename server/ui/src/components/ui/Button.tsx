import type { ButtonHTMLAttributes, ReactNode } from "react";
import styles from "./Button.module.css";

type Variant = "primary" | "secondary" | "danger" | "ghost";
type Size = "sm" | "md";

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
    variant?: Variant;
    size?: Size;
    children: ReactNode;
}

/**
 * Button primitive. Variants:
 * - primary: accent-colored, main CTA per dialog/screen
 * - secondary: hairline border, neutral
 * - danger: destructive (delete, etc.)
 * - ghost: no border/bg, used inline in toolbars
 */
export function Button({
    variant = "secondary",
    size = "md",
    className,
    children,
    ...rest
}: ButtonProps) {
    const classes = [
        styles.btn,
        styles[`btn--${variant}`],
        styles[`btn--${size}`],
        className,
    ]
        .filter(Boolean)
        .join(" ");
    return (
        <button className={classes} {...rest}>
            {children}
        </button>
    );
}
