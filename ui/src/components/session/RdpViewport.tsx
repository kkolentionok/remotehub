import { useCallback, useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { Maximize2, Minimize2, Minus, PictureInPicture2, X } from "lucide-react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import { readText as clipboardReadText, readImage as clipboardReadImage } from "@tauri-apps/plugin-clipboard-manager";

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
    /** Host label shown in the connection bar (mstsc-style). */
    hostLabel?: string;
    /** True once the session is connected (state "ready") — briefly auto-shows
     *  the connection bar so the fullscreen affordance is discoverable. */
    connected?: boolean;
    /** Push the local OS clipboard text up (for paste into the remote). */
    onLocalClipboard?: (text: string) => void;
    /** Push a local OS clipboard image up (raw RGBA base64) for remote paste. */
    onLocalClipboardImage?: (width: number, height: number, rgbaBase64: string) => void;
    /** Viewport size changed (device px) → request a DisplayControl resize. */
    onResize?: (width: number, height: number) => void;
    /** Enable the continuous window-resize → DisplayControl reflow. Off by
     *  default; the parent turns it on for GFX sessions (the legacy RemoteFX
     *  reactivation degrades repaint rate — IronRDP #447 — so it stays off
     *  there). The fullscreen one-shot resize is independent and always on. */
    enableDynamicResize?: boolean;
    /** Toggle OS-level keyboard capture (true on fullscreen, false on exit). */
    onKbdCapture?: (on: boolean) => void;
    /** Detach this session into its own OS window (shown as a bar button).
     *  Omitted inside the pop-out window itself. */
    onPopOut?: () => void;
    /** Minimise the host OS window (shown as a bar button). Passed only by the
     *  pop-out window host. */
    onMinimize?: () => void;
    /** Return this session to a tab in the main app window (re-dock). Passed
     *  only by the pop-out window host; rendered as a close affordance. */
    onPopIn?: () => void;
    className?: string;
}

const BTN: Record<number, RdpMouseButton> = { 0: "left", 1: "middle", 2: "right" };

/** Uint8Array → base64 (chunked to avoid call-stack limits on big buffers). */
function bytesToBase64(bytes: Uint8Array): string {
    let bin = "";
    const chunk = 0x8000;
    for (let i = 0; i < bytes.length; i += chunk) {
        bin += String.fromCharCode(...bytes.subarray(i, i + chunk));
    }
    return btoa(bin);
}

/** Turn a server cursor bitmap (non-premultiplied RGBA, base64) into a CSS
 *  `cursor` value with the correct hotspot. Returned cursors track the local
 *  mouse natively, so the remote pointer feels instant. */
