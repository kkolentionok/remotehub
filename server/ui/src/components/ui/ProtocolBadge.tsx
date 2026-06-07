import type { Protocol } from "../../lib/types";
import styles from "./ProtocolBadge.module.css";

interface Props {
    protocol: Protocol;
    /** Inline small variant for sidebar; default is normal size. */
    size?: "sm" | "md";
}

/**
 * Tiny colored label for SSH/RDP. SSH = green, RDP = blue (from tokens).
 */
export function ProtocolBadge({ protocol, size = "md" }: Props) {
    return (
        <span
            className={`${styles.badge} ${styles[`badge--${protocol}`]} ${styles[`badge--${size}`]}`}
        >
            {protocol.toUpperCase()}
        </span>
    );
}
