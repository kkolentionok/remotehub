import { useCallback, useMemo, useState } from "react";
import {
    ChevronDown,
    ChevronRight,
    Copy,
    Folder,
    FolderInput,
    FolderPlus,
    FolderX,
    KeyRound,
    Pencil,
    Plug,
    Plus,
    Settings,
} from "lucide-react";

import { useT } from "../../i18n";
import {
    isDraftDirty,
    isDraftPromotable,
    useGroupsStore,
    useHostsStore,
    useSessionsStore,
    useUiStore,
} from "../../store";
import { credentials as credentialsApi, hosts as hostsApi } from "../../lib/ipc";
import type { GroupId, HostDto, HostGroupDto } from "../../lib/types";
import { HostIcon } from "../host/HostIcon";
import { Button } from "../ui/Button";
import { ContextMenu, type MenuItem } from "../ui/ContextMenu";
import { ProtocolBadge } from "../ui/ProtocolBadge";
import styles from "./Sidebar.module.css";

type Guard = (cb: () => void) => void;

/**
 * Left pane. Stage 1.5.2:
 * - clicking a host (or starting a draft) is intercepted by `guardNavigation`
 *   that surfaces a discard-changes confirm if the current draft is dirty
 *   but not yet promotable.
 * - drafts render as a separate section above the tree.
 */
export function Sidebar() {
    const { t } = useT();
    const hosts = useHostsStore((s) => s.items);
    const hostsLoading = useHostsStore((s) => s.loading);
    const groups = useGroupsStore((s) => s.items);
    const draft = useUiStore((s) => s.draft);
    const searchQuery = useUiStore((s) => s.searchQuery);
    const startDraft = useUiStore((s) => s.startDraft);
    const setDialog = useUiStore((s) => s.setDialog);

    const guardNavigation: Guard = useCallback(
        (proceed) => {
            if (draft && isDraftDirty(draft) && !isDraftPromotable(draft)) {
                setDialog({
                    kind: "discard-changes-confirm",
                    onConfirm: () => proceed(),
                });
                return;
            }
            proceed();
        },
        [draft, setDialog],
    );

    const filteredHosts = useMemo(() => {
        const q = searchQuery.trim().toLowerCase();
        if (!q) return hosts;
        return hosts.filter(
            (h) =>
                h.name.toLowerCase().includes(q) ||
                h.hostname.toLowerCase().includes(q) ||
                h.tags.some((tag) => tag.toLowerCase().includes(q)),
        );
    }, [hosts, searchQuery]);

    const tree = useMemo(() => buildTree(groups, filteredHosts), [groups, filteredHosts]);

    return (
        <aside className={styles.sidebar}>
            <nav className={styles.tree}>
                {draft && <DraftRow />}

                {tree.length === 0 && !hostsLoading && !draft ? (
                    <div className={styles.emptyTree}>
                        {searchQuery ? t("sidebar.emptySearch") : t("sidebar.empty")}
                    </div>
                ) : null}

                {tree.map((node) =>
                    node.kind === "group" ? (
                        <GroupNode
                            key={node.group.id}
                            group={node.group}
                            hosts={node.hosts}
                            guardNavigation={guardNavigation}
                        />
                    ) : (
                        <UngroupedNode
                            key="__ungrouped"
                            hosts={node.hosts}
                            guardNavigation={guardNavigation}
                        />
                    ),
                )}
            </nav>

            <footer className={styles.footer}>
                <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => guardNavigation(() => startDraft(null))}
                    title={t("sidebar.newHost")}
                >
                    <Plus size={14} /> {t("sidebar.newHost")}
                </Button>
                <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => setDialog({ kind: "group-create", parentId: null })}
                    title={t("sidebar.newGroup")}
                >
                    <FolderPlus size={14} /> {t("sidebar.newGroup")}
                </Button>
                <div className={styles.footerSpacer} />
                <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => setDialog({ kind: "credentials-list" })}
                    title={t("sidebar.credentials")}
                    aria-label={t("sidebar.credentials")}
                >
                    <KeyRound size={14} />
                </Button>
                <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => setDialog({ kind: "settings" })}
                    title={t("sidebar.settings")}
                    aria-label={t("sidebar.settings")}
                >
                    <Settings size={14} />
                </Button>
            </footer>
        </aside>
    );
}

// =====================================================================
// Tree nodes (defined outside of Sidebar so React can keep them stable)
// =====================================================================

