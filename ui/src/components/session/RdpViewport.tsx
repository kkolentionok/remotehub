import { useCallback, useEffect, useRef, useState } from "react";
import { Maximize2, Minimize2 } from "lucide-react";

import { registerSessionViewport } from "../../store";
import { useT } from "../../i18n";
import type { RdpInputEvent, RdpMouseButton, RdpSessionEvent } from "../../lib/types";
import styles from "./RdpViewport.module.css";

interface Props {
    /** Session key — used to receive server frames routed by the store. */
    sessionKey: string;
    /** Backing resolution (negotiated with the server). */
    width: number;
    height: number;
    /** Every input event the viewport produces is handed up here. */
    onInput: (ev: RdpInputEvent) => void;
    className?: string;
}

const BTN: Record<number, RdpMouseButton> = { 0: "left", 1: "middle", 2: "right" };

/**
 * RDP rendering + input surface. The store routes server `RdpSessionEvent`s
 * here (registered by `sessionKey`); user input goes up via `onInput`.
 *
 * Focus / modifier sync (spec requirement): the canvas is focusable. On
 * blur we emit `release_all_modifiers`; on focus we emit `sync_modifiers`
 * with the last-known physical modifier state (tracked from every mouse
 * and key event via `getModifierState`). Fix for the classic "stuck
 * Ctrl/Alt/Shift after Alt-Tab" RDP bug. (Input wire lands in round 2b-2.)
 */
