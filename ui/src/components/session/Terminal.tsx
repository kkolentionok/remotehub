import { useEffect, useRef } from "react";
import { Terminal as XTerm } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { readText, writeText } from "@tauri-apps/plugin-clipboard-manager";
import "@xterm/xterm/css/xterm.css";

import { registerSessionTerminal, useSessionsStore, useSettingsStore } from "../../store";
import { TERMINAL_THEMES } from "../../lib/terminalThemes";
import styles from "./Terminal.module.css";

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
    active,
}: {
    sessionKey: string;
    active: boolean;
}) {
    const containerRef = useRef<HTMLDivElement>(null);
    const termRef = useRef<XTerm | null>(null);
    const fitRef = useRef<FitAddon | null>(null);
    const sendInput = useSessionsStore((s) => s.sendInput);
    const resize = useSessionsStore((s) => s.resize);
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
                const createdAt = Date.now();
                addon.onContextLoss(() => {
                    addon.dispose();
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
        try {
            fit.fit();
        } catch {
            /* element not laid out yet — resize observer below recovers */
        }

        const unregister = registerSessionTerminal(sessionKey, (data) =>
            term.write(data),
        );
        const dataDisp = term.onData((d) =>
            sendInput(sessionKey, new TextEncoder().encode(d)),
        );
        const resizeDisp = term.onResize(({ cols, rows }) =>
            resize(sessionKey, cols, rows),
        );
        resize(sessionKey, term.cols, term.rows);

        const ro = new ResizeObserver(() => {
            // While the pane is display:none its box is 0×0 — fitting then
            // would shrink the terminal to a tiny width and reflow the
            // buffer into garbage. Only fit when actually visible.
            if (el.clientWidth === 0 || el.clientHeight === 0) return;
            try {
                fit.fit();
                term.refresh(0, term.rows - 1);
            } catch {
                /* ignore transient layout */
            }
        });
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

        return () => {
            ro.disconnect();
            el.removeEventListener("mousedown", onMouseDown);
            document.removeEventListener("mouseup", onDocMouseUp);
            el.removeEventListener("contextmenu", onContextMenu);
            unregister();
            dataDisp.dispose();
            resizeDisp.dispose();
            term.dispose();
            termRef.current = null;
            fitRef.current = null;
        };
        // sendInput/resize are stable zustand actions.
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [sessionKey]);

    // When this tab becomes active, the pane goes from display:none to
    // visible — refit to the now-real size, force a full repaint to clear
    // any stale frame, and grab focus.
    useEffect(() => {
        if (!active) return;
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
            term.focus();
        });
        return () => cancelAnimationFrame(id);
    }, [active]);

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
