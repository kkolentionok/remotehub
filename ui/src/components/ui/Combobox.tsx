import {
    useEffect,
    useId,
    useLayoutEffect,
    useMemo,
    useRef,
    useState,
    type KeyboardEvent,
} from "react";
import { Check, ChevronDown, Plus } from "lucide-react";
import { createPortal } from "react-dom";

import { useT } from "../../i18n";
import styles from "./Combobox.module.css";

export interface ComboboxOption {
    /** Stable identifier returned via onChange. */
    value: string;
    /** Visible label, used for filtering. */
    label: string;
}

interface Props {
    options: ComboboxOption[];
    /** Current selection — must be one of `options[i].value`, or `""` for empty. */
    value: string;
    /** Called with the chosen value (existing option). */
    onChange: (value: string) => void;
    /**
     * Called when the user picks the "Create '...'" row. Receives the
     * raw typed string. Returning a Promise lets the parent show a
     * loading state; the dropdown closes once the promise resolves.
     */
    onCreateNew?: (label: string) => Promise<void> | void;
    /** Placeholder shown in the input when nothing is selected. */
    placeholder?: string;
    /** Optional label for the "create" row prefix, e.g. "Create group". */
    createLabel?: string;
    /** Disable the whole control. */
    disabled?: boolean;
    /**
     * If true, blanking the input emits onChange(""), letting the
     * caller represent "no selection". Default: true.
     */
    clearable?: boolean;
}

/**
 * Combobox: input with a filtered dropdown of existing options plus
 * an optional "Create '<typed>'" row when the typed text doesn't
 * match anything.
 *
 * Behavior:
 * - Focus opens dropdown; Esc/blur closes it.
 * - Typing filters by substring (case-insensitive).
 * - ↑/↓ moves the highlighted row; Enter picks it.
 * - If the typed text doesn't match any option AND onCreateNew is
 *   provided, a "Create '<typed>'" row appears first and is
 *   highlighted by default.
 *
 * This is intentionally a single-component primitive (~200 lines)
 * rather than pulling in Downshift/Radix — our needs are constrained
 * and we want zero new deps.
 */