function DraftRow() {
    const { t } = useT();
    const draft = useUiStore((s) => s.draft);
    if (!draft) return null;
    const displayName =
        draft.label.trim() || draft.hostname.trim() || t("host.newHost");
    return (
        <div className={styles.draftSection}>
            <div className={styles.draftHeader}>{t("sidebar.draft")}</div>
            <div
                className={`${styles.hostRow} ${styles.hostRowSelected} ${styles.hostRowDraft}`}
            >
                <HostIcon />
                <span className={styles.hostName}>{displayName}</span>
                <ProtocolBadge protocol={draft.protocol} size="sm" />
            </div>
        </div>
    );
}

function GroupNode({
    group,
    hosts,
    guardNavigation,
}: {
    group: HostGroupDto;
    hosts: HostDto[];
    guardNavigation: Guard;
}) {
    const { t } = useT();
    const collapsedIds = useUiStore((s) => s.collapsedGroupIds);
    const toggle = useUiStore((s) => s.toggleGroupCollapsed);
    const startDraft = useUiStore((s) => s.startDraft);
    const setDialog = useUiStore((s) => s.setDialog);
    const collapsed = collapsedIds.has(group.id);

    return (
        <div className={styles.group}>
            <div className={styles.groupHeader}>
                <button
                    className={styles.groupToggle}
                    onClick={() => toggle(group.id)}
                    type="button"
                >
                    {collapsed ? <ChevronRight size={12} /> : <ChevronDown size={12} />}
                    <span className={styles.groupName}>{group.name}</span>
                    <span className={styles.groupCount}>{hosts.length}</span>
                </button>
                <div className={styles.groupActions}>
                    <button
                        className={styles.groupAction}
                        onClick={() => guardNavigation(() => startDraft(group.id))}
                        title={t("sidebar.addHostInGroup")}
                        aria-label={t("sidebar.addHostInGroup")}
                        type="button"
                    >
                        <Plus size={12} />
                    </button>
                    <button
                        className={`${styles.groupAction} ${styles.groupActionEdit}`}
                        onClick={() =>
                            setDialog({ kind: "group-rename", groupId: group.id })
                        }
                        title={t("sidebar.editGroup")}
                        aria-label={t("sidebar.editGroup")}
                        type="button"
                    >
                        <Pencil size={11} />
                    </button>
                </div>
            </div>
            {!collapsed && (
                <ul className={styles.hostList}>
                    {hosts.map((h) => (
                        <HostRow key={h.id} host={h} guardNavigation={guardNavigation} />
                    ))}
                </ul>
            )}
        </div>
    );
}

function UngroupedNode({
    hosts,
    guardNavigation,
}: {
    hosts: HostDto[];
    guardNavigation: Guard;
}) {
    const { t } = useT();
    const startDraft = useUiStore((s) => s.startDraft);
    if (hosts.length === 0) return null;
    return (
        <div className={styles.group}>
            <div className={styles.groupHeader}>
                <div className={styles.ungroupedTitle}>{t("sidebar.ungrouped")}</div>
                <div className={styles.groupActions}>
                    <button
                        className={styles.groupAction}
                        onClick={() => guardNavigation(() => startDraft(null))}
                        title={t("sidebar.addHostInGroup")}
                        aria-label={t("sidebar.addHostInGroup")}
                        type="button"
                    >
                        <Plus size={12} />
                    </button>
                </div>
            </div>
            <ul className={styles.hostList}>
                {hosts.map((h) => (
                    <HostRow key={h.id} host={h} guardNavigation={guardNavigation} />
                ))}
            </ul>
        </div>
    );
}

/**
 * Create a copy of `host` (same name, all fields pre-filled) as a real host,
 * then open it selected so the user can tweak it. Mirrors the normal create
 * path (default credential passed to host_create); extra linked credentials
 * are re-linked explicitly.
 */
async function duplicateHost(host: HostDto) {
    try {
        const full = await hostsApi.get(host.id);
        const { id: newId } = await hostsApi.create({
            name: full.name,
            display_name: full.display_name,
            group_id: full.group_id,
            protocol: full.protocol,
            hostname: full.hostname,
            port: full.port,
            username: full.username,
            tags: full.tags,
            color: full.color,
            notes: full.notes,
            startup_command: full.startup_command,
            env_vars: full.env_vars,
            default_credential_id: full.default_credential_id,
            jump_host_id: full.jump_host_id,
            agent_forwarding: full.agent_forwarding,
            favorite: false,
        });
        for (const cid of full.credential_ids) {
            if (cid !== full.default_credential_id) {
                await credentialsApi.linkHost({ host_id: newId, credential_id: cid });
            }
        }
        await useHostsStore.getState().load();
        useUiStore.getState().selectHost(newId);
    } catch (e) {
        console.error("duplicate host failed:", e);
    }
}

