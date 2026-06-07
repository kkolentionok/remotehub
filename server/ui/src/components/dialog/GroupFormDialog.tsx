import { useEffect, useState } from "react";
import { Trash2 } from "lucide-react";

import { useT } from "../../i18n";
import { groups as groupsApi } from "../../lib/ipc";
import { formatApiError } from "../../lib/types";
import type { GroupId, HostGroupDto } from "../../lib/types";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/Dialog";
import { Input, TextField } from "../ui/TextField";
import styles from "./HostFormDialog.module.css";

interface CreateProps {
    mode: "create";
    open: boolean;
    onClose: () => void;
    parentId?: GroupId | null;
}

interface RenameProps {
    mode: "rename";
    open: boolean;
    onClose: () => void;
    group: HostGroupDto;
    /** Hand off to the confirm-delete dialog. */
    onRequestDelete: () => void;
}

type Props = CreateProps | RenameProps;

export function GroupFormDialog(props: Props) {
    const { t } = useT();
    const [name, setName] = useState(props.mode === "rename" ? props.group.name : "");
    const [submitting, setSubmitting] = useState(false);
    const [error, setError] = useState<string | null>(null);

    useEffect(() => {
        if (props.open) {
            setName(props.mode === "rename" ? props.group.name : "");
            setError(null);
        }
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [props.open]);

    async function submit() {
        setSubmitting(true);
        setError(null);
        try {
            if (props.mode === "create") {
                await groupsApi.create({ name: name.trim(), parent_id: props.parentId ?? null });
            } else {
                await groupsApi.rename({ id: props.group.id, name: name.trim() });
            }
            props.onClose();
        } catch (e: unknown) {
            setError(formatApiError(e));
        } finally {
            setSubmitting(false);
        }
    }

    const title =
        props.mode === "create" ? t("dialog.group.newTitle") : t("dialog.group.editTitle");

    return (
        <Dialog
            open={props.open}
            onClose={props.onClose}
            title={title}
            size="sm"
            footer={
                <>
                    {props.mode === "rename" && (
                        <>
                            <Button
                                variant="danger"
                                onClick={props.onRequestDelete}
                                disabled={submitting}
                            >
                                <Trash2 size={14} /> {t("common.delete")}
                            </Button>
                            <div style={{ flex: 1 }} />
                        </>
                    )}
                    <Button variant="secondary" onClick={props.onClose} disabled={submitting}>
                        {t("common.cancel")}
                    </Button>
                    <Button variant="primary" onClick={submit} disabled={submitting}>
                        {submitting
                            ? t("common.saving")
                            : props.mode === "create"
                              ? t("common.create")
                              : t("common.save")}
                    </Button>
                </>
            }
        >
            <form
                className={styles.form}
                onSubmit={(e) => {
                    e.preventDefault();
                    void submit();
                }}
            >
                <TextField label={t("dialog.host.label")}>
                    <Input
                        value={name}
                        onChange={(e) => setName(e.target.value)}
                        placeholder={t("dialog.group.namePlaceholder")}
                        required
                        autoFocus
                    />
                </TextField>
                {error && <div className={styles.errorBox}>{error}</div>}
            </form>
        </Dialog>
    );
}
