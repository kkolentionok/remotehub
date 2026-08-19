import { useState } from "react";

import { useT } from "../../../i18n";
import { hotkeys, type UnstickHotkey } from "../../../lib/ipc";
import styles from "../SettingsDialog.module.css";

const LS_KEY = "pingie.unstickHotkey";
const DEFAULT: UnstickHotkey = { ctrl: true, alt: true, shift: false, meta: false, code: "KeyK" };
const DISABLED: UnstickHotkey = { ctrl: false, alt: false, shift: false, meta: false, code: null };

const MOD_CODES = new Set([
    "ControlLeft",
    "ControlRight",
    "AltLeft",
    "AltRight",
    "ShiftLeft",
    "ShiftRight",
    "MetaLeft",
    "MetaRight",
]);

function loadHotkey(): UnstickHotkey {
    try {
        const raw = localStorage.getItem(LS_KEY);
        if (raw) return JSON.parse(raw) as UnstickHotkey;
    } catch {
        /* ignore */
    }
    return DEFAULT;
}

function codeLabel(code: string | null): string | null {
    if (!code) return null;
    if (/^Key[A-Z]$/.test(code)) return code.slice(3);
    if (/^Digit[0-9]$/.test(code)) return code.slice(5);
    if (/^F([1-9]|1[0-9]|2[0-4])$/.test(code)) return code;
    const named: Record<string, string> = {
        Space: "Space", Enter: "Enter", Backspace: "Backspace", Tab: "Tab",
        ArrowUp: "↑", ArrowDown: "↓", ArrowLeft: "←", ArrowRight: "→",
        Home: "Home", End: "End", PageUp: "PgUp", PageDown: "PgDn",
        Insert: "Ins", Delete: "Del", Minus: "-", Equal: "=",
        BracketLeft: "[", BracketRight: "]", Semicolon: ";", Quote: "'",
        Backquote: "`", Comma: ",", Period: ".", Slash: "/", Backslash: "\\",
    };
    return named[code] ?? code;
}

function label(hk: UnstickHotkey): string {
    const parts: string[] = [];
    if (hk.ctrl) parts.push("Ctrl");
    if (hk.alt) parts.push("Alt");
    if (hk.shift) parts.push("Shift");
    if (hk.meta) parts.push("Win");
    const k = codeLabel(hk.code);
    if (k) parts.push(k);
    return parts.join(" + ");
}

export function HotkeyRecorder() {
    const { t } = useT();
    const [hk, setHk] = useState<UnstickHotkey>(loadHotkey);
    const [recording, setRecording] = useState(false);

    function apply(next: UnstickHotkey) {
        setHk(next);
        try {
            localStorage.setItem(LS_KEY, JSON.stringify(next));
        } catch {
            /* ignore */
        }
        void hotkeys.setUnstick(next);
    }

    function onKeyDown(e: React.KeyboardEvent) {
        if (!recording) return;
        e.preventDefault();
        e.stopPropagation();
        if (e.code === "Escape") {
            setRecording(false);
            return;
        }
        if (MOD_CODES.has(e.code)) return; // wait for a real key
        const next: UnstickHotkey = {
            ctrl: e.ctrlKey,
            alt: e.altKey,
            shift: e.shiftKey,
            meta: e.metaKey,
            code: e.code,
        };
        if (!next.ctrl && !next.alt && !next.shift && !next.meta) return; // require a modifier
        apply(next);
        setRecording(false);
    }

    const shown = label(hk) || t("settings.hotkey.disabled");

    return (
        <div className={styles.field}>
            <label className={styles.fieldLabel}>{t("settings.hotkey.unstickLabel")}</label>
            <div className={styles.hotkeyRow}>
                <button
                    type="button"
                    className={`${styles.hotkeyBtn} ${recording ? styles.hotkeyBtnRec : ""}`}
                    onClick={() => setRecording(true)}
                    onKeyDown={onKeyDown}
                    onBlur={() => setRecording(false)}
                >
                    {recording ? t("settings.hotkey.press") : shown}
                </button>
                <button type="button" className={styles.hotkeyAux} onClick={() => apply(DEFAULT)}>
                    {t("settings.hotkey.reset")}
                </button>
                <button type="button" className={styles.hotkeyAux} onClick={() => apply(DISABLED)}>
                    {t("settings.hotkey.disable")}
                </button>
            </div>
            <div className={styles.fieldHint}>{t("settings.hotkey.unstickHint")}</div>
        </div>
    );
}
