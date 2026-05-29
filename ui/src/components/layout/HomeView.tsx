import { Sidebar } from "../sidebar/Sidebar";
import { HostDetail } from "../host/HostDetail";
import { CommandBar } from "./CommandBar";
import styles from "./HomeView.module.css";

/**
 * The pinned "Vault" tab: the host manager. Search bar on top, host
 * tree on the left, the selected host's editor on the right. This is
 * the home base; sessions open as sibling tabs in the TabBar.
 */
export function HomeView() {
    return (
        <div className={styles.home}>
            <CommandBar />
            <div className={styles.body}>
                <Sidebar />
                <HostDetail />
            </div>
        </div>
    );
}
