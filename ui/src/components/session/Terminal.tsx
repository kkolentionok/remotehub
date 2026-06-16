import { useCallback, useEffect, useRef, useState } from "react";
import { Terminal as XTerm } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { SearchAddon, type ISearchOptions } from "@xterm/addon-search";
import type { ITheme } from "@xterm/xterm";
import { ChevronDown, ChevronUp, Search, X } from "lucide-react";
import { readText, writeText } from "@tauri-apps/plugin-clipboard-manager";
import "@xterm/xterm/css/xterm.css";

import { useT } from "../../i18n";
import {
    registerSessionSearch,
    registerSessionTerminal,
    useSessionsStore,
    useSettingsStore,
    useUiStore,
} from "../../store";
import { app as appApi, settings as settingsApi } from "../../lib/ipc";
import { TERMINAL_THEMES } from "../../lib/terminalThemes";
import { createLogHighlighter } from "../../lib/logHighlight";
import styles from "./Terminal.module.css";

/** Terminal font-size zoom bounds (Ctrl+wheel). */
const FONT_MIN = 8;
const FONT_MAX = 32;

/** Scrollback lines kept per session. Generous so a large `cat`/log dump
 *  stays scrollable instead of falling off the top (xterm default is 1000). */
const SCROLLBACK = 50_000;

const MONO_FALLBACK =
    "ui-monospace, 'Cascadia Mono', 'SF Mono', Consolas, monospace";

/** Build an xterm fontFamily stack from the chosen primary family. */
function fontStack(family: string): string {
    if (!family || family === "monospace") return MONO_FALLBACK;
    return `'${family}', ${MONO_FALLBACK}`;
}

/** Add an alpha channel to a #rgb/#rrggbb color; pass rgba()/named through. */
function withAlpha(color: string, alpha: number): string {
    const c = color.trim();
    const m6 = /^#([0-9a-f]{6})$/i.exec(c);
    if (m6) {
        const n = parseInt(m6[1]!, 16);
        return `rgba(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255}, ${alpha})`;
    }
    const m3 = /^#([0-9a-f]{3})$/i.exec(c);
    if (m3) {
        const h = m3[1]!;
        const r = parseInt(h[0]! + h[0]!, 16);
        const g = parseInt(h[1]! + h[1]!, 16);
        const b = parseInt(h[2]! + h[2]!, 16);
        return `rgba(${r}, ${g}, ${b}, ${alpha})`;
    }
    return c;
}

/** SearchAddon options. All matches get a soft accent (blue) highlight; the
 *  *active* match — the one Enter/Shift+Enter lands on — gets a contrasting
 *  warm (amber) fill + a bright amber outline so it's obvious where focus is.
 *  Colors are read live so they track Navy / Redpanda / light themes. */
function searchOptions(incremental: boolean): ISearchOptions {
    const cs = getComputedStyle(document.documentElement);
    const accent = cs.getPropertyValue("--color-accent").trim() || "#4c8eff";
    const warn = cs.getPropertyValue("--color-warning").trim() || "#fbbf24";
    return {
        incremental,
        decorations: {
            matchBackground: withAlpha(accent, 0.28),
            matchBorder: accent,
            matchOverviewRuler: accent,
            activeMatchBackground: withAlpha(warn, 0.5),
            activeMatchBorder: warn,
            activeMatchColorOverviewRuler: warn,
        },
    };
}

// --- Live terminal pool -------------------------------------------------
// Splitting/moving a pane restructures the workspace tree, which forces React
// to unmount and remount the pane's <Terminal>. If we recreated xterm each
// time, the buffer (scrollback, the current screen, selection, search
// highlights) would be lost — serialising 50k lines out and back in is both
// lossy and slow. Instead we keep the *live xterm instance* alive in a pool
// keyed by session and just re-parent its DOM element into whichever
// container is currently mounted. The instance (and its PTY output sink) is
// disposed only when the session is genuinely gone — never on a remount.
interface PooledTerm {
    term: XTerm;
    fit: FitAddon;
    search: SearchAddon;
    /** The element xterm renders into; moved between pane containers. */
    el: HTMLDivElement;
    dispose: () => void;
}
const termPool = new Map<string, PooledTerm>();

/** Current i18n translator, refreshed by the mounted component so the link
 *  hover-tip (built inside the long-lived instance) tracks locale changes. */
let activeTranslate: (key: string) => string = (k) => k;

