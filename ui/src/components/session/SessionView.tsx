import { useCallback } from "react";
import { Columns2, Loader2, RefreshCw, Rows2, Server, X } from "lucide-react";

import { useT } from "../../i18n";
import type { SessionTab } from "../../store";
import { useHostsStore, useSessionsStore } from "../../store";
import { Button } from "../ui/Button";
import { EmptyState } from "../ui/EmptyState";
import { Terminal } from "./Terminal";
import styles from "./SessionView.module.css";

export function SessionView({
    session,
    visible,
    focused,
    showHeader,
}: {
    session: SessionTab;
    visible: boolean;
    focused: boolean;
    showHeader: boolean;
}) {
    const { t } = useT();
    const close = useSessionsStore((s) => s.close);
    const open = useSessionsStore((s) => s.open);
    const requestSplit = useSessionsStore((s) => s.requestSplit);
    const setDraggingSession = useSessionsStore((s) => s.setDraggingSession);
    const acceptHostKey = useSessionsStore((s) => s.acceptHostKey);
    const rejectHostKey = useSessionsStore((s) => s.rejectHostKey);
    const hosts = useHostsStore((s) => s.items);

    const isDead = session.state === "closed" || session.state === "failed";
    const isConnecting =
        session.state === "connecting" || session.state === "authenticating";

    const reconnect = useCallback(() => {
        const host = hosts.find((h) => h.id === session.hostId);
        void close(session.key).then(() => {
            if (host) void open(host);
        });
    }, [hosts, session.hostId, session.key, close, open]);

    return (
        <main className={styles.view}>
            {showHeader && (
                <div
                    className={styles.paneHeader}
                    draggable
                    onDragStart={(e) => {
                        setDraggingSession(session.key);
                        e.dataTransfer.effectAllowed = "move";
                        e.dataTransfer.setData("text/plain", session.key);
                    }}
                    onDragEnd={() => setDraggingSession(null)}
                >
                    <span className={`${styles.headerDot} ${styles[`dot--${session.state}`] ?? ""}`} />
                    <span className={styles.headerTitle}>{session.title}</span>
                    <span className={styles.headerProto}>{session.protocol}</span>
                    <span className={styles.headerSpacer} />
                    <button
                        type="button"
                        className={styles.headerBtn}
                        title={t("pane.splitRight")}
                        onClick={() => requestSplit("row")}
                    >
                        <Columns2 size={13} />
                    </button>
                    <button
                        type="button"
                        className={styles.headerBtn}
                        title={t("pane.splitDown")}
                        onClick={() => requestSplit("col")}
                    >
                        <Rows2 size={13} />
                    </button>
                    <button
                        type="button"
                        className={styles.headerBtn}
                        title={t("common.close")}
                        onClick={() => void close(session.key)}
                    >
                        <X size={13} />
                    </button>
                </div>
            )}
            {session.hostKey && (
                <div className={styles.hostKeyPrompt}>
                    <div className={styles.hostKeyText}>
                        {t("session.hostKey.prompt")}
                        <code className={styles.fingerprint}>
                            {session.hostKey.keyType} · {session.hostKey.fingerprint}
                        </code>
                    </div>
                    <div className={styles.hostKeyActions}>
                        <Button
                            variant="primary"
                            onClick={() => void acceptHostKey(session.key)}
                        >
                            {t("session.hostKey.accept")}
                        </Button>
                        <Button onClick={() => void rejectHostKey(session.key)}>
                            {t("session.hostKey.reject")}
                        </Button>
                    </div>
                </div>
            )}

            <div className={styles.body}>
                {isDead ? (
                    <div className={styles.dead}>
                        <EmptyState
                            title={
                                session.state === "failed"
                                    ? t("session.failed")
                                    : t("session.closed")
                            }
                            description={session.message ?? undefined}
                            action={
                                <Button variant="primary" onClick={reconnect}>
                                    <RefreshCw size={14} /> {t("session.reconnect")}
                                </Button>
                            }
                        />
                    </div>
                ) : (
                    <>
                        <Terminal
                            sessionKey={session.key}
                            visible={visible}
                            focused={focused}
                        />
                        {isConnecting && <ConnectingOverlay session={session} />}
                    </>
                )}
            </div>
        </main>
    );
}

/** Termius-style connection card shown over the terminal until ready. */
function ConnectingOverlay({ session }: { session: SessionTab }) {
    const { t } = useT();
    const close = useSessionsStore((s) => s.close);
    const host = useHostsStore((s) =>
        s.items.find((h) => h.id === session.hostId),
    );
    const target = host ? `${host.hostname}:${host.port}` : "";

    return (
        <div className={styles.connecting}>
            <div className={styles.card}>
                <div className={styles.cardHead}>
                    <div className={styles.cardIcon}>
                        <Server size={20} />
                    </div>
                    <div className={styles.cardHeadText}>
                        <div className={styles.cardTitle}>{session.title}</div>
                        <div className={styles.cardSub}>
                            {session.protocol.toUpperCase()}
                            {target ? ` · ${target}` : ""}
                        </div>
                    </div>
                </div>

                <div className={styles.progress}>
                    <div className={styles.progressBar} />
                </div>

                <div className={styles.statusRow}>
                    <Loader2 size={15} className={styles.spin} />
                    <span>{t(`session.state.${session.state}`)}…</span>
                </div>

                <div className={styles.cardActions}>
                    <Button onClick={() => void close(session.key)}>
                        {t("common.close")}
                    </Button>
                </div>
            </div>
        </div>
    );
}
