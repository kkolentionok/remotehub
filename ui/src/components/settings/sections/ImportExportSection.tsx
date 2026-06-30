import { useState } from "react";
import {
    AlertCircle,
    AlertTriangle,
    ArrowLeft,
    Check,
    Download,
    Eye,
    EyeOff,
    FileLock2,
    Folder,
    Info,
    KeyRound,
    Loader2,
    Lock,
    RotateCcw,
    Server,
    ShieldCheck,
    Trash2,
    Upload,
    X,
} from "lucide-react";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";

import { useT } from "../../../i18n";
import { vault } from "../../../lib/ipc";
import { formatApiError, type VaultImportResponse } from "../../../lib/types";
import {
    useCredentialsStore,
    useGroupsStore,
    useHostsStore,
} from "../../../store";
import dlg from "../SettingsDialog.module.css";
import s from "./ImportExportSection.module.css";

/** A vault file staged for import: its display name, byte size, and decrypted-
 *  on-the-backend body text. Acquired via the native Open dialog (path → Rust
 *  read) or drag-and-drop (File → text). */
type StagedFile = { name: string; size: number; body: string };

const VAULT_FILTERS = [{ name: "Pingie Vault", extensions: ["rvault"] }];

/**
 * Settings -> Import / Export. Two separated flows behind a toggle:
 *   - Export: you *set* a master password (strength + confirm), seal the local
 *     store (Argon2id + AES-256-GCM) and download a .rvault file.
 *   - Import: pick a file, *enter* its password, choose Merge or Replace.
 *     Replace wipes the local store, so it asks for explicit confirmation (the
 *     backend decrypts before deleting, so a wrong password is harmless).
 */
export function ImportExportSection() {
    const { t } = useT();
    const [tab, setTab] = useState<"export" | "import">("export");

    return (
        <div className={dlg.section}>
            <div className={dlg.sectionTitle}>{t("settings.io.title")}</div>
            <p className={dlg.sectionDescription}>{t("settings.io.lead")}</p>

            <div className={s.tabs} role="tablist">
                <button
                    role="tab"
                    aria-selected={tab === "export"}
                    className={`${s.tab} ${tab === "export" ? s.tabOn : ""}`}
                    onClick={() => setTab("export")}
                >
                    <Upload size={15} /> {t("settings.io.export")}
                </button>
                <button
                    role="tab"
                    aria-selected={tab === "import"}
                    className={`${s.tab} ${tab === "import" ? s.tabOn : ""}`}
                    onClick={() => setTab("import")}
                >
                    <Download size={15} /> {t("settings.io.import")}
                </button>
            </div>

            {tab === "export" ? <ExportFlow /> : <ImportFlow />}
        </div>
    );
}

/* shared password input with eye toggle */
function PwInput({
    value,
    onChange,
    placeholder,
    state,
    onEnter,
    autoFocus,
}: {
    value: string;
    onChange: (v: string) => void;
    placeholder: string;
    state?: "err" | "ok" | "";
    onEnter?: () => void;
    autoFocus?: boolean;
}) {
    const { t } = useT();
    const [show, setShow] = useState(false);
    return (
        <div className={s.pw}>
            <input
                className={`${s.input} ${s.mono} ${state === "err" ? s.inputErr : state === "ok" ? s.inputOk : ""}`}
                type={show ? "text" : "password"}
                value={value}
                onChange={(e) => onChange(e.target.value)}
                placeholder={placeholder}
                autoFocus={autoFocus}
                spellCheck={false}
                autoComplete="new-password"
                onKeyDown={(e) => {
                    if (e.key === "Enter" && onEnter) onEnter();
                }}
            />
            <button
                type="button"
                tabIndex={-1}
                className={s.pwEye}
                onClick={() => setShow((v) => !v)}
                title={show ? t("common.hide") : t("common.show")}
            >
                {show ? <EyeOff size={15} /> : <Eye size={15} />}
            </button>
        </div>
    );
}

const STRENGTH_LABELS = ["weakest", "weak", "fair", "good", "strong"] as const;
type StrengthLabel = (typeof STRENGTH_LABELS)[number];
interface Strength {
    score: number;
    label: StrengthLabel | "";
    color: string;
}
function strength(pw: string): Strength {
    if (!pw) return { score: 0, label: "", color: "var(--text-4)" };
    let n = 0;
    if (pw.length >= 8) n++;
    if (pw.length >= 12) n++;
    if (/[0-9]/.test(pw) && /[a-zа-я]/i.test(pw)) n++;
    if (/[^A-Za-zА-Яа-я0-9]/.test(pw)) n++;
    n = Math.min(n, 4);
    const colors = [
        "var(--color-danger)",
        "var(--color-danger)",
        "var(--color-warn)",
        "var(--color-warn)",
        "var(--color-success)",
    ];
    return { score: n, label: STRENGTH_LABELS[n] ?? "", color: colors[n] ?? "var(--text-4)" };
}

