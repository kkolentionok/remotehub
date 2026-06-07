import { useState } from "react";
import { Zap } from "lucide-react";

import { useT } from "../../i18n";
import {
    credentials as credApi,
    encodeSecret,
    hosts as hostsApi,
} from "../../lib/ipc";
import { formatApiError } from "../../lib/types";
import type { HostId } from "../../lib/types";
import { useHostsStore, useSessionsStore } from "../../store";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/Dialog";
import { Input, TextField } from "../ui/TextField";
import styles from "./HostFormDialog.module.css";

interface Props {
    open: boolean;
    onClose: () => void;
    target: { username: string; hostname: string; port: string };
    /** Saved host matching the address, if one exists (else we create it). */
    existingHostId: HostId | null;
}

/**
 * Compact one-shot connect prompt. Shows the parsed `[user@]host:port` and a
 * single password field. On connect it (1) creates the host if it isn't saved
 * yet, (2) creates + links a password credential (when a password was typed),
 * and (3) opens the session through the normal pipeline. Reuses the saved-host
 * connect path — the backend resolves the secret from the keychain by host id.
 */
export function QuickConnectDialog({ open, onClose, target, existingHostId }: Props) {
    const { t } = useT();
    const [password, setPassword] = useState("");
    const [submitting, setSubmitting] = useState(false);
    const [error, setError] = useState<string | null>(null);

    const port = target.port ? Number(target.port) : 22;
    const addr = `${target.username ? `${target.username}@` : ""}${target.hostname}:${port}`;

    async function connect() {
        setSubmitting(true);
        setError(null);
        try {
            let hostId = existingHostId;
            if (!hostId) {
                const created = await hostsApi.create({
                    name: target.username
                        ? `${target.username}@${target.hostname}`
                        : target.hostname,
                    protocol: "ssh",
                    hostname: target.hostname,
                    port,
                    username: target.username || null,
                });
                hostId = created.id;
            }
            // Password is optional: blank just attempts whatever the host
            // already offers (e.g. an agent/key on an existing host).
            if (password) {
                const cred = await credApi.create({
                    name: target.hostname,
                    kind: "password",
                    // The SSH auth username comes from the credential, so it
                    // must carry the login the user typed.
                    username: target.username,
                    secret: encodeSecret(password),
                });
                await credApi.linkHost({
                    host_id: hostId,
                    credential_id: cred.id,
                    set_as_default: true,
                });
            }
            // Refetch so the session pipeline gets the full, freshly-linked host.
            await useHostsStore.getState().load();
            const host = useHostsStore.getState().items.find((h) => h.id === hostId);
            onClose();
            if (host) void useSessionsStore.getState().open(host);
        } catch (e: unknown) {
            setError(formatApiError(e));
            setSubmitting(false);
        }
    }

    return (
        <Dialog
            open={open}
            onClose={onClose}
            title={t("quickConnect.title")}
            size="sm"
            footer={
                <>
                    <Button variant="secondary" onClick={onClose} disabled={submitting}>
                        {t("common.cancel")}
                    </Button>
                    <Button variant="primary" onClick={() => void connect()} disabled={submitting}>
                        <Zap size={14} />{" "}
                        {submitting ? t("quickConnect.connecting") : t("quickConnect.connect")}
                    </Button>
                </>
            }
        >
            <form
                className={styles.form}
                onSubmit={(e) => {
                    e.preventDefault();
                    void connect();
                }}
            >
                <div className={styles.quickTarget}>{addr}</div>
                <TextField label={t("dialog.host.passwordLabel")}>
                    <Input
                        type="password"
                        value={password}
                        onChange={(e) => setPassword(e.target.value)}
                        placeholder={t("dialog.host.credentialPasswordPlaceholder")}
                        autoFocus
                        autoComplete="off"
                    />
                </TextField>
                {error && <div className={styles.errorBox}>{error}</div>}
            </form>
        </Dialog>
    );
}
