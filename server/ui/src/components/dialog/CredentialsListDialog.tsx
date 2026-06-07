import { useEffect, useState } from "react";
import { KeyRound, Plus, Server, ShieldCheck, Trash2 } from "lucide-react";

import { useT } from "../../i18n";
import type { CredentialDto, KnownHostEntryDto, RdpCertEntryDto } from "../../lib/types";
import { knownHosts as knownHostsApi, rdpCerts as rdpCertsApi } from "../../lib/ipc";
import { useCredentialsStore, useUiStore } from "../../store";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/Dialog";
import { EmptyState } from "../ui/EmptyState";
import styles from "./CredentialsListDialog.module.css";

interface Props {
    open: boolean;
    onClose: () => void;
}

type Tab = "credentials" | "knownHosts" | "rdpCerts";

export function CredentialsListDialog({ open, onClose }: Props) {
    const { t } = useT();
    const setDialog = useUiStore((s) => s.setDialog);
    const [tab, setTab] = useState<Tab>("credentials");

    return (
        <Dialog
            open={open}
            onClose={onClose}
            title={t("dialog.security.title")}
            size="lg"
            footer={
                <>
                    <Button variant="secondary" onClick={onClose}>
                        {t("common.done")}
                    </Button>
                    {tab === "credentials" && (
                        <Button
                            variant="primary"
                            onClick={() => setDialog({ kind: "credential-create" })}
                        >
                            <Plus size={14} /> {t("dialog.credentials.new")}
                        </Button>
                    )}
                </>
            }
        >
            <div className={styles.tabs}>
                <button
                    type="button"
                    className={`${styles.tab} ${tab === "credentials" ? styles.tabActive : ""}`}
                    onClick={() => setTab("credentials")}
                >
                    <KeyRound size={14} /> {t("dialog.credentials.title")}
                </button>
                <button
                    type="button"
                    className={`${styles.tab} ${tab === "knownHosts" ? styles.tabActive : ""}`}
                    onClick={() => setTab("knownHosts")}
                >
                    <Server size={14} /> {t("dialog.knownHosts.title")}
                </button>
                <button
                    type="button"
                    className={`${styles.tab} ${tab === "rdpCerts" ? styles.tabActive : ""}`}
                    onClick={() => setTab("rdpCerts")}
                >
                    <ShieldCheck size={14} /> {t("dialog.rdpCerts.title")}
                </button>
            </div>

            {tab === "credentials" ? (
                <CredentialsTab />
            ) : tab === "knownHosts" ? (
                <KnownHostsTab open={open} />
            ) : (
                <RdpCertsTab open={open} />
            )}
        </Dialog>
    );
}

function CredentialsTab() {
    const { t } = useT();
    const credentials = useCredentialsStore((s) => s.items);
    const setDialog = useUiStore((s) => s.setDialog);

    if (credentials.length === 0) {
        return (
            <EmptyState
                icon={<KeyRound size={32} />}
                title={t("dialog.credentials.empty.title")}
                description={t("dialog.credentials.empty.description")}
                action={
                    <Button
                        variant="primary"
                        onClick={() => setDialog({ kind: "credential-create" })}
                    >
                        <Plus size={14} /> {t("dialog.credentials.add")}
                    </Button>
                }
            />
        );
    }

    return (
        <ul className={styles.list}>
            {credentials.map((c) => (
                <CredentialRow key={c.id} credential={c} />
            ))}
        </ul>
    );
}

function CredentialRow({ credential }: { credential: CredentialDto }) {
    const { t } = useT();
    const setDialog = useUiStore((s) => s.setDialog);

    return (
        <li className={styles.row}>
            <div className={styles.info}>
                <div className={styles.name}>{credential.name}</div>
                <div className={styles.meta}>
                    <span className={styles.kind}>{credential.kind.replace("_", " ")}</span>
                    {credential.username && (
                        <>
                            <span className={styles.dot}>·</span>
                            <span className={styles.username}>{credential.username}</span>
                        </>
                    )}
                </div>
            </div>
            <Button
                variant="ghost"
                size="sm"
                onClick={() =>
                    setDialog({
                        kind: "credential-delete-confirm",
                        credentialId: credential.id,
                    })
                }
                aria-label={t("common.delete")}
                title={t("common.delete")}
            >
                <Trash2 size={14} />
            </Button>
        </li>
    );
}

