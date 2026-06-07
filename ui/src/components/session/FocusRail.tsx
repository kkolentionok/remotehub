import { PanelLeftClose, X } from "lucide-react";

import { useT } from "../../i18n";
import { leafKeys } from "../../lib/paneTree";
import { useHostsStore, useSessionsStore } from "../../store";
import styles from "./FocusRail.module.css";

/** Status-dot colour class for a session state. */
function dotClass(state: string): string | undefined {
    switch (state) {
        case "ready":
            return styles.dotReady;
        case "resolving":
        case "connecting":
        case "authenticating":
        case "host_key_pending":
            return styles.dotWarn;
        case "failed":
        case "closed":
            return styles.dotBad;
        default:
            return styles.dotIdle;
    }
}

/**
 * Left rail shown only in focus mode: lists every live session of the focused
 * tab. Clicking a row focuses that session; the active row exposes a close
 * button. The header's collapse button exits focus mode back to the split.
 */
export function FocusRail({ tabId }: { tabId: string }) {
    const { t } = useT();
    const tab = useSessionsStore((s) => s.tabs.find((x) => x.id === tabId));
    const sessions = useSessionsStore((s) => s.sessions);
    const setFocusPane = useSessionsStore((s) => s.setFocusPane);
    const close = useSessionsStore((s) => s.close);
    const hosts = useHostsStore((s) => s.items);

    if (!tab) return null;
    const keys = leafKeys(tab.root);

    return (
        <aside className={styles.rail}>
            <div className={styles.head}>
                <span className={styles.headTitle}>{t("focusRail.title")}</span>
                <span className={styles.count}>{keys.length}</span>
                <span className={styles.headSpacer} />
                <button
                    type="button"
                    className={styles.exitBtn}
                    title={t("pane.exitFocus")}
                    onClick={() => setFocusPane(tabId, null)}
                >
                    <PanelLeftClose size={15} />
                </button>
            </div>
            <div className={styles.list}>
                {keys.map((k) => {
                    const sess = sessions.find((x) => x.key === k);
                    if (!sess) return null;
                    const host = hosts.find((h) => h.id === sess.hostId);
                    const user = host?.username ?? "";
                    const active = tab.focusKey === k;
                    return (
                        <div
                            key={k}
                            className={`${styles.row} ${active ? styles.rowOn : ""}`}
                            onClick={() => setFocusPane(tabId, k)}
                        >
                            <span className={`${styles.dot} ${dotClass(sess.state)}`} />
                            <div className={styles.meta}>
                                <div className={styles.name}>{sess.title}</div>
                                <div className={styles.sub}>
                                    {sess.protocol}
                                    {user ? ` · ${user}` : ""}
                                </div>
                            </div>
                            {active && (
                                <button
                                    type="button"
                                    className={styles.rowX}
                                    title={t("common.close")}
                                    onClick={(e) => {
                                        e.stopPropagation();
                                        void close(k);
                                    }}
                                >
                                    <X size={14} />
                                </button>
                            )}
                        </div>
                    );
                })}
            </div>
        </aside>
    );
}
