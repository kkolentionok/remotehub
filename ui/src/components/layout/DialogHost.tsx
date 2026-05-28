import { useEffect, useState } from "react";

import { useT } from "../../i18n";
import { credentials as credApi, groups as groupsApi, hosts as hostsApi } from "../../lib/ipc";
import type { HostGroupDto } from "../../lib/types";
import { useGroupsStore, useUiStore } from "../../store";
import { ConfirmDialog } from "../dialog/ConfirmDialog";
import { CredentialFormDialog } from "../dialog/CredentialFormDialog";
import { CredentialsListDialog } from "../dialog/CredentialsListDialog";
import { GroupFormDialog } from "../dialog/GroupFormDialog";
import { SettingsDialog } from "../settings/SettingsDialog";

/**
 * Renders the active dialog from UiStore.
 *
 * After Stage 1.5.2, host edit/create no longer use dialogs — they
 * live in HostDetail directly. Remaining dialogs cover destructive
 * confirms, group operations, and credential management.
 */
export function DialogHost() {
    const { t } = useT();
    const dialog = useUiStore((s) => s.dialog);
    const closeDialog = useUiStore((s) => s.closeDialog);
    const setDialog = useUiStore((s) => s.setDialog);
    const selectHost = useUiStore((s) => s.selectHost);

    const [renamingGroup, setRenamingGroup] = useState<HostGroupDto | null>(null);

    useEffect(() => {
        if (dialog.kind === "group-rename") {
            const g = useGroupsStore.getState().items.find((x) => x.id === dialog.groupId);
            if (g) setRenamingGroup(g);
        }
        if (dialog.kind === "none") {
            setRenamingGroup(null);
        }
    }, [dialog]);

    switch (dialog.kind) {
        case "none":
            return null;

        case "host-delete-confirm":
            return (
                <ConfirmDialog
                    open
                    onClose={closeDialog}
                    title={t("dialog.confirm.deleteHost.title")}
                    description={t("dialog.confirm.deleteHost.description")}
                    confirmLabel={t("common.delete")}
                    onConfirm={async () => {
                        await hostsApi.delete(dialog.hostId);
                        const sel = useUiStore.getState().selectedHostId;
                        if (sel === dialog.hostId) selectHost(null);
                    }}
                />
            );

        case "group-create":
            return (
                <GroupFormDialog
                    mode="create"
                    open
                    onClose={closeDialog}
                    parentId={dialog.parentId ?? null}
                />
            );

        case "group-rename":
            if (!renamingGroup) return null;
            return (
                <GroupFormDialog
                    mode="rename"
                    open
                    onClose={closeDialog}
                    group={renamingGroup}
                    onRequestDelete={() =>
                        setDialog({
                            kind: "group-delete-confirm",
                            groupId: renamingGroup.id,
                        })
                    }
                />
            );

        case "group-delete-confirm":
            return (
                <ConfirmDialog
                    open
                    onClose={closeDialog}
                    title={t("dialog.confirm.deleteGroup.title")}
                    description={t("dialog.confirm.deleteGroup.description")}
                    confirmLabel={t("common.delete")}
                    onConfirm={async () => {
                        await groupsApi.delete(dialog.groupId);
                    }}
                />
            );

        case "credentials-list":
            return <CredentialsListDialog open onClose={closeDialog} />;

        case "credential-create":
            return (
                <CredentialFormDialog
                    open
                    onClose={() => setDialog({ kind: "credentials-list" })}
                />
            );

        case "credential-delete-confirm":
            return (
                <ConfirmDialog
                    open
                    onClose={() => setDialog({ kind: "credentials-list" })}
                    title={t("dialog.confirm.deleteCredential.title")}
                    description={t("dialog.confirm.deleteCredential.description")}
                    confirmLabel={t("common.delete")}
                    onConfirm={async () => {
                        await credApi.delete(dialog.credentialId);
                    }}
                />
            );

        case "discard-changes-confirm":
            return (
                <ConfirmDialog
                    open
                    onClose={closeDialog}
                    title={t("dialog.confirm.discardChanges.title")}
                    description={t("dialog.confirm.discardChanges.description")}
                    confirmLabel={t("dialog.confirm.discardChanges.action")}
                    onConfirm={async () => {
                        dialog.onConfirm();
                        // Clear the draft after discarding.
                        useUiStore.getState().clearDraft();
                    }}
                />
            );

        case "settings":
            return <SettingsDialog onClose={closeDialog} />;
    }
}
