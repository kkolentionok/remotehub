import { useEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { emit } from "@tauri-apps/api/event";
import { PictureInPicture2 } from "lucide-react";

import { useT } from "../../i18n";
import {
    openSessionSearch,
    useSessionsStore,
    useSettingsStore,
} from "../../store";
import { Terminal } from "./Terminal";

/**
 * Standalone window that hosts a single popped-out terminal (SSH or local)
 * session. Rendered by `App` when the URL hash starts with `#popout-term`.
 *
 * The session already lives in the backend (it was opened in the main
 * window); here we re-home its byte stream to this webview via
 * `attachExternalTerm` (reattach → the backend replays scrollback) and render
 * a `<Terminal>` edge-to-edge. Closing the window (native X) and the "return
 * to tab" button do the SAME thing: ask the main window to re-dock — they
 * never end the session (that's done by closing the tab in the main window).
 */
export function TermPopoutApp() {
    const { t } = useT();
    const params = new URLSearchParams(
        window.location.hash.replace(/^#popout-term\??/, ""),
    );
    const sid = params.get("sid") ?? "";
    const title = params.get("t") ?? "Terminal";
    const local = params.get("local") === "1";

    const attachExternalTerm = useSessionsStore((s) => s.attachExternalTerm);
    const [key, setKey] = useState<string | null>(null);
    const [shown, setShown] = useState(false);
    const didAttach = useRef(false);
    // Shared so a redock is requested exactly once: the native-X path and the
    // button both go through here, and once a redock is in flight the close
    // handler stops preventing the close (so the main window's destroy lands).
    const redocking = useRef(false);

    const requestRedock = () => {
        if (redocking.current) return;
        redocking.current = true;
        void emit("term:request-redock", { sid });
    };

    // One-time: bind to the live backend session + reveal the window.
    useEffect(() => {
        if (didAttach.current || !sid) return;
        didAttach.current = true;
        // Pop-out is its own webview/JS context — load settings so the
        // terminal colour scheme / theme reflect the user's choice here too.
        void useSettingsStore.getState().load();
        setKey(attachExternalTerm({ sessionId: sid, title, local }));
        document.title = title;
        // The window was created hidden (dark surface). Reveal it only after
        // the terminal has had a couple of frames to paint, then fade the
        // content in — no white flash, no abrupt pop.
        const win = getCurrentWindow();
        requestAnimationFrame(() =>
            requestAnimationFrame(() => {
                void (async () => {
                    try {
                        await win.show();
                        await win.setFocus();
                    } catch {
                        /* already visible */
                    }
                    setShown(true);
                })();
            }),
        );
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, []);

    // Native window X (or OS close) → return the session to its tab rather
    // than ending it. Registered in its own effect (NOT gated by the
    // attach-once ref) so it survives StrictMode's mount→cleanup→mount and is
    // always live when the user actually closes the window.
    useEffect(() => {
        const p = getCurrentWindow().onCloseRequested((e) => {
            if (redocking.current) return; // let the main window's destroy land
            e.preventDefault();
            requestRedock();
        });
        return () => {
            void p.then((f) => f());
        };
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, []);

    // Ctrl/Cmd+F → find-in-output. The main window wires this in AppShell; the
    // pop-out has no AppShell, so wire it here for this single terminal.
    useEffect(() => {
        if (!key) return;
        const onKey = (e: KeyboardEvent) => {
            if (!(e.ctrlKey || e.metaKey) || e.altKey || e.shiftKey) return;
            if (e.code !== "KeyF") return;
            e.preventDefault();
            e.stopImmediatePropagation();
            openSessionSearch(key);
        };
        window.addEventListener("keydown", onKey, { capture: true });
        return () => window.removeEventListener("keydown", onKey, { capture: true });
    }, [key]);

    const iconBtn: React.CSSProperties = {
        display: "grid",
        placeItems: "center",
        width: 24,
        height: 22,
        border: "none",
        background: "transparent",
        color: "var(--text-2)",
        cursor: "pointer",
        borderRadius: "var(--radius-sm)",
    };

    if (!key) {
        return (
            <div
                style={{
                    width: "100vw",
                    height: "100vh",
                    background: "var(--color-canvas)",
                    color: "var(--text-3)",
                    display: "grid",
                    placeItems: "center",
                    fontSize: 13,
                }}
            >
                {title}
            </div>
        );
    }

    return (
        <div
            style={{
                display: "flex",
                flexDirection: "column",
                width: "100vw",
                height: "100vh",
                background: "var(--color-canvas)",
                opacity: shown ? 1 : 0,
                transition: "opacity 160ms ease-out",
            }}
        >
            <div
                style={{
                    flex: "none",
                    display: "flex",
                    alignItems: "center",
                    gap: "var(--space-2)",
                    height: 30,
                    padding: "0 var(--space-2)",
                    borderBottom: "1px solid var(--color-border)",
                    userSelect: "none",
                }}
            >
                <span
                    style={{
                        flex: 1,
                        minWidth: 0,
                        fontSize: "var(--text-xs)",
                        color: "var(--text-2)",
                        overflow: "hidden",
                        textOverflow: "ellipsis",
                        whiteSpace: "nowrap",
                    }}
                >
                    {title}
                </span>
                <button
                    type="button"
                    title={t("session.redock")}
                    style={iconBtn}
                    onClick={requestRedock}
                >
                    <PictureInPicture2 size={14} />
                </button>
            </div>
            <div style={{ flex: 1, minHeight: 0, position: "relative" }}>
                <Terminal sessionKey={key} visible focused />
            </div>
        </div>
    );
}
