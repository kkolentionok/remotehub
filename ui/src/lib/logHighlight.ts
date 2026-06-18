// Client-side log highlighting for the terminal.
//
// We wrap a curated set of recognised tokens (log levels, ok/fail words,
// IPv4[:port]) in ANSI SGR *before* the bytes reach xterm, so plain-text
// program output (e.g. `nginx -t`, build logs, journald) gets readable colour
// even though the program emitted no colour itself.
//
// Hard rules so we never corrupt a real terminal stream:
//  • Only plain-text runs are touched. Bytes inside an escape sequence are
//    emitted verbatim.
//  • If the program has an ACTIVE style (it set its own colour/attribute and
//    hasn't reset), we leave its text alone — never fight `ls --color`,
//    `grep --color`, `systemctl`, `git`, htop, vim, etc. They keep their look.
//  • UTF-8 is decoded with a streaming decoder, so a multibyte char split
//    across chunks is handled. An escape sequence split across chunks is
//    carried to the next chunk instead of being mis-parsed as text.

const RESET = "\x1b[0m";
const C = {
    red: "\x1b[91m",
    yellow: "\x1b[93m",
    green: "\x1b[92m",
    cyan: "\x1b[96m",
} as const;

// Curated whole-word rules (case-insensitive). Kept conservative on purpose —
// very common short words (up, on, pass, set…) are deliberately excluded to
// avoid colouring ordinary prose. Easy to extend later.
const WORD_COLOR = new Map<string, string>();
const addWords = (words: string[], color: string) => {
    for (const w of words) WORD_COLOR.set(w, color);
};
addWords(
    ["error", "errors", "err", "fail", "failed", "failure", "fatal",
     "critical", "crit", "panic", "denied", "refused", "unreachable",
     "timeout", "invalid", "corrupt", "corrupted", "fatalerror"],
    C.red,
);
addWords(
    ["warn", "warning", "warnings", "deprecated", "ignored", "conflicting",
     "skipped", "retry", "retrying"],
    C.yellow,
);
addWords(
    ["ok", "success", "successful", "succeeded", "done", "active", "running",
     "enabled", "listening", "ready", "started", "online", "healthy", "passed",
     "valid"],
    C.green,
);
addWords(
    ["info", "notice", "debug", "trace", "starting", "loading"],
    C.cyan,
);

// IPv4 (optional :port), or a word of 2+ letters. One pass over a text run.
const TOKEN_RE = /\b\d{1,3}(?:\.\d{1,3}){3}(?::\d{1,5})?\b|[A-Za-z]{2,}/g;

// Max length of a single plain-text run we will colourise. Bigger runs (bulk
// dumps, `cat` of a large file) pass through untouched so highlighting never
// becomes a throughput bottleneck.
const MAX_COLORIZE = 16384;

function colorizeText(s: string): string {
    return s.replace(TOKEN_RE, (tok) => {
        if (tok.charCodeAt(0) <= 57) {
            // Starts with a digit → IPv4[:port].
            return C.cyan + tok + RESET;
        }
        const col = WORD_COLOR.get(tok.toLowerCase());
        return col ? col + tok + RESET : tok;
    });
}

// A complete ANSI escape sequence: CSI, OSC, or a short two-char ESC.
const ESC_RE =
    /\x1b(?:\[[0-9;?]*[ -/]*[@-~]|\][\s\S]*?(?:\x07|\x1b\\)|[@-Z\\-_])/g;
// Same, anchored — used to test whether a trailing fragment is already a
// complete sequence (else it is carried to the next chunk).
const ESC_HEAD =
    /^\x1b(?:\[[0-9;?]*[ -/]*[@-~]|\][\s\S]*?(?:\x07|\x1b\\)|[@-Z\\-_])/;

// Update the "program has an active style" flag from a CSI SGR sequence.
// Only `\x1b[...m` matters; any other escape leaves the flag unchanged.
function applySgr(seq: string, styled: boolean): boolean {
    if (seq.length < 3 || seq[1] !== "[" || seq[seq.length - 1] !== "m") {
        return styled;
    }
    const params = seq.slice(2, -1);
    // A pure reset is empty (`\x1b[m`) or all-zero params (`0`, `00`, `0;0`…).
    // Anything else means the program set some active style.
    if (params === "" || /^0*(?:;0*)*$/.test(params)) return false;
    return true;
}

interface HiState {
    styled: boolean;
    carry: string;
    // True while a full-screen TUI owns the alternate screen buffer (vim, mc,
    // mcedit, htop, less…). We must NOT touch its output: injecting our SGR
    // resets would clobber its background fills and corrupt the layout.
    alt: boolean;
}

// Detect alternate-screen enter/leave (xterm private modes 1049/1047/47).
function applyAlt(seq: string, alt: boolean): boolean {
    const mm = /^\x1b\[\?(1049|1047|47)([hl])$/.exec(seq);
    if (!mm) return alt;
    return mm[2] === "h";
}

/** Create a per-terminal highlighter. Returns a function that takes a raw PTY
 *  chunk and returns the (possibly colourised) string to hand to xterm. */
export function createLogHighlighter(): (data: Uint8Array) => string {
    const decoder = new TextDecoder("utf-8");
    const state: HiState = { styled: false, carry: "", alt: false };

    const emitText = (text: string): string => {
        if (text.length === 0) return text;
        // Never touch a full-screen TUI's output.
        if (state.alt) return text;
        // Colour almost never persists across a line break — programs reset
        // their SGR at end of line. So treat each newline as a fresh, unstyled
        // start. This also self-heals if a program's colour region was cut off
        // without a reset (e.g. `systemctl status | head`) or reset in a form
        // we didn't track: the next line gets highlighting back instead of
        // staying dead forever.
        let out = "";
        let i = 0;
        while (i < text.length) {
            const nl = text.indexOf("\n", i);
            const end = nl === -1 ? text.length : nl + 1;
            const seg = text.slice(i, end);
            out +=
                state.styled || seg.length > MAX_COLORIZE
                    ? seg
                    : colorizeText(seg);
            if (nl !== -1) state.styled = false;
            i = end;
        }
        return out;
    };

    return (data: Uint8Array): string => {
        let work = state.carry + decoder.decode(data, { stream: true });
        state.carry = "";

        // Hold back a trailing incomplete escape sequence so we don't mistake
        // its fragment for text (and so SGR state stays correct).
        const lastEsc = work.lastIndexOf("\x1b");
        if (lastEsc !== -1 && !ESC_HEAD.test(work.slice(lastEsc))) {
            state.carry = work.slice(lastEsc);
            work = work.slice(0, lastEsc);
        }

        let out = "";
        let last = 0;
        ESC_RE.lastIndex = 0;
        let m: RegExpExecArray | null;
        while ((m = ESC_RE.exec(work)) !== null) {
            out += emitText(work.slice(last, m.index));
            out += m[0];
            state.styled = applySgr(m[0], state.styled);
            state.alt = applyAlt(m[0], state.alt);
            last = m.index + m[0].length;
        }
        out += emitText(work.slice(last));
        return out;
    };
}
