import { useEffect, useMemo, useState } from "react";

import { useT } from "../../i18n";
import { credentials as credApi, encodeSecret, hosts as hostsApi } from "../../lib/ipc";
import { formatApiError } from "../../lib/types";
import type {
    CredentialId,
    GroupId,
    HostCreateRequest,
    HostFullDto,
    HostUpdateRequest,
    Protocol,
} from "../../lib/types";
import { useCredentialsStore, useGroupsStore } from "../../store";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/Dialog";
import { Input, Select, TextField, Textarea } from "../ui/TextField";
import styles from "./HostFormDialog.module.css";

interface CreateProps {
    mode: "create";
    open: boolean;
    onClose: () => void;
    onCreated: (id: string) => void;
    defaultGroupId?: GroupId | null;
    /** Used by Duplicate action: pre-fills the form from another host. */
    sourceHost?: HostFullDto;
}

interface EditProps {
    mode: "edit";
    open: boolean;
    onClose: () => void;
    onSaved: () => void;
    host: HostFullDto;
}

type Props = CreateProps | EditProps;

type CredentialChoice =
    | { kind: "saved"; credentialId: CredentialId | "" }
    | { kind: "inline"; username: string; password: string };

interface FormState {
    label: string;            // user-facing host "name"
    hostname: string;         // IP or hostname
    port: string;             // empty = default for protocol
    protocol: Protocol;
    groupId: GroupId | "";
    tagsRaw: string;
    notes: string;
    credential: CredentialChoice;
}

function defaultPortFor(p: Protocol): number {
    return p === "ssh" ? 22 : 3389;
}

function emptyState(defaultGroupId: GroupId | null = null): FormState {
    return {
        label: "",
        hostname: "",
        port: "",
        protocol: "ssh",
        groupId: defaultGroupId ?? "",
        tagsRaw: "",
        notes: "",
        credential: { kind: "saved", credentialId: "" },
    };
}

function fromHost(h: HostFullDto): FormState {
    return {
        label: h.name,
        hostname: h.hostname,
        port: String(h.port),
        protocol: h.protocol,
        groupId: h.group_id ?? "",
        tagsRaw: h.tags.join(", "),
        notes: h.notes ?? "",
        credential: { kind: "saved", credentialId: h.default_credential_id ?? "" },
    };
}

/**
 * Pre-fill from another host for Duplicate action. Adjusts the name to
 * "<original> copy" and clears the credential link (user re-picks).
 */
function fromDuplicate(h: HostFullDto, copySuffix: string): FormState {
    return {
        label: `${h.name} ${copySuffix}`.trim(),
        hostname: h.hostname,
        port: String(h.port),
        protocol: h.protocol,
        groupId: h.group_id ?? "",
        tagsRaw: h.tags.join(", "),
        notes: h.notes ?? "",
        credential: { kind: "saved", credentialId: "" },
    };
}

