import { useEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { emit } from "@tauri-apps/api/event";

import { useSessionsStore, useSettingsStore } from "../../store";
import { rdpSession as rdpSessionApi } from "../../lib/ipc";
import { RdpViewport } from "./RdpViewport";

/**
 * Standalone window that hosts a single popped-out RDP session.
 *
 * Rendered by `App` when the URL hash starts with `#popout`. The session
 * already lives in the backend (it was opened in the main window); here we
 * re-home its frame stream to this webview via `attachExternalRdp` (which
 * reattaches + forces a full repaint) and draw it edge-to-edge. Window
 * decorations stay native (minimise / maximise / close); our own bar only
 * adds the title + fullscreen affordance.
 */
export function RdpPopoutApp() {
    const params = new URLSearchParams(window.location.hash.replace(/^#popout\??/, ""));
    const sid = params.get("sid") ?? "";
    const title = params.get("t") ?? "RDP";
    const w = Number(params.get("w")) || 1280;
    const h = Number(params.get("h")) || 800;

    const attachExternalRdp = useSessionsStore((s) => s.attachExternalRdp);
    const gfxOn = useSettingsStore((s) => s.settings?.rdp_gfx ?? false);
    const [key, setKey] = useState<string | null>(null);
    const didAttach = useRef(false);

    useEffect(() => {
        if (didAttach.current || !sid) return;
        didAttach.current = true;
        // Pop-out is its own webview/JS context — load settings so the GFX
        // gate (enableDynamicResize) reflects the user's choice here too.
        void useSettingsStore.getState().load();
        setKey(attachExternalRdp({ sessionId: sid, title, width: w, height: h }));
        // Tell the main window when this window is closing so it can drop the
        // tab (unless the close was a re-dock the main window initiated).
        const unlisten = getCurrentWindow().onCloseRequested(() => {
            void emit("rdp:popout-closed", { sid });
        });
        document.title = title;
        return () => {
            void unlisten.then((f) => f());
        };
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, []);

    // Live tab (dims update from the server's `resized` events).
    const tab = useSessionsStore((s) => s.sessions.find((t) => t.key === key));

    if (!key || !tab) {
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
        <div style={{ width: "100vw", height: "100vh", background: "#000", overflow: "hidden" }}>
            <RdpViewport
                sessionKey={key}
                width={tab.rdpWidth ?? w}
                height={tab.rdpHeight ?? h}
                hostLabel={title}
                connected
                onInput={(ev) => {
                    if (sid) void rdpSessionApi.sendInput({ session_id: sid, event: ev });
                }}
                onLocalClipboard={(text) => {
                    if (sid) void rdpSessionApi.setClipboard(sid, text);
                }}
                onLocalClipboardImage={(cw, ch, rgbaBase64) => {
                    if (sid) void rdpSessionApi.setClipboardImage(sid, cw, ch, rgbaBase64);
                }}
                onResize={(rw, rh) => {
                    if (sid) void rdpSessionApi.resize(sid, rw, rh);
                }}
                enableDynamicResize={gfxOn}
                onKbdCapture={(on) => {
                    if (sid) void rdpSessionApi.kbdCapture(sid, on);
                }}
            />
        </div>
    );
}
