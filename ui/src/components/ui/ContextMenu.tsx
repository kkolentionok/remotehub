import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { ChevronRight, type LucideIcon } from "lucide-react";

import styles from "./ContextMenu.module.css";

/** One row in a context menu. A separator carries only `{ separator: true }`. */
export type MenuItem =
    | {
          id: string;
          label: string;
          icon?: LucideIcon;
          shortcut?: string;
          danger?: boolean;
          disabled?: boolean;
          /** Leaf action. Omitted when the item only opens a submenu. */
          onSelect?: () => void;
          /** Submenu rows (mutually exclusive with a meaningful onSelect). */
          children?: MenuItem[];
          separator?: false;
      }
    | { id: string; separator: true };

function MenuList({
    items,
    onClose,
    submenu,
}: {
    items: MenuItem[];
    onClose: () => void;
    submenu?: boolean;
}) {
    const [openSub, setOpenSub] = useState<string | null>(null);
    return (
        <div
            className={`${styles.menu} ${submenu ? styles.submenu : ""}`}
            role="menu"
        >
            {items.map((it) =>
                "separator" in it && it.separator ? (
                    <div key={it.id} className={styles.sep} role="separator" />
                ) : (
                    <Row
                        key={it.id}
                        item={it as Extract<MenuItem, { label: string }>}
                        onClose={onClose}
                        open={openSub === it.id}
                        onHover={() => setOpenSub(it.id)}
                        onToggle={() =>
                            setOpenSub((cur) => (cur === it.id ? null : it.id))
                        }
                    />
                ),
            )}
        </div>
    );
}

function Row({
    item,
    onClose,
    open,
    onHover,
    onToggle,
}: {
    item: Extract<MenuItem, { label: string }>;
    onClose: () => void;
    open: boolean;
    onHover: () => void;
    onToggle: () => void;
}) {
    const Icon = item.icon;
    const hasChildren = !!item.children?.length;
    return (
        <div
            className={`${styles.item} ${item.danger ? styles.danger : ""} ${
                item.disabled ? styles.disabled : ""
            }`}
            role="menuitem"
            aria-haspopup={hasChildren || undefined}
            onMouseEnter={onHover}
            onClick={(e) => {
                if (item.disabled) return;
                if (hasChildren) {
                    // Parent row: clicking toggles its flyout (in addition to
                    // hover-open) so it works even if the hover didn't catch.
                    e.stopPropagation();
                    onToggle();
                    return;
                }
                item.onSelect?.();
                onClose();
            }}
        >
            {Icon ? (
                <Icon size={14} className={styles.icon} aria-hidden="true" />
            ) : (
                <span className={styles.icon} />
            )}
            <span className={styles.label}>{item.label}</span>
            {item.shortcut && <span className={styles.shortcut}>{item.shortcut}</span>}
            {hasChildren && <ChevronRight size={14} className={styles.chev} />}
            {hasChildren && open && (
                <MenuList items={item.children!} onClose={onClose} submenu />
            )}
        </div>
    );
}

/**
 * A context menu anchored at viewport coordinates, rendered in a portal so it
 * never gets clipped by a pane's overflow. Closes on outside-click, Esc,
 * scroll, resize, or blur. The owner holds the open state:
 *
 *   const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
 *   onContextMenu={(e) => { e.preventDefault(); setMenu({ x: e.clientX, y: e.clientY }); }}
 *   {menu && <ContextMenu x={menu.x} y={menu.y} items={...} onClose={() => setMenu(null)} />}
 */
export function ContextMenu({
    x,
    y,
    items,
    onClose,
}: {
    x: number;
    y: number;
    items: MenuItem[];
    onClose: () => void;
}) {
    const ref = useRef<HTMLDivElement>(null);
    const [pos, setPos] = useState({ x, y });

    // Clamp inside the viewport once measured.
    useLayoutEffect(() => {
        const el = ref.current;
        if (!el) return;
        const r = el.getBoundingClientRect();
        const m = 8;
        const nx =
            x + r.width > window.innerWidth - m
                ? Math.max(m, window.innerWidth - r.width - m)
                : x;
        const ny =
            y + r.height > window.innerHeight - m
                ? Math.max(m, window.innerHeight - r.height - m)
                : y;
        setPos({ x: nx, y: ny });
    }, [x, y]);

    useEffect(() => {
        const onDown = (e: MouseEvent) => {
            if (ref.current && !ref.current.contains(e.target as Node)) onClose();
        };
        const onKey = (e: KeyboardEvent) => {
            if (e.key === "Escape") {
                e.stopPropagation();
                onClose();
            }
        };
        document.addEventListener("mousedown", onDown, true);
        document.addEventListener("keydown", onKey, true);
        window.addEventListener("scroll", onClose, true);
        window.addEventListener("resize", onClose);
        window.addEventListener("blur", onClose);
        return () => {
            document.removeEventListener("mousedown", onDown, true);
            document.removeEventListener("keydown", onKey, true);
            window.removeEventListener("scroll", onClose, true);
            window.removeEventListener("resize", onClose);
            window.removeEventListener("blur", onClose);
        };
    }, [onClose]);

    return createPortal(
        <div
            ref={ref}
            className={styles.root}
            style={{ left: pos.x, top: pos.y }}
            onContextMenu={(e) => e.preventDefault()}
        >
            <MenuList items={items} onClose={onClose} />
        </div>,
        document.body,
    );
}