export function HostFormDialog(props: Props) {
    const { t } = useT();
    const groups = useGroupsStore((s) => s.items);
    const credentials = useCredentialsStore((s) => s.items);

    const initial = useMemo<FormState>(() => {
        if (props.mode === "edit") return fromHost(props.host);
        if (props.sourceHost) return fromDuplicate(props.sourceHost, "copy");
        return emptyState(props.defaultGroupId);
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [props.open]);

    const [form, setForm] = useState<FormState>(initial);
    const [submitting, setSubmitting] = useState(false);
    const [error, setError] = useState<string | null>(null);

    useEffect(() => {
        if (props.open) {
            setForm(initial);
            setError(null);
        }
    }, [props.open, initial]);

    const update = <K extends keyof FormState>(key: K, value: FormState[K]) =>
        setForm((s) => ({ ...s, [key]: value }));

    const setCredential = (c: CredentialChoice) =>
        setForm((s) => ({ ...s, credential: c }));

    async function submit() {
        setSubmitting(true);
        setError(null);
        try {
            const portNum = form.port.trim() === "" ? null : Number(form.port);
            if (portNum !== null && (!Number.isInteger(portNum) || portNum < 1 || portNum > 65535)) {
                throw {
                    kind: "validation",
                    field: t("dialog.host.port"),
                    reason: "1..65535",
                };
            }
            const tags = form.tagsRaw
                .split(",")
                .map((s) => s.trim())
                .filter(Boolean);

            // Resolve credential: if inline + non-empty, create one first.
            let credentialId: CredentialId | null = null;
            if (form.credential.kind === "saved") {
                credentialId = form.credential.credentialId || null;
            } else {
                const { username, password } = form.credential;
                if (username.trim() !== "" || password !== "") {
                    // Pick a unique name based on the host label.
                    const baseName = form.label.trim() || form.hostname.trim() || "credential";
                    const name = uniqueCredentialName(baseName, credentials.map((c) => c.name));
                    const created = await credApi.create({
                        name,
                        kind: "password",
                        username: username.trim(),
                        secret: encodeSecret(password),
                    });
                    credentialId = created.id;
                }
            }

            if (props.mode === "create") {
                const req: HostCreateRequest = {
                    name: form.label.trim(),
                    hostname: form.hostname.trim(),
                    protocol: form.protocol,
                    port: portNum,
                    group_id: form.groupId || null,
                    default_credential_id: credentialId,
                    tags: tags.length > 0 ? tags : null,
                    notes: form.notes.trim() || null,
                };
                const res = await hostsApi.create(req);
                // Link the inline-created credential as default.
                if (credentialId) {
                    await credApi.linkHost({
                        host_id: res.id,
                        credential_id: credentialId,
                        set_as_default: true,
                    });
                }
                props.onCreated(res.id);
            } else {
                const req: HostUpdateRequest = {
                    id: props.host.id,
                    name: form.label.trim(),
                    hostname: form.hostname.trim(),
                    protocol: form.protocol,
                    port: portNum ?? undefined,
                    group_id: form.groupId || null,
                    default_credential_id: credentialId,
                    tags,
                    notes: form.notes.trim() || null,
                };
                await hostsApi.update(req);
                // If we created a new inline credential, link it too.
                if (
                    credentialId &&
                    credentialId !== props.host.default_credential_id
                ) {
                    await credApi.linkHost({
                        host_id: props.host.id,
                        credential_id: credentialId,
                        set_as_default: true,
                    });
                }
                props.onSaved();
            }
        } catch (e: unknown) {
            setError(formatApiError(e));
        } finally {
            setSubmitting(false);
        }
    }

    const title =
        props.mode === "create" ? t("dialog.host.newTitle") : t("dialog.host.editTitle");

    return (
        <Dialog
            open={props.open}
            onClose={props.onClose}
            title={title}
            size="md"
            footer={
                <>
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
                {/* Address section — most important field, no label, just placeholder. */}
                <section className={styles.section}>
                    <div className={styles.sectionTitle}>{t("dialog.host.address")}</div>
                    <div className={styles.row}>
                        <Input
                            value={form.hostname}
                            onChange={(e) => update("hostname", e.target.value)}
                            placeholder={t("dialog.host.addressPlaceholder")}
                            required
                            autoFocus
                        />
                        <div className={styles.portCell}>
                            <Select
                                value={form.protocol}
                                onChange={(e) =>
                                    update("protocol", e.target.value as Protocol)
                                }
                                aria-label={t("dialog.host.protocol")}
                            >
                                <option value="ssh">SSH</option>
                                <option value="rdp">RDP</option>
                            </Select>
                            <Input
                                type="number"
                                min={1}
                                max={65535}
                                value={form.port}
                                onChange={(e) => update("port", e.target.value)}
                                placeholder={String(defaultPortFor(form.protocol))}
                                aria-label={t("dialog.host.port")}
                            />
                        </div>
                    </div>
                </section>

                {/* General — label, group, tags, notes */}
                <section className={styles.section}>
                    <TextField label={t("dialog.host.label")}>
                        <Input
                            value={form.label}
                            onChange={(e) => update("label", e.target.value)}
                            placeholder={t("dialog.host.labelPlaceholder")}
                            required
                        />
                    </TextField>

                    <TextField label={t("dialog.host.groupField")}>
                        <Select
                            value={form.groupId}
                            onChange={(e) => update("groupId", e.target.value)}
                        >
                            <option value="">{t("dialog.host.groupNone")}</option>
                            {groups
                                .filter((g) => !g.parent_id)
                                .map((g) => (
                                    <option key={g.id} value={g.id}>
                                        {g.name}
                                    </option>
                                ))}
                        </Select>
                    </TextField>

                    <TextField label={t("dialog.host.tags")} hint={t("dialog.host.tagsHint")}>
                        <Input
                            value={form.tagsRaw}
                            onChange={(e) => update("tagsRaw", e.target.value)}
                            placeholder={t("dialog.host.tagsPlaceholder")}
                        />
                    </TextField>
                </section>

                {/* Credentials section */}
                <section className={styles.section}>
                    <div className={styles.sectionTitle}>
                        {t("dialog.host.credentialsSection")}
                    </div>
                    <CredentialSection value={form.credential} onChange={setCredential} />
                </section>

                <section className={styles.section}>
                    <TextField label={t("dialog.host.notes")} hint={t("dialog.host.notesHint")}>
                        <Textarea
                            value={form.notes}
                            onChange={(e) => update("notes", e.target.value)}
                            rows={3}
                            placeholder={t("dialog.host.notesPlaceholder")}
                        />
                    </TextField>
                </section>

                {error && <div className={styles.errorBox}>{error}</div>}
            </form>
        </Dialog>
    );
}

// =====================================================================
// Credential sub-section: tabs between "use saved" and "enter directly"
// =====================================================================

function CredentialSection({
    value,
    onChange,
}: {
    value: CredentialChoice;
    onChange: (v: CredentialChoice) => void;
}) {
    const { t } = useT();
    const credentials = useCredentialsStore((s) => s.items);

    return (
        <div className={styles.credentialSection}>
            <div className={styles.credentialTabs} role="tablist">
                <button
                    type="button"
                    role="tab"
                    aria-selected={value.kind === "saved"}
                    className={`${styles.credentialTab} ${
                        value.kind === "saved" ? styles.credentialTabActive : ""
                    }`}
                    onClick={() => onChange({ kind: "saved", credentialId: "" })}
                >
                    {t("dialog.host.credentialUseExisting")}
                </button>
                <button
                    type="button"
                    role="tab"
                    aria-selected={value.kind === "inline"}
                    className={`${styles.credentialTab} ${
                        value.kind === "inline" ? styles.credentialTabActive : ""
                    }`}
                    onClick={() => onChange({ kind: "inline", username: "", password: "" })}
                >
                    {t("dialog.host.credentialUseInline")}
                </button>
            </div>

            {value.kind === "saved" ? (
                <div className={styles.credentialBody}>
                    <Select
                        value={value.credentialId}
                        onChange={(e) =>
                            onChange({ kind: "saved", credentialId: e.target.value })
                        }
                    >
                        <option value="">{t("dialog.host.credentialSelectNone")}</option>
                        {credentials.map((c) => (
                            <option key={c.id} value={c.id}>
                                {c.name} ({c.kind.replace("_", " ")})
                            </option>
                        ))}
                    </Select>
                </div>
            ) : (
                <div className={styles.credentialBody}>
                    <Input
                        value={value.username}
                        onChange={(e) =>
                            onChange({ ...value, username: e.target.value })
                        }
                        placeholder={t("dialog.host.credentialUsername")}
                        autoComplete="off"
                    />
                    <Input
                        type="password"
                        value={value.password}
                        onChange={(e) =>
                            onChange({ ...value, password: e.target.value })
                        }
                        placeholder={t("dialog.host.credentialPasswordPlaceholder")}
                        autoComplete="off"
                    />
                    <div className={styles.credentialInlineHint}>
                        {t("dialog.host.credentialInlineHint")}
                    </div>
                </div>
            )}
        </div>
    );
}

// =====================================================================
// Helpers
// =====================================================================

/**
 * Build a unique credential name based on a host label, appending
 * " (n)" if the base is taken. Stays human-readable; users can rename
 * later from the credentials dialog.
 */
function uniqueCredentialName(base: string, existing: string[]): string {
    const taken = new Set(existing);
    if (!taken.has(base)) return base;
    for (let i = 2; i < 1000; i++) {
        const candidate = `${base} (${i})`;
        if (!taken.has(candidate)) return candidate;
    }
    return `${base} (${Date.now()})`;
}