const STRENGTH_KEY = {
    weakest: "settings.io.strength.weakest",
    weak: "settings.io.strength.weak",
    fair: "settings.io.strength.fair",
    good: "settings.io.strength.good",
    strong: "settings.io.strength.strong",
} as const;

/* EXPORT */
function ExportFlow() {
    const { t } = useT();
    const [phase, setPhase] = useState<"form" | "working" | "done">("form");
    const [pw, setPw] = useState("");
    const [pw2, setPw2] = useState("");
    const [error, setError] = useState<string | null>(null);
    const [fname, setFname] = useState("");

    const st = strength(pw);
    const match = pw2.length > 0 && pw === pw2;
    const ready = st.score >= 2 && match;

    async function run() {
        if (!ready || phase === "working") return;
        setPhase("working");
        setError(null);
        try {
            const body = await vault.export(pw);
            const name = `rhub-vault-${new Date().toISOString().slice(0, 10)}.rvault`;
            const path = await saveDialog({ defaultPath: name, filters: VAULT_FILTERS });
            if (!path) {
                // User dismissed the native Save dialog — no error, back to form.
                setPhase("form");
                return;
            }
            await vault.writeFile(path, body);
            setFname(path.split(/[\\/]/).pop() ?? name);
            setPhase("done");
        } catch (e: unknown) {
            setError(formatApiError(e));
            setPhase("form");
        }
    }
    function reset() {
        setPw("");
        setPw2("");
        setError(null);
        setPhase("form");
    }

    if (phase === "done") {
        return (
            <div className={s.flow}>
                <div className={s.done}>
                    <div className={s.doneBadge}>
                        <ShieldCheck size={32} />
                    </div>
                    <div className={s.doneT}>{t("settings.io.exportDoneTitle")}</div>
                    <div className={s.doneS}>{t("settings.io.exportDoneText")}</div>
                    <div className={s.doneCard}>
                        <span className={s.doneCardIc}>
                            <FileLock2 size={20} />
                        </span>
                        <div style={{ flex: 1, minWidth: 0 }}>
                            <div className={s.doneFn}>{fname}</div>
                            <div className={s.doneFp}>Argon2id + AES-256-GCM</div>
                        </div>
                    </div>
                    <div className={s.doneActions}>
                        <button className={`${s.btn} ${s.btnGhost}`} onClick={reset}>
                            <RotateCcw size={15} /> {t("settings.io.exportAgain")}
                        </button>
                    </div>
                </div>
            </div>
        );
    }

    return (
        <div className={s.flow}>
            <div className={s.field}>
                <div className={s.fieldL}>
                    <Lock size={12} /> {t("settings.io.masterPassword")}
                </div>
                <PwInput
                    value={pw}
                    onChange={(v) => {
                        setPw(v);
                        setError(null);
                    }}
                    placeholder={t("settings.io.exportPwPlaceholder")}
                    autoFocus
                    state={pw && st.score < 2 ? "err" : pw && st.score >= 2 ? "ok" : ""}
                />
                <div className={s.str}>
                    <div className={s.strBars}>
                        {[0, 1, 2, 3].map((i) => (
                            <i
                                key={i}
                                style={{ background: i < st.score ? st.color : "var(--color-subtle)" }}
                            />
                        ))}
                    </div>
                    {pw && st.label && (
                        <span className={s.strLbl} style={{ color: st.color }}>
                            {t(STRENGTH_KEY[st.label])}
                        </span>
                    )}
                </div>
            </div>

            <div className={s.field}>
                <div className={s.fieldL}>
                    <Lock size={12} /> {t("settings.io.repeatPassword")}
                </div>
                <PwInput
                    value={pw2}
                    onChange={setPw2}
                    placeholder={t("settings.io.repeatPlaceholder")}
                    onEnter={run}
                    state={pw2 ? (match ? "ok" : "err") : ""}
                />
                {pw2 && (
                    <div className={`${s.match} ${match ? s.matchOk : s.matchNo}`}>
                        {match ? <Check size={13} /> : <X size={13} />}{" "}
                        {match ? t("settings.io.pwMatch") : t("settings.io.pwNoMatch")}
                    </div>
                )}
            </div>

            <div className={`${s.note} ${s.noteDanger}`}>
                <span className={s.noteIc}>
                    <AlertTriangle size={15} />
                </span>
                <span>{t("settings.io.exportWarning")}</span>
            </div>

            {error && <div className={dlg.errorBox}>{error}</div>}

            <div className={s.actions}>
                <button
                    className={`${s.btn} ${s.btnPrimary}`}
                    disabled={!ready || phase === "working"}
                    onClick={run}
                >
                    {phase === "working" ? (
                        <>
                            <Loader2 size={16} className={s.spin} /> {t("settings.io.encrypting")}
                        </>
                    ) : (
                        <>
                            <ShieldCheck size={16} /> {t("settings.io.encryptSave")}
                        </>
                    )}
                </button>
            </div>
        </div>
    );
}