export function RdpViewport({ sessionKey, width, height, onInput, className }: Props) {
    const { t } = useT();
    const canvasRef = useRef<HTMLCanvasElement | null>(null);
    const ctxRef = useRef<CanvasRenderingContext2D | null>(null);
    const wrapRef = useRef<HTMLDivElement | null>(null);
    const [isFs, setIsFs] = useState(false);
    const [showHint, setShowHint] = useState(false);
    // Serializes region blits in arrival order. Regions decode in parallel,
    // but draw strictly in sequence so a slower-decoding region from an
    // older frame can't land on top of a newer one (the drag tearing).
    const drawSeq = useRef<Promise<void>>(Promise.resolve());

    // Last-known physical modifier state, refreshed on any input event.
    const mods = useRef({
        ctrl: false,
        alt: false,
        shift: false,
        meta: false,
        caps_lock: false,
        num_lock: false,
        scroll_lock: false,
    });

    useEffect(() => {
        const c = canvasRef.current;
        if (!c) return;
        const ctx = c.getContext("2d", { alpha: false });
        ctxRef.current = ctx;
        if (ctx) {
            ctx.fillStyle = "#000";
            ctx.fillRect(0, 0, width, height);
        }
    }, [width, height]);

    // Track fullscreen state (Esc/other exits fire fullscreenchange too).
    useEffect(() => {
        const onChange = () => {
            const on = document.fullscreenElement === wrapRef.current;
            setIsFs(on);
            if (on) {
                setShowHint(true);
                window.setTimeout(() => setShowHint(false), 2600);
            }
        };
        document.addEventListener("fullscreenchange", onChange);
        return () => document.removeEventListener("fullscreenchange", onChange);
    }, []);

    const toggleFs = useCallback(() => {
        if (document.fullscreenElement) {
            void document.exitFullscreen().catch(() => {});
        } else {
            void wrapRef.current?.requestFullscreen().catch(() => {});
        }
        // Keep input focus on the canvas after toggling.
        window.setTimeout(() => canvasRef.current?.focus(), 0);
    }, []);

    // Draw a decoded framebuffer region. Stable identity (reads refs only)
    // so the store registration runs once per session.
    const applyEvent = useCallback((ev: RdpSessionEvent) => {
        const ctx = ctxRef.current;
        if (!ctx) return;

        // A whole frame's tiles arrive together. Decode them all in parallel,
        // then draw them in a single synchronous pass so the frame appears
        // coherently — the compositor never samples a half-updated canvas, so
        // fast motion (window drags) no longer tears. Batches are chained to
        // keep frame order.
        if (ev.kind === "frame_batch") {
            const tiles = ev.tiles;
            const decoded = Promise.all(
                tiles.map((t) => {
                    const img = new Image();
                    img.src = `data:image/${t.format};base64,${t.base64}`;
                    return img
                        .decode()
                        .then(() => ({ img, x: t.x, y: t.y }))
                        .catch(() => null);
                }),
            );
            drawSeq.current = drawSeq.current.then(async () => {
                const items = await decoded;
                const c = ctxRef.current;
                if (!c) return;
                for (const it of items) {
                    if (it) c.drawImage(it.img, it.x, it.y);
                }
            });
            return;
        }

        if (ev.kind !== "frame") return;
        const { region, format, data } = ev;
        const len = region.width * region.height * 4;
        const src = data instanceof Uint8Array ? data : new Uint8Array(data);
        const out = new Uint8ClampedArray(len);
        if (format === "rgba8") {
            out.set(src.subarray(0, len));
        } else {
            // BGRA8 → RGBA8 swap so ImageData renders correctly.
            for (let i = 0; i + 3 < src.length && i + 3 < len; i += 4) {
                out[i] = src[i + 2]!;
                out[i + 1] = src[i + 1]!;
                out[i + 2] = src[i]!;
                out[i + 3] = src[i + 3]!;
            }
        }
        const img = new ImageData(out, region.width, region.height);
        ctx.putImageData(img, region.x, region.y);
    }, []);

    // Subscribe to server frames for this session.
    useEffect(() => registerSessionViewport(sessionKey, applyEvent), [sessionKey, applyEvent]);

    // --- coordinate mapping: display (CSS) px → backing px ---
    const toCanvas = (clientX: number, clientY: number): { x: number; y: number } => {
        const c = canvasRef.current;
        if (!c) return { x: 0, y: 0 };
        const r = c.getBoundingClientRect();
        if (r.width <= 0 || r.height <= 0) return { x: 0, y: 0 };
        // The canvas is displayed with `object-fit: contain`: the backing
        // bitmap (width×height) is scaled uniformly to fit inside the element
        // box and centered, leaving letterbox margins when aspect ratios
        // differ. Map relative to that fitted rectangle, not the raw element
        // box — otherwise the pointer lands offset (the bug where the cursor
        // on the left highlighted an icon on the right).
        const scale = Math.min(r.width / width, r.height / height);
        const offX = (r.width - width * scale) / 2;
        const offY = (r.height - height * scale) / 2;
        const x = Math.max(
            0,
            Math.min(width - 1, Math.round((clientX - r.left - offX) / scale)),
        );
        const y = Math.max(
            0,
            Math.min(height - 1, Math.round((clientY - r.top - offY) / scale)),
        );
        return { x, y };
    };

    const refreshMods = (
        e:
            | React.KeyboardEvent<HTMLCanvasElement>
            | React.MouseEvent<HTMLCanvasElement>,
    ): void => {
        mods.current = {
            ctrl: e.getModifierState("Control"),
            alt: e.getModifierState("Alt"),
            shift: e.getModifierState("Shift"),
            meta: e.getModifierState("Meta"),
            caps_lock: e.getModifierState("CapsLock"),
            num_lock: e.getModifierState("NumLock"),
            scroll_lock: e.getModifierState("ScrollLock"),
        };
    };

    const lastMoveSent = useRef(0);
    const onMouseMove = useCallback(
        (e: React.MouseEvent<HTMLCanvasElement>) => {
            refreshMods(e);
            // Throttle moves: a flood of micro-moves otherwise backs up the
            // input pipeline (each is one IPC call). Clicks carry their own
            // coordinates, so dropping intermediate moves is safe.
            const now = performance.now();
            if (now - lastMoveSent.current < 40) return;
            lastMoveSent.current = now;
            const { x, y } = toCanvas(e.clientX, e.clientY);
            onInput({ kind: "mouse_move", x, y });
        },
        [onInput, width, height],
    );

    const onMouseButton = useCallback(
        (e: React.MouseEvent<HTMLCanvasElement>, pressed: boolean) => {
            refreshMods(e);
            const button = BTN[e.button];
            if (!button) return;
            const { x, y } = toCanvas(e.clientX, e.clientY);
            onInput({ kind: "mouse_button", button, pressed, x, y });
        },
        [onInput, width, height],
    );

    const onWheel = useCallback(
        (e: React.WheelEvent<HTMLCanvasElement>) => {
            const { x, y } = toCanvas(e.clientX, e.clientY);
            // Browser deltaY: positive = scroll down. RDP wheel: positive = up.
            const delta = Math.max(-32768, Math.min(32767, Math.round(-e.deltaY)));
            onInput({ kind: "mouse_wheel", delta, x, y });
        },
        [onInput, width, height],
    );

    const onKey = useCallback(
        (e: React.KeyboardEvent<HTMLCanvasElement>, pressed: boolean) => {
            // Reserved client shortcut: Ctrl+Alt+Enter toggles fullscreen.
            // Intercepted here so it never reaches the remote (now or once
            // keyboard forwarding lands).
            if (pressed && e.ctrlKey && e.altKey && e.code === "Enter") {
                e.preventDefault();
                toggleFs();
                return;
            }
            refreshMods(e);
            // Keep focus-stealing browser shortcuts (Tab, /, etc.) out of
            // the way — the remote desktop wants the raw keys.
            e.preventDefault();
            onInput({ kind: "key", code: e.code, pressed, repeat: e.repeat });
        },
        [onInput, toggleFs],
    );

    const onFocus = useCallback(() => {
        onInput({ kind: "sync_modifiers", ...mods.current });
    }, [onInput]);

    const onBlur = useCallback(() => {
        onInput({ kind: "release_all_modifiers" });
    }, [onInput]);

    return (
        <div ref={wrapRef} className={styles.wrap}>
            <canvas
                ref={canvasRef}
                width={width}
                height={height}
                tabIndex={0}
                className={`${styles.canvas}${className ? ` ${className}` : ""}`}
                onMouseMove={onMouseMove}
                onMouseDown={(e) => onMouseButton(e, true)}
                onMouseUp={(e) => onMouseButton(e, false)}
                onWheel={onWheel}
                onContextMenu={(e) => e.preventDefault()}
                onKeyDown={(e) => onKey(e, true)}
                onKeyUp={(e) => onKey(e, false)}
                onFocus={onFocus}
                onBlur={onBlur}
            />
            <button
                type="button"
                className={styles.fsBtn}
                title={t("session.fullscreen")}
                aria-label={t("session.fullscreen")}
                onClick={toggleFs}
            >
                {isFs ? <Minimize2 size={15} /> : <Maximize2 size={15} />}
            </button>
            {showHint && <div className={styles.fsHint}>{t("session.fullscreenHint")}</div>}
        </div>
    );
}
