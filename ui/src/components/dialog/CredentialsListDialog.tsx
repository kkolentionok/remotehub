import { KeyRound, Plus, Trash2 } from "lucide-react";

import { useT } from "../../i18n";
import type { CredentialDto } from "../../lib/types";
import { useCredentialsStore, useUiStore } from "../../store";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/Dialog";
import { EmptyState } from "../ui/EmptyState";
import styles from "./CredentialsListDialog.module.css";

interface Props {
    open: boolean;
    onClose: () => void;
}

export function CredentialsListDialog({ open, onClose }: Props) {
    const { t } = useT();
    const credentials = useCredentialsStore((s) => s.items);
    const setDialog = useUiStore((s) => s.setDialog);

    return (
        <Dialog
            open={open}
            onClose={onClose}
            title={t("dialog.credentials.title")}
            size="lg"
            footer={
                <>
                    <Button variant="secondary" onClick={onClose}>
                        {t("common.done")}
                    </Button>
                    <Button
                        variant="primary"
                        onClick={() => setDialog({ kind: "credential-create" })}
                    >
                        <Plus size={14} /> {t("dialog.credentials.new")}
                    </Button>
                </>
            }
        >
            {credentials.length === 0 ? (
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
            ) : (
                <ul className={styles.list}>
                    {credentials.map((c) => (
                        <CredentialRow key={c.id} credential={c} />
                    ))}
                </ul>
            )}
        </Dialog>
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
