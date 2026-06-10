import { AppShell } from "./components/layout/AppShell";
import { RdpPopoutApp } from "./components/session/RdpPopoutApp";
import { TermPopoutApp } from "./components/session/TermPopoutApp";

/**
 * Root component. Stage 1.5: AppShell with real sidebar + host detail
 * + CRUD dialogs for hosts / groups / credentials.
 *
 * A secondary window opened for a popped-out session loads the same bundle
 * with a `#popout…` hash — render just that session there. `#popout-term`
 * (SSH / local terminal) is checked before the RDP `#popout` prefix.
 */
export function App() {
    if (typeof window !== "undefined") {
        const hash = window.location.hash;
        if (hash.startsWith("#popout-term")) return <TermPopoutApp />;
        if (hash.startsWith("#popout")) return <RdpPopoutApp />;
    }
    return <AppShell />;
}
