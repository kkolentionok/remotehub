import { useEffect, useRef } from "react";
import { Terminal as XTerm } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { SerializeAddon } from "@xterm/addon-serialize";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { readText, writeText } from "@tauri-apps/plugin-clipboard-manager";
import "@xterm/xterm/css/xterm.css";

import { useT } from "../../i18n";
import {
    registerSessionTerminal,
    takeSessionSnapshot,
    saveSessionSnapshot,
    useSessionsStore,
    useSettingsStore,
    useUiStore,
} from "../../store";
import { app as appApi, settings as settingsApi } from "../../lib/ipc";
import { TERMINAL_THEMES } from "../../lib/terminalThemes";
import styles from "./Terminal.module.css";

/** Terminal font-size zoom bounds (Ctrl+wheel). */
const FONT_MIN = 8;
const FONT_MAX = 32;

const MONO_FALLBACK =
    "ui-monospace, 'Cascadia Mono', 'SF Mono', Consolas, monospace";

/** Build an xterm fontFamily stack from the chosen primary family. */
function fontStack(family: string): string {
    if (!family || family === "monospace") return MONO_FALLBACK;
    return `'${family}', ${MONO_FALLBACK}`;
}

/**
 * xterm.js bound to one session tab. PTY output is fed via the store's
 * output registry (buffered until mount); keystrokes go back through
 * `sendInput`, resizes through `resize`.
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
    const { t } = useT();
    const fitRef = useRef<FitAddon | null>(null);
    const serializeRef = useRef<SerializeAddon | null>(null);
    const webglRef = useRef<WebglAddon | null>(null);
    const sendInput = useSessionsStore((s) => s.sendInput);
    const resize = useSessionsStore((s) => s.resize);
    // Backend session id arrives a tick after the tab is created. The
    // first resize (fired at mount) is dropped while it's null, and a
    // later refit of the *same* size won't re-fire onResize — so the PTY
    // would stay at its 80×24 default while xterm renders wider, garbling
    // PSReadLine redraws. Re-send the real size once the id is set.
    const sessionId = useSessionsStore(
        (s) => s.sessions.find((t) => t.key === sessionKey)?.sessionId ?? null,
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
    );
    const dialogOpen = useUiStore((s) => s.dialog.kind !== "none");
    const theme = TERMINAL_THEMES[scheme] ?? TERMINAL_THEMES.default;

    useEffect(() => {
        const el = containerRef.current;
        if (!el) return;

        const term = new XTerm({
            fontFamily: fontStack(fontFamily),
            fontSize,
            cursorBlink: true,
            cursorStyle,
            theme,
        });
        const fit = new FitAddon();
        term.loadAddon(fit);
        const serialize = new SerializeAddon();
        term.loadAddon(serialize);
        // Links: underline on hover with a "Ctrl + Click" tooltip; open in the
        // system browser on Ctrl/Cmd+click only (a plain click stays inert so
        // it never hijacks a normal click/selection in the shell).
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
                k2.textContent = t("terminal.clickWord");
                const txt = document.createElement("span");
                txt.textContent = t("terminal.openLinkHint");
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
        const openLink = (uri: string) => {
            appApi.open(uri).catch((err) => console.error("open link failed:", err));
        };
        const links = new WebLinksAddon(
            (event, uri) => {
                if (event.ctrlKey || event.metaKey) openLink(uri);
            },
            {
                hover: (event) => showLinkTip(event as MouseEvent),
                leave: hideLinkTip,
            },
        );
        term.loadAddon(links);
        term.open(el);

        // GPU renderer for snappy, low-latency drawing (xterm's default
        // DOM renderer feels laggy under per-keystroke echo). The WebGL
        // context can be lost (window minimized, GPU reset, too many live
        // contexts) — when that happens we recreate it so we don't get
        // stuck on the slow DOM renderer. If a fresh context dies almost
        // immediately, WebGL is unstable here, so we stop and stay on DOM.
        const loadWebgl = () => {
            try {
                const addon = new WebglAddon();
                webglRef.current = addon;
                const createdAt = Date.now();
                addon.onContextLoss(() => {
                    addon.dispose();
                    if (webglRef.current === addon) webglRef.current = null;
                    if (Date.now() - createdAt > 1000) {
                        setTimeout(loadWebgl, 200);
                    }
                });
                term.loadAddon(addon);
            } catch {
                /* no WebGL — xterm keeps the DOM renderer */
            }
        };
        loadWebgl();

        termRef.current = term;
        fitRef.current = fit;
        serializeRef.current = serialize;
        try {
            fit.fit();
        } catch {
            /* element not laid out yet — resize observer below recovers */
        }

        // Restore the buffer captured on a prior unmount (split/move), so
        // scrollback survives pane remounts. Live output since then is
        // replayed right after by registerSessionTerminal.
        const snapshot = takeSessionSnapshot(sessionKey);
        if (snapshot) term.write(snapshot);

        const unregister = registerSessionTerminal(sessionKey, (data) =>
            term.write(data),
        );
        const dataDisp = term.onData((d) =>
            sendInput(sessionKey, new TextEncoder().encode(d)),
        );
        // The PTY resize (window-change) must stay in lockstep with xterm's
        // own buffer reflow: if the shell learns the new size later than
        // xterm reflows, readline redraws the prompt against a stale layout
        // and leaves ghost prompt lines. So send it synchronously on every
        // xterm resize. Storms are prevented upstream — the *fit* is
        // debounced (scheduleFit) and the store dedups identical sizes — so
        // onResize fires at most once per settled size.
        const resizeDisp = term.onResize(({ cols, rows }) =>
            resize(sessionKey, cols, rows),
        );
        resize(sessionKey, term.cols, term.rows);

        // Refit when the pane settles. During a divider drag the box
        // changes continuously; fitting on every frame would fire a
        // window-change per step and make the shell redraw its prompt
        // repeatedly (stacked/blank lines). Debounce so the PTY is
        // resized once movement stops.
        let fitTimer = 0;
        const scheduleFit = () => {
            clearTimeout(fitTimer);
            fitTimer = window.setTimeout(() => {
                // While the pane is display:none its box is 0×0 — fitting
                // then would shrink the terminal and reflow into garbage.
                if (el.clientWidth === 0 || el.clientHeight === 0) return;
                try {
                    fit.fit(); // triggers onResize → PTY resize, in sync
                    term.refresh(0, term.rows - 1);
                } catch {
                    /* ignore transient layout */
                }
            }, 140);
        };
        const ro = new ResizeObserver(scheduleFit);
        ro.observe(el);

        // Copy-on-select (Termius-style): write the selection to the
        // clipboard when the mouse is released.
        // Copy-on-select (Termius-style). xterm commits the selection on
        // its own document-level mouseup, so we must run *after* that:
        // mark that a drag started in THIS terminal (mousedown on its el),
        // then on the document mouseup defer a tick (let xterm finalize)
        // and write the selection via the Tauri clipboard plugin (works
        // without a user-gesture, unlike execCommand).
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

        // Right-click pastes the clipboard (terminal convention) instead
        // of the WebView's native context menu.
        const onContextMenu = (e: MouseEvent) => {
            e.preventDefault();
            void readText()
                .then((text) => {
                    if (text) term.paste(text);
                })
                .catch(() => {});
        };
        el.addEventListener("contextmenu", onContextMenu);

        // Ctrl + mouse wheel zooms the terminal font. We drive the global
        // `terminal_font_size` setting so every terminal stays in step and the
        // size persists; apply locally at once and debounce the backend write
        // so a scroll burst doesn't storm settings_update / reloads. A plain
        // wheel (no Ctrl) is left to xterm for scrollback.
        let zoomTimer = 0;
        const onWheel = (e: WheelEvent) => {
            if (!e.ctrlKey) return;
            e.preventDefault();
            const s = useSettingsStore.getState().settings;
            if (!s) return;
            const cur = s.terminal_font_size ?? 13;
            const next = Math.min(
                FONT_MAX,
                Math.max(FONT_MIN, cur + (e.deltaY < 0 ? 1 : -1)),
            );
            if (next === cur) return;
            useSettingsStore.setState({ settings: { ...s, terminal_font_size: next } });
            clearTimeout(zoomTimer);
            zoomTimer = window.setTimeout(() => {
                void settingsApi.update({ patches: { terminal_font_size: next } });
            }, 500);
        };
        el.addEventListener("wheel", onWheel, { passive: false });

        return () => {
            clearTimeout(fitTimer);
            clearTimeout(zoomTimer);
            ro.disconnect();
            el.removeEventListener("mousedown", onMouseDown);
            document.removeEventListener("mouseup", onDocMouseUp);
            el.removeEventListener("contextmenu", onContextMenu);
            el.removeEventListener("wheel", onWheel);
            if (linkTip) linkTip.remove();
            unregister();
            dataDisp.dispose();
            resizeDisp.dispose();
            try {
                saveSessionSnapshot(sessionKey, serialize.serialize());
            } catch {
                /* serialize can fail on a torn-down term — skip */
            }
            term.dispose();
            termRef.current = null;
            fitRef.current = null;
            serializeRef.current = null;
            webglRef.current = null;
        };
        // sendInput/resize are stable zustand actions.
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [sessionKey]);

    // Once the backend session id is set, push the current terminal size
    // so the PTY/remote stops assuming the 80×24 default it opened with.
    useEffect(() => {
        if (!sessionId) return;
        const term = termRef.current;
        if (term) resize(sessionKey, term.cols, term.rows);
        // resize is a stable zustand action.
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [sessionId, sessionKey]);

    // When this pane becomes visible (its tab activated), it goes from
    // display:none to real size — refit, repaint, and focus if it's the
    // focused pane.
    useEffect(() => {
        if (!visible) return;
        const id = requestAnimationFrame(() => {
            const term = termRef.current;
            const el = containerRef.current;
            if (!term || !el || el.clientWidth === 0) return;
            try {
                fitRef.current?.fit();
                term.refresh(0, term.rows - 1);
            } catch {
                /* ignore */
            }
            if (focused) term.focus();
        });
        return () => cancelAnimationFrame(id);
    }, [visible, focused]);

    // A dialog (Settings, confirms…) renders in a portal and steals
    // keyboard focus. When it closes, the active terminal must take focus
    // back — otherwise keystrokes go nowhere until the user clicks it.
    useEffect(() => {
        if (!visible || !focused || dialogOpen) return;
        const id = requestAnimationFrame(() => termRef.current?.focus());
        return () => cancelAnimationFrame(id);
    }, [dialogOpen, visible, focused]);

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

    return (
        <div
            ref={containerRef}
            className={styles.term}
            style={{ backgroundColor: theme.background }}
        />
    );
}
