import { useCallback } from "react";
import { Maximize2, Minimize2, Pencil, PictureInPicture2, RefreshCw, X } from "lucide-react";

import { useT } from "../../i18n";
import type { SessionTab } from "../../store";
import {
    useHostsStore,
    useSessionsStore,
    useSettingsStore,
    useUiStore,
} from "../../store";
import { rdpSession as rdpSessionApi } from "../../lib/ipc";
import { Button } from "../ui/Button";
import { ConnState, connCategory } from "./ConnState";
import { ReauthPanel } from "./ReauthPanel";
import { Terminal } from "./Terminal";
import { RdpViewport } from "./RdpViewport";
import type { RdpInputEvent } from "../../lib/types";
import styles from "./SessionView.module.css";




export function SessionView({
    session,
    visible,
    focused,
    showHeader,
    tabId,
    inFocusMode,
}: {
    session: SessionTab;
    visible: boolean;
    focused: boolean;
    showHeader: boolean;
    tabId: string;
    inFocusMode: boolean;
}) {
    const { t } = useT();
    const gfxOn = useSettingsStore((s) => s.settings?.rdp_gfx ?? false);
    const close = useSessionsStore((s) => s.close);
    const open = useSessionsStore((s) => s.open);
    const setFocusPane = useSessionsStore((s) => s.setFocusPane);
    const setDraggingSession = useSessionsStore((s) => s.setDraggingSession);
    const acceptHostKey = useSessionsStore((s) => s.acceptHostKey);
    const rejectHostKey = useSessionsStore((s) => s.rejectHostKey);
    const detachRdpToWindow = useSessionsStore((s) => s.detachRdpToWindow);
    const redockRdp = useSessionsStore((s) => s.redockRdp);
    const detachTermToWindow = useSessionsStore((s) => s.detachTermToWindow);
    const redockTerm = useSessionsStore((s) => s.redockTerm);
    const isPoppedOut = useSessionsStore((s) => !!s.poppedOut[session.key]);
    const hosts = useHostsStore((s) => s.items);

    const isDead = session.state === "closed" || session.state === "failed";
    const isConnecting =
        session.state === "resolving" ||
        session.state === "connecting" ||
        session.state === "authenticating";

    // Connection presentation: category + identity for the handshake screen.
    const hostSummary = hosts.find((h) => h.id === session.hostId);
    const connAddr = hostSummary?.hostname ?? "";
    const connPort = hostSummary?.port ?? (session.protocol === "rdp" ? 3389 : 22);
    const connUser = hostSummary?.username ?? "";
    const category = connCategory(
        session.state,
        session.message,
        !!session.hostKey,
        session.authMethod,
    );
    const isAuthScreen = category === "auth" || category === "badpass";
    const attempts = useSessionsStore((s) => s.authAttempts[session.hostId] ?? 0);

    const reconnect = useCallback(() => {
        const host = hosts.find((h) => h.id === session.hostId);
        void close(session.key).then(() => {
            if (host) void open(host);
        });
    }, [hosts, session.hostId, session.key, close, open]);

    // Jump to the Storage tab with this host opened for editing — saves
    // hunting back through tabs after a failed connection.
    const setActiveTab = useSessionsStore((s) => s.setActiveTab);
    const selectHost = useUiStore((s) => s.selectHost);
    const editHost = useCallback(() => {
        setActiveTab(null);
        selectHost(session.hostId);
    }, [setActiveTab, selectHost, session.hostId]);

    // RDP input sink: forward viewport events to the session actor. Mouse
    // is handled server-side; keyboard/modifier-sync land in 2b-2b (the
    // actor currently ignores those). Fire-and-forget — input is lossy by
    // nature and we don't want to await per mouse-move.
    const handleRdpInput = useCallback(
        (ev: RdpInputEvent) => {
            const sid = session.sessionId;
            if (!sid) return; // not connected yet
            void rdpSessionApi.sendInput({ session_id: sid, event: ev });
        },
        [session.sessionId],
    );

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
                    {session.protocol !== "rdp" && !isDead && !isPoppedOut && (
                        <button
                            type="button"
                            className={styles.headerBtn}
                            title={t("session.popOut")}
                            onClick={() => void detachTermToWindow(session.key)}
                        >
                            <PictureInPicture2 size={13} />
                        </button>
                    )}
                    <button
                        type="button"
                        className={styles.headerBtn}
                        title={inFocusMode ? t("pane.exitFocus") : t("pane.focus")}
                        onClick={() =>
                            setFocusPane(tabId, inFocusMode ? null : session.key)
                        }
                    >
                        {inFocusMode ? <Minimize2 size={13} /> : <Maximize2 size={13} />}
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
            <div className={styles.body}>
                {session.hostKey ? (
                    <ConnState
                        category="hostkey"
                        state={session.state}
                        protocol={session.protocol as "ssh" | "rdp"}
                        hostName={session.title}
                        user={connUser}
                        addr={connAddr}
                        port={connPort}
                        changed={session.hostKey.changed}
                        fingerprint={session.hostKey.fingerprint}
                        keyType={session.hostKey.keyType}
                    >
                        <Button
                            variant="primary"
                            style={{ background: "var(--color-warn)", color: "#14110a" }}
                            onClick={() => void acceptHostKey(session.key)}
                        >
                            {t("session.hostKey.accept")}
                        </Button>
                        <Button
                            variant="ghost"
                            onClick={() => void rejectHostKey(session.key)}
                        >
                            {t("session.hostKey.reject")}
                        </Button>
                    </ConnState>
                ) : isDead ? (
                    <ConnState
                        category={category}
                        state={session.state}
                        protocol={session.protocol as "ssh" | "rdp"}
                        hostName={session.title}
                        user={connUser}
                        addr={connAddr}
                        port={connPort}
                        rawMessage={session.message}
                        attempt={isAuthScreen ? attempts : undefined}
                        reauthSlot={
                            isAuthScreen ? (
                                <ReauthPanel
                                    hostId={session.hostId}
                                    sessionKey={session.key}
                                    defaultMethod={
                                        category === "badpass" ? "password" : "key"
                                    }
                                />
                            ) : undefined
                        }
                    >
                        {!isAuthScreen && (
                            <Button variant="primary" onClick={reconnect}>
                                <RefreshCw size={14} /> {t("session.reconnect")}
                            </Button>
                        )}
                        <Button variant="ghost" onClick={editHost}>
                            <Pencil size={14} /> {t("conn.editHost")}
                        </Button>
                    </ConnState>
                ) : (
                    <>
                        {session.protocol === "rdp" ? (
                            isPoppedOut ? (
                                <div className={styles.poppedOut}>
                                    <PictureInPicture2 size={30} strokeWidth={1.5} />
                                    <p>{t("session.poppedOutTitle")}</p>
                                    <Button
                                        variant="secondary"
                                        onClick={() => void redockRdp(session.key)}
                                    >
                                        {t("session.redock")}
                                    </Button>
                                </div>
                            ) : (
                                <RdpViewport
                                    sessionKey={session.key}
                                    width={session.rdpWidth ?? 1280}
                                    height={session.rdpHeight ?? 800}
                                    onInput={handleRdpInput}
                                    hostLabel={session.title}
                                    connected={session.state === "ready"}
                                    onPopOut={() => void detachRdpToWindow(session.key)}
                                    onLocalClipboard={(text) => {
                                        const sid = session.sessionId;
                                        if (sid) void rdpSessionApi.setClipboard(sid, text);
                                    }}
                                    onLocalClipboardImage={(w, h, rgbaBase64) => {
                                        const sid = session.sessionId;
                                        if (sid) void rdpSessionApi.setClipboardImage(sid, w, h, rgbaBase64);
                                    }}
                                    onResize={(w, h) => {
                                        const sid = session.sessionId;
                                        if (sid) void rdpSessionApi.resize(sid, w, h);
                                    }}
                                    enableDynamicResize={gfxOn}
                                    onKbdCapture={(on) => {
                                        const sid = session.sessionId;
                                        if (sid) void rdpSessionApi.kbdCapture(sid, on);
                                    }}
                                />
                            )
                        ) : isPoppedOut ? (
                            <div className={styles.poppedOut}>
                                <PictureInPicture2 size={30} strokeWidth={1.5} />
                                <p>{t("session.poppedOutTitle")}</p>
                                <Button
                                    variant="secondary"
                                    onClick={() => void redockTerm(session.key)}
                                >
                                    {t("session.redock")}
                                </Button>
                            </div>
                        ) : (
                            <Terminal
                                sessionKey={session.key}
                                visible={visible}
                                focused={focused}
                            />
                        )}
                        {isConnecting && !isPoppedOut && (
                            <div className={styles.connecting}>
                                <ConnState
                                    category="connecting"
                                    state={session.state}
                                    protocol={session.protocol as "ssh" | "rdp"}
                                    hostName={session.title}
                                    user={connUser}
                                    addr={connAddr}
                                    port={connPort}
                                >
                                    <Button onClick={() => void close(session.key)}>
                                        <X size={14} /> {t("session.cancelConnect")}
                                    </Button>
                                </ConnState>
                            </div>
                        )}
                    </>
                )}
            </div>
        </main>
    );
}

