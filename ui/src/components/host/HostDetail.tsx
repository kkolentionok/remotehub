import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ArrowRightToLine, Check, ChevronDown, ChevronRight, Copy, Eye, EyeOff, Files, Info, KeyRound, Lock, Monitor, Pencil, Plus, Server, Star, Terminal, Trash2, Upload, User, X, Zap } from "lucide-react";

import { useT } from "../../i18n";
import {
    credentials as credApi,
    encodeSecret,
    groups as groupsApi,
    hosts as hostsApi,
} from "../../lib/ipc";
import { formatApiError } from "../../lib/types";
import type { CredentialKind, EnvVar, HostFullDto, KnownHostKeyDto, Protocol } from "../../lib/types";
import { useDebouncedCallback } from "../../lib/useDebouncedCallback";
import {
    useCredentialsStore,
    useGroupsStore,
    useHostsStore,
    useSessionsStore,
    useUiStore,
} from "../../store";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/Dialog";
import { Combobox } from "../ui/Combobox";
import { EmptyState } from "../ui/EmptyState";
import { Input, Textarea } from "../ui/TextField";
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
            display_name: draft.label || null,
            group_id: draft.groupId,
            protocol: draft.protocol,
            hostname: draft.hostname,
            port:
                draft.port.trim() === ""
                    ? draft.protocol === "ssh"
                        ? 22
                        : 3389
                    : Number(draft.port),
            username: draft.inlineUsername,
            tags: draft.tags,
            color: null,
            detected_os: null,
            default_credential_id: draft.pickedCredentialId,
            jump_host_id: null,
            agent_forwarding: false,
            favorite: false,
            last_connected_at: null,
            credential_ids: draft.pickedCredentialId
                ? [draft.pickedCredentialId]
                : [],
            created_at: new Date().toISOString(),
            updated_at: new Date().toISOString(),
            notes: draft.notes || null,
            startup_command: draft.startupCommand || null,
            env_vars: draft.envVars,
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
            initialInlineAuthKind={draft?.inlineAuthKind ?? "password"}
            initialInlinePrivateKey={draft?.inlinePrivateKey ?? ""}
            initialInlinePassphrase={draft?.inlinePassphrase ?? ""}
            initialInlineKeyName={draft?.inlineKeyName ?? ""}
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
    startupCommand: string;
    /** Selected jump-host id (ProxyJump bastion), or "" for direct. */
    jumpHostId: string;
    /** Forward the local SSH agent to this host (ssh -A). */
    agentForwarding: boolean;
    /** User-pinned favorite (star). */
    favorite: boolean;
    /** Raw textarea contents: one `KEY=VALUE` per line. */
    envRaw: string;
    inlineUsername: string;
    inlinePassword: string;
    inlineAuthKind: "password" | "key";
    inlinePrivateKey: string;
    inlinePassphrase: string;
    /** Imported key file name (used to name the created credential). */
    inlineKeyName: string;
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
    initialInlineAuthKind?: "password" | "key";
    initialInlinePrivateKey?: string;
    initialInlinePassphrase?: string;
    initialInlineKeyName?: string;
}

