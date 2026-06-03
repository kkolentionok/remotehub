import { AppShell } from "./components/layout/AppShell";
import { RdpPopoutApp } from "./components/session/RdpPopoutApp";

/**
 * Root component. Stage 1.5: AppShell with real sidebar + host detail
 * + CRUD dialogs for hosts / groups / credentials.
 *
 * A secondary window opened for a popped-out RDP session loads the same
 * bundle with a `#popout…` hash — render just that session there.
 */
export function App() {
    if (typeof window !== "undefined" && window.location.hash.startsWith("#popout")) {
        return <RdpPopoutApp />;
    }
    return <AppShell />;
}
