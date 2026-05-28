import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Eye, EyeOff, Files, Info, Plus, Server, Trash2, Zap } from "lucide-react";

import { useT } from "../../i18n";
import {
    credentials as credApi,
    encodeSecret,
    groups as groupsApi,
    hosts as hostsApi,
} from "../../lib/ipc";
import { formatApiError } from "../../lib/types";
import type { HostFullDto, Protocol } from "../../lib/types";
import { useDebouncedCallback } from "../../lib/useDebouncedCallback";
import { useCredentialsStore, useGroupsStore, useUiStore } from "../../store";
import { Button } from "../ui/Button";
import { Combobox } from "../ui/Combobox";
import { EmptyState } from "../ui/EmptyState";
import { Input, Textarea } from "../ui/TextField";
import { ProtocolBadge } from "../ui/ProtocolBadge";
import { SaveStatusIndicator, type SaveStatus } from "./SaveStatusIndicator";
import styles from "./HostDetail.module.css";

const DEBOUNCE_FIELD_MS = 400;
const DEBOUNCE_NOTES_MS = 1000;

/**
 * Three render paths: welcome / form (draft or existing host).
 *
 * Critical: we render a SINGLE <HostForm> for both draft and edit
 * modes, without a `key` tied to the host id. This keeps the React
 * tree stable across a draft → real-host promotion, so the input
 * keeps focus and the user can continue typing without re-clicking.
 */