function HostRow({
    host,
    guardNavigation,
}: {
    host: HostDto;
    guardNavigation: Guard;
}) {
    const selectedId = useUiStore((s) => s.selectedHostId);
    const selectHost = useUiStore((s) => s.selectHost);
    const groups = useGroupsStore((s) => s.items);
    const { t } = useT();
    const isSelected = selectedId === host.id;
    const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);

    const moveHost = useCallback(
        (groupId: GroupId | null) => {
            void hostsApi
                .update({ id: host.id, group_id: groupId })
                .then(() => useHostsStore.getState().load())
                .catch((e) => console.error("move host failed:", e));
        },
        [host.id],
    );

    // Move-to-folder submenu: top-level groups (the ones shown in the tree)
    // plus "No group". The host's current location is disabled.
    const topGroups = groups.filter((g) => !g.parent_id);
    const moveChildren: MenuItem[] = [
        {
            id: "grp-none",
            label: t("host.menu.noGroup"),
            icon: FolderX,
            disabled: host.group_id === null,
            onSelect: () => moveHost(null),
        },
        ...(topGroups.length > 0
            ? ([{ id: "grp-sep", separator: true }] as MenuItem[])
            : []),
        ...topGroups.map(
            (g): MenuItem => ({
                id: `grp-${g.id}`,
                label: g.name,
                icon: Folder,
                disabled: host.group_id === g.id,
                onSelect: () => moveHost(g.id),
            }),
        ),
    ];

    const items: MenuItem[] = [
        {
            id: "connect",
            label: t("host.menu.connect"),
            icon: Plug,
            onSelect: () => void useSessionsStore.getState().open(host),
        },
        {
            id: "edit",
            label: t("host.menu.edit"),
            icon: Pencil,
            onSelect: () => guardNavigation(() => selectHost(host.id)),
        },
        {
            id: "duplicate",
            label: t("host.menu.duplicate"),
            icon: Copy,
            onSelect: () => guardNavigation(() => void duplicateHost(host)),
        },
        { id: "sep", separator: true },
        {
            id: "move",
            label: t("host.menu.move"),
            icon: FolderInput,
            children: moveChildren,
        },
    ];

    return (
        <li>
            <button
                className={`${styles.hostRow} ${isSelected ? styles.hostRowSelected : ""}`}
                onClick={() => guardNavigation(() => selectHost(host.id))}
                onDoubleClick={() => {
                    if (host.protocol === "ssh") {
                        void useSessionsStore.getState().open(host);
                    }
                }}
                onContextMenu={(e) => {
                    e.preventDefault();
                    e.stopPropagation();
                    setMenu({ x: e.clientX, y: e.clientY });
                }}
                title={host.protocol === "ssh" ? t("host.doubleClickConnect") : undefined}
                type="button"
            >
                <HostIcon detectedOs={host.detected_os} />
                <span className={styles.hostName}>{host.name}</span>
                <ProtocolBadge protocol={host.protocol} size="sm" />
            </button>
            {menu && (
                <ContextMenu
                    x={menu.x}
                    y={menu.y}
                    items={items}
                    onClose={() => setMenu(null)}
                />
            )}
        </li>
    );
}

// =====================================================================
// Tree assembly
// =====================================================================

type TreeNode =
    | { kind: "group"; group: HostGroupDto; hosts: HostDto[] }
    | { kind: "ungrouped"; hosts: HostDto[] };

function buildTree(groups: HostGroupDto[], hosts: HostDto[]): TreeNode[] {
    const byGroup = new Map<GroupId, HostDto[]>();
    const ungrouped: HostDto[] = [];

    for (const h of hosts) {
        if (h.group_id) {
            const list = byGroup.get(h.group_id);
            if (list) list.push(h);
            else byGroup.set(h.group_id, [h]);
        } else {
            ungrouped.push(h);
        }
    }

    for (const list of byGroup.values()) {
        list.sort((a, b) =>
            a.name.localeCompare(b.name, undefined, { sensitivity: "base" }),
        );
    }
    ungrouped.sort((a, b) =>
        a.name.localeCompare(b.name, undefined, { sensitivity: "base" }),
    );

    const result: TreeNode[] = [];
    for (const g of groups) {
        if (g.parent_id) continue;
        result.push({ kind: "group", group: g, hosts: byGroup.get(g.id) ?? [] });
    }
    if (ungrouped.length > 0) {
        result.push({ kind: "ungrouped", hosts: ungrouped });
    }
    return result;
}