function HostForm(props: HostFormProps) {
    const { t } = useT();
    const groups = useGroupsStore((s) => s.items);
    const allHosts = useHostsStore((s) => s.items);
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

    // All credentials linked to this host, split by kind. A host can have
    // both a password and an SSH key — the backend tries each at connect.
    const linkedCreds = useMemo(() => {
        const ids = props.host.credential_ids ?? [];
        return credentials.filter((c) => ids.includes(c.id));
    }, [props.host.credential_ids, credentials]);
    const linkedPwCred = useMemo(
        () => linkedCreds.find((c) => c.kind === "password") ?? null,
        [linkedCreds],
    );
    const linkedKeyCred = useMemo(
        () => linkedCreds.find((c) => c.kind === "ssh_key") ?? null,
        [linkedCreds],
    );
    const linkedAgentCred = useMemo(
        () => linkedCreds.find((c) => c.kind === "ssh_key_agent") ?? null,
        [linkedCreds],
    );

    // Build initial form state from the host. Re-derived only on host.id change.
    const [form, setForm] = useState<FormState>(() =>
        buildFormState(props, linkedCred?.username, linkedCred?.kind),
    );
    // Always-current form snapshot for callbacks that must not capture a
    // stale closure (e.g. flushing the save before connecting).
    const formRef = useRef(form);
    formRef.current = form;

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
            const kind = linkedCred.kind === "ssh_key" ? "key" : "password";
            // Note: do NOT touch inlineUsername here. Username lives on the
            // host, not the credential — picking a shared key must not
            // overwrite this host's login with another host's.
            setForm((s) => ({
                ...s,
                inlinePassword: "", // never auto-fill the secret
                inlineAuthKind: kind,
                inlinePrivateKey: "", // never auto-fill the stored key
                inlinePassphrase: "",
                inlineKeyName: "",
            }));
            committedUsernameRef.current = props.host.username;
            committedPasswordRef.current = ""; // we don't know the stored secret
            committedPrivateKeyRef.current = "";
            committedPassphraseRef.current = "";
            committedKindRef.current = kind;
        } else {
            // Credential was unlinked or never existed.
            committedUsernameRef.current = props.host.username;
            committedPasswordRef.current = "";
            committedPrivateKeyRef.current = "";
            committedPassphraseRef.current = "";
        }
    }, [linkedCred]);

    // Save status: shown as a small indicator in the header.
    // - pending: user just typed; debounce timer is running
    // - saving:  debounce fired, IPC call in flight
    // - saved:   IPC succeeded; auto-resets to idle after 1.5s
    // - error:   sticky; only cleared by a successful save
    const [saveStatus, setSaveStatus] = useState<SaveStatus>({ kind: "idle" });
    const [advancedOpen, setAdvancedOpen] = useState(false);
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
    const committedPrivateKeyRef = useRef<string>("");
    const committedPassphraseRef = useRef<string>("");
    // The kind of the credential we currently consider "committed" for this
    // host. Used to detect when the user switches auth method (password ⇄
    // key), which can't be an in-place update — it needs a fresh credential.
    const committedKindRef = useRef<"password" | "key">("password");

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
        // Real change: load the new host's values into the form,
        // including the linked credential's username (if any).
        setForm(buildFormState(props, linkedCred?.username, linkedCred?.kind));
        committedUsernameRef.current = linkedCred?.username ?? "";
        committedPasswordRef.current = "";
        committedPrivateKeyRef.current = "";
        committedPassphraseRef.current = "";
        committedKindRef.current =
            linkedCred?.kind === "ssh_key" ? "key" : "password";
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
                        display_name: state.label.trim() || null,
                        hostname: state.hostname.trim(),
                        protocol: state.protocol,
                        port: portNum,
                        username: state.inlineUsername.trim(),
                        group_id: state.groupId || null,
                        tags,
                        notes: state.notes.trim() || null,
                        startup_command:
                            state.protocol === "ssh"
                                ? state.startupCommand.trim() || null
                                : null,
                        env_vars: parseEnv(state.envRaw),
                    });

                    // Handle inline credentials (password or SSH key).
                    //
                    // Diff against committed refs to avoid no-op writes on
                    // every keystroke.
                    const passwordFilled = state.inlinePassword !== "";
                    const keyFilled = state.inlinePrivateKey.trim() !== "";

                    const makeName = (preferred?: string) =>
                        uniqueCredentialName(
                            (preferred && preferred.trim()) ||
                                state.label.trim() ||
                                state.hostname.trim() ||
                                "credential",
                            credentials.map((c) => c.name),
                        );

                    // Independent methods: a host may have BOTH a password
                    // and a key. Handle each on its own — create+link if the
                    // method isn't linked yet, rotate if it is. Never let one
                    // method's secret overwrite the other's.
                    const linked = useCredentialsStore.getState().items;
                    const linkedIds = props.host.credential_ids ?? [];
                    const pwCred =
                        linked.find(
                            (c) =>
                                linkedIds.includes(c.id) && c.kind === "password",
                        ) ?? null;
                    const keyCred =
                        linked.find(
                            (c) =>
                                linkedIds.includes(c.id) && c.kind === "ssh_key",
                        ) ?? null;
                    const anyLinked = pwCred !== null || keyCred !== null;

                    // ----- Password -----
                    // A change from the committed baseline drives the action:
                    //  • emptied (was non-empty) → remove the linked password
                    //  • new/changed text → create+link or rotate
                    if (state.inlinePassword !== committedPasswordRef.current) {
                        if (state.inlinePassword === "") {
                            if (pwCred) {
                                await credApi.unlinkHost({
                                    host_id: props.host.id,
                                    credential_id: pwCred.id,
                                });
                            }
                        } else if (pwCred) {
                            await credApi.rotateSecret({
                                id: pwCred.id,
                                secret: encodeSecret(state.inlinePassword),
                            });
                        } else {
                            const created = await credApi.create({
                                name: makeName(),
                                kind: "password",
                                username: state.inlineUsername.trim(),
                                secret: encodeSecret(state.inlinePassword),
                            });
                            await credApi.linkHost({
                                host_id: props.host.id,
                                credential_id: created.id,
                                set_as_default: !anyLinked,
                            });
                        }
                        committedPasswordRef.current = state.inlinePassword;
                    }

                    // ----- SSH key (entry) -----
                    const keyChanged =
                        keyFilled &&
                        state.inlinePrivateKey !== committedPrivateKeyRef.current;
                    const passphraseChanged =
                        state.inlinePassphrase !== committedPassphraseRef.current;
                    if (keyFilled && (keyChanged || passphraseChanged)) {
                        if (keyCred) {
                            await credApi.rotateSecret({
                                id: keyCred.id,
                                secret: encodeSecret(state.inlinePrivateKey),
                                passphrase:
                                    state.inlinePassphrase !== ""
                                        ? encodeSecret(state.inlinePassphrase)
                                        : undefined,
                            });
                            const keyName = state.inlineKeyName.trim();
                            if (keyName && keyName !== keyCred.name) {
                                await credApi.update({
                                    id: keyCred.id,
                                    name: keyName,
                                });
                            }
                        } else {
                            const created = await credApi.create({
                                name: makeName(state.inlineKeyName),
                                kind: "ssh_key",
                                username: state.inlineUsername.trim(),
                                secret: encodeSecret(state.inlinePrivateKey),
                                passphrase:
                                    state.inlinePassphrase !== ""
                                        ? encodeSecret(state.inlinePassphrase)
                                        : undefined,
                            });
                            await credApi.linkHost({
                                host_id: props.host.id,
                                credential_id: created.id,
                                set_as_default: !anyLinked && !passwordFilled,
                            });
                        }
                        committedPrivateKeyRef.current = state.inlinePrivateKey;
                        committedPassphraseRef.current = state.inlinePassphrase;
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
                        // Collect every method the user supplied. A picked
                        // existing key, an inline password, and an inline key
                        // can all coexist — link them all; the first becomes
                        // the default. The backend tries each at connect.
                        const toLink: string[] = [];
                        const picked = props.host.default_credential_id;
                        const uTrim = state.inlineUsername.trim();
                        if (picked) toLink.push(picked);
                        if (state.inlinePassword !== "") {
                            const created = await credApi.create({
                                name: uniqueCredentialName(
                                    state.label.trim() ||
                                        state.hostname.trim() ||
                                        "credential",
                                    credentials.map((c) => c.name),
                                ),
                                kind: "password",
                                username: uTrim,
                                secret: encodeSecret(state.inlinePassword),
                            });
                            toLink.push(created.id);
                            committedPasswordRef.current = state.inlinePassword;
                        }
                        if (!picked && state.inlinePrivateKey.trim() !== "") {
                            const created = await credApi.create({
                                name: uniqueCredentialName(
                                    state.inlineKeyName.trim() ||
                                        state.label.trim() ||
                                        state.hostname.trim() ||
                                        "credential",
                                    credentials.map((c) => c.name),
                                ),
                                kind: "ssh_key",
                                username: uTrim,
                                secret: encodeSecret(state.inlinePrivateKey),
                                passphrase:
                                    state.inlinePassphrase !== ""
                                        ? encodeSecret(state.inlinePassphrase)
                                        : undefined,
                            });
                            toLink.push(created.id);
                            committedPrivateKeyRef.current = state.inlinePrivateKey;
                            committedPassphraseRef.current = state.inlinePassphrase;
                        }
                        const res = await hostsApi.create({
                            name: state.label.trim() || state.hostname.trim(),
                            display_name: state.label.trim() || null,
                            hostname: state.hostname.trim(),
                            protocol: state.protocol,
                            port:
                                state.port.trim() === "" ? null : Number(state.port),
                            username: uTrim || null,
                            group_id: state.groupId || null,
                            tags: tags.length > 0 ? tags : null,
                            notes: state.notes.trim() || null,
                            startup_command:
                                state.protocol === "ssh"
                                    ? state.startupCommand.trim() || null
                                    : null,
                            env_vars: parseEnv(state.envRaw),
                            default_credential_id: toLink[0] ?? null,
                        });
                        for (let i = 0; i < toLink.length; i++) {
                            await credApi.linkHost({
                                host_id: res.id,
                                credential_id: toLink[i]!,
                                set_as_default: i === 0,
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
                                display_name: p.label.trim() || null,
                                hostname: p.hostname.trim(),
                                protocol: p.protocol,
                                port: pendingPort,
                                username: p.inlineUsername.trim(),
                                group_id: p.groupId || null,
                                tags: pendingTags,
                                notes: p.notes.trim() || null,
                                startup_command:
                                    p.protocol === "ssh"
                                        ? p.startupCommand.trim() || null
                                        : null,
                                env_vars: parseEnv(p.envRaw),
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
                // Switching protocol: if the port is still empty or the old
                // protocol's default, follow the switch (SSH 22 ↔ RDP 3389).
                // A custom port is left untouched.
                if (key === "protocol") {
                    const oldDefault = prev.protocol === "ssh" ? "22" : "3389";
                    const newDefault = value === "ssh" ? "22" : "3389";
                    const port = prev.port.trim();
                    if (port === "" || port === oldDefault) {
                        next.port = newDefault;
                    }
                }
                if (props.mode === "draft") {
                    // Mirror into draft store for navigate-away detection.
                    const mirror: Record<string, unknown> = {};
                    if (key === "tagsRaw") {
                        mirror.tags = parseTags(value as string);
                    } else if (key === "envRaw") {
                        mirror.envVars = parseEnv(value as string);
                    } else if (key === "groupId") {
                        mirror.groupId = (value as string) || null;
                    } else {
                        mirror[key as string] = value;
                    }
                    // Protocol switch may have moved the port — mirror it.
                    if (key === "protocol" && next.port !== prev.port) {
                        mirror.port = next.port;
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

    // Enforce a single key/agent slot: unlink any currently-linked ssh_key /
    // ssh_key_agent credential (except `exceptId`) before linking a new one,
    // so switching keys replaces rather than stacks. Edit mode only.
    const dropLinkedKeyAuth = useCallback(
        async (exceptId: string | null) => {
            const ids = props.host.credential_ids ?? [];
            const items = useCredentialsStore.getState().items;
            for (const c of items) {
                if (
                    c.id !== exceptId &&
                    ids.includes(c.id) &&
                    (c.kind === "ssh_key" || c.kind === "ssh_key_agent")
                ) {
                    await credApi.unlinkHost({
                        host_id: props.host.id,
                        credential_id: c.id,
                    });
                }
            }
        },
        [props],
    );

    const linkCredential = useCallback(
        async (credentialId: string) => {
            // On a draft the host doesn't exist yet — remember the choice and
            // link it during promotion. The draft host's default_credential_id
            // mirrors this, so the form shows it as linked immediately.
            if (props.mode !== "edit") {
                updateDraft({ pickedCredentialId: credentialId });
                return;
            }
            setSaveStatus({ kind: "saving" });
            try {
                // Single slot: drop any previously-linked key/agent first.
                await dropLinkedKeyAuth(credentialId);
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
        [props, flashSaved, updateDraft, dropLinkedKeyAuth],
    );

    // Remove a single auth method from the host (✕ on a method). Drafts
    // only have a picked key, so unlinking there just clears the choice.
    const unlinkCredential = useCallback(
        async (credentialId: string) => {
            if (props.mode !== "edit") {
                updateDraft({ pickedCredentialId: null });
                return;
            }
            setSaveStatus({ kind: "saving" });
            try {
                await credApi.unlinkHost({
                    host_id: props.host.id,
                    credential_id: credentialId,
                });
                // Reset the matching committed ref so re-adding the method
                // later is detected as a change.
                const removed = useCredentialsStore
                    .getState()
                    .items.find((c) => c.id === credentialId);
                if (removed?.kind === "password") {
                    committedPasswordRef.current = "";
                } else if (removed?.kind === "ssh_key") {
                    committedPrivateKeyRef.current = "";
                    committedPassphraseRef.current = "";
                }
                const fresh = await hostsApi.get(props.host.id);
                props.onHostUpdated(fresh);
                flashSaved();
            } catch (e: unknown) {
                setSaveStatus({ kind: "error", message: formatApiError(e) });
            }
        },
        [props, flashSaved, updateDraft],
    );

    // Create a brand-new SSH key credential (from the Add-key modal) and
    // apply it to the host immediately: stored in the keychain, then linked
    // (edit) or remembered on the draft (promoted on save).
    const onAddKey = useCallback(
        async ({
            key,
            passphrase,
            name,
        }: {
            key: string;
            passphrase: string;
            name: string;
        }) => {
            if (key.trim() === "") return;
            setSaveStatus({ kind: "saving" });
            try {
                const f = formRef.current;
                const credName = uniqueCredentialName(
                    name.trim() || f.label.trim() || f.hostname.trim() || "key",
                    useCredentialsStore.getState().items.map((c) => c.name),
                );
                const created = await credApi.create({
                    name: credName,
                    kind: "ssh_key",
                    username: f.inlineUsername.trim(),
                    secret: encodeSecret(key.trim()),
                    passphrase:
                        passphrase !== "" ? encodeSecret(passphrase) : undefined,
                });
                if (props.mode === "edit") {
                    const anyLinked =
                        (props.host.credential_ids ?? []).length > 0;
                    await credApi.linkHost({
                        host_id: props.host.id,
                        credential_id: created.id,
                        set_as_default: !anyLinked,
                    });
                    const fresh = await hostsApi.get(props.host.id);
                    props.onHostUpdated(fresh);
                } else {
                    updateDraft({ pickedCredentialId: created.id });
                }
                flashSaved();
            } catch (e: unknown) {
                setSaveStatus({ kind: "error", message: formatApiError(e) });
            }
        },
        [props, flashSaved, updateDraft],
    );

    // Use the OS SSH agent for this host: link an `ssh_key_agent`
    // credential (no secret stored — the agent signs). Agent creds are
    // interchangeable, so reuse an existing one rather than proliferating.
    const onUseAgent = useCallback(async () => {
        setSaveStatus({ kind: "saving" });
        try {
            const existing = useCredentialsStore
                .getState()
                .items.find((c) => c.kind === "ssh_key_agent");
            const credId = existing
                ? existing.id
                : (
                      await credApi.create({
                          name: uniqueCredentialName(
                              "SSH agent",
                              useCredentialsStore.getState().items.map((c) => c.name),
                          ),
                          kind: "ssh_key_agent",
                          username: "",
                      })
                  ).id;
            if (props.mode === "edit") {
                // Single slot: drop any previously-linked key/agent first.
                await dropLinkedKeyAuth(credId);
                await credApi.linkHost({
                    host_id: props.host.id,
                    credential_id: credId,
                    set_as_default: true,
                });
                const fresh = await hostsApi.get(props.host.id);
                props.onHostUpdated(fresh);
            } else {
                updateDraft({ pickedCredentialId: credId });
            }
            flashSaved();
        } catch (e: unknown) {
            setSaveStatus({ kind: "error", message: formatApiError(e) });
        }
    }, [props, flashSaved, updateDraft, dropLinkedKeyAuth]);

    // Seed the field with the revealed stored password as the edit
    // baseline. We set committed = value so no save fires on reveal; only
    // a later edit (or clearing it → delete) counts as a change.
    const onPasswordRevealed = useCallback((value: string) => {
        committedPasswordRef.current = value;
        setForm((f) => ({ ...f, inlinePassword: value }));
    }, []);

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

    // Jump-host options: other SSH hosts (a host can't jump through itself).
    const jumpOptions = useMemo(
        () =>
            allHosts
                .filter((h) => h.protocol === "ssh" && h.id !== props.host.id)
                .map((h) => ({ value: h.id, label: h.name })),
        [allHosts, props.host.id],
    );

    // The jump host is a discrete choice — persist it immediately rather
    // than threading it through the debounced text-field autosave.
    const setJumpHost = useCallback(
        (value: string) => {
            setForm((f) => ({ ...f, jumpHostId: value }));
            if (props.mode !== "edit") return;
            void hostsApi
                .update({ id: props.host.id, jump_host_id: value || null })
                .then(() => flashSaved())
                .catch((e: unknown) =>
                    setSaveStatus({ kind: "error", message: formatApiError(e) }),
                );
        },
        [props.mode, props.host.id, flashSaved],
    );

    // Agent forwarding is a toggle — persist immediately, same as jump host.
    const setAgentForwarding = useCallback(
        (value: boolean) => {
            setForm((f) => ({ ...f, agentForwarding: value }));
            if (props.mode !== "edit") return;
            void hostsApi
                .update({ id: props.host.id, agent_forwarding: value })
                .then(() => flashSaved())
                .catch((e: unknown) =>
                    setSaveStatus({ kind: "error", message: formatApiError(e) }),
                );
        },
        [props.mode, props.host.id, flashSaved],
    );

    // Favorite is a header toggle — persist immediately (no debounce).
    const toggleFavorite = useCallback(() => {
        const value = !form.favorite;
        setForm((f) => ({ ...f, favorite: value }));
        if (props.mode !== "edit") return;
        void hostsApi
            .update({ id: props.host.id, favorite: value })
            .then(() => flashSaved())
            .catch((e: unknown) =>
                setSaveStatus({ kind: "error", message: formatApiError(e) }),
            );
    }, [props.mode, props.host.id, form.favorite, flashSaved]);

    // ---------- Inline action handlers --------------------------------

    const handleDuplicate = useCallback(() => {
        if (props.mode !== "edit") return;
        startDraft(props.host.group_id ?? null);
        updateDraft({
            label: `${props.host.display_name ?? props.host.hostname} copy`,
            hostname: props.host.hostname,
            port: String(props.host.port),
            protocol: props.host.protocol,
            groupId: props.host.group_id ?? null,
            tags: [...props.host.tags],
            notes: props.host.notes ?? "",
            startupCommand: props.host.startup_command ?? "",
            envVars: props.host.env_vars.map((v) => ({ ...v })),
        });
    }, [props.mode, props.host, startDraft, updateDraft]);

    const handleConnect = useCallback(async () => {
        if (props.mode !== "edit") return;
        try {
            // Persist any just-typed credential/host edits before opening
            // the session, so a freshly entered password/key is linked and
            // the backend doesn't see "host has no credential".
            debouncedField.cancel();
            debouncedNotes.cancel();
            await saveAction(formRef.current);
            // The just-typed secrets are now persisted. Drop them from the
            // form so the field locks to the saved credential — revealing it
            // then pulls the LIVE secret from the keychain. Without this the
            // locally-typed value lingers and the eye would show a stale
            // password if it later changes (e.g. via the re-auth screen).
            committedPasswordRef.current = "";
            committedPrivateKeyRef.current = "";
            committedPassphraseRef.current = "";
            setForm((f) => ({
                ...f,
                inlinePassword: "",
                inlinePrivateKey: "",
                inlinePassphrase: "",
                inlineKeyName: "",
            }));
            const fresh = await hostsApi.get(props.host.id);
            void useSessionsStore.getState().open(fresh);
        } catch {
            // Save failed (shown in the status indicator); still attempt to
            // open with whatever is persisted.
            void useSessionsStore.getState().open(props.host);
        }
    }, [props.mode, props.host, saveAction, debouncedField, debouncedNotes]);

    const handleDelete = useCallback(() => {
        if (props.mode === "draft") {
            clearDraft();
            return;
        }
        setDialog({ kind: "host-delete-confirm", hostId: props.host.id });
    }, [props.mode, props.host.id, clearDraft, setDialog]);

    // ---------- Render --------------------------------------------------

    return (
        <main className={styles.scroll}>
            <div className={styles.content}>
            <FormHeader
                label={form.label}
                hostname={form.hostname}
                port={form.port}
                protocol={form.protocol}
                detectedOs={props.host.detected_os}
                mode={props.mode}
                hostId={props.host.id}
                createdAt={props.host.created_at}
                updatedAt={props.host.updated_at}
                lastConnectedAt={props.host.last_connected_at}
                saveStatus={saveStatus}
                favorite={form.favorite}
                onToggleFavorite={toggleFavorite}
                onConnect={handleConnect}
                canConnect={props.mode === "edit"}
                onDuplicate={handleDuplicate}
                onRequestDelete={handleDelete}
            />


            <div className={styles.body}>
            <div className={styles.frow}>
                <div className={styles.frow__l}>{t("dialog.host.label")}</div>
                <div className={styles.frow__c}>
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
                </div>
            </div>

            <div className={styles.frow}>
                <div className={styles.frow__l}>{t("dialog.host.address")}</div>
                <div className={styles.frow__c}>
                    <Input
                        value={form.hostname}
                        onChange={(e) => update("hostname", e.target.value)}
                        placeholder={t("dialog.host.addressPlaceholder")}
                        autoFocus={props.mode === "draft"}
                        spellCheck={false}
                    />
                    <div className={styles.frow__port}>
                        <span className={styles.frow__sep}>:</span>
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
                </div>
            </div>

            <div className={styles.frow}>
                <div className={styles.frow__l}>{t("dialog.host.protocol")}</div>
                <div className={styles.frow__c}>
                    <div className={styles.segmented}>
                        <button
                            type="button"
                            className={`${styles.seg} ${form.protocol === "ssh" ? styles.segOn : ""}`}
                            onClick={() => update("protocol", "ssh")}
                        >
                            <Terminal size={14} /> SSH
                        </button>
                        <button
                            type="button"
                            className={`${styles.seg} ${form.protocol === "rdp" ? styles.segOn : ""}`}
                            onClick={() => update("protocol", "rdp")}
                        >
                            <Monitor size={14} /> RDP
                        </button>
                    </div>
                </div>
            </div>

            <CredentialPanel
                protocol={form.protocol}
                username={form.inlineUsername}
                password={form.inlinePassword}
                onUsername={(v) => update("inlineUsername", v)}
                onPassword={(v) => update("inlinePassword", v)}
                onPasswordRevealed={onPasswordRevealed}
                onPickSaved={linkCredential}
                onAddKey={onAddKey}
                onUseAgent={onUseAgent}
                onUnlink={unlinkCredential}
                linkedPasswordId={linkedPwCred?.id ?? null}
                linkedKeyId={linkedKeyCred?.id ?? null}
                linkedKeyName={linkedKeyCred?.name ?? null}
                linkedAgentId={linkedAgentCred?.id ?? null}
            />

            <div className={styles.frow}>
                <div className={styles.frow__l}>{t("dialog.host.groupField")}</div>
                <div className={styles.frow__c}>
                    <Combobox
                        options={groupOptions}
                        value={form.groupId}
                        onChange={(v) => update("groupId", v)}
                        onCreateNew={createGroup}
                        placeholder={t("dialog.host.groupNone")}
                        createLabel={t("dialog.host.createGroup")}
                    />
                </div>
            </div>

            <section className={styles.section}>
                <button
                    type="button"
                    className={styles.advancedToggle}
                    onClick={() => setAdvancedOpen((v) => !v)}
                    aria-expanded={advancedOpen}
                >
                    {advancedOpen ? (
                        <ChevronDown size={14} />
                    ) : (
                        <ChevronRight size={14} />
                    )}
                    {t("dialog.host.advanced")}
                </button>

                {advancedOpen && (
                    <div className={styles.advancedBody}>
                        <div>
                            <div className={styles.fieldLabel}>
                                {t("dialog.host.tags")}
                            </div>
                            <Input
                                value={form.tagsRaw}
                                onChange={(e) => update("tagsRaw", e.target.value)}
                                placeholder={t("dialog.host.tagsPlaceholder")}
                                spellCheck={false}
                            />
                        </div>

                        {form.protocol === "ssh" && (
                            <div>
                                <div className={styles.fieldLabel}>
                                    {t("dialog.host.startupCommand")}
                                </div>
                                <Input
                                    value={form.startupCommand}
                                    onChange={(e) =>
                                        update("startupCommand", e.target.value)
                                    }
                                    placeholder={t(
                                        "dialog.host.startupCommandPlaceholder",
                                    )}
                                    spellCheck={false}
                                />
                                <div className={styles.fieldHint}>
                                    {t("dialog.host.startupCommandHint")}
                                </div>
                            </div>
                        )}

                        {form.protocol === "ssh" && props.mode === "edit" && (
                            <div>
                                <div className={styles.fieldLabel}>
                                    {t("dialog.host.jumpHost")}
                                </div>
                                <Combobox
                                    options={jumpOptions}
                                    value={form.jumpHostId}
                                    onChange={setJumpHost}
                                    placeholder={t("dialog.host.jumpHostNone")}
                                />
                                <div className={styles.fieldHint}>
                                    {t("dialog.host.jumpHostHint")}
                                </div>
                            </div>
                        )}

                        {form.protocol === "ssh" && props.mode === "edit" && (
                            <div>
                                <label className={styles.checkboxRow}>
                                    <input
                                        type="checkbox"
                                        checked={form.agentForwarding}
                                        onChange={(e) =>
                                            setAgentForwarding(e.target.checked)
                                        }
                                    />
                                    <span>{t("dialog.host.agentForwarding")}</span>
                                </label>
                                <div className={styles.fieldHint}>
                                    {t("dialog.host.agentForwardingHint")}
                                </div>
                            </div>
                        )}

                        <div>
                            <div className={styles.fieldLabel}>
                                {t("dialog.host.envVars")}
                            </div>
                            <Textarea
                                value={form.envRaw}
                                onChange={(e) =>
                                    update("envRaw", e.target.value, true)
                                }
                                rows={3}
                                placeholder={t("dialog.host.envVarsPlaceholder")}
                                spellCheck={false}
                            />
                            <div className={styles.fieldHint}>
                                {t("dialog.host.envVarsHint")}
                            </div>
                        </div>

                        <div>
                            <div className={styles.fieldLabel}>
                                {t("host.notes")}
                            </div>
                            <Textarea
                                value={form.notes}
                                onChange={(e) =>
                                    update("notes", e.target.value, true)
                                }
                                rows={4}
                                placeholder={t("dialog.host.notesPlaceholder")}
                            />
                        </div>
                    </div>
                )}
            </section>
            </div>
            <div className={styles.footer}>
                <button
                    type="button"
                    className={styles.deleteBtn}
                    onClick={handleDelete}
                >
                    <Trash2 size={14} />{" "}
                    {props.mode === "draft"
                        ? t("storage.cancelCreate")
                        : t("common.delete")}
                </button>
                <span style={{ flex: 1 }} />
                {props.mode === "edit" && (
                    <button
                        type="button"
                        className={styles.headerIconButton}
                        onClick={handleDuplicate}
                        title={t("common.duplicate")}
                        aria-label={t("common.duplicate")}
                    >
                        <Files size={15} />
                    </button>
                )}
            </div>
            </div>
        </main>
    );
}

// =====================================================================
// CopyableValue — monospace value with a click-to-copy affordance.
// Used in the technical-info popover (host ID, host-key fingerprint).
// =====================================================================

function CopyableValue({
    value,
    display,
    hint,
}: {
    value: string;
    /** Shorter text to show instead of `value` (full value still copied). */
    display?: string;
    hint?: string;
}) {
    const { t } = useT();
    const [copied, setCopied] = useState(false);
    const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

    useEffect(
        () => () => {
            if (timer.current) clearTimeout(timer.current);
        },
        [],
    );

    const copy = useCallback(() => {
        // navigator.clipboard needs a secure context; the Tauri webview
        // qualifies. Fall back silently if it's somehow unavailable.
        const text = value;
        const done = () => {
            setCopied(true);
            if (timer.current) clearTimeout(timer.current);
            timer.current = setTimeout(() => setCopied(false), 1500);
        };
        if (navigator.clipboard?.writeText) {
            void navigator.clipboard.writeText(text).then(done).catch(() => {});
        }
    }, [value]);

    return (
        <span className={styles.copyable}>
            <span
                className={`${styles.infoValue} ${display ? styles.infoValueTruncated : ""}`}
                title={display ? value : undefined}
            >
                {display ?? value}
                {hint ? <span className={styles.infoHint}> {hint}</span> : null}
            </span>
            <button
                type="button"
                className={styles.copyButton}
                onClick={copy}
                title={t("common.copy")}
                aria-label={t("common.copy")}
            >
                {copied ? <Check size={13} /> : <Copy size={13} />}
            </button>
        </span>
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
    detectedOs: string | null;
    mode: "edit" | "draft";
    hostId: string;
    createdAt: string;
    updatedAt: string;
    lastConnectedAt: string | null;
    saveStatus: SaveStatus;
    favorite: boolean;
    onToggleFavorite: () => void;
    onConnect: () => void;
    canConnect: boolean;
    onDuplicate: () => void;
    onRequestDelete: () => void;
}

function FormHeader(p: FormHeaderProps) {
    const { t, formatDate } = useT();
    const [infoOpen, setInfoOpen] = useState(false);
    const infoRef = useRef<HTMLDivElement>(null);
    // Pinned host key, lazily loaded when the info panel opens. `undefined`
    // = not loaded yet, `null` = loaded but nothing pinned.
    const [hostKey, setHostKey] = useState<KnownHostKeyDto | null | undefined>(
        undefined,
    );

    useEffect(() => {
        if (!infoOpen) return;
        let cancelled = false;
        setHostKey(undefined);
        void hostsApi
            .knownHostKey(p.hostId)
            .then((r) => {
                if (!cancelled) setHostKey(r.key);
            })
            .catch(() => {
                if (!cancelled) setHostKey(null);
            });
        return () => {
            cancelled = true;
        };
    }, [infoOpen, p.hostId]);

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
    const selectHost = useUiStore((s) => s.selectHost);
    const clearDraft = useUiStore((s) => s.clearDraft);
    const closeInspector = () => {
        clearDraft();
        selectHost(null);
    };

    return (
        <>
            <header className={styles.header}>
                <div className={styles.headerIcon}>
                    {p.protocol === "rdp" ? <Monitor size={16} /> : <Server size={16} />}
                </div>
                <div className={styles.headerLeft}>
                    <h1 className={styles.title}>{title}</h1>
                    <div className={styles.saveChipWrap}>
                        <SaveStatusIndicator status={p.saveStatus} />
                    </div>
                </div>
                <div className={styles.headerActions}>
                    {p.mode === "edit" && (
                        <button
                            className={`${styles.headerIconButton} ${p.favorite ? styles.favActive : ""}`}
                            onClick={p.onToggleFavorite}
                            title={t("host.favorite")}
                            aria-label={t("host.favorite")}
                            aria-pressed={p.favorite}
                            type="button"
                        >
                            <Star size={15} fill={p.favorite ? "currentColor" : "none"} />
                        </button>
                    )}
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
                                        <span className={styles.infoLabel}>
                                            {t("host.lastConnected")}
                                        </span>
                                        <span className={styles.infoValue}>
                                            {p.lastConnectedAt
                                                ? formatDate(p.lastConnectedAt)
                                                : t("host.lastConnectedNever")}
                                        </span>
                                    </div>
                                    <div className={styles.infoRow}>
                                        <span className={styles.infoLabel}>
                                            {t("host.fingerprint")}
                                        </span>
                                        {hostKey ? (
                                            <CopyableValue
                                                value={`SHA256:${hostKey.fingerprint_sha256}`}
                                                hint={hostKey.key_type}
                                            />
                                        ) : (
                                            <span className={styles.infoMuted}>
                                                {hostKey === undefined
                                                    ? "…"
                                                    : t("host.fingerprintNone")}
                                            </span>
                                        )}
                                    </div>
                                </div>
                            )}
                        </div>
                    )}
                    <button
                        className={styles.collapseBtn}
                        onClick={closeInspector}
                        title={t("common.close")}
                        aria-label={t("common.close")}
                        type="button"
                    >
                        <ArrowRightToLine size={16} />
                    </button>
                </div>
            </header>
            <div className={styles.connectRow}>
                <Button
                    variant="primary"
                    onClick={p.onConnect}
                    disabled={!p.canConnect}
                    title={p.canConnect ? t("host.connect") : t("host.connectSaveFirst")}
                >
                    <Zap size={14} /> {t("host.connect")}
                </Button>
            </div>
        </>
    );
}


// =====================================================================
// Credential panel — two fields, always editable. Underneath sits a
// "+ Use existing" button; if a credential is linked, a small chip
// shows which one (with a ✕ to unlink).
// =====================================================================

interface CredentialPanelProps {
    protocol: Protocol;
    username: string;
    password: string;
    onUsername: (v: string) => void;
    onPassword: (v: string) => void;
    /** Set the field to the revealed stored password as the edit baseline
        (no save scheduled). Clearing it then removes the password. */
    onPasswordRevealed: (v: string) => void;
    /** Picking a saved credential links it; the parent flows the chosen id through linkHost. */
    onPickSaved: (credentialId: string) => Promise<void>;
    /** Create a new SSH key (paste/import) and apply it to the host now. */
    onAddKey: (args: {
        key: string;
        passphrase: string;
        name: string;
    }) => Promise<void>;
    /** Link an `ssh_key_agent` credential (use the OS SSH agent). */
    onUseAgent: () => Promise<void>;
    /** Unlink a linked credential (✕ on a method). */
    onUnlink: (credentialId: string) => void;
    /** Linked password credential id, if any — enables reveal + ✕. */
    linkedPasswordId: string | null;
    /** Linked SSH key credential, if any — shown as a chip with ✕. */
    linkedKeyId: string | null;
    linkedKeyName: string | null;
    /** Linked SSH-agent credential id, if any — shown as an agent chip. */
    linkedAgentId: string | null;
}

const REVEAL_SECONDS = 10;

function CredentialPanel(props: CredentialPanelProps) {
    const { t } = useT();
    const [pickerOpen, setPickerOpen] = useState(false);
    const [addKeyOpen, setAddKeyOpen] = useState(false);
    const [keyFilter, setKeyFilter] = useState("");
    const comboRef = useRef<HTMLDivElement>(null);
    const [showPassword, setShowPassword] = useState(false);
    // Saved methods are locked by default (protects against accidental
    // edits/removal). A pencil unlocks editing; only then is ✕ shown.
    const [pwEditing, setPwEditing] = useState(false);
    const [authMode, setAuthMode] = useState<"key" | "password">(
        props.linkedKeyId !== null || props.linkedAgentId !== null
            ? "key"
            : "password",
    );

    const keyLinked = props.linkedKeyId !== null;
    const agentLinked = props.linkedAgentId !== null;
    const methodLinked = keyLinked || agentLinked;
    const pwLocked = props.linkedPasswordId !== null && !pwEditing;

    // Close the key combobox when clicking outside it (field + dropdown both
    // live inside `comboRef`, so picking/typing doesn't self-close).
    useEffect(() => {
        if (!pickerOpen) return;
        const onDoc = (e: MouseEvent) => {
            if (comboRef.current && !comboRef.current.contains(e.target as Node)) {
                setPickerOpen(false);
            }
        };
        document.addEventListener("mousedown", onDoc);
        return () => document.removeEventListener("mousedown", onDoc);
    }, [pickerOpen]);

    // Revealed stored password (plaintext from keychain), shown briefly.
    const [revealed, setRevealed] = useState<string | null>(null);
    const [secondsLeft, setSecondsLeft] = useState(0);
    const revealTimer = useRef<ReturnType<typeof setInterval> | null>(null);

    const hasTyped = props.password !== "";
    const canRevealStored = !hasTyped && props.linkedPasswordId !== null;
    const pwRowRef = useRef<HTMLDivElement>(null);

    const stopReveal = useCallback(() => {
        if (revealTimer.current) clearInterval(revealTimer.current);
        revealTimer.current = null;
        setRevealed(null);
        setSecondsLeft(0);
    }, []);

    useEffect(() => stopReveal, [stopReveal]);
    useEffect(() => {
        stopReveal();
        setShowPassword(false);
        setPickerOpen(false);
        setAddKeyOpen(false);
        setPwEditing(false);
        if (props.linkedKeyId !== null || props.linkedAgentId !== null) {
            setAuthMode("key");
        }
    }, [props.linkedPasswordId, props.linkedKeyId, props.linkedAgentId, stopReveal]);

    const revealStored = useCallback(async () => {
        if (!props.linkedPasswordId) return;
        try {
            const res = await credApi.reveal(props.linkedPasswordId);
            if (res.secret == null) return;
            setRevealed(res.secret);
            setSecondsLeft(REVEAL_SECONDS);
            if (revealTimer.current) clearInterval(revealTimer.current);
            revealTimer.current = setInterval(() => {
                setSecondsLeft((s) => {
                    if (s <= 1) {
                        stopReveal();
                        return 0;
                    }
                    return s - 1;
                });
            }, 1000);
        } catch {
            // Reveal failed — stay hidden silently.
        }
    }, [props.linkedPasswordId, stopReveal]);

    // Re-lock the password when the user clicks/taps outside its row
    // (including hitting Connect, another field, etc.) — and stop any
    // open reveal. Keeps "saved = locked & muted" as the resting state.
    useEffect(() => {
        if (!pwEditing && revealed === null) return;
        const onDoc = (e: MouseEvent) => {
            if (pwRowRef.current && !pwRowRef.current.contains(e.target as Node)) {
                setPwEditing(false);
                setShowPassword(false);
                stopReveal();
            }
        };
        document.addEventListener("mousedown", onDoc);
        return () => document.removeEventListener("mousedown", onDoc);
    }, [pwEditing, revealed, stopReveal]);

    const onEyeClick = () => {
        if (hasTyped) {
            setShowPassword((v) => !v);
        } else if (revealed !== null) {
            stopReveal();
        } else {
            void revealStored();
        }
    };

    // Pencil → unlock for editing. Reveal the stored secret into the field
    // (as plaintext) so the user can edit it — or clear it to delete the
    // password, like any text field. No ✕ shortcut.
    const beginPasswordEdit = useCallback(async () => {
        stopReveal();
        setShowPassword(true);
        setPwEditing(true);
        if (props.linkedPasswordId) {
            try {
                const res = await credApi.reveal(props.linkedPasswordId);
                props.onPasswordRevealed(res.secret ?? "");
            } catch {
                props.onPasswordRevealed("");
            }
        }
    }, [props, stopReveal]);

    const showEye = hasTyped || canRevealStored || revealed !== null;
    const valueShown = revealed !== null;
    const fieldType = showPassword || valueShown ? "text" : "password";
    const fieldValue = valueShown ? revealed! : props.password;

    const passwordField = (
        <div className={styles.frow}>
            <div className={styles.frow__l}>{t("dialog.host.passwordLabel")}</div>
            <div className={styles.frow__c}>
            <div
                ref={pwRowRef}
                className={`${styles.passwordWrap} ${props.linkedPasswordId ? styles.removable : ""}`}
            >
                <Lock size={14} className={styles.fieldIcon} />
                <Input
                    className={`${styles.iconInput} ${pwLocked ? styles.pwMuted : ""}`}
                    type={fieldType}
                    value={fieldValue}
                    onChange={(e) => {
                        if (valueShown) stopReveal();
                        props.onPassword(e.target.value);
                    }}
                    readOnly={valueShown || pwLocked}
                    placeholder={
                        props.linkedPasswordId
                            ? "••••••••"
                            : t("dialog.host.credentialPasswordPlaceholder")
                    }
                    autoComplete="off"
                />
                <div className={styles.fieldControls}>
                    {valueShown && (
                        <span className={styles.revealTimer}>
                            {t("host.reveal.timeLeft", { seconds: secondsLeft })}
                        </span>
                    )}
                    {showEye && (
                        <button
                            type="button"
                            className={styles.passwordEye}
                            onClick={onEyeClick}
                            title={
                                showPassword || valueShown
                                    ? t("common.hide")
                                    : t("common.show")
                            }
                            aria-label={
                                showPassword || valueShown
                                    ? t("common.hide")
                                    : t("common.show")
                            }
                        >
                            {showPassword || valueShown ? (
                                <EyeOff size={14} />
                            ) : (
                                <Eye size={14} />
                            )}
                        </button>
                    )}
                    {pwLocked && (
                        <button
                            type="button"
                            className={styles.passwordEye}
                            onClick={() => void beginPasswordEdit()}
                            title={t("common.edit")}
                            aria-label={t("common.edit")}
                        >
                            <Pencil size={14} />
                        </button>
                    )}
                </div>
            </div>
            </div>
        </div>
    );

    return (
        <div className={styles.credentialPanel}>
            {/* Login */}
            <div className={styles.frow}>
                <div className={styles.frow__l}>{t("dialog.host.loginLabel")}</div>
                <div className={styles.frow__c}>
                    <div className={styles.iconField}>
                        <User size={14} className={styles.fieldIcon} />
                        <Input
                            className={styles.iconInput}
                            value={props.username}
                            onChange={(e) => props.onUsername(e.target.value)}
                            placeholder={t("dialog.host.credentialUsername")}
                            autoComplete="off"
                            spellCheck={false}
                        />
                    </div>
                </div>
            </div>

            {props.protocol === "ssh" ? (
                <>
                    {/* Authentication method */}
                    <div className={styles.frow}>
                        <div className={styles.frow__l}>
                            {t("dialog.host.authLabel")}
                        </div>
                        <div className={styles.frow__c}>
                            <div className={styles.segmented}>
                                <button
                                    type="button"
                                    className={`${styles.seg} ${authMode === "key" ? styles.segOn : ""}`}
                                    onClick={() => setAuthMode("key")}
                                >
                                    <KeyRound size={14} /> {t("dialog.host.authKind.key")}
                                </button>
                                <button
                                    type="button"
                                    className={`${styles.seg} ${authMode === "password" ? styles.segOn : ""}`}
                                    onClick={() => setAuthMode("password")}
                                >
                                    <Lock size={14} /> {t("dialog.host.authKind.password")}
                                </button>
                            </div>
                        </div>
                    </div>

                    {authMode === "key" ? (
                        <div className={styles.frow}>
                            <div className={styles.frow__l}>
                                {t("dialog.host.keyLabel")}
                            </div>
                            <div className={styles.frow__c}>
                            <div className={styles.authTriggerWrap} ref={comboRef}>
                                <div
                                    className={`${styles.keyCombo}${pickerOpen ? ` ${styles.keyComboOpen}` : ""}`}
                                >
                                    <KeyRound size={14} className={styles.keySelectIcon} />
                                    <input
                                        type="text"
                                        className={styles.keyComboInput}
                                        value={
                                            pickerOpen
                                                ? keyFilter
                                                : props.linkedKeyName ??
                                                  (agentLinked
                                                      ? t("dialog.host.authKind.agent")
                                                      : "")
                                        }
                                        placeholder={t("dialog.host.keyNone")}
                                        readOnly={!pickerOpen}
                                        onFocus={() => {
                                            if (!pickerOpen) {
                                                setKeyFilter("");
                                                setPickerOpen(true);
                                            }
                                        }}
                                        onClick={() => {
                                            if (!pickerOpen) {
                                                setKeyFilter("");
                                                setPickerOpen(true);
                                            }
                                        }}
                                        onChange={(e) => setKeyFilter(e.target.value)}
                                        onKeyDown={(e) => {
                                            if (e.key === "Escape") {
                                                setPickerOpen(false);
                                                e.currentTarget.blur();
                                            }
                                        }}
                                    />
                                    {methodLinked ? (
                                        <button
                                            type="button"
                                            className={styles.keyComboBtn}
                                            title={t("dialog.host.keyClear")}
                                            aria-label={t("dialog.host.keyClear")}
                                            onMouseDown={(e) => e.preventDefault()}
                                            onClick={() => {
                                                setPickerOpen(false);
                                                if (props.linkedKeyId)
                                                    props.onUnlink(props.linkedKeyId);
                                                if (props.linkedAgentId)
                                                    props.onUnlink(props.linkedAgentId);
                                            }}
                                        >
                                            <X size={14} />
                                        </button>
                                    ) : (
                                        <button
                                            type="button"
                                            className={styles.keyComboBtn}
                                            aria-label={t("dialog.host.keyLabel")}
                                            onMouseDown={(e) => e.preventDefault()}
                                            onClick={() => {
                                                setKeyFilter("");
                                                setPickerOpen((v) => !v);
                                            }}
                                        >
                                            <ChevronDown size={14} />
                                        </button>
                                    )}
                                </div>
                                {pickerOpen && (
                                    <SavedCredentialPicker
                                        filter={keyFilter}
                                        onPick={async (id) => {
                                            setPickerOpen(false);
                                            await props.onPickSaved(id);
                                        }}
                                        onAddNew={() => {
                                            setPickerOpen(false);
                                            setAddKeyOpen(true);
                                        }}
                                        onUseAgent={async () => {
                                            setPickerOpen(false);
                                            await props.onUseAgent();
                                        }}
                                    />
                                )}
                            </div>
                            </div>
                        </div>
                    ) : (
                        passwordField
                    )}
                </>
            ) : (
                passwordField
            )}

            {addKeyOpen && (
                <AddKeyModal
                    onClose={() => setAddKeyOpen(false)}
                    onAdd={async (args) => {
                        setAddKeyOpen(false);
                        await props.onAddKey(args);
                    }}
                />
            )}
        </div>
    );
}

export function SavedCredentialPicker({
    filter,
    onPick,
    onAddNew,
    onUseAgent,
}: {
    /** Case-insensitive substring filter over key names. */
    filter: string;
    onPick: (credentialId: string) => Promise<void>;
    onAddNew: () => void;
    /** Optional — when provided, shows a pinned "Use SSH agent" entry. */
    onUseAgent?: () => void;
}) {
    const { t } = useT();
    const credentials = useCredentialsStore((s) => s.items);
    const q = filter.trim().toLowerCase();
    const keys = credentials
        .filter((c) => c.kind === "ssh_key")
        .filter((c) => q === "" || c.name.toLowerCase().includes(q));

    return (
        <div className={styles.savedPicker}>
            <div className={styles.savedPickerScroll}>
                {keys.length === 0 ? (
                    <div className={styles.savedPickerEmpty}>
                        {t("dialog.host.keyNoMatch")}
                    </div>
                ) : (
                    keys.map((c) => (
                        <button
                            key={c.id}
                            type="button"
                            className={styles.savedPickerRow}
                            onClick={() => void onPick(c.id)}
                        >
                            <KeyRound size={14} />
                            <span className={styles.savedPickerName}>{c.name}</span>
                        </button>
                    ))
                )}
            </div>
            <div className={styles.savedPickerPinned}>
                <button
                    type="button"
                    className={`${styles.savedPickerRow} ${styles.savedPickerAdd}`}
                    onClick={onAddNew}
                >
                    <Plus size={14} />
                    <span className={styles.savedPickerName}>
                        {t("dialog.host.addNewKey")}
                    </span>
                </button>
                {onUseAgent && (
                    <button
                        type="button"
                        className={`${styles.savedPickerRow} ${styles.savedPickerAdd}`}
                        onClick={onUseAgent}
                    >
                        <Server size={14} />
                        <span className={styles.savedPickerName}>
                            {t("dialog.host.useAgent")}
                        </span>
                    </button>
                )}
            </div>
        </div>
    );
}

// Modal for adding a brand-new SSH key: paste or import from file
// (OpenSSH / PEM / PuTTY .ppk), optional passphrase. The key is created
// in the keychain and applied to the host immediately on confirm.
export function AddKeyModal({
    onClose,
    onAdd,
}: {
    onClose: () => void;
    onAdd: (args: {
        key: string;
        passphrase: string;
        name: string;
    }) => Promise<void>;
}) {
    const { t } = useT();
    const [key, setKey] = useState("");
    const [passphrase, setPassphrase] = useState("");
    const [name, setName] = useState("");
    const [note, setNote] = useState<string | null>(null);
    const [busy, setBusy] = useState(false);
    const fileRef = useRef<HTMLInputElement>(null);
    const canAdd = key.trim() !== "" && !busy;

    const confirm = async () => {
        if (!canAdd) return;
        setBusy(true);
        try {
            await onAdd({ key: key.trim(), passphrase, name: name.trim() });
        } finally {
            setBusy(false);
        }
    };

    return (
        <Dialog
            open
            onClose={onClose}
            title={t("dialog.host.addKeyTitle")}
            size="md"
            footer={
                <>
                    <Button variant="secondary" onClick={onClose}>
                        {t("common.cancel")}
                    </Button>
                    <Button variant="primary" onClick={confirm} disabled={!canAdd}>
                        {t("common.add")}
                    </Button>
                </>
            }
        >
            <div className={styles.addKeyBody}>
                <Input
                    value={name}
                    onChange={(e) => setName(e.target.value)}
                    placeholder={t("dialog.host.keyNamePlaceholder")}
                    spellCheck={false}
                    autoComplete="off"
                />
                <Textarea
                    value={key}
                    onChange={(e) => {
                        setNote(null);
                        setKey(e.target.value);
                    }}
                    placeholder={
                        "-----BEGIN OPENSSH PRIVATE KEY-----\n...\n-----END OPENSSH PRIVATE KEY-----"
                    }
                    rows={7}
                    spellCheck={false}
                    autoComplete="off"
                    style={{
                        fontFamily: "var(--font-mono)",
                        fontSize: "var(--text-sm)",
                    }}
                />
                <div className={styles.useSavedRow}>
                    <button
                        type="button"
                        className={styles.useSavedButton}
                        onClick={() => fileRef.current?.click()}
                    >
                        <Upload size={13} />
                        {t("dialog.host.importKeyFile")}
                    </button>
                </div>
                {note && <span className={styles.inlineHint}>{note}</span>}
                <input
                    ref={fileRef}
                    type="file"
                    accept=".pem,.key,.ppk,.openssh,application/x-pem-file"
                    style={{ display: "none" }}
                    onChange={(e) => {
                        const file = e.target.files?.[0];
                        e.target.value = "";
                        if (!file) return;
                        if (name.trim() === "") setName(file.name);
                        const reader = new FileReader();
                        reader.onload = () => {
                            const text = String(reader.result ?? "").trim();
                            setKey(text);
                            setNote(
                                /PuTTY-User-Key-File/.test(text)
                                    ? t("host.key.ppkImported")
                                    : null,
                            );
                        };
                        reader.readAsText(file);
                    }}
                />
                <Input
                    type="password"
                    value={passphrase}
                    onChange={(e) => setPassphrase(e.target.value)}
                    placeholder={t("dialog.host.passphrasePlaceholder")}
                    autoComplete="off"
                />
            </div>
        </Dialog>
    );
}


// =====================================================================
// Helpers
// =====================================================================

function buildFormState(
    props: HostFormProps,
    linkedUsername?: string,
    linkedKind?: CredentialKind,
): FormState {
    // Stage 1.8: the Label input now binds directly to the explicit
    // `display_name` column. No more `name === hostname` heuristic —
    // an unset label is simply `display_name === null`, and the input
    // shows the current hostname as its placeholder.
    return {
        label: props.host.display_name ?? "",
        hostname: props.host.hostname,
        port:
            props.mode === "draft" && props.host.hostname === ""
                ? ""
                : String(props.host.port),
        protocol: props.host.protocol,
        groupId: props.host.group_id ?? "",
        tagsRaw: props.host.tags.join(", "),
        notes: props.host.notes ?? "",
        startupCommand: props.host.startup_command ?? "",
        jumpHostId: props.host.jump_host_id ?? "",
        agentForwarding: props.host.agent_forwarding,
        favorite: props.host.favorite,
        envRaw: formatEnv(props.host.env_vars),
        // Username lives on the host now. Prefer it; for drafts use the
        // remembered draft value; fall back to the linked credential's
        // username only for hosts saved before the per-host migration.
        inlineUsername:
            props.host.username ||
            props.initialInlineUsername ||
            linkedUsername ||
            "",
        inlinePassword: props.initialInlinePassword ?? "",
        // Auth method follows the linked credential's kind when present,
        // otherwise the draft's remembered choice (default: password).
        inlineAuthKind:
            linkedKind === "ssh_key"
                ? "key"
                : linkedKind === "password"
                  ? "password"
                  : props.initialInlineAuthKind ?? "password",
        // Never seed the stored key/passphrase — we don't auto-reveal them.
        inlinePrivateKey: props.initialInlinePrivateKey ?? "",
        inlinePassphrase: props.initialInlinePassphrase ?? "",
        inlineKeyName: props.initialInlineKeyName ?? "",
    };
}

/** Serialize env vars to the textarea form: one `KEY=VALUE` per line. */
function formatEnv(vars: EnvVar[]): string {
    return vars.map((v) => `${v.key}=${v.value}`).join("\n");
}

/**
 * Parse the env textarea back into a list. Rules:
 * - One variable per line, split on the first `=`.
 * - Blank lines and lines starting with `#` are ignored.
 * - Keys are trimmed; lines with an empty key are dropped.
 * - Values keep their surrounding whitespace trimmed (a leading/trailing
 *   space in an env value is almost always a typo).
 */
function parseEnv(raw: string): EnvVar[] {
    const out: EnvVar[] = [];
    const seen = new Set<string>();
    for (const line of raw.split("\n")) {
        const trimmed = line.trim();
        if (trimmed === "" || trimmed.startsWith("#")) continue;
        const eq = trimmed.indexOf("=");
        if (eq < 0) continue;
        const key = trimmed.slice(0, eq).trim();
        const value = trimmed.slice(eq + 1).trim();
        if (key === "" || seen.has(key)) continue;
        seen.add(key);
        out.push({ key, value });
    }
    return out;
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