export function HostDetail() {
    const { t } = useT();
    const selectedHostId = useUiStore((s) => s.selectedHostId);
    const draft = useUiStore((s) => s.draft);

    const [editingHost, setEditingHost] = useState<HostFullDto | null>(null);
    const [loading, setLoading] = useState(false);
    const [loadError, setLoadError] = useState<string | null>(null);

    // `promotedId` records the id of a draft that was just promoted to
    // a real host. Once this is set and the new host has been loaded
    // into `editingHost`, we render edit mode using it — even if the
    // UiStore hasn't yet caught up. This gives HostForm uninterrupted
    // continuity (same React node, same input DOM, same focus).
    const [promotedId, setPromotedId] = useState<string | null>(null);

    // Load the existing host when one is selected. Keep the prior host
    // visible while the new one loads to avoid flicker — but if the new
    // hostId differs from the cached one, show a loading state.
    useEffect(() => {
        if (!selectedHostId) {
            setEditingHost(null);
            setLoadError(null);
            return;
        }
        if (editingHost?.id === selectedHostId) return;

        let cancelled = false;
        setLoading(true);
        hostsApi
            .get(selectedHostId)
            .then((h) => {
                if (cancelled) return;
                setEditingHost(h);
                setLoadError(null);
            })
            .catch((e: unknown) => {
                if (!cancelled) setLoadError(formatApiError(e));
            })
            .finally(() => {
                if (!cancelled) setLoading(false);
            });
        return () => {
            cancelled = true;
        };
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [selectedHostId]);

    // Build the host object the form should display.
    //
    // Priority:
    // 1. If we have a `promotedId` AND `editingHost` matches it — we just
    //    promoted a draft. Render edit mode immediately using the cached
    //    host, regardless of UiStore state. This is the linchpin of focus
    //    preservation: HostForm sees a continuous host (id flips from
    //    "__draft__" to a real id), its internal useEffect treats this
    //    as a promotion and DOES NOT reset form state.
    // 2. Otherwise: standard draft / edit / welcome logic from UiStore.
    let mode: "draft" | "edit" | null = null;
    let host: HostFullDto | null = null;

    if (promotedId && editingHost?.id === promotedId) {
        mode = "edit";
        host = editingHost;
    } else if (draft) {
        mode = "draft";
        host = {
            id: "__draft__",
            name: draft.label,
            group_id: draft.groupId,
            protocol: draft.protocol,
            hostname: draft.hostname,
            port:
                draft.port.trim() === ""
                    ? draft.protocol === "ssh"
                        ? 22
                        : 3389
                    : Number(draft.port),
            tags: draft.tags,
            color: null,
            default_credential_id: null,
            created_at: new Date().toISOString(),
            updated_at: new Date().toISOString(),
            notes: draft.notes || null,
        };
    } else if (selectedHostId && editingHost) {
        mode = "edit";
        host = editingHost;
    }

    // Once we're rendering the post-promotion host, fold the UiStore in
    // sync: clear the draft and select the new host. This happens AFTER
    // the form is already rendering the real host, so no remount.
    useEffect(() => {
        if (!promotedId) return;
        if (editingHost?.id !== promotedId) return;
        // We're safely showing the real host now; clear the transient state.
        const ui = useUiStore.getState();
        if (ui.draft) ui.clearDraft();
        if (ui.selectedHostId !== promotedId) ui.selectHost(promotedId);
        setPromotedId(null);
    }, [promotedId, editingHost?.id]);

    if (loading && !host) {
        return (
            <main className={styles.main}>
                <div className={styles.loading}>{t("common.loading")}</div>
            </main>
        );
    }

    if (loadError) {
        return (
            <main className={styles.main}>
                <EmptyState
                    title={t("host.error.loadFailed")}
                    description={loadError}
                />
            </main>
        );
    }

    if (!mode || !host) {
        return (
            <main className={styles.main}>
                <EmptyState
                    icon={<Server size={32} />}
                    title={t("host.welcome.title")}
                    description={t("host.welcome.description")}
                />
            </main>
        );
    }

    return (
        <HostForm
            mode={mode}
            host={host}
            onHostUpdated={setEditingHost}
            onDraftPromoted={(fresh) => {
                // Cache the new host immediately, then mark it as the
                // promoted id. The effect above will sync UiStore once
                // the render with the new host has flushed.
                setEditingHost(fresh);
                setPromotedId(fresh.id);
            }}
            initialInlineUsername={draft?.inlineUsername ?? ""}
            initialInlinePassword={draft?.inlinePassword ?? ""}
        />
    );
}

// =====================================================================
// HostForm — unified form
// =====================================================================

interface FormState {
    label: string;
    hostname: string;
    port: string;
    protocol: Protocol;
    groupId: string;
    tagsRaw: string;
    notes: string;
    inlineUsername: string;
    inlinePassword: string;
}

interface HostFormProps {
    mode: "edit" | "draft";
    host: HostFullDto;
    /**
     * Updates the host displayed by HostDetail. In edit mode this is
     * the freshest server snapshot after a save.
     */
    onHostUpdated: (h: HostFullDto) => void;
    /**
     * Called once after a draft is promoted to a real host. The parent
     * caches the host and schedules the UiStore transition to happen
     * AFTER the render with the real host commits — keeping HostForm
     * mounted continuously so input focus survives.
     */
    onDraftPromoted: (h: HostFullDto) => void;
    initialInlineUsername?: string;
    initialInlinePassword?: string;
}

function HostForm(props: HostFormProps) {
    const { t } = useT();
    const groups = useGroupsStore((s) => s.items);
    const credentials = useCredentialsStore((s) => s.items);
    const updateDraft = useUiStore((s) => s.updateDraft);
    const clearDraft = useUiStore((s) => s.clearDraft);
    const startDraft = useUiStore((s) => s.startDraft);
    const setDialog = useUiStore((s) => s.setDialog);

    const linkedCred = useMemo(
        () =>
            props.host.default_credential_id
                ? credentials.find((c) => c.id === props.host.default_credential_id) ?? null
                : null,
        [props.host.default_credential_id, credentials],
    );

    // Build initial form state from the host. Re-derived only on host.id change.
    const [form, setForm] = useState<FormState>(() => buildFormState(props));

    // When a credential is linked (either at load time or after the user
    // picks one), surface its username into the form. Password stays
    // empty — the user can use the eye button on the credentials dialog
    // to reveal it; we don't auto-reveal here.
    //
    // Also reset committed refs to the new credential's values. After
    // this, saveAction's diff check will correctly identify when the
    // user has actually changed something.
    const lastLinkedIdRef = useRef<string | null>(null);
    useEffect(() => {
        const linkedId = linkedCred?.id ?? null;
        if (linkedId === lastLinkedIdRef.current) return;
        lastLinkedIdRef.current = linkedId;
        if (linkedCred) {
            setForm((s) => ({
                ...s,
                inlineUsername: linkedCred.username,
                inlinePassword: "", // never auto-fill the secret
            }));
            committedUsernameRef.current = linkedCred.username;
            committedPasswordRef.current = ""; // we don't know the stored secret
        } else {
            // Credential was unlinked or never existed.
            committedUsernameRef.current = "";
            committedPasswordRef.current = "";
        }
    }, [linkedCred]);

    // Save status: shown as a small indicator in the header.
    // - pending: user just typed; debounce timer is running
    // - saving:  debounce fired, IPC call in flight
    // - saved:   IPC succeeded; auto-resets to idle after 1.5s
    // - error:   sticky; only cleared by a successful save
    const [saveStatus, setSaveStatus] = useState<SaveStatus>({ kind: "idle" });
    const savedResetTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

    const flashSaved = useCallback(() => {
        setSaveStatus({ kind: "saved" });
        if (savedResetTimer.current) clearTimeout(savedResetTimer.current);
        savedResetTimer.current = setTimeout(() => {
            setSaveStatus({ kind: "idle" });
        }, 1500);
    }, []);

    useEffect(() => {
        return () => {
            if (savedResetTimer.current) clearTimeout(savedResetTimer.current);
        };
    }, []);

    // Lock to prevent concurrent draft promotions (race when user types
    // multiple characters faster than the first create completes).
    // `pendingDuringPromote` captures the latest state seen while a
    // promotion was in flight, so we don't lose keystrokes typed in
    // those few hundred milliseconds.
    const promotingRef = useRef(false);
    const pendingDuringPromote = useRef<FormState | null>(null);

    // Track what we've committed for credential fields, so saveAction
    // can skip no-op writes. Without this, every keystroke would trigger
    // a fresh rotateSecret/update — wasteful and visible as constant
    // "saving" flicker. The refs are kept in sync at three points:
    // 1. Initial load (effect below).
    // 2. After a successful save inside saveAction.
    // 3. When a credential picker selects a different saved credential.
    const committedUsernameRef = useRef<string>("");
    const committedPasswordRef = useRef<string>("");

    // Track the previous host.id so we can distinguish two cases:
    //
    // 1. Draft promotion: id changes from "__draft__" to a real id.
    //    The user just got their first save through; we MUST NOT
    //    reset form state — they want to keep typing, with focus
    //    preserved on whichever input they were in.
    //
    // 2. User selected a different host in the sidebar: id changes
    //    from one real id to another. Form state is reset to reflect
    //    the new host.
    const prevHostIdRef = useRef(props.host.id);

    useEffect(() => {
        const prev = prevHostIdRef.current;
        const curr = props.host.id;
        prevHostIdRef.current = curr;

        const isPromotion = prev === "__draft__" && curr !== "__draft__";
        if (isPromotion) {
            // Don't touch form state — the user is still typing.
            return;
        }
        // Real change: load the new host's values into the form.
        setForm(buildFormState(props));
        setSaveStatus({ kind: "idle" });
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [props.host.id]);

    // ---------- Save action -----------------------------------------
    //
    // Single debounced action that either:
    // - In edit mode: updates the host (and, if inline creds were typed,
    //   creates+links a credential as a side effect).
    // - In draft mode: promotes the draft to a real host (gated by
    //   promotingRef so concurrent keystrokes don't double-create).
    //
    // Folding both into one function lets us share the debouncer and
    // ensures keystrokes-while-saving collapse into a single tail call.

    const saveAction = useCallback(
        async (state: FormState) => {
            // Silently skip when hostname looks unfinished. The user is
            // still in the middle of typing something that's not yet a
            // valid host; we don't want to spam the backend or surface
            // an error. Port validation also has to be silent — bad
            // ports just mean "wait until you fix it".
            const hostnameOk = isValidHostname(state.hostname);
            if (!hostnameOk) {
                // For draft mode, we genuinely don't have anything to
                // do yet (nothing to promote). For edit mode, the
                // existing host's hostname remains in the DB unchanged.
                setSaveStatus({ kind: "idle" });
                return;
            }
            const portNum =
                state.port.trim() === "" ? undefined : Number(state.port);
            if (
                portNum !== undefined &&
                (!Number.isInteger(portNum) || portNum < 1 || portNum > 65535)
            ) {
                setSaveStatus({ kind: "idle" });
                return;
            }
            setSaveStatus({ kind: "saving" });
            try {
                const tags = parseTags(state.tagsRaw);

                if (props.mode === "edit") {
                    await hostsApi.update({
                        id: props.host.id,
                        name:
                            state.label.trim() ||
                            state.hostname.trim() ||
                            props.host.name,
                        hostname: state.hostname.trim(),
                        protocol: state.protocol,
                        port: portNum,
                        group_id: state.groupId || null,
                        tags,
                        notes: state.notes.trim() || null,
                    });

                    // Handle inline credentials.
                    //
                    // Diff against the committed refs to avoid no-op
                    // writes on every keystroke. We only call the
                    // backend when the user has actually changed
                    // something from what we already committed.
                    //
                    // Invariant: never create a credential with one
                    // of {username, password} empty — the OS keychain
                    // rejects empty secrets and the user would see a
                    // spurious "secret must not be empty" error.
                    const usernameTrimmed = state.inlineUsername.trim();
                    const usernameFilled = usernameTrimmed !== "";
                    const passwordFilled = state.inlinePassword !== "";

                    if (props.host.default_credential_id === null) {
                        if (usernameFilled && passwordFilled) {
                            const base =
                                state.label.trim() ||
                                state.hostname.trim() ||
                                "credential";
                            const name = uniqueCredentialName(
                                base,
                                credentials.map((c) => c.name),
                            );
                            const created = await credApi.create({
                                name,
                                kind: "password",
                                username: usernameTrimmed,
                                secret: encodeSecret(state.inlinePassword),
                            });
                            await credApi.linkHost({
                                host_id: props.host.id,
                                credential_id: created.id,
                                set_as_default: true,
                            });
                            committedUsernameRef.current = usernameTrimmed;
                            committedPasswordRef.current = state.inlinePassword;
                        }
                    } else if (linkedCred) {
                        const usernameChanged =
                            usernameTrimmed !== committedUsernameRef.current;
                        const passwordChanged =
                            passwordFilled &&
                            state.inlinePassword !== committedPasswordRef.current;

                        if (usernameChanged && usernameFilled) {
                            await credApi.update({
                                id: linkedCred.id,
                                username: usernameTrimmed,
                            });
                            committedUsernameRef.current = usernameTrimmed;
                        }
                        if (passwordChanged) {
                            await credApi.rotateSecret({
                                id: linkedCred.id,
                                secret: encodeSecret(state.inlinePassword),
                            });
                            committedPasswordRef.current = state.inlinePassword;
                            // NOTE: deliberately not clearing inlinePassword.
                            // We want the user to see their masked dots (proof
                            // their password is set), and the eye button to be
                            // available for review. The committedPasswordRef
                            // check above ensures we don't keep rotating on
                            // every subsequent keystroke.
                        }
                    }

                    const fresh = await hostsApi.get(props.host.id);
                    props.onHostUpdated(fresh);
                    flashSaved();
                    return;
                }

                // ---------- Draft promotion ----------
                if (props.mode === "draft") {
                    if (state.hostname.trim() === "") return; // not promotable
                    if (promotingRef.current) {
                        // Promotion already in flight; capture the latest
                        // state so we can re-apply it as an update on the
                        // freshly-created host once we know its id.
                        pendingDuringPromote.current = state;
                        return;
                    }
                    promotingRef.current = true;
                    pendingDuringPromote.current = null;
                    try {
                        let credentialId: string | null = null;
                        // Only create a credential when BOTH username and
                        // password are filled — same invariant as in edit
                        // mode. A half-filled credential would trigger a
                        // backend "secret must not be empty" error.
                        if (
                            state.inlineUsername.trim() !== "" &&
                            state.inlinePassword !== ""
                        ) {
                            const base =
                                state.label.trim() ||
                                state.hostname.trim() ||
                                "credential";
                            const name = uniqueCredentialName(
                                base,
                                credentials.map((c) => c.name),
                            );
                            const created = await credApi.create({
                                name,
                                kind: "password",
                                username: state.inlineUsername.trim(),
                                secret: encodeSecret(state.inlinePassword),
                            });
                            credentialId = created.id;
                            // Now committed — keystrokes won't trigger
                            // another create or rotate.
                            committedUsernameRef.current = state.inlineUsername.trim();
                            committedPasswordRef.current = state.inlinePassword;
                        }
                        const res = await hostsApi.create({
                            name: state.label.trim() || state.hostname.trim(),
                            hostname: state.hostname.trim(),
                            protocol: state.protocol,
                            port:
                                state.port.trim() === "" ? null : Number(state.port),
                            group_id: state.groupId || null,
                            tags: tags.length > 0 ? tags : null,
                            notes: state.notes.trim() || null,
                            default_credential_id: credentialId,
                        });
                        if (credentialId) {
                            await credApi.linkHost({
                                host_id: res.id,
                                credential_id: credentialId,
                                set_as_default: true,
                            });
                        }

                        // If the user typed more while we were promoting,
                        // apply those changes as an immediate update on
                        // the new host BEFORE we hand off to EditPane —
                        // this avoids losing the trailing keystrokes
                        // when DraftPane unmounts. Loop in case more
                        // keystrokes arrive while we're applying the
                        // first batch.
                        while (true) {
                            const pending = pendingDuringPromote.current;
                            if (pending === null) break;
                            // TS5 has a known bug with control-flow analysis
                            // across `while(true)` loops with ref reads — it
                            // narrows the local back to `never` after the
                            // ref reassignment below. Casting through unknown
                            // sidesteps it without losing any safety because
                            // we just checked `=== null`.
                            const p = pending as FormState;
                            pendingDuringPromote.current = null;
                            const pendingTags = parseTags(p.tagsRaw);
                            const pendingPort =
                                p.port.trim() === ""
                                    ? undefined
                                    : Number(p.port);
                            await hostsApi.update({
                                id: res.id,
                                name:
                                    p.label.trim() ||
                                    p.hostname.trim() ||
                                    "host",
                                hostname: p.hostname.trim(),
                                protocol: p.protocol,
                                port: pendingPort,
                                group_id: p.groupId || null,
                                tags: pendingTags,
                                notes: p.notes.trim() || null,
                            });
                        }

                        flashSaved();
                        // Hand the new host to the parent. HostDetail will
                        // render it via the `promotedId` path — same
                        // HostForm React node, no unmount. UiStore is
                        // synced *after* that render commits, via a
                        // useEffect in HostDetail.
                        const fresh = await hostsApi.get(res.id);
                        props.onDraftPromoted(fresh);
                    } finally {
                        promotingRef.current = false;
                    }
                }
            } catch (e: unknown) {
                const msg = formatApiError(e);
                setSaveStatus({ kind: "error", message: msg });
            }
        },
        // eslint-disable-next-line react-hooks/exhaustive-deps
        [props.mode, props.host.id, props.host.default_credential_id, t, flashSaved],
    );

    const debouncedField = useDebouncedCallback(saveAction, DEBOUNCE_FIELD_MS);
    const debouncedNotes = useDebouncedCallback(saveAction, DEBOUNCE_NOTES_MS);

    // ---------- Field updaters -----------------------------------------

    /**
     * Per-field update: writes to local state, mirrors into draft store
     * (so navigate-away dialog can detect dirtiness), and schedules a
     * debounced save. Both edit and draft modes go through the same path.
     */
    const update = useCallback(
        <K extends keyof FormState>(
            key: K,
            value: FormState[K],
            useNotesDebounce = false,
        ) => {
            setForm((prev) => {
                const next = { ...prev, [key]: value };
                if (props.mode === "draft") {
                    // Mirror into draft store for navigate-away detection.
                    const mirror: Record<string, unknown> = {};
                    if (key === "tagsRaw") {
                        mirror.tags = parseTags(value as string);
                    } else if (key === "groupId") {
                        mirror.groupId = (value as string) || null;
                    } else {
                        mirror[key as string] = value;
                    }
                    updateDraft(mirror);
                }
                // Reflect intent: a keystroke is pending until the debounce
                // fires and saveAction switches us to "saving".
                setSaveStatus({ kind: "pending" });
                const debouncer = useNotesDebounce ? debouncedNotes : debouncedField;
                debouncer.call(next);
                return next;
            });
        },
        [props.mode, updateDraft, debouncedField, debouncedNotes],
    );

    // ---------- Credentials -------------------------------------------

    const linkCredential = useCallback(
        async (credentialId: string) => {
            if (props.mode !== "edit") return;
            setSaveStatus({ kind: "saving" });
            try {
                await credApi.linkHost({
                    host_id: props.host.id,
                    credential_id: credentialId,
                    set_as_default: true,
                });
                const fresh = await hostsApi.get(props.host.id);
                props.onHostUpdated(fresh);
                flashSaved();
            } catch (e: unknown) {
                const msg = formatApiError(e);
                setSaveStatus({ kind: "error", message: msg });
            }
        },
        [props, flashSaved],
    );

    // ---------- Group combobox -----------------------------------------

    const groupOptions = useMemo(
        () =>
            groups
                .filter((g) => !g.parent_id)
                .map((g) => ({ value: g.id, label: g.name })),
        [groups],
    );

    const createGroup = useCallback(
        async (name: string) => {
            try {
                const res = await groupsApi.create({ name, parent_id: null });
                update("groupId", res.id);
            } catch (e: unknown) {
                const msg = formatApiError(e);
                setSaveStatus({ kind: "error", message: msg });
            }
        },
        [update],
    );

    // ---------- Inline action handlers --------------------------------

    const handleDuplicate = useCallback(() => {
        if (props.mode !== "edit") return;
        startDraft(props.host.group_id ?? null);
        updateDraft({
            label: `${props.host.name} copy`,
            hostname: props.host.hostname,
            port: String(props.host.port),
            protocol: props.host.protocol,
            groupId: props.host.group_id ?? null,
            tags: [...props.host.tags],
            notes: props.host.notes ?? "",
        });
    }, [props.mode, props.host, startDraft, updateDraft]);

    const handleDelete = useCallback(() => {
        if (props.mode === "draft") {
            clearDraft();
            return;
        }
        setDialog({ kind: "host-delete-confirm", hostId: props.host.id });
    }, [props.mode, props.host.id, clearDraft, setDialog]);

    // ---------- Render --------------------------------------------------

    return (
        <main className={styles.main}>
            <FormHeader
                label={form.label}
                hostname={form.hostname}
                port={form.port}
                protocol={form.protocol}
                mode={props.mode}
                hostId={props.host.id}
                createdAt={props.host.created_at}
                updatedAt={props.host.updated_at}
                saveStatus={saveStatus}
                onDuplicate={handleDuplicate}
                onRequestDelete={handleDelete}
            />


            <section className={styles.section}>
                <div className={styles.sectionTitle}>{t("dialog.host.address")}</div>
                <div className={styles.addressRow}>
                    <Input
                        value={form.hostname}
                        onChange={(e) => update("hostname", e.target.value)}
                        placeholder={t("dialog.host.addressPlaceholder")}
                        autoFocus={props.mode === "draft"}
                        spellCheck={false}
                    />
                    <select
                        className={styles.protocolSelect}
                        value={form.protocol}
                        onChange={(e) =>
                            update("protocol", e.target.value as Protocol)
                        }
                        aria-label={t("dialog.host.protocol")}
                    >
                        <option value="ssh">SSH</option>
                        <option value="rdp">RDP</option>
                    </select>
                    <Input
                        type="number"
                        min={1}
                        max={65535}
                        value={form.port}
                        onChange={(e) => update("port", e.target.value)}
                        placeholder={String(form.protocol === "ssh" ? 22 : 3389)}
                        aria-label={t("dialog.host.port")}
                        className={styles.portInput}
                    />
                </div>
            </section>

            <section className={styles.section}>
                <div className={styles.fieldLabel}>{t("dialog.host.label")}</div>
                <Input
                    value={form.label}
                    onChange={(e) => update("label", e.target.value)}
                    placeholder={
                        form.hostname.trim() !== ""
                            ? form.hostname
                            : t("dialog.host.labelPlaceholder")
                    }
                    spellCheck={false}
                />
            </section>

            <section className={styles.section}>
                <div className={styles.fieldLabel}>{t("dialog.host.groupField")}</div>
                <Combobox
                    options={groupOptions}
                    value={form.groupId}
                    onChange={(v) => update("groupId", v)}
                    onCreateNew={createGroup}
                    placeholder={t("dialog.host.groupNone")}
                    createLabel={t("dialog.host.createGroup")}
                />
            </section>

            <section className={styles.section}>
                <div className={styles.fieldLabel}>{t("dialog.host.tags")}</div>
                <Input
                    value={form.tagsRaw}
                    onChange={(e) => update("tagsRaw", e.target.value)}
                    placeholder={t("dialog.host.tagsPlaceholder")}
                    spellCheck={false}
                />
            </section>

            <section className={styles.section}>
                <div className={styles.sectionTitle}>
                    {t("dialog.host.credentialsSection")}
                </div>
                <CredentialPanel
                    username={form.inlineUsername}
                    password={form.inlinePassword}
                    onUsername={(v) => update("inlineUsername", v)}
                    onPassword={(v) => update("inlinePassword", v)}
                    onPickSaved={linkCredential}
                    linkedCredentialName={linkedCred?.name ?? null}
                />
            </section>

            <section className={styles.section}>
                <div className={styles.fieldLabel}>{t("host.notes")}</div>
                <Textarea
                    value={form.notes}
                    onChange={(e) => update("notes", e.target.value, true)}
                    rows={4}
                    placeholder={t("dialog.host.notesPlaceholder")}
                />
            </section>
        </main>
    );
}

// =====================================================================
// FormHeader (title + buttons + info popover)
// =====================================================================

interface FormHeaderProps {
    label: string;
    hostname: string;
    port: string;
    protocol: Protocol;
    mode: "edit" | "draft";
    hostId: string;
    createdAt: string;
    updatedAt: string;
    saveStatus: SaveStatus;
    onDuplicate: () => void;
    onRequestDelete: () => void;
}

function FormHeader(p: FormHeaderProps) {
    const { t, formatDate } = useT();
    const [infoOpen, setInfoOpen] = useState(false);
    const infoRef = useRef<HTMLDivElement>(null);

    useEffect(() => {
        if (!infoOpen) return;
        const onDoc = (e: MouseEvent) => {
            if (infoRef.current && !infoRef.current.contains(e.target as Node)) {
                setInfoOpen(false);
            }
        };
        document.addEventListener("mousedown", onDoc);
        return () => document.removeEventListener("mousedown", onDoc);
    }, [infoOpen]);

    const title = p.label.trim() || p.hostname.trim() || t("host.newHost");
    const showAddress = p.hostname.trim() !== "";
    const portDisplay =
        p.port.trim() === "" ? (p.protocol === "ssh" ? 22 : 3389) : p.port;

    return (
        <header className={styles.header}>
            <div className={styles.headerLeft}>
                <div className={styles.titleRow}>
                    <h1 className={styles.title}>{title}</h1>
                    <ProtocolBadge protocol={p.protocol} />
                </div>
                {showAddress && (
                    <div className={styles.connection}>
                        <span className={styles.address}>{p.hostname}</span>
                        <span className={styles.port}>:{portDisplay}</span>
                    </div>
                )}
            </div>

            <div className={styles.headerActions}>
                <Button variant="primary" disabled title={t("host.connectDisabled")}>
                    <Zap size={14} /> {t("host.connect")}
                </Button>

                {p.mode === "edit" && (
                    <button
                        className={styles.headerIconButton}
                        onClick={p.onDuplicate}
                        title={t("common.duplicate")}
                        aria-label={t("common.duplicate")}
                        type="button"
                    >
                        <Files size={15} />
                    </button>
                )}

                <button
                    className={styles.headerIconButton}
                    onClick={p.onRequestDelete}
                    title={t("common.delete")}
                    aria-label={t("common.delete")}
                    type="button"
                >
                    <Trash2 size={15} />
                </button>

                <SaveStatusIndicator status={p.saveStatus} />

                {p.mode === "edit" && (
                    <div ref={infoRef} className={styles.infoWrap}>
                        <button
                            className={styles.headerIconButton}
                            onClick={() => setInfoOpen((v) => !v)}
                            title={t("host.technicalInfo")}
                            aria-label={t("host.technicalInfo")}
                            type="button"
                        >
                            <Info size={15} />
                        </button>
                        {infoOpen && (
                            <div className={styles.infoPopover}>
                                <div className={styles.infoRow}>
                                    <span className={styles.infoLabel}>{t("host.created")}</span>
                                    <span className={styles.infoValue}>{formatDate(p.createdAt)}</span>
                                </div>
                                <div className={styles.infoRow}>
                                    <span className={styles.infoLabel}>{t("host.updated")}</span>
                                    <span className={styles.infoValue}>{formatDate(p.updatedAt)}</span>
                                </div>
                                <div className={styles.infoRow}>
                                    <span className={styles.infoLabel}>{t("host.id")}</span>
                                    <span className={styles.infoValue}>{p.hostId}</span>
                                </div>
                            </div>
                        )}
                    </div>
                )}
            </div>
        </header>
    );
}


// =====================================================================
// Credential panel — two fields, always editable. Underneath sits a
// "+ Use existing" button; if a credential is linked, a small chip
// shows which one (with a ✕ to unlink).
// =====================================================================

interface CredentialPanelProps {
    username: string;
    password: string;
    onUsername: (v: string) => void;
    onPassword: (v: string) => void;
    /** Picking a saved credential links it; the parent flows the chosen id through linkHost. */
    onPickSaved: (credentialId: string) => Promise<void>;
    /** Non-null when the host has a default_credential_id linked; we display the name as a chip. */
    linkedCredentialName: string | null;
}

function CredentialPanel(props: CredentialPanelProps) {
    const { t } = useT();
    const credentials = useCredentialsStore((s) => s.items);
    const [pickerOpen, setPickerOpen] = useState(false);
    const [showPassword, setShowPassword] = useState(false);

    const noneAvailable = credentials.length === 0;

    return (
        <div className={styles.credentialPanel}>
            <Input
                value={props.username}
                onChange={(e) => props.onUsername(e.target.value)}
                placeholder={t("dialog.host.credentialUsername")}
                autoComplete="off"
                spellCheck={false}
            />
            <div className={styles.passwordWrap}>
                <Input
                    type={showPassword ? "text" : "password"}
                    value={props.password}
                    onChange={(e) => props.onPassword(e.target.value)}
                    placeholder={
                        props.linkedCredentialName
                            ? "••••••••" // password is stored, leave it
                            : t("dialog.host.credentialPasswordPlaceholder")
                    }
                    autoComplete="off"
                />
                {props.password !== "" && (
                    <button
                        type="button"
                        className={styles.passwordEye}
                        onClick={() => setShowPassword((v) => !v)}
                        title={
                            showPassword ? t("common.hide") : t("common.show")
                        }
                        aria-label={
                            showPassword ? t("common.hide") : t("common.show")
                        }
                    >
                        {showPassword ? <EyeOff size={14} /> : <Eye size={14} />}
                    </button>
                )}
            </div>
            <div className={styles.useSavedRow}>
                <button
                    type="button"
                    className={styles.useSavedButton}
                    onClick={() => setPickerOpen(true)}
                    disabled={noneAvailable}
                    title={
                        noneAvailable
                            ? t("dialog.host.credentialNoSaved")
                            : t("dialog.host.credentialUseSaved")
                    }
                >
                    <Plus size={13} />
                    {t("dialog.host.credentialUseSaved")}
                </button>
                {props.linkedCredentialName && (
                    <span className={styles.linkedChip}>
                        {t("host.credentialLinked", {
                            name: props.linkedCredentialName,
                        })}
                    </span>
                )}
            </div>
            {pickerOpen && (
                <SavedCredentialPicker
                    onClose={() => setPickerOpen(false)}
                    onPick={async (id) => {
                        setPickerOpen(false);
                        await props.onPickSaved(id);
                    }}
                />
            )}
        </div>
    );
}

function SavedCredentialPicker({
    onPick,
    onClose,
}: {
    onPick: (credentialId: string) => Promise<void>;
    onClose: () => void;
}) {
    const credentials = useCredentialsStore((s) => s.items);
    const ref = useRef<HTMLDivElement>(null);

    useEffect(() => {
        const onDoc = (e: MouseEvent) => {
            if (ref.current && !ref.current.contains(e.target as Node)) onClose();
        };
        document.addEventListener("mousedown", onDoc);
        return () => document.removeEventListener("mousedown", onDoc);
    }, [onClose]);

    return (
        <div ref={ref} className={styles.savedPicker}>
            {credentials.map((c) => (
                <button
                    key={c.id}
                    type="button"
                    className={styles.savedPickerRow}
                    onClick={() => void onPick(c.id)}
                >
                    <span className={styles.savedPickerName}>{c.name}</span>
                    <span className={styles.savedPickerMeta}>
                        {c.kind.replace("_", " ")}
                        {c.username && ` · ${c.username}`}
                    </span>
                </button>
            ))}
        </div>
    );
}


// =====================================================================
// Helpers
// =====================================================================

function buildFormState(props: HostFormProps): FormState {
    // Heuristic: if name === hostname, treat label as un-set (an auto-fill
    // happened earlier). This makes the Label input show a dynamic
    // placeholder of the current hostname rather than baking the address
    // into the label and freezing it. Edge case: a user who deliberately
    // typed label === hostname will see it reset to placeholder on next
    // load — acceptable; we'll address with a `display_name` column when
    // we touch the Rust schema again.
    const labelIsAuto =
        props.host.name.trim() !== "" &&
        props.host.name.trim() === props.host.hostname.trim();

    return {
        label: labelIsAuto ? "" : props.host.name,
        hostname: props.host.hostname,
        port:
            props.mode === "draft" && props.host.hostname === ""
                ? ""
                : String(props.host.port),
        protocol: props.host.protocol,
        groupId: props.host.group_id ?? "",
        tagsRaw: props.host.tags.join(", "),
        notes: props.host.notes ?? "",
        inlineUsername: props.initialInlineUsername ?? "",
        inlinePassword: props.initialInlinePassword ?? "",
    };
}

function parseTags(raw: string): string[] {
    return raw
        .split(",")
        .map((t) => t.trim())
        .filter(Boolean);
}

/**
 * Lightweight client-side hostname validation. Accepts:
 * - DNS hostnames per RFC 1123 (labels of letters/digits/hyphens,
 *   ≤ 63 chars per label, total ≤ 253 chars, no leading/trailing
 *   hyphen on any label).
 * - IPv4 dotted-quad addresses.
 * - IPv6 (loose: anything with two or more colons).
 *
 * This is intentionally lenient; the backend has the final say. We
 * use this only to silently skip auto-save while the user is in the
 * middle of typing something not yet recognisable as a host.
 */
function isValidHostname(s: string): boolean {
    const trimmed = s.trim();
    if (trimmed.length === 0 || trimmed.length > 253) return false;
    // IPv6 (basic shape check)
    if (trimmed.includes(":") && trimmed.split(":").length >= 3) return true;
    // IPv4
    if (/^(\d{1,3}\.){3}\d{1,3}$/.test(trimmed)) {
        return trimmed
            .split(".")
            .every((octet) => {
                const n = Number(octet);
                return n >= 0 && n <= 255;
            });
    }
    // DNS hostname
    const labels = trimmed.split(".");
    return labels.every(
        (label) =>
            label.length > 0 &&
            label.length <= 63 &&
            /^[a-zA-Z0-9]([a-zA-Z0-9-]*[a-zA-Z0-9])?$/.test(label),
    );
}

function uniqueCredentialName(base: string, existing: string[]): string {
    const taken = new Set(existing);
    if (!taken.has(base)) return base;
    for (let i = 2; i < 1000; i++) {
        const candidate = `${base} (${i})`;
        if (!taken.has(candidate)) return candidate;
    }
    return `${base} (${Date.now()})`;
}