function pointerToCss(ev: {
    width: number;
    height: number;
    hotspot_x: number;
    hotspot_y: number;
    rgba_base64: string;
}): string {
    if (ev.width === 0 || ev.height === 0) return "none";
    try {
        const bin = atob(ev.rgba_base64);
        const len = ev.width * ev.height * 4;
        const bytes = new Uint8ClampedArray(len);
        for (let i = 0; i < len && i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
        const off = document.createElement("canvas");
        off.width = ev.width;
        off.height = ev.height;
        const octx = off.getContext("2d");
        if (!octx) return "default";
        octx.putImageData(new ImageData(bytes, ev.width, ev.height), 0, 0);
        // Chromium ignores cursor images > 128px; large pointers fall back to
        // the default arrow, which is acceptable.
        return `url("${off.toDataURL("image/png")}") ${ev.hotspot_x} ${ev.hotspot_y}, auto`;
    } catch {
        return "default";
    }
}

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
export function RdpViewport({ sessionKey, width, height, onInput, hostLabel, connected, onLocalClipboard, onLocalClipboardImage, onResize, enableDynamicResize, onKbdCapture, onPopOut, onMinimize, onPopIn, className }: Props) {
    const { t } = useT();
    const canvasRef = useRef<HTMLCanvasElement | null>(null);
    const ctxRef = useRef<CanvasRenderingContext2D | null>(null);
    const wrapRef = useRef<HTMLDivElement | null>(null);
    const [isFs, setIsFs] = useState(false);
    const [showHint, setShowHint] = useState(false);
    // mstsc-style auto-hide: the bar shows only when the pointer is near the
    // top edge of the viewport, and hides as soon as it leaves.
    const [barOn, setBarOn] = useState(false);
    // Floating bar ("shield") placement: the user can drag it by the host name
    // to any corner/edge. `barAnchor` is the snapped resting spot; `barDrag`
    // holds live pixel coords (relative to the wrap) only while dragging.
    const [barAnchor, setBarAnchor] = useState<"tl" | "tc" | "tr" | "bl" | "bc" | "br">("tc");
    const [barDrag, setBarDrag] = useState<{ x: number; y: number } | null>(null);
    const barRef = useRef<HTMLDivElement | null>(null);
    const dragOff = useRef<{ dx: number; dy: number } | null>(null);

    const onBarPointerMove = useCallback((e: PointerEvent) => {
        const wrap = wrapRef.current;
        const off = dragOff.current;
        if (!wrap || !off) return;
        const wr = wrap.getBoundingClientRect();
        const bar = barRef.current;
        const bw = bar?.offsetWidth ?? 0;
        const bh = bar?.offsetHeight ?? 0;
        const x = Math.max(0, Math.min(e.clientX - wr.left - off.dx, wr.width - bw));
        const y = Math.max(0, Math.min(e.clientY - wr.top - off.dy, wr.height - bh));
        setBarDrag({ x, y });
    }, []);

    const onBarPointerUp = useCallback(() => {
        window.removeEventListener("pointermove", onBarPointerMove);
        const wrap = wrapRef.current;
        const bar = barRef.current;
        if (wrap && bar) {
            const wr = wrap.getBoundingClientRect();
            const br = bar.getBoundingClientRect();
            const cx = br.left - wr.left + br.width / 2;
            const cy = br.top - wr.top + br.height / 2;
            // Snap to the nearest perimeter anchor: 3 columns × 2 rows.
            const h = cx < wr.width * 0.33 ? "l" : cx > wr.width * 0.67 ? "r" : "c";
            const v = cy < wr.height * 0.5 ? "t" : "b";
            setBarAnchor(`${v}${h}` as "tl" | "tc" | "tr" | "bl" | "bc" | "br");
        }
        dragOff.current = null;
        setBarDrag(null);
    }, [onBarPointerMove]);

    const onBarPointerDown = useCallback(
        (e: ReactPointerEvent<HTMLSpanElement>) => {
            if (e.button !== 0) return;
            const wrap = wrapRef.current;
            const bar = barRef.current;
            if (!wrap || !bar) return;
            e.preventDefault();
            const wr = wrap.getBoundingClientRect();
            const br = bar.getBoundingClientRect();
            dragOff.current = { dx: e.clientX - br.left, dy: e.clientY - br.top };
            setBarDrag({ x: br.left - wr.left, y: br.top - wr.top });
            window.addEventListener("pointermove", onBarPointerMove);
            window.addEventListener("pointerup", onBarPointerUp, { once: true });
        },
        [onBarPointerMove, onBarPointerUp],
    );

    // Drop the window-level drag listeners if we unmount mid-drag.
    useEffect(
        () => () => {
            window.removeEventListener("pointermove", onBarPointerMove);
            window.removeEventListener("pointerup", onBarPointerUp);
        },
        [onBarPointerMove, onBarPointerUp],
    );
    // Serializes region blits in arrival order. Regions decode in parallel,
    // but draw strictly in sequence so a slower-decoding region from an
    // older frame can't land on top of a newer one (the drag tearing).
    const drawSeq = useRef<Promise<void>>(Promise.resolve());
    // Size key (`WxH`) of the last clipboard image pushed to the remote, so
    // refocusing doesn't re-transfer the same image. Cleared when no image.
    const lastImgKey = useRef<string>("");

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

    // Latest onResize, read from event handlers without re-subscribing.
    const onResizeRef = useRef(onResize);
    onResizeRef.current = onResize;

    // Briefly reveal the connection bar for a few seconds once connected, so
    // the fullscreen / controls are discoverable, then auto-hide.
    useEffect(() => {
        if (!connected) return;
        setBarOn(true);
        const tmr = window.setTimeout(() => setBarOn(false), 3200);
        return () => window.clearTimeout(tmr);
    }, [connected]);

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
            // Keyboard Lock: in fullscreen, capture system shortcuts (Alt+Tab,
            // Win, etc.) so they reach the remote desktop instead of the local
            // OS. Windowed mode can't — Windows grabs Alt+Tab first. Exit
            // fullscreen with the button or Ctrl+Alt+Enter (Esc now goes to
            // the remote).
            const kb = (
                navigator as unknown as {
                    keyboard?: { lock?: (keys?: string[]) => Promise<void>; unlock?: () => void };
                }
            ).keyboard;
            if (on) {
                void kb?.lock?.().catch(() => {});
                setShowHint(true);
                window.setTimeout(() => setShowHint(false), 2600);
            } else {
                kb?.unlock?.();
            }
            // OS-level key capture: while fullscreen, a low-level Windows hook
            // routes system keys (Win, Alt+Tab, …) to the remote instead of
            // the local OS. On exit, release any modifiers the remote may
            // still think are held from hook-forwarded presses.
            onKbdCapture?.(on);
            if (!on) onInput({ kind: "release_all_modifiers" });
            // Fill the new surface without letterbox bars: the windowed pane
            // aspect ≠ the monitor's, so on the fullscreen transition we ask
            // the server (one-shot DisplayControl resize) to match — monitor
            // size entering, the pane size leaving. Deferred a frame so the
            // element box has settled to its new size.
            requestAnimationFrame(() => {
                // Physical pixels (CSS × DPR) — native-DPI rendering, no
                // upscale blur on scaled displays (see store.createSession).
                const dpr = Math.min(window.devicePixelRatio || 1, 3);
                const w = on
                    ? Math.round(window.screen.width * dpr)
                    : Math.round((wrapRef.current?.clientWidth ?? 0) * dpr);
                const h = on
                    ? Math.round(window.screen.height * dpr)
                    : Math.round((wrapRef.current?.clientHeight ?? 0) * dpr);
                if (w >= 200 && h >= 200) onResizeRef.current?.(w, h);
            });
        };
        document.addEventListener("fullscreenchange", onChange);
        return () => document.removeEventListener("fullscreenchange", onChange);
    }, [onKbdCapture, onInput]);

    // The OS hook relays Ctrl+Alt+Enter (pressed while captured) as a request
    // to leave fullscreen — the local key never reaches the web view.
    useEffect(() => {
        const un = listen("rdp:exit-fullscreen", () => {
            if (document.fullscreenElement) void document.exitFullscreen().catch(() => {});
        });
        return () => {
            void un.then((f) => f());
        };
    }, []);

    const toggleFs = useCallback(() => {
        if (document.fullscreenElement) {
            void document.exitFullscreen().catch(() => {});
        } else {
            // Windows quirk: entering fullscreen from an already-maximized
            // window keeps the maximized client rect (= work area, screen minus
            // taskbar), so the surface grows to the full screen but the content
            // stays work-area-tall — leaving a black strip (taskbar height) at
            // the bottom. Restore the window first so fullscreen starts clean.
            void (async () => {
                try {
                    const win = getCurrentWindow();
                    if (await win.isMaximized()) await win.unmaximize();
                } catch {
                    /* fall through to fullscreen regardless */
                }
                await wrapRef.current?.requestFullscreen().catch(() => {});
            })();
        }
        // Keep input focus on the canvas after toggling.
        window.setTimeout(() => canvasRef.current?.focus(), 0);
    }, []);

    // Reveal the connection bar when the pointer nears the edge the bar is
    // anchored to (~56px): top edge for top anchors, bottom edge for bottom
    // anchors — so a bar dragged to the bottom reveals where it actually sits.
    const onPointerMove = useCallback((e: React.MouseEvent) => {
        const wr = wrapRef.current?.getBoundingClientRect();
        if (!wr) return;
        const near = barAnchor[0] === "b" ? wr.bottom - e.clientY : e.clientY - wr.top;
        setBarOn(near < 56);
    }, [barAnchor]);

    // Draw a decoded framebuffer region. Stable identity (reads refs only)
    // so the store registration runs once per session.
    const applyEvent = useCallback((ev: RdpSessionEvent) => {
        // Server cursor shape → CSS cursor on the canvas. Handled before the
        // ctx guard since these don't draw to the framebuffer.
        if (ev.kind === "pointer_bitmap") {
            const c = canvasRef.current;
            if (c) c.style.cursor = pointerToCss(ev);
            return;
        }
        if (ev.kind === "pointer_hidden") {
            const c = canvasRef.current;
            if (c) c.style.cursor = "none";
            return;
        }
        if (ev.kind === "pointer_default") {
            const c = canvasRef.current;
            if (c) c.style.cursor = "default";
            return;
        }

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

    // Dynamic resize (DisplayControl): when the viewport box settles at a new
    // size, ask the server to re-render at that resolution so the desktop
    // fills the pane instead of letterboxing.
    //
    // Gated by `enableDynamicResize` (parent enables it for GFX sessions). On
    // the legacy RemoteFX path the post-resize reactivation degrades the server
    // repaint rate (IronRDP #447) and can leave an unrepainted region, so the
    // parent leaves it off there; GFX's self-correcting fb-diff repaints the
    // new surface cleanly. The DVC/backend path is always wired.
    useEffect(() => {
        if (!enableDynamicResize) return;
        const el = wrapRef.current;
        if (!el) return;
        let timer = 0;
        let lastSent = "";
        const fire = () => {
            // Physical pixels (CSS × DPR): the server renders at native DPI and
            // the canvas backing matches 1:1 physical — crisp on scaled displays.
            const dpr = Math.min(window.devicePixelRatio || 1, 3);
            const w = Math.round(el.clientWidth * dpr);
            const h = Math.round(el.clientHeight * dpr);
            if (w < 200 || h < 200) return;
            const dim = `${w}x${h}`;
            if (dim === lastSent) return;
            lastSent = dim;
            onResizeRef.current?.(w, h);
        };
        const ro = new ResizeObserver(() => {
            clearTimeout(timer);
            timer = window.setTimeout(fire, 250);
        });
        ro.observe(el);
        return () => {
            clearTimeout(timer);
            ro.disconnect();
        };
    }, [sessionKey, enableDynamicResize]);

    // --- coordinate mapping: display (CSS) px → backing px ---
    const toCanvas = (clientX: number, clientY: number): { x: number; y: number } => {
        const c = canvasRef.current;
        if (!c) return { x: 0, y: 0 };
        const r = c.getBoundingClientRect();
        if (r.width <= 0 || r.height <= 0) return { x: 0, y: 0 };
        // The canvas is displayed with `object-fit: contain`: the backing
        // bitmap (width×height) is scaled uniformly to fit the element box and
        // centered, leaving letterbox bars on the axis with extra space. Undo
        // that transform (scale + centering offset) so clicks land on the
        // right pixel; points in the bars clamp to the nearest edge.
        const scale = Math.min(r.width / width, r.height / height);
        const dispW = width * scale;
        const dispH = height * scale;
        const offX = (r.width - dispW) / 2;
        const offY = (r.height - dispH) / 2;
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
        // Make the current local clipboard available for paste into the remote
        // (CLIPRDR client→server advertise). Prefer an image if the clipboard
        // holds one; otherwise text. The image rgba() transfer is heavy, so we
        // skip it when the size matches the last image we already pushed.
        void (async () => {
            try {
                const img = await clipboardReadImage();
                const size = await img.size();
                const key = `${size.width}x${size.height}`;
                if (key !== lastImgKey.current) {
                    const rgba = await img.rgba();
                    lastImgKey.current = key;
                    onLocalClipboardImage?.(size.width, size.height, bytesToBase64(rgba));
                }
                return; // an image is on the clipboard — done
            } catch {
                lastImgKey.current = ""; // no image now; allow the next one
            }
            try {
                const t = await clipboardReadText();
                if (t) onLocalClipboard?.(t);
            } catch {
                /* clipboard empty / unsupported */
            }
        })();
    }, [onInput, onLocalClipboard, onLocalClipboardImage]);

    const onBlur = useCallback(() => {
        onInput({ kind: "release_all_modifiers" });
    }, [onInput]);

    return (
        <div
            ref={wrapRef}
            className={styles.wrap}
            onMouseMove={onPointerMove}
            onMouseLeave={() => setBarOn(false)}
        >
            <canvas
                ref={canvasRef}
                width={width}
                height={height}
                tabIndex={0}
                className={`${styles.canvas}${className ? ` ${className}` : ""}`}
                style={isFs ? { width: "100vw", height: "100vh" } : undefined}
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
            <div
                ref={barRef}
                className={`${styles.bar} ${barDrag ? styles.barDragging : styles[`pos_${barAnchor}`]}${barOn || barDrag ? ` ${styles.barOn}` : ""}`}
                style={barDrag ? { left: barDrag.x, top: barDrag.y, right: "auto", bottom: "auto", transform: "none" } : undefined}
                role="toolbar"
                aria-label={hostLabel}
            >
                {hostLabel && (
                    <span
                        className={styles.barTitle}
                        onPointerDown={onBarPointerDown}
                        title={t("session.moveBar")}
                    >
                        {hostLabel}
                    </span>
                )}
                {onMinimize && (onPopIn || isFs) && (
                    <button
                        type="button"
                        className={styles.barBtn}
                        title={t("session.minimize")}
                        aria-label={t("session.minimize")}
                        onClick={onMinimize}
                    >
                        <Minus size={15} />
                    </button>
                )}
                {/* Fullscreen toggle: hidden in main-window fullscreen — the X
                    below exits instead (per the agreed fullscreen layout:
                    title · minimize · close). Pop-out keeps its toggle. */}
                {(!isFs || onPopIn) && (
                    <button
                        type="button"
                        className={styles.barBtn}
                        title={t("session.fullscreen")}
                        aria-label={t("session.fullscreen")}
                        onClick={toggleFs}
                    >
                        {isFs ? <Minimize2 size={15} /> : <Maximize2 size={15} />}
                    </button>
                )}
                {onPopOut && !isFs && (
                    <button
                        type="button"
                        className={styles.barBtn}
                        title={t("session.popOut")}
                        aria-label={t("session.popOut")}
                        onClick={onPopOut}
                    >
                        <PictureInPicture2 size={15} />
                    </button>
                )}
                {isFs && !onPopIn && (
                    <button
                        type="button"
                        className={`${styles.barBtn} ${styles.barClose}`}
                        title={t("session.exitFullscreen")}
                        aria-label={t("session.exitFullscreen")}
                        onClick={toggleFs}
                    >
                        <X size={15} />
                    </button>
                )}
                {onPopIn && (
                    <button
                        type="button"
                        className={`${styles.barBtn} ${styles.barClose}`}
                        title={t("session.redock")}
                        aria-label={t("session.redock")}
                        onClick={onPopIn}
                    >
                        <X size={15} />
                    </button>
                )}
            </div>
            {showHint && <div className={styles.fsHint}>{t("session.fullscreenHint")}</div>}
        </div>
    );
}
