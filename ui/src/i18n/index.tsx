/**
 * Tiny i18n: dictionary-backed, no runtime parser, no third-party dep.
 *
 * Why not react-i18next or react-intl? At our scale (one app, two locales,
 * no plural-form complexity beyond a future {count}-string or two) the
 * overhead of a real i18n framework isn't justified. A typed dictionary +
 * `t(key, vars?)` covers 100% of current needs and stays under 60 lines.
 *
 * Switch is reactive via React state: `useLocale()` reads the current
 * locale, `setLocale(...)` swaps and re-renders everything that consumes
 * `useT()`. Persistence into Settings happens in Stage 1.6.
 */

import {
    createContext,
    useCallback,
    useContext,
    useEffect,
    useMemo,
    useState,
    type ReactNode,
} from "react";

import { messages as enMessages, type MessageKey } from "./en";
import { messages as ruMessages } from "./ru";

export type Locale = "en" | "ru";

// =====================================================================
// Translation function
// =====================================================================

/** Build a `t()` bound to a specific locale. */
function makeT(locale: Locale) {
    const primary = locale === "ru" ? ruMessages : enMessages;
    return (key: MessageKey, vars?: Record<string, string | number>): string => {
        // Fallback chain: primary locale → en → key itself.
        const raw =
            (primary as Record<string, string>)[key] ??
            (enMessages as Record<string, string>)[key] ??
            key;
        if (!vars) return raw;
        return raw.replace(/\{(\w+)\}/g, (m, name: string) => {
            const v = vars[name];
            return v !== undefined ? String(v) : m;
        });
    };
}

// =====================================================================
// Date formatting tied to the same locale.
// =====================================================================

/** Format a RFC3339 timestamp string for display. */
export function formatDateTime(rfc3339: string, locale: Locale): string {
    const d = new Date(rfc3339);
    if (Number.isNaN(d.getTime())) return rfc3339; // fallback for parse failures
    const bcp47 = locale === "ru" ? "ru-RU" : "en-GB";
    return new Intl.DateTimeFormat(bcp47, {
        day: "2-digit",
        month: locale === "ru" ? "2-digit" : "short",
        year: "numeric",
        hour: "2-digit",
        minute: "2-digit",
    }).format(d);
}

// =====================================================================
// Context
// =====================================================================

interface I18nContextValue {
    locale: Locale;
    setLocale: (locale: Locale) => void;
    t: (key: MessageKey, vars?: Record<string, string | number>) => string;
    formatDate: (rfc3339: string) => string;
}

const I18nContext = createContext<I18nContextValue | null>(null);

const STORAGE_KEY = "remotehub.locale";

function initialLocale(): Locale {
    if (typeof window === "undefined") return "en";
    const stored = window.localStorage.getItem(STORAGE_KEY);
    if (stored === "ru" || stored === "en") return stored;
    // Browser default: try navigator.language. Tauri returns the OS locale.
    const nav = navigator.language?.toLowerCase() ?? "";
    return nav.startsWith("ru") ? "ru" : "en";
}

export function I18nProvider({ children }: { children: ReactNode }) {
    const [locale, setLocaleState] = useState<Locale>(initialLocale);

    useEffect(() => {
        document.documentElement.setAttribute("lang", locale);
        window.localStorage.setItem(STORAGE_KEY, locale);
    }, [locale]);

    const setLocale = useCallback((next: Locale) => {
        setLocaleState(next);
    }, []);

    const value = useMemo<I18nContextValue>(() => {
        const t = makeT(locale);
        return {
            locale,
            setLocale,
            t,
            formatDate: (rfc3339) => formatDateTime(rfc3339, locale),
        };
    }, [locale, setLocale]);

    return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}

/** Read-only access. Use in any component. */
export function useT() {
    const ctx = useContext(I18nContext);
    if (!ctx) {
        throw new Error("useT must be used inside <I18nProvider>");
    }
    return ctx;
}