/* IMPORT */
function ImportFlow() {
    const { t } = useT();
    const [phase, setPhase] = useState<
        "pick" | "unlock" | "confirm" | "working" | "error" | "done"
    >("pick");
    const [pw, setPw] = useState("");
    const [over, setOver] = useState(false);
    const [strategy, setStrategy] = useState<"merge" | "replace">("merge");
    const [file, setFile] = useState<StagedFile | null>(null);
    const [result, setResult] = useState<VaultImportResponse | null>(null);
    const [error, setError] = useState<string | null>(null);

    const localCount = useHostsStore((store) => store.items.length);

    function onFile(f: StagedFile) {
        setFile(f);
        setPw("");
        setError(null);
        setPhase("unlock");
    }
    /** Native Open dialog → read the chosen file's text on the backend. */
    async function pickNative() {
        try {
            const path = await openDialog({ multiple: false, filters: VAULT_FILTERS });
            if (!path || Array.isArray(path)) return;
            const f = await vault.readFile(path);
            onFile({ name: f.name, size: f.size, body: f.body });
        } catch (e: unknown) {
            setError(formatApiError(e));
        }
    }
    /** Drag-and-drop: read the dropped File's text directly in the webview. */
    async function onDropFile(f: File) {
        try {
            const body = await f.text();
            onFile({ name: f.name, size: f.size, body });
        } catch (e: unknown) {
            setError(formatApiError(e));
        }
    }
    function removeFile() {
        setFile(null);
        setPw("");
        setError(null);
        setPhase("pick");
    }
    function onPrimary() {
        if (!pw) return;
        if (strategy === "replace") setPhase("confirm");
        else void run("merge");
    }
    async function run(mode: "merge" | "replace") {
        if (!file) return;
        setPhase("working");
        setError(null);
        try {
            const res = await vault.import(pw, file.body, mode);
            await Promise.all([
                useHostsStore.getState().load(),
                useGroupsStore.getState().load(),
                useCredentialsStore.getState().load(),
            ]);
            setResult(res);
            setPhase("done");
        } catch (e: unknown) {
            setError(formatApiError(e));
            setPhase("error");
        }
    }

    if (phase === "confirm") {
        return (
            <div className={s.flow}>
                <FileCard file={file} />
                <div className={`${s.note} ${s.noteDanger}`}>
                    <span className={s.noteIc}>
                        <AlertTriangle size={15} />
                    </span>
                    <span>{t("settings.io.replaceConfirm", { n: String(localCount) })}</span>
                </div>
                <div className={s.actions}>
                    <button className={`${s.btn} ${s.btnGhost}`} onClick={() => setPhase("unlock")}>
                        <ArrowLeft size={15} /> {t("common.cancel")}
                    </button>
                    <button className={`${s.btn} ${s.btnDanger}`} onClick={() => void run("replace")}>
                        <Trash2 size={15} /> {t("settings.io.replaceConfirmBtn")}
                    </button>
                </div>
            </div>
        );
    }

    if (phase === "done") {
        const replace = strategy === "replace";
        return (
            <div className={s.flow}>
                <div className={s.done}>
                    <div className={s.doneBadge}>
                        <Check size={34} />
                    </div>
                    <div className={s.doneT}>
                        {replace ? t("settings.io.replaceDoneTitle") : t("settings.io.importDoneTitle")}
                    </div>
                    <div className={s.doneS}>
                        {replace ? t("settings.io.replaceDoneText") : t("settings.io.importDoneText")}
                    </div>
                    <div className={s.prev}>
                        <div className={s.prevRow}>
                            <span className={s.prevIc}>
                                <Server size={15} />
                            </span>
                            <span className={s.prevNm}>{t("settings.io.rowHosts")}</span>
                            <span className={s.prevN}>{result?.hosts ?? 0}</span>
                        </div>
                        <div className={s.prevRow}>
                            <span className={s.prevIc}>
                                <Folder size={15} />
                            </span>
                            <span className={s.prevNm}>{t("settings.io.rowGroups")}</span>
                            <span className={s.prevN}>{result?.groups ?? 0}</span>
                        </div>
                        <div className={s.prevRow}>
                            <span className={s.prevIc}>
                                <KeyRound size={15} />
                            </span>
                            <span className={s.prevNm}>{t("settings.io.rowCreds")}</span>
                            <span className={s.prevN}>{result?.credentials ?? 0}</span>
                        </div>
                    </div>
                    <div className={s.doneActions}>
                        <button className={`${s.btn} ${s.btnPrimary}`} onClick={removeFile}>
                            <Check size={16} /> {t("common.done")}
                        </button>
                    </div>
                </div>
            </div>
        );
    }

    return (
        <div className={s.flow}>
            {phase === "pick" ? (
                <>
                    <div
                        className={`${s.drop} ${over ? s.dropOver : ""}`}
                        onClick={() => void pickNative()}
                        onDragOver={(e) => {
                            e.preventDefault();
                            setOver(true);
                        }}
                        onDragLeave={() => setOver(false)}
                        onDrop={(e) => {
                            e.preventDefault();
                            setOver(false);
                            const f = e.dataTransfer.files?.[0];
                            if (f) void onDropFile(f);
                        }}
                    >
                        <span className={s.dropIc}>
                            <Upload size={22} />
                        </span>
                        <div className={s.dropT}>{t("settings.io.dropTitle")}</div>
                        <div className={s.dropS}>.rvault</div>
                    </div>
                    <div className={s.note}>
                        <span className={s.noteIc}>
                            <Info size={15} />
                        </span>
                        <span>{t("settings.io.pickHint")}</span>
                    </div>
                </>
            ) : (
                <>
                    <FileCard file={file} onRemove={removeFile} />

                    <div className={s.field}>
                        <div className={s.fieldL}>
                            <Lock size={12} /> {t("settings.io.filePassword")}
                        </div>
                        <div className={phase === "error" ? s.shake : ""}>
                            <PwInput
                                value={pw}
                                onChange={(v) => {
                                    setPw(v);
                                    if (phase === "error") setPhase("unlock");
                                }}
                                placeholder={t("settings.io.filePwPlaceholder")}
                                autoFocus
                                onEnter={onPrimary}
                                state={phase === "error" ? "err" : ""}
                            />
                        </div>
                        {phase === "error" && (
                            <div className={`${s.match} ${s.matchNo}`} style={{ color: "var(--color-danger)" }}>
                                <AlertCircle size={13} /> {error ?? t("settings.io.wrongPassword")}
                            </div>
                        )}
                    </div>

                    <div className={s.fieldL} style={{ marginTop: 4 }}>
                        {t("settings.io.howToImport")}
                    </div>
                    <div className={s.radio}>
                        <button
                            className={`${s.opt} ${strategy === "merge" ? s.optOn : ""}`}
                            onClick={() => setStrategy("merge")}
                        >
                            <span className={s.optRd} />
                            <span className={s.optTx}>
                                <span className={s.optT}>{t("settings.io.mergeTitle")}</span>
                                <span className={s.optS}>{t("settings.io.mergeDesc")}</span>
                            </span>
                        </button>
                        <button
                            className={`${s.opt} ${strategy === "replace" ? s.optOn : ""}`}
                            onClick={() => setStrategy("replace")}
                        >
                            <span className={s.optRd} />
                            <span className={s.optTx}>
                                <span className={s.optT}>{t("settings.io.replaceTitle")}</span>
                                <span className={s.optS}>{t("settings.io.replaceDesc")}</span>
                            </span>
                        </button>
                    </div>

                    <div className={s.actions}>
                        <button
                            className={`${s.btn} ${s.btnPrimary}`}
                            disabled={!pw || phase === "working"}
                            onClick={onPrimary}
                        >
                            {phase === "working" ? (
                                <>
                                    <Loader2 size={16} className={s.spin} /> {t("settings.io.decrypting")}
                                </>
                            ) : strategy === "replace" ? (
                                <>
                                    <Lock size={15} /> {t("settings.io.decryptReplace")}
                                </>
                            ) : (
                                <>
                                    <Lock size={15} /> {t("settings.io.decryptImport")}
                                </>
                            )}
                        </button>
                        <button className={`${s.btn} ${s.btnGhost}`} onClick={removeFile}>
                            {t("common.back")}
                        </button>
                    </div>
                </>
            )}
        </div>
    );
}

function FileCard({ file, onRemove }: { file: StagedFile | null; onRemove?: () => void }) {
    const { t } = useT();
    const kb = file ? `${(file.size / 1024).toFixed(1)} ${t("settings.io.kb")}` : "";
    return (
        <div className={s.file}>
            <span className={s.fileIc}>
                <FileLock2 size={20} />
            </span>
            <div className={s.fileM}>
                <div className={s.fileN}>{file?.name ?? ""}</div>
                <div className={s.fileS}>
                    {kb} ·{" "}
                    <span className={s.lockPill}>
                        <Lock size={10} /> {t("settings.io.encrypted")}
                    </span>
                </div>
            </div>
            {onRemove && (
                <button className={s.fileX} title={t("settings.io.removeFile")} onClick={onRemove}>
                    <X size={16} />
                </button>
            )}
        </div>
    );
}
