/**
 * Localize backend sync/auth error messages.
 *
 * The sync backend returns free-form English strings (e.g. "invalid email or
 * password", "request failed: …", "unauthorized — log in again"). Showing them
 * raw leaks English into a Russian UI, so we match the known ones to translated
 * keys and fall back to the raw reason (without the noisy "field:" prefix) for
 * anything we don't recognize.
 */
import { type MessageKey } from "../i18n/en";
import { formatApiError, isApiError } from "./types";

type TFn = (key: MessageKey, vars?: Record<string, string | number>) => string;

export function localizeSyncError(t: TFn, err: unknown): string {
    let raw: string;
    if (isApiError(err)) {
        switch (err.kind) {
            case "validation":
                raw = err.reason;
                break;
            case "not_found":
                raw = `${err.entity} not found`;
                break;
            case "not_implemented":
                raw = err.feature;
                break;
            default:
                raw = (err as { message?: string }).message ?? "";
                break;
        }
    } else if (typeof err === "string") {
        raw = err;
    } else {
        raw = formatApiError(err);
    }

    const m = raw.toLowerCase();
    const has = (s: string) => m.includes(s);

    if (has("invalid email or password")) return t("settings.sync.err.invalidCredentials");
    if (has("email already registered")) return t("settings.sync.err.emailTaken");
    if (has("wrong vault password") || has("decrypt")) return t("settings.sync.err.wrongMaster");
    if (has("endpoint not set")) return t("settings.sync.err.noEndpoint");
    if (has("not logged in")) return t("settings.sync.err.notLoggedIn");
    if (has("unauthorized")) return t("settings.sync.err.unauthorized");
    if (has("could not open browser")) return t("settings.sync.err.browser");
    if (has("oauth") || has("no token in the") || has("callback")) {
        return t("settings.sync.err.oauth");
    }
    if (
        has("request failed") ||
        has("loopback") ||
        has("network") ||
        has("timed out") ||
        has("timeout") ||
        has("dns") ||
        has("connection") ||
        has("connect") ||
        has("unreachable")
    ) {
        return t("settings.sync.err.network");
    }
    if (
        has("server returned") ||
        has("http ") ||
        has(": http") ||
        has("not utf-8") ||
        has("decode") ||
        has("no token") ||
        has("no email")
    ) {
        return t("settings.sync.err.server");
    }

    return raw || t("settings.sync.err.generic");
}
