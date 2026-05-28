import { useCallback, useEffect, useRef } from "react";

/**
 * Debounced callback hook.
 *
 * Returns a stable function `call` that defers invoking the latest
 * version of `fn` until `delay` milliseconds have elapsed since the
 * last `call(...)`.
 *
 * Also returns:
 * - `flush()` — invoke immediately with the pending args (if any) and
 *   cancel the timer. Returns the result of `fn`.
 * - `cancel()` — drop the pending invocation without running.
 *
 * Why this shape: live-save needs to flush on host switch. A bare
 * lodash.debounce wouldn't expose pending state in a way that
 * survives React's render cycle ergonomically.
 */
export function useDebouncedCallback<TArgs extends unknown[]>(
    fn: (...args: TArgs) => void | Promise<void>,
    delay: number,
) {
    // Keep the latest fn in a ref so callers don't have to memoize it.
    const fnRef = useRef(fn);
    useEffect(() => {
        fnRef.current = fn;
    }, [fn]);

    const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
    const pendingArgs = useRef<TArgs | null>(null);

    const flush = useCallback(() => {
        if (timer.current !== null) {
            clearTimeout(timer.current);
            timer.current = null;
        }
        const args = pendingArgs.current;
        pendingArgs.current = null;
        if (args) {
            void fnRef.current(...args);
        }
    }, []);

    const cancel = useCallback(() => {
        if (timer.current !== null) {
            clearTimeout(timer.current);
            timer.current = null;
        }
        pendingArgs.current = null;
    }, []);

    const call = useCallback(
        (...args: TArgs) => {
            pendingArgs.current = args;
            if (timer.current !== null) clearTimeout(timer.current);
            timer.current = setTimeout(() => {
                timer.current = null;
                const a = pendingArgs.current;
                pendingArgs.current = null;
                if (a) void fnRef.current(...a);
            }, delay);
        },
        [delay],
    );

    // Best-effort: flush on unmount so changes aren't lost if the
    // component is torn down mid-debounce. If the caller wants to
    // discard instead, they should call cancel() before unmount.
    useEffect(() => {
        return () => {
            flush();
        };
    }, [flush]);

    return { call, flush, cancel };
}
