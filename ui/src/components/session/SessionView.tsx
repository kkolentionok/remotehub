import { useCallback, useState } from "react";
import { Columns2, KeyRound, Loader2, Pencil, RefreshCw, Rows2, Server, X } from "lucide-react";

import { useT } from "../../i18n";
import type { SessionTab } from "../../store";
import {
    useCredentialsStore,
    useHostsStore,
    useSessionsStore,
    useUiStore,
} from "../../store";
import { credentials as credApi, hosts as hostsApi, encodeSecret } from "../../lib/ipc";
import { Button } from "../ui/Button";
import { AddKeyModal, SavedCredentialPicker } from "../host/HostDetail";
import { EmptyState } from "../ui/EmptyState";
import { Input } from "../ui/TextField";
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

    // Jump to the Storage tab with this host opened for editing — saves
    // hunting back through tabs after a failed connection.
    const setActiveTab = useSessionsStore((s) => s.setActiveTab);
    const selectHost = useUiStore((s) => s.selectHost);
    const editHost = useCallback(() => {
        setActiveTab(null);
        selectHost(session.hostId);
    }, [setActiveTab, selectHost, session.hostId]);

    // Inline re-auth on auth failure: type a password (or pick/add an SSH
    // key) and reconnect without leaving the session tab.
    const [pw, setPw] = useState("");
    const [reauthBusy, setReauthBusy] = useState(false);
    const [keyPickerOpen, setKeyPickerOpen] = useState(false);
    const [addKeyOpen, setAddKeyOpen] = useState(false);
    const authFailed =
        isDead && (session.message ?? "").toLowerCase().includes("auth");

    // Link a credential to the host (if not already) and reconnect.
    const linkAndReconnect = useCallback(
        async (credentialId: string) => {
            if (reauthBusy) return;
            setReauthBusy(true);
            try {
                const host = await hostsApi.get(session.hostId);
                const ids = host.credential_ids ?? [];
                if (!ids.includes(credentialId)) {
                    await credApi.linkHost({
                        host_id: host.id,
                        credential_id: credentialId,
                        set_as_default: ids.length === 0,
                    });
                }
                const fresh = await hostsApi.get(host.id);
                await close(session.key);
                void open(fresh);
            } catch {
                setReauthBusy(false);
            }
        },
        [session.hostId, session.key, reauthBusy, close, open],
    );

    // Create a new SSH key (paste/import), link it, and reconnect.
    const addKeyAndReconnect = useCallback(
        async ({
            key,
            passphrase,
            name,
        }: {
            key: string;
            passphrase: string;
            name: string;
        }) => {
            const creds = useCredentialsStore.getState().items;
            const taken = new Set(creds.map((c) => c.name));
            let n = name.trim() || "key";
            let i = 2;
            while (taken.has(n)) n = `${name.trim() || "key"} ${i++}`;
            const created = await credApi.create({
                name: n,
                kind: "ssh_key",
                username: "",
                secret: encodeSecret(key.trim()),
                passphrase: passphrase !== "" ? encodeSecret(passphrase) : undefined,
            });
            await linkAndReconnect(created.id);
        },
        [linkAndReconnect],
    );

    const connectWithPassword = useCallback(async () => {
        const summary = hosts.find((h) => h.id === session.hostId);
        if (!summary || pw === "" || reauthBusy) return;
        setReauthBusy(true);
        try {
            // The hosts store holds summaries; fetch the full host for its
            // linked credential ids.
            const host = await hostsApi.get(session.hostId);
            const creds = useCredentialsStore.getState().items;
            const ids = host.credential_ids ?? [];
            const pwCred = creds.find(
                (c) => ids.includes(c.id) && c.kind === "password",
            );
            if (pwCred) {
                await credApi.rotateSecret({
                    id: pwCred.id,
                    secret: encodeSecret(pw),
                });
            } else {
                const base =
                    host.display_name || host.name || host.hostname || "password";
                const taken = new Set(creds.map((c) => c.name));
                let name = base;
                let i = 2;
                while (taken.has(name)) name = `${base} ${i++}`;
                const created = await credApi.create({
                    name,
                    kind: "password",
                    username: "",
                    secret: encodeSecret(pw),
                });
                await credApi.linkHost({
                    host_id: host.id,
                    credential_id: created.id,
                    set_as_default: ids.length === 0,
                });
            }
            const fresh = await hostsApi.get(host.id);
            setPw("");
            await close(session.key);
            void open(fresh);
        } catch {
            setReauthBusy(false);
        }
    }, [hosts, session.hostId, session.key, pw, reauthBusy, close, open]);

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
                        {authFailed ? (
                            <div className={styles.reauth}>
                                <div className={styles.reauthTitle}>
                                    {t("session.failed")}
                                </div>
                                <div className={styles.reauthMsg}>
                                    {session.message ?? undefined}
                                </div>
                                <div className={styles.reauthPwRow}>
                                    <Input
                                        className={styles.reauthPwInput}
                                        type="password"
                                        value={pw}
                                        onChange={(e) => setPw(e.target.value)}
                                        placeholder={t(
                                            "dialog.host.credentialPasswordPlaceholder",
                                        )}
                                        autoComplete="off"
                                        autoFocus
                                        onKeyDown={(e) => {
                                            if (e.key === "Enter")
                                                void connectWithPassword();
                                        }}
                                    />
                                    <button
                                        type="button"
                                        className={styles.reauthKeyBtn}
                                        onClick={() => setKeyPickerOpen((v) => !v)}
                                        title={t("dialog.host.authKind.key")}
                                        aria-label={t("dialog.host.authKind.key")}
                                    >
                                        <KeyRound size={16} />
                                    </button>
                                    {keyPickerOpen && (
                                        <SavedCredentialPicker
                                            onClose={() => setKeyPickerOpen(false)}
                                            onPick={async (id) => {
                                                setKeyPickerOpen(false);
                                                await linkAndReconnect(id);
                                            }}
                                            onAddNew={() => {
                                                setKeyPickerOpen(false);
                                                setAddKeyOpen(true);
                                            }}
                                        />
                                    )}
                                </div>
                                <div className={styles.reauthButtons}>
                                    <Button
                                        variant="primary"
                                        className={styles.reauthConnectBtn}
                                        disabled={pw === "" || reauthBusy}
                                        onClick={() => void connectWithPassword()}
                                    >
                                        <RefreshCw size={14} />{" "}
                                        {t("session.connectSave")}
                                    </Button>
                                </div>
                                {addKeyOpen && (
                                    <AddKeyModal
                                        onClose={() => setAddKeyOpen(false)}
                                        onAdd={async (args) => {
                                            setAddKeyOpen(false);
                                            await addKeyAndReconnect(args);
                                        }}
                                    />
                                )}
                            </div>
                        ) : (
                            <EmptyState
                                title={
                                    session.state === "failed"
                                        ? t("session.failed")
                                        : t("session.closed")
                                }
                                description={session.message ?? undefined}
                                action={
                                    <div className={styles.deadActions}>
                                        <Button
                                            variant="primary"
                                            onClick={reconnect}
                                        >
                                            <RefreshCw size={14} />{" "}
                                            {t("session.reconnect")}
                                        </Button>
                                        <Button
                                            variant="secondary"
                                            onClick={editHost}
                                        >
                                            <Pencil size={14} /> {t("common.edit")}
                                        </Button>
                                    </div>
                                }
                            />
                        )}
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
