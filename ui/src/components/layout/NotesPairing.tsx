import { useCallback, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { KeyRound, Loader2, Monitor, X } from "lucide-react";

import { useT } from "../../i18n";
import { notes as notesApi } from "../../lib/ipc";
import { formatApiError, type PairCode, type PairedDevice } from "../../lib/types";
import styles from "./NotesPairing.module.css";

/** mm:ss remaining, or null once the code is dead. */
function useCountdown(iso: string | undefined): string | null {
    const [left, setLeft] = useState<number>(0);
    useEffect(() => {
        if (!iso) return;
        const end = new Date(iso).getTime();
        const tick = () => setLeft(Math.max(0, end - Date.now()));
        tick();
        const id = window.setInterval(tick, 1000);
        return () => window.clearInterval(id);
    }, [iso]);
    if (!iso || left <= 0) return null;
    const s = Math.floor(left / 1000);
    return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

/**
 * The access-code button and its popover.
 *
 * Anchored manually into a portal: the notes list clips its overflow, and a
 * popover rendered inside it would be cut off.
 */
export function AccessCodeButton({ onToast }: { onToast: (s: string) => void }) {
    const { t } = useT();
    const btnRef = useRef<HTMLButtonElement | null>(null);
    const [open, setOpen] = useState(false);
    const [at, setAt] = useState<{ left: number; top: number } | null>(null);
    const [code, setCode] = useState<PairCode | null>(null);
    const [busy, setBusy] = useState(false);
    const [devices, setDevices] = useState<PairedDevice[]>([]);
    const [confirming, setConfirming] = useState<string | null>(null);
    const left = useCountdown(code?.expires_at);

    const place = useCallback(() => {
        const r = btnRef.current?.getBoundingClientRect();
        if (r) setAt({ left: Math.max(12, r.right - 320), top: r.bottom + 6 });
    }, []);

    const loadDevices = useCallback(async () => {
        try {
            setDevices(await notesApi.pairDevices());
        } catch {
            /* the popover is still useful without the list */
        }
    }, []);

    async function generate() {
        setBusy(true);
        try {
            setCode(await notesApi.pairCreate());
        } catch (e) {
            onToast(formatApiError(e));
            setOpen(false);
        } finally {
            setBusy(false);
        }
    }

    async function toggle() {
        if (open) {
            setOpen(false);
            return;
        }
        place();
        setOpen(true);
        setCode(null);
        setConfirming(null);
        void generate();
        void loadDevices();
    }

    async function revoke(id: string) {
        try {
            await notesApi.pairRevoke(id);
            onToast(t("tools.notes.code.revoked"));
            await loadDevices();
        } catch (e) {
            onToast(formatApiError(e));
        } finally {
            setConfirming(null);
        }
    }

    // Esc closes; so does a click anywhere outside.
    useEffect(() => {
        if (!open) return;
        const onKey = (e: KeyboardEvent) => {
            if (e.key === "Escape") setOpen(false);
        };
        const onDown = (e: MouseEvent) => {
            const el = e.target as HTMLElement;
            if (!el.closest(`.${styles.pop}`) && el !== btnRef.current) setOpen(false);
        };
        window.addEventListener("keydown", onKey);
        window.addEventListener("mousedown", onDown);
        window.addEventListener("resize", place);
        return () => {
            window.removeEventListener("keydown", onKey);
            window.removeEventListener("mousedown", onDown);
            window.removeEventListener("resize", place);
        };
    }, [open, place]);

    return (
        <>
            <button
                ref={btnRef}
                type="button"
                className={styles.trigger}
                title={t("tools.notes.code")}
                onClick={() => void toggle()}
            >
                <KeyRound size={14} />
            </button>

            {open &&
                at &&
                createPortal(
                    <div className={styles.pop} style={{ left: at.left, top: at.top }}>
                        <div className={styles.popHead}>
                            <span>{t("tools.notes.code.title")}</span>
                            <button type="button" onClick={() => setOpen(false)}>
                                <X size={13} />
                            </button>
                        </div>

                        <div className={styles.codeBox}>
                            {busy ? (
                                <Loader2 size={18} className={styles.spin} />
                            ) : (
                                <button
                                    type="button"
                                    className={`${styles.code} ${left ? "" : styles.dead}`}
                                    onClick={() => {
                                        if (!code || !left) return;
                                        void navigator.clipboard.writeText(code.code);
                                        onToast(t("tools.notes.code.copied"));
                                    }}
                                >
                                    {code?.code ?? "········"}
                                </button>
                            )}
                        </div>

                        {!busy &&
                            (left ? (
                                <p className={styles.hint}>
                                    {t("tools.notes.code.left", { t: left })} · {t("tools.notes.code.hint")}
                                </p>
                            ) : (
                                <button
                                    type="button"
                                    className={styles.regen}
                                    onClick={() => void generate()}
                                >
                                    {t("tools.notes.code.new")}
                                </button>
                            ))}

                        <div className={styles.sect}>{t("tools.notes.code.devices")}</div>
                        {devices.length === 0 && (
                            <div className={styles.none}>{t("tools.notes.code.none")}</div>
                        )}
                        {devices.map((d) => (
                            <div key={d.id} className={styles.dev}>
                                <Monitor size={13} />
                                <span className={styles.devName}>{d.label}</span>
                                {confirming === d.id ? (
                                    <button
                                        type="button"
                                        className={styles.confirm}
                                        onClick={() => void revoke(d.id)}
                                    >
                                        {t("tools.notes.code.revokeSure")}
                                    </button>
                                ) : (
                                    <button
                                        type="button"
                                        className={styles.revoke}
                                        onClick={() => setConfirming(d.id)}
                                    >
                                        {t("tools.notes.code.revoke")}
                                    </button>
                                )}
                            </div>
                        ))}
                    </div>,
                    document.body,
                )}
        </>
    );
}

/**
 * Shown instead of the notes UI on a device that is neither signed in nor
 * paired. The code is the only credential it needs.
 */
export function ClaimScreen({ onPaired }: { onPaired: () => void }) {
    const { t } = useT();
    const [value, setValue] = useState("");
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);

    // Accept whatever the user pastes: strip punctuation, upper-case, and
    // re-group so the field always reads back the way the code was shown.
    function onChange(raw: string) {
        const clean = raw
            .replace(/[^a-zA-Z0-9]/g, "")
            .toUpperCase()
            .slice(0, 8);
        setValue(clean.length > 4 ? `${clean.slice(0, 4)}-${clean.slice(4)}` : clean);
        setError(null);
    }

    async function submit() {
        const clean = value.replace(/-/g, "");
        if (clean.length !== 8 || busy) return;
        setBusy(true);
        setError(null);
        try {
            const label = navigator.userAgent.includes("Windows") ? "Windows" : "Device";
            await notesApi.pairClaim(clean, label);
            onPaired();
        } catch (e) {
            const msg = formatApiError(e);
            // The backend flags an unreachable server distinctly; anything
            // else is about the code itself and is already readable.
            setError(msg.includes("unreachable") ? t("tools.notes.claim.offline") : msg);
        } finally {
            setBusy(false);
        }
    }

    return (
        <div className={styles.claim}>
            <KeyRound size={30} />
            <h2>{t("tools.notes.claim.title")}</h2>
            <p>{t("tools.notes.claim.body")}</p>
            <input
                className={styles.claimInput}
                value={value}
                onChange={(e) => onChange(e.target.value)}
                onKeyDown={(e) => {
                    if (e.key === "Enter") void submit();
                }}
                placeholder="XXXX-XXXX"
                spellCheck={false}
                autoFocus
            />
            {error && <div className={styles.claimErr}>{error}</div>}
            <button
                type="button"
                className={styles.claimGo}
                disabled={value.replace(/-/g, "").length !== 8 || busy}
                onClick={() => void submit()}
            >
                {busy ? t("tools.notes.claim.busy") : t("tools.notes.claim.go")}
            </button>
        </div>
    );
}
