import { useEffect, useState } from "react";
import { Copy, Minus, Square, X } from "lucide-react";
import { getCurrentWindow } from "@tauri-apps/api/window";

import styles from "./WindowControls.module.css";

const appWindow = getCurrentWindow();

/** Custom minimize / maximize-restore / close buttons for the
 *  decorations-less window. Sit flush in the top-right of the tab bar. */
export function WindowControls() {
    const [maximized, setMaximized] = useState(false);

    useEffect(() => {
        let unlisten: (() => void) | undefined;
        void appWindow.isMaximized().then(setMaximized).catch(() => {});
        appWindow
            .onResized(() => {
                void appWindow.isMaximized().then(setMaximized).catch(() => {});
            })
            .then((u) => {
                unlisten = u;
            })
            .catch(() => {});
        return () => unlisten?.();
    }, []);

    return (
        <div className={styles.controls}>
            <button
                type="button"
                className={styles.btn}
                onClick={() => void appWindow.minimize()}
                aria-label="Minimize"
            >
                <Minus size={16} />
            </button>
            <button
                type="button"
                className={styles.btn}
                onClick={() => void appWindow.toggleMaximize()}
                aria-label={maximized ? "Restore" : "Maximize"}
            >
                {maximized ? <Copy size={12} /> : <Square size={12} />}
            </button>
            <button
                type="button"
                className={`${styles.btn} ${styles.close}`}
                onClick={() => void appWindow.close()}
                aria-label="Close"
            >
                <X size={16} />
            </button>
        </div>
    );
}