export function Combobox({
    options,
    value,
    onChange,
    onCreateNew,
    placeholder,
    createLabel,
    disabled,
    clearable = true,
}: Props) {
    const { t } = useT();
    const id = useId();
    const rootRef = useRef<HTMLDivElement>(null);
    const inputRef = useRef<HTMLInputElement>(null);
    const listRef = useRef<HTMLUListElement>(null);

    // Mode: either we show the saved label (when value matches an option),
    // or we're typing (open dropdown). `query` is the live input string;
    // when dropdown closes, the input snaps back to the chosen label.
    const [open, setOpen] = useState(false);
    const [query, setQuery] = useState("");
    const [highlighted, setHighlighted] = useState(0);
    // The dropdown is rendered with position:fixed at coordinates measured from
    // the input, so it escapes any ancestor with `overflow` clipping (e.g. a
    // horizontally-scrollable form card). Flips upward when space below is tight.
    const [coords, setCoords] = useState<{
        left: number;
        top?: number;
        bottom?: number;
        width: number;
        maxH: number;
        up: boolean;
    } | null>(null);

    // Display value when not actively typing.
    const selectedLabel = useMemo(() => {
        if (!value) return "";
        return options.find((o) => o.value === value)?.label ?? "";
    }, [value, options]);

    // What the input shows: typed query when open, selected label otherwise.
    const inputValue = open ? query : selectedLabel;

    // Filter options by query.
    const filtered = useMemo(() => {
        const q = query.trim().toLowerCase();
        if (!q) return options;
        return options.filter((o) => o.label.toLowerCase().includes(q));
    }, [options, query]);

    // Should we offer "Create '<query>'"? Only when query is non-empty,
    // doesn't match any existing label exactly, and the caller wired up
    // onCreateNew.
    const exactMatch = useMemo(
        () => options.some((o) => o.label.toLowerCase() === query.trim().toLowerCase()),
        [options, query],
    );
    const showCreateRow = !!onCreateNew && query.trim().length > 0 && !exactMatch;

    // Reset highlighted index when the filtered list shape changes.
    useEffect(() => {
        setHighlighted(0);
    }, [query, open]);

    // Position the fixed dropdown from the input's viewport rect. Re-measures
    // on any scroll (capture=true catches scrollable ancestors) and on resize.
    useLayoutEffect(() => {
        if (!open) {
            setCoords(null);
            return;
        }
        const measure = () => {
            const el = inputRef.current;
            if (!el) return;
            const r = el.getBoundingClientRect();
            const gap = 4;
            const spaceBelow = window.innerHeight - r.bottom - 8;
            const spaceAbove = r.top - 8;
            const up = spaceBelow < 180 && spaceAbove > spaceBelow;
            const maxH = Math.min(240, Math.max(120, up ? spaceAbove : spaceBelow));
            setCoords({
                left: r.left,
                width: r.width,
                maxH,
                up,
                // Anchor by the edge nearest the input so the list always hugs
                // it, regardless of how many rows it ends up rendering.
                ...(up
                    ? { bottom: window.innerHeight - r.top + gap }
                    : { top: r.bottom + gap }),
            });
        };
        measure();
        window.addEventListener("scroll", measure, true);
        window.addEventListener("resize", measure);
        return () => {
            window.removeEventListener("scroll", measure, true);
            window.removeEventListener("resize", measure);
        };
    }, [open]);

    // Click-outside closes the dropdown.
    useEffect(() => {
        if (!open) return;
        const onDocMouseDown = (e: MouseEvent) => {
            const target = e.target as Node;
            if (rootRef.current?.contains(target) || listRef.current?.contains(target)) {
                return;
            }
            close();
        };
        document.addEventListener("mousedown", onDocMouseDown);
        return () => document.removeEventListener("mousedown", onDocMouseDown);
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [open]);

    function openDropdown() {
        if (disabled) return;
        // Start with an empty query so the FULL list of options is shown on
        // click/focus (the current selection is marked with a check), rather
        // than pre-filtering to just the selected label.
        setQuery("");
        setOpen(true);
    }

    function close() {
        setOpen(false);
        setQuery("");
    }

    async function pickAt(index: number) {
        // Index 0 is the create row when showCreateRow=true; otherwise it's
        // the first filtered option.
        if (showCreateRow && index === 0) {
            if (onCreateNew) {
                const typed = query.trim();
                await onCreateNew(typed);
            }
            close();
            return;
        }
        const optionIndex = showCreateRow ? index - 1 : index;
        const opt = filtered[optionIndex];
        if (opt) {
            onChange(opt.value);
        }
        close();
    }

    function handleKeyDown(e: KeyboardEvent<HTMLInputElement>) {
        const total = filtered.length + (showCreateRow ? 1 : 0);
        if (e.key === "ArrowDown") {
            e.preventDefault();
            if (!open) {
                openDropdown();
            } else {
                setHighlighted((i) => (i + 1) % Math.max(total, 1));
            }
        } else if (e.key === "ArrowUp") {
            e.preventDefault();
            setHighlighted((i) => (i - 1 + Math.max(total, 1)) % Math.max(total, 1));
        } else if (e.key === "Enter") {
            e.preventDefault();
            if (total > 0) void pickAt(highlighted);
        } else if (e.key === "Escape") {
            e.preventDefault();
            close();
        } else if (e.key === "Backspace" && open && query === "" && value && clearable) {
            // Empty input + backspace → clear selection.
            onChange("");
        }
    }

    function handleChange(e: React.ChangeEvent<HTMLInputElement>) {
        if (!open) setOpen(true);
        setQuery(e.target.value);
    }

    return (
        <div ref={rootRef} className={styles.root}>
            <input
                ref={inputRef}
                id={id}
                type="text"
                className={styles.input}
                value={inputValue}
                placeholder={placeholder}
                disabled={disabled}
                onFocus={openDropdown}
                onClick={() => {
                    if (!open) openDropdown();
                }}
                onChange={handleChange}
                onKeyDown={handleKeyDown}
                autoComplete="off"
                spellCheck={false}
                role="combobox"
                aria-expanded={open}
                aria-controls={`${id}-list`}
                aria-autocomplete="list"
            />
            <ChevronDown
                size={14}
                className={`${styles.caret} ${open ? styles.caretOpen : ""}`}
                aria-hidden
                onMouseDown={(e) => {
                    e.preventDefault();
                    if (disabled) return;
                    if (open) {
                        close();
                    } else {
                        openDropdown();
                        inputRef.current?.focus();
                    }
                }}
            />
            {open &&
                createPortal(
                    <ul
                        ref={listRef}
                        id={`${id}-list`}
                        className={styles.list}
                        role="listbox"
                        style={
                            coords
                                ? {
                                      position: "fixed",
                                      left: coords.left,
                                      width: coords.width,
                                      maxHeight: coords.maxH,
                                      zIndex: 9999,
                                      ...(coords.up
                                          ? { bottom: coords.bottom }
                                          : { top: coords.top }),
                                  }
                                : { position: "fixed", visibility: "hidden", zIndex: 9999 }
                        }
                    >
                    {showCreateRow && (
                        <li
                            role="option"
                            aria-selected={highlighted === 0}
                            className={`${styles.row} ${styles.createRow} ${
                                highlighted === 0 ? styles.rowActive : ""
                            }`}
                            onMouseDown={(e) => {
                                e.preventDefault();
                                void pickAt(0);
                            }}
                            onMouseEnter={() => setHighlighted(0)}
                        >
                            <Plus size={12} />
                            <span>
                                {(createLabel ?? t("common.create"))} "
                                <strong>{query.trim()}</strong>"
                            </span>
                        </li>
                    )}
                    {filtered.length === 0 && !showCreateRow && (
                        <li className={`${styles.row} ${styles.rowEmpty}`}>
                            {t("sidebar.emptySearch")}
                        </li>
                    )}
                    {filtered.map((opt, i) => {
                        const index = showCreateRow ? i + 1 : i;
                        const active = highlighted === index;
                        const selected = opt.value === value;
                        return (
                            <li
                                key={opt.value}
                                role="option"
                                aria-selected={active}
                                className={`${styles.row} ${active ? styles.rowActive : ""}`}
                                onMouseDown={(e) => {
                                    e.preventDefault();
                                    void pickAt(index);
                                }}
                                onMouseEnter={() => setHighlighted(index)}
                            >
                                <span className={styles.rowLabel}>{opt.label}</span>
                                {selected && <Check size={12} className={styles.rowCheck} />}
                            </li>
                        );
                    })}
                    </ul>,
                    document.body,
                )}
        </div>
    );
}
