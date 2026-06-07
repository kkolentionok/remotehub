import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Plus, Search } from "lucide-react";

import { useT } from "../../i18n";
import type { Protocol } from "../../lib/types";
import { useHostsStore, useUiStore } from "../../store";
import styles from "./CommandBar.module.css";

/**
 * Top command bar (Stage 1.9). The single search/command surface for
 * the app:
 *
 *  - Typing filters the sidebar host tree live (it owns `searchQuery`;
 *    the sidebar no longer has its own box — one search, not two).
 *  - When the text parses as a connection string
 *    `[ssh|rdp]://[user@]host[:port]` that isn't an existing host, a
 *    slim "new host" suggestion appears below; activating it opens a
 *    pre-filled draft. (Real connect lands in Stage 2 — this is quick
 *    host creation for now.)
 *
 * Ctrl/Cmd+K focuses. Enter creates the suggested host, or opens the
 * sole match if exactly one host matches. Esc clears.
 */

interface ParsedConnection {
    username: string | null;
    hostname: string;
    port: number | null;
    protocol: Protocol;
    explicit: boolean;
}

function looksLikeHost(h: string): boolean {
    if (h.includes(":") && h.split(":").length >= 3) return true; // IPv6-ish
    if (/^(\d{1,3}\.){3}\d{1,3}$/.test(h)) return true; // IPv4
    if (!h.includes(".")) return false;
    return h
        .split(".")
        .every((l) => /^[a-zA-Z0-9]([a-zA-Z0-9-]*[a-zA-Z0-9])?$/.test(l));
}

function parseConnection(raw: string): ParsedConnection | null {
    let rest = raw.trim();
    if (rest === "") return null;

    let schemeProto: Protocol | null = null;
    const scheme = rest.match(/^(ssh|rdp):\/\//i);
    if (scheme) {
        schemeProto = scheme[1]!.toLowerCase() as Protocol;
        rest = rest.slice(scheme[0].length);
    }

    let username: string | null = null;
    const at = rest.indexOf("@");
    if (at >= 0) {
        username = rest.slice(0, at).trim() || null;
        rest = rest.slice(at + 1);
    }

    let port: number | null = null;
    const colon = rest.lastIndexOf(":");
    if (colon >= 0) {
        const maybe = rest.slice(colon + 1);
        if (/^\d{1,5}$/.test(maybe)) {
            port = Number(maybe);
            rest = rest.slice(0, colon);
        }
    }

    const hostname = rest.trim();
    if (hostname === "" || !looksLikeHost(hostname)) return null;
    if (port !== null && (port < 1 || port > 65535)) return null;

    const explicit =
        username !== null ||
        port !== null ||
        schemeProto !== null ||
        hostname.includes(".") ||
        hostname.includes(":");

    const protocol: Protocol = schemeProto ?? (port === 3389 ? "rdp" : "ssh");
    return { username, hostname, port, protocol, explicit };
}

export function CommandBar() {
    const { t } = useT();
    const hosts = useHostsStore((s) => s.items);
    const query = useUiStore((s) => s.searchQuery);
    const setSearchQuery = useUiStore((s) => s.setSearchQuery);
    const selectHost = useUiStore((s) => s.selectHost);
    const startDraft = useUiStore((s) => s.startDraft);
    const updateDraft = useUiStore((s) => s.updateDraft);

    const [open, setOpen] = useState(false);
    const inputRef = useRef<HTMLInputElement>(null);
    const wrapRef = useRef<HTMLDivElement>(null);

    useEffect(() => {
        const onKey = (e: KeyboardEvent) => {
            if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
                e.preventDefault();
                inputRef.current?.focus();
                inputRef.current?.select();
            }
        };
        document.addEventListener("keydown", onKey);
        return () => document.removeEventListener("keydown", onKey);
    }, []);

    useEffect(() => {
        if (!open) return;
        const onDoc = (e: MouseEvent) => {
            if (wrapRef.current && !wrapRef.current.contains(e.target as Node)) {
                setOpen(false);
            }
        };
        document.addEventListener("mousedown", onDoc);
        return () => document.removeEventListener("mousedown", onDoc);
    }, [open]);

    const newAction = useMemo(() => {
        const parsed = parseConnection(query);
        if (!parsed || !parsed.explicit) return null;
        const exact = hosts.some(
            (h) => h.hostname.toLowerCase() === parsed.hostname.toLowerCase(),
        );
        if (exact) return null;
        const label =
            (parsed.username ? `${parsed.username}@` : "") +
            parsed.hostname +
            (parsed.port ? `:${parsed.port}` : "");
        return { parsed, label };
    }, [query, hosts]);

    const reset = useCallback(() => {
        setSearchQuery("");
        setOpen(false);
        inputRef.current?.blur();
    }, [setSearchQuery]);

    const createFromString = useCallback(() => {
        if (!newAction) return;
        const { parsed } = newAction;
        startDraft(null);
        updateDraft({
            label: "",
            hostname: parsed.hostname,
            port: parsed.port ? String(parsed.port) : "",
            protocol: parsed.protocol,
            inlineUsername: parsed.username ?? "",
        });
        reset();
    }, [newAction, startDraft, updateDraft, reset]);

    const onKeyDown = (e: React.KeyboardEvent) => {
        if (e.key === "Escape") {
            reset();
            return;
        }
        if (e.key !== "Enter") return;
        e.preventDefault();
        if (newAction) {
            createFromString();
            return;
        }
        const q = query.trim().toLowerCase();
        if (q === "") return;
        const matches = hosts.filter(
            (h) =>
                h.name.toLowerCase().includes(q) ||
                h.hostname.toLowerCase().includes(q) ||
                h.tags.some((tag) => tag.toLowerCase().includes(q)),
        );
        if (matches.length === 1) {
            selectHost(matches[0]!.id);
            reset();
        }
    };

    const showDropdown = open && newAction !== null;

    return (
        <div className={styles.bar}>
            <div ref={wrapRef} className={styles.wrap}>
                <div className={styles.inputRow}>
                    <Search size={15} className={styles.searchIcon} />
                    <input
                        ref={inputRef}
                        className={styles.input}
                        value={query}
                        onChange={(e) => {
                            setSearchQuery(e.target.value);
                            setOpen(true);
                        }}
                        onFocus={() => setOpen(true)}
                        onKeyDown={onKeyDown}
                        placeholder={t("command.placeholder")}
                        spellCheck={false}
                        autoComplete="off"
                    />
                    <kbd className={styles.kbd}>Ctrl K</kbd>
                </div>

                {showDropdown && newAction && (
                    <div className={styles.dropdown}>
                        <button
                            type="button"
                            className={styles.row}
                            onClick={createFromString}
                        >
                            <Plus size={14} className={styles.newIcon} />
                            <span className={styles.rowName}>
                                {t("command.newHost", { target: newAction.label })}
                            </span>
                            <kbd className={styles.rowKbd}>Enter</kbd>
                        </button>
                    </div>
                )}
            </div>
        </div>
    );
}
