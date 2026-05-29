import { useRef, useState } from "react";

import { useSessionsStore } from "../../store";
import type { PaneNode } from "../../lib/paneTree";
import { SessionView } from "../session/SessionView";
import styles from "./PaneGroup.module.css";

interface Ctx {
    tabId: string;
    activePaneKey: string;
    /** The whole tab is the active tab (its panes are visible). */
    tabVisible: boolean;
    paneCount: number;
}

/**
 * Renders a tab's pane tree. Leaves host a SessionView; splits lay their
 * two children out in a row/column at the stored ratio with a draggable
 * divider. `path` addresses the current node from the tab root (sequence
 * of "a"/"b" steps) so a divider drag knows which split to resize.
 */
export function PaneGroup({
    node,
    ctx,
    path = [],
}: {
    node: PaneNode;
    ctx: Ctx;
    path?: ("a" | "b")[];
}) {
    if (node.t === "leaf") {
        return <PaneLeaf sessionKey={node.key} ctx={ctx} />;
    }
    return <Split node={node} ctx={ctx} path={path} />;
}

function Split({
    node,
    ctx,
    path,
}: {
    node: Extract<PaneNode, { t: "split" }>;
    ctx: Ctx;
    path: ("a" | "b")[];
}) {
    const ref = useRef<HTMLDivElement>(null);
    const setSplitRatio = useSessionsStore((s) => s.setSplitRatio);

    const aPct = `${(node.ratio * 100).toFixed(2)}%`;
    const bPct = `${((1 - node.ratio) * 100).toFixed(2)}%`;
    const isRow = node.dir === "row";

    const onDividerPointerDown = (e: React.PointerEvent) => {
        e.preventDefault();
        const container = ref.current;
        if (!container) return;
        const rect = container.getBoundingClientRect();
        const divider = e.currentTarget as HTMLElement;
        divider.setPointerCapture(e.pointerId);
        document.body.style.cursor = isRow ? "col-resize" : "row-resize";
        document.body.style.userSelect = "none";

        const onMove = (ev: PointerEvent) => {
            const r = isRow
                ? (ev.clientX - rect.left) / rect.width
                : (ev.clientY - rect.top) / rect.height;
            setSplitRatio(ctx.tabId, path, r); // store clamps to 15–85%
        };
        const onUp = (ev: PointerEvent) => {
            divider.releasePointerCapture(ev.pointerId);
            divider.removeEventListener("pointermove", onMove);
            divider.removeEventListener("pointerup", onUp);
            document.body.style.cursor = "";
            document.body.style.userSelect = "";
        };
        divider.addEventListener("pointermove", onMove);
        divider.addEventListener("pointerup", onUp);
    };

    return (
        <div
            ref={ref}
            className={`${styles.split} ${isRow ? styles.row : styles.col}`}
        >
            <div className={styles.cell} style={{ flexBasis: aPct }}>
                <PaneGroup node={node.a} ctx={ctx} path={[...path, "a"]} />
            </div>
            <div
                className={`${styles.divider} ${isRow ? styles.dividerV : styles.dividerH}`}
                onPointerDown={onDividerPointerDown}
            />
            <div className={styles.cell} style={{ flexBasis: bPct }}>
                <PaneGroup node={node.b} ctx={ctx} path={[...path, "b"]} />
            </div>
        </div>
    );
}

type Edge = "left" | "right" | "top" | "bottom";

function PaneLeaf({ sessionKey, ctx }: { sessionKey: string; ctx: Ctx }) {
    const session = useSessionsStore((s) =>
        s.sessions.find((x) => x.key === sessionKey),
    );
    const setActivePane = useSessionsStore((s) => s.setActivePane);
    const dragging = useSessionsStore((s) => s.draggingSession);
    const moveSessionIntoSplit = useSessionsStore((s) => s.moveSessionIntoSplit);
    const setDraggingSession = useSessionsStore((s) => s.setDraggingSession);
    const ref = useRef<HTMLDivElement>(null);
    const [edge, setEdge] = useState<Edge | null>(null);

    if (!session) return null;

    const focused = ctx.activePaneKey === sessionKey;
    // A session drag is in progress and it isn't this very pane. (When a tab
    // is dragged, the target tab is previewed, so the visible pane is never
    // the dragged one.)
    const dropActive = dragging !== null && dragging !== sessionKey;

    const edgeAt = (e: React.DragEvent): Edge => {
        const r = ref.current!.getBoundingClientRect();
        const x = (e.clientX - r.left) / r.width;
        const y = (e.clientY - r.top) / r.height;
        const d = { left: x, right: 1 - x, top: y, bottom: 1 - y };
        return (Object.keys(d) as Edge[]).reduce((m, k) => (d[k] < d[m] ? k : m), "left");
    };

    const onDrop = (e: React.DragEvent) => {
        e.preventDefault();
        const src = useSessionsStore.getState().draggingSession;
        const ed = edge ?? edgeAt(e);
        setEdge(null);
        setDraggingSession(null);
        if (!src || src === sessionKey) return;
        const dir = ed === "left" || ed === "right" ? "row" : "col";
        const newFirst = ed === "left" || ed === "top";
        moveSessionIntoSplit(src, sessionKey, dir, newFirst);
    };

    return (
        <div
            ref={ref}
            className={`${styles.leaf} ${ctx.paneCount > 1 && focused ? styles.focused : ""}`}
            onMouseDownCapture={() => {
                if (!focused) setActivePane(ctx.tabId, sessionKey);
            }}
        >
            <SessionView
                session={session}
                visible={ctx.tabVisible}
                focused={ctx.tabVisible && focused}
                showHeader={ctx.paneCount > 1}
            />
            {dropActive && (
                <div
                    className={styles.dropOverlay}
                    onDragOver={(e) => {
                        e.preventDefault();
                        e.dataTransfer.dropEffect = "move";
                        const ed = edgeAt(e);
                        if (ed !== edge) setEdge(ed);
                    }}
                    onDragLeave={(e) => {
                        // Only clear when leaving the overlay itself.
                        if (e.currentTarget === e.target) setEdge(null);
                    }}
                    onDrop={onDrop}
                >
                    {edge && (
                        <div className={`${styles.dropZone} ${styles[`zone--${edge}`]}`} />
                    )}
                </div>
            )}
        </div>
    );
}