function acquireTerm(
    sessionKey: string,
    initial: {
        fontFamily: string;
        fontSize: number;
        cursorStyle: "block" | "underline" | "bar";
        theme: ITheme;
    },
): PooledTerm {
    const existing = termPool.get(sessionKey);
    if (existing) return existing;

    const term = new XTerm({
        fontFamily: fontStack(initial.fontFamily),
        fontSize: initial.fontSize,
        scrollback: SCROLLBACK,
        // SearchAddon highlight decorations use xterm's proposed decoration
        // API; without this registerDecoration throws inside findNext and
        // search silently reports zero matches.
        allowProposedApi: true,
        cursorBlink: true,
        cursorStyle: initial.cursorStyle,
        theme: initial.theme,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    const search = new SearchAddon();
    term.loadAddon(search);

    // Links: underline on hover with a "Ctrl + Click" tip; open in the system
    // browser on Ctrl/Cmd+click only.
    let linkTip: HTMLDivElement | null = null;
    const showLinkTip = (e: MouseEvent) => {
        if (!linkTip) {
            linkTip = document.createElement("div");
            linkTip.className = styles.linkTip ?? "";
            const k1 = document.createElement("kbd");
            k1.className = styles.linkTipKbd ?? "";
            k1.textContent = "Ctrl";
            const k2 = document.createElement("kbd");
            k2.className = styles.linkTipKbd ?? "";
            k2.textContent = activeTranslate("terminal.clickWord");
            const txt = document.createElement("span");
            txt.textContent = activeTranslate("terminal.openLinkHint");
            linkTip.append(k1, document.createTextNode("+"), k2, txt);
            document.body.appendChild(linkTip);
        }
        linkTip.style.left = `${e.clientX + 12}px`;
        linkTip.style.top = `${e.clientY - 34}px`;
        linkTip.style.display = "flex";
    };
    const hideLinkTip = () => {
        if (linkTip) linkTip.style.display = "none";
    };
    const links = new WebLinksAddon(
        (event, uri) => {
            if (event.ctrlKey || event.metaKey)
                appApi.open(uri).catch((err) => console.error("open link failed:", err));
        },
        {
            hover: (event) => showLinkTip(event as MouseEvent),
            leave: hideLinkTip,
        },
    );
    term.loadAddon(links);

    const el = document.createElement("div");
    el.style.width = "100%";
    el.style.height = "100%";
    term.open(el);

    // GPU renderer; recreate the context if it's lost (minimise / GPU reset).
    // If a fresh context dies almost immediately, WebGL is unstable here, so
    // stop and stay on the DOM renderer.
    let webgl: WebglAddon | null = null;
    const loadWebgl = () => {
        try {
            const addon = new WebglAddon();
            webgl = addon;
            const createdAt = Date.now();
            addon.onContextLoss(() => {
                addon.dispose();
                if (webgl === addon) webgl = null;
                if (Date.now() - createdAt > 1000) setTimeout(loadWebgl, 200);
            });
            term.loadAddon(addon);
        } catch {
            /* no WebGL — xterm keeps the DOM renderer */
        }
    };
    loadWebgl();

    // Keystrokes → PTY; xterm reflow → PTY window-change (kept in lockstep).
    const dataDisp = term.onData((d) =>
        useSessionsStore.getState().sendInput(sessionKey, new TextEncoder().encode(d)),
    );
    const resizeDisp = term.onResize(({ cols, rows }) =>
        useSessionsStore.getState().resize(sessionKey, cols, rows),
    );

    // Copy-on-select (Termius-style). xterm commits the selection on its own
    // document-level mouseup, so run after it: mark a drag started in THIS
    // terminal, then on document mouseup defer a tick and copy via Tauri.
    let selecting = false;
    const onMouseDown = () => {
        selecting = true;
    };
    const onDocMouseUp = () => {
        if (!selecting) return;
        selecting = false;
        setTimeout(() => {
            const sel = term.getSelection();
            if (!sel) return;
            writeText(sel).catch(() => {
                void navigator.clipboard?.writeText(sel).catch(() => {});
            });
        }, 0);
    };
    el.addEventListener("mousedown", onMouseDown);
    document.addEventListener("mouseup", onDocMouseUp);

    // Right-click pastes (terminal convention) instead of the WebView menu.
    const onContextMenu = (e: MouseEvent) => {
        e.preventDefault();
        void readText()
            .then((text) => {
                if (text) term.paste(text);
            })
            .catch(() => {});
    };
    el.addEventListener("contextmenu", onContextMenu);

    // Ctrl + wheel zooms the shared terminal_font_size setting (debounced
    // backend write). Plain wheel is left to xterm for scrollback.
    let zoomTimer = 0;
    const onWheel = (e: WheelEvent) => {
        if (!e.ctrlKey) return;
        e.preventDefault();
        const s = useSettingsStore.getState().settings;
        if (!s) return;
        const cur = s.terminal_font_size ?? 13;
        const next = Math.min(FONT_MAX, Math.max(FONT_MIN, cur + (e.deltaY < 0 ? 1 : -1)));
        if (next === cur) return;
        useSettingsStore.setState({ settings: { ...s, terminal_font_size: next } });
        clearTimeout(zoomTimer);
        zoomTimer = window.setTimeout(() => {
            void settingsApi.update({ patches: { terminal_font_size: next } });
        }, 500);
    };
    el.addEventListener("wheel", onWheel, { passive: false });

    // Live PTY output sink — lives with the instance, so output keeps landing
    // in the buffer even while the pane is briefly detached during a split.
    // Output passes through a client-side log highlighter (Termius-style):
    // it colourises plain-text tokens (log levels, ok/fail, IP:port) but never
    // touches escape sequences or text the program already styled.
    const highlight = createLogHighlighter();
    const unregisterOutput = registerSessionTerminal(sessionKey, (data) =>
        term.write(highlight(data)),
    );

    const pooled: PooledTerm = {
        term,
        fit,
        search,
        el,
        dispose: () => {
            clearTimeout(zoomTimer);
            el.removeEventListener("mousedown", onMouseDown);
            document.removeEventListener("mouseup", onDocMouseUp);
            el.removeEventListener("contextmenu", onContextMenu);
            el.removeEventListener("wheel", onWheel);
            if (linkTip) linkTip.remove();
            unregisterOutput();
            dataDisp.dispose();
            resizeDisp.dispose();
            try {
                term.dispose();
            } catch {
                /* already torn down */
            }
            el.remove();
        },
    };
    termPool.set(sessionKey, pooled);
    return pooled;
}

/** Tear down a session's terminal for good (real close, not a remount). */
function disposeTerm(sessionKey: string) {
    const p = termPool.get(sessionKey);
    if (!p) return;
    termPool.delete(sessionKey);
    p.dispose();
}

/**
 * xterm.js bound to one session tab. The heavy xterm instance is pooled (see
 * above) and survives pane remounts caused by splits/moves; this component
 * just attaches the pooled element, keeps options in sync with settings, and
 * renders the find-in-output box.
 */
export function Terminal({
    sessionKey,
    visible,
    focused,
}: {
    sessionKey: string;
    visible: boolean;
    focused: boolean;
}) {
    const containerRef = useRef<HTMLDivElement>(null);
    const termRef = useRef<XTerm | null>(null);
    const fitRef = useRef<FitAddon | null>(null);
    const searchRef = useRef<SearchAddon | null>(null);
    const searchInputRef = useRef<HTMLInputElement>(null);
    const { t } = useT();
    // Find-in-output box (toggled by the tab-bar magnifier or Ctrl+F).
    const [searchOpen, setSearchOpen] = useState(false);
    const [searchTerm, setSearchTerm] = useState("");
    const [matches, setMatches] = useState({ index: -1, count: 0 });

    const sessionId = useSessionsStore(
        (s) => s.sessions.find((tb) => tb.key === sessionKey)?.sessionId ?? null,
    );
    const fontFamily = useSettingsStore(
        (s) => s.settings?.terminal_font_family ?? "JetBrains Mono",
    );
    const fontSize = useSettingsStore((s) => s.settings?.terminal_font_size ?? 13);
    const scheme = useSettingsStore(
        (s) => s.settings?.terminal_color_scheme ?? "default",
    );
    const cursorStyle = useSettingsStore(
        (s) => s.settings?.terminal_cursor_style ?? "block",
    ) as "block" | "underline" | "bar";
    const dialogOpen = useUiStore((s) => s.dialog.kind !== "none");
    const theme = TERMINAL_THEMES[scheme] ?? TERMINAL_THEMES.default;

    // Keep the instance's link-tip translator current.
    activeTranslate = t as (key: string) => string;

    useEffect(() => {
        const container = containerRef.current;
        if (!container) return;

        const pooled = acquireTerm(sessionKey, {
            fontFamily,
            fontSize,
            cursorStyle,
            theme,
        });
        termRef.current = pooled.term;
        fitRef.current = pooled.fit;
        searchRef.current = pooled.search;

        // Attach the pooled element into this pane's container.
        container.appendChild(pooled.el);

        // The tab-bar magnifier toggles the find box; Ctrl/Cmd+F (handled
        // globally in AppShell so it wins over xterm) opens it.
        const unregisterSearch = registerSessionSearch(sessionKey, (mode) => {
            if (mode === "open") {
                setSearchOpen(true);
                requestAnimationFrame(() => {
                    searchInputRef.current?.focus();
                    searchInputRef.current?.select();
                });
            } else {
                setSearchOpen((o) => !o);
            }
        });

        const refit = () => {
            if (container.clientWidth === 0 || container.clientHeight === 0) return;
            try {
                pooled.fit.fit();
                pooled.term.refresh(0, pooled.term.rows - 1);
            } catch {
                /* ignore transient layout */
            }
        };
        // Fit now, after the next frame, and once webfonts settle (their wider
        // cell would otherwise clip the last row).
        requestAnimationFrame(refit);
        void document.fonts?.ready.then(refit);
        // Push the current size to the PTY so it stops assuming 80×24.
        useSessionsStore.getState().resize(sessionKey, pooled.term.cols, pooled.term.rows);
        if (focused) requestAnimationFrame(() => pooled.term.focus());

        // Refit when the pane settles (divider drag changes the box
        // continuously — debounce so the PTY is resized once movement stops).
        let fitTimer = 0;
        const scheduleFit = () => {
            clearTimeout(fitTimer);
            fitTimer = window.setTimeout(() => {
                if (container.clientWidth === 0 || container.clientHeight === 0) return;
                try {
                    pooled.fit.fit();
                    pooled.term.refresh(0, pooled.term.rows - 1);
                } catch {
                    /* ignore transient layout */
                }
            }, 140);
        };
        const ro = new ResizeObserver(scheduleFit);
        ro.observe(container);

        return () => {
            clearTimeout(fitTimer);
            ro.disconnect();
            unregisterSearch();
            if (pooled.el.parentElement === container) container.removeChild(pooled.el);
            termRef.current = null;
            fitRef.current = null;
            searchRef.current = null;
            // Dispose the instance only on a real close. During a split/move
            // the session still exists in the store and is about to remount —
            // keep it alive so its buffer survives.
            const stillAlive = useSessionsStore
                .getState()
                .sessions.some((s) => s.key === sessionKey);
            if (!stillAlive) disposeTerm(sessionKey);
        };
        // Settings are applied by dedicated effects below; only sessionKey
        // drives (re)attachment.
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [sessionKey]);

    // Once the backend session id is set, push the current terminal size so
    // the PTY/remote stops assuming the 80×24 default it opened with.
    useEffect(() => {
        if (!sessionId) return;
        const term = termRef.current;
        if (term) useSessionsStore.getState().resize(sessionKey, term.cols, term.rows);
    }, [sessionId, sessionKey]);

    // When this pane becomes visible (its tab activated) or the active pane,
    // refit/repaint and focus. Focus is looked up by sessionKey through the
    // pool so it always targets THIS pane's terminal, never a stale ref.
    useEffect(() => {
        if (!visible) return;
        const id = requestAnimationFrame(() => {
            const term = termPool.get(sessionKey)?.term;
            const el = containerRef.current;
            if (!term) return;
            if (el && el.clientWidth > 0) {
                try {
                    fitRef.current?.fit();
                    term.refresh(0, term.rows - 1);
                } catch {
                    /* ignore */
                }
            }
            if (focused) term.focus();
        });
        return () => cancelAnimationFrame(id);
    }, [visible, focused, sessionKey]);

    // A dialog (Settings, confirms…) renders in a portal and steals keyboard
    // focus. When it closes, the active terminal must take focus back.
    useEffect(() => {
        if (!visible || !focused || dialogOpen) return;
        const id = requestAnimationFrame(() => termPool.get(sessionKey)?.term.focus());
        return () => cancelAnimationFrame(id);
    }, [dialogOpen, visible, focused, sessionKey]);

    // Live-apply font changes from settings.
    useEffect(() => {
        const term = termRef.current;
        const el = containerRef.current;
        if (!term) return;
        term.options.fontFamily = fontStack(fontFamily);
        term.options.fontSize = fontSize;
        if (el && el.clientWidth > 0 && el.clientHeight > 0) {
            try {
                fitRef.current?.fit();
                term.refresh(0, term.rows - 1);
            } catch {
                /* ignore */
            }
        }
    }, [fontFamily, fontSize]);

    // Live-apply color scheme + cursor style from settings.
    useEffect(() => {
        const term = termRef.current;
        if (!term) return;
        term.options.theme = theme;
        term.options.cursorStyle = cursorStyle;
        try {
            term.refresh(0, term.rows - 1);
        } catch {
            /* ignore */
        }
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [scheme, cursorStyle]);

    // --- Find-in-output ----------------------------------------------------
    const runSearch = useCallback((term: string, forward: boolean) => {
        const s = searchRef.current;
        if (!s) return;
        if (!term) {
            s.clearDecorations();
            setMatches({ index: -1, count: 0 });
            return;
        }
        const opts = searchOptions(/* incremental */ false);
        if (forward) s.findNext(term, opts);
        else s.findPrevious(term, opts);
    }, []);

    const onSearchInput = useCallback((value: string) => {
        setSearchTerm(value);
        const s = searchRef.current;
        if (!s) return;
        if (!value) {
            s.clearDecorations();
            setMatches({ index: -1, count: 0 });
            return;
        }
        // Incremental: highlight + select the nearest match without jumping
        // forward on every keystroke.
        s.findNext(value, searchOptions(true));
    }, []);

    const closeSearch = useCallback(() => {
        searchRef.current?.clearDecorations();
        setSearchOpen(false);
        setMatches({ index: -1, count: 0 });
        requestAnimationFrame(() => termRef.current?.focus());
    }, []);

    // Focus (and select) the field when the box opens; clear highlights when
    // it closes (covers the magnifier toggle, not just Esc/X).
    useEffect(() => {
        if (!searchOpen) {
            searchRef.current?.clearDecorations();
            setMatches({ index: -1, count: 0 });
            return;
        }
        const id = requestAnimationFrame(() => {
            searchInputRef.current?.focus();
            searchInputRef.current?.select();
            if (searchTerm) searchRef.current?.findNext(searchTerm, searchOptions(true));
        });
        return () => cancelAnimationFrame(id);
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [searchOpen]);

    // Mirror match results into the count label.
    useEffect(() => {
        const s = searchRef.current;
        if (!s) return;
        const disp = s.onDidChangeResults((r) =>
            setMatches({ index: r.resultIndex, count: r.resultCount }),
        );
        return () => disp.dispose();
    }, [sessionKey]);

    const countLabel =
        matches.count > 0
            ? `${matches.index + 1}/${matches.count}`
            : searchTerm
              ? t("terminal.search.none")
              : "";

    return (
        <div className={styles.root} style={{ backgroundColor: theme.background }}>
            <div
                ref={containerRef}
                className={styles.term}
                // Pointer-down anywhere in the pane focuses the terminal. xterm
                // only self-focuses on clicks inside its own screen; layout
                // churn (split / focus-mode toggles, pane reparenting) can leave
                // a visible terminal without keyboard focus, and this is the
                // reliable way to take it back — click the pane to type.
                onMouseDown={() => termPool.get(sessionKey)?.term.focus()}
            />
            {searchOpen && (
                <div className={styles.search} role="search">
                    <Search size={13} className={styles.searchIcon} aria-hidden="true" />
                    <input
                        ref={searchInputRef}
                        className={styles.searchInput}
                        type="text"
                        value={searchTerm}
                        placeholder={t("terminal.search.placeholder")}
                        spellCheck={false}
                        autoComplete="off"
                        onChange={(e) => onSearchInput(e.target.value)}
                        onKeyDown={(e) => {
                            if (e.key === "Enter") {
                                e.preventDefault();
                                runSearch(searchTerm, !e.shiftKey);
                            } else if (e.key === "Escape") {
                                e.preventDefault();
                                closeSearch();
                            }
                        }}
                    />
                    <span className={styles.searchCount}>{countLabel}</span>
                    <button
                        type="button"
                        className={styles.searchBtn}
                        title={t("terminal.search.prev")}
                        aria-label={t("terminal.search.prev")}
                        disabled={!searchTerm}
                        onClick={() => runSearch(searchTerm, false)}
                    >
                        <ChevronUp size={14} />
                    </button>
                    <button
                        type="button"
                        className={styles.searchBtn}
                        title={t("terminal.search.next")}
                        aria-label={t("terminal.search.next")}
                        disabled={!searchTerm}
                        onClick={() => runSearch(searchTerm, true)}
                    >
                        <ChevronDown size={14} />
                    </button>
                    <button
                        type="button"
                        className={styles.searchBtn}
                        title={t("common.close")}
                        aria-label={t("common.close")}
                        onClick={closeSearch}
                    >
                        <X size={14} />
                    </button>
                </div>
            )}
        </div>
    );
}
