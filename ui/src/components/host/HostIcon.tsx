import { Server } from "lucide-react";

import styles from "../sidebar/Sidebar.module.css";

/**
 * Host icon slot in the sidebar (and elsewhere). Stage 1.7 renders a
 * generic server glyph for everything; Stage 2.2 will switch on
 * `detectedOs` (populated after the first SSH session reads
 * `/etc/os-release`) and pick a Simple Icons SVG for Ubuntu, Debian,
 * CentOS, RHEL, Windows, etc.
 *
 * Keeping this as a separate component so adding the switch later
 * doesn't require touching Sidebar or HostDetail.
 */
export function HostIcon({
    detectedOs: _detectedOs,
}: {
    /** Reserved for Stage 2.2. Ignored for now. */
    detectedOs?: string | null;
}) {
    return (
        <span className={styles.hostIcon}>
            <Server size={14} />
        </span>
    );
}