function KnownHostsTab({ open }: { open: boolean }) {
    const { t, formatDate } = useT();
    const [entries, setEntries] = useState<KnownHostEntryDto[] | null>(null);

    const reload = () => {
        void knownHostsApi
            .list()
            .then((r) => setEntries(r.entries))
            .catch(() => setEntries([]));
    };

    // Reload whenever the dialog (re)opens.
    useEffect(() => {
        if (open) reload();
    }, [open]);

    const forget = async (e: KnownHostEntryDto) => {
        try {
            await knownHostsApi.forget(e.hostname, e.port);
        } catch {
            /* ignore — refetch shows truth */
        }
        reload();
    };

    if (entries === null) {
        return <div className={styles.loading}>{t("common.loading")}</div>;
    }
    if (entries.length === 0) {
        return (
            <EmptyState
                icon={<Server size={32} />}
                title={t("dialog.knownHosts.empty.title")}
                description={t("dialog.knownHosts.empty.description")}
            />
        );
    }

    return (
        <ul className={styles.list}>
            {entries.map((e) => (
                <li key={`${e.hostname}:${e.port}`} className={styles.row}>
                    <div className={styles.info}>
                        <div className={styles.name}>
                            {e.hostname}
                            <span className={styles.port}>:{e.port}</span>
                        </div>
                        <div className={styles.meta}>
                            <span className={styles.kind}>{e.key_type}</span>
                            <span className={styles.dot}>·</span>
                            <span
                                className={styles.fingerprint}
                                title={`SHA256:${e.fingerprint_sha256}`}
                            >
                                SHA256:{e.fingerprint_sha256}
                            </span>
                            <span className={styles.dot}>·</span>
                            <span className={styles.username}>
                                {formatDate(e.created_at)}
                            </span>
                        </div>
                    </div>
                    <Button
                        variant="ghost"
                        size="sm"
                        onClick={() => void forget(e)}
                        aria-label={t("dialog.knownHosts.forget")}
                        title={t("dialog.knownHosts.forget")}
                    >
                        <Trash2 size={14} />
                    </Button>
                </li>
            ))}
        </ul>
    );
}

function RdpCertsTab({ open }: { open: boolean }) {
    const { t, formatDate } = useT();
    const [entries, setEntries] = useState<RdpCertEntryDto[] | null>(null);

    const reload = () => {
        void rdpCertsApi
            .list()
            .then((r) => setEntries(r.entries))
            .catch(() => setEntries([]));
    };

    useEffect(() => {
        if (open) reload();
    }, [open]);

    const forget = async (e: RdpCertEntryDto) => {
        try {
            await rdpCertsApi.forget(e.hostname, e.port);
        } catch {
            /* ignore — refetch shows truth */
        }
        reload();
    };

    if (entries === null) {
        return <div className={styles.loading}>{t("common.loading")}</div>;
    }
    if (entries.length === 0) {
        return (
            <EmptyState
                icon={<ShieldCheck size={32} />}
                title={t("dialog.rdpCerts.empty.title")}
                description={t("dialog.rdpCerts.empty.description")}
            />
        );
    }

    return (
        <ul className={styles.list}>
            {entries.map((e) => (
                <li key={`${e.hostname}:${e.port}`} className={styles.row}>
                    <div className={styles.info}>
                        <div className={styles.name}>
                            {e.hostname}
                            <span className={styles.port}>:{e.port}</span>
                        </div>
                        <div className={styles.meta}>
                            <span className={styles.kind}>{e.subject}</span>
                            <span className={styles.dot}>·</span>
                            <span
                                className={styles.fingerprint}
                                title={`SHA256:${e.fingerprint_sha256}`}
                            >
                                SHA256:{e.fingerprint_sha256}
                            </span>
                            <span className={styles.dot}>·</span>
                            <span className={styles.username}>
                                {formatDate(e.trusted_at)}
                            </span>
                        </div>
                    </div>
                    <Button
                        variant="ghost"
                        size="sm"
                        onClick={() => void forget(e)}
                        aria-label={t("dialog.knownHosts.forget")}
                        title={t("dialog.knownHosts.forget")}
                    >
                        <Trash2 size={14} />
                    </Button>
                </li>
            ))}
        </ul>
    );
}
