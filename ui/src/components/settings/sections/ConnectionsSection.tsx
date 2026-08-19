import { useEffect, useState } from "react";

import { useT } from "../../../i18n";
import { useSettingsStore } from "../../../store";
import { useDebouncedCallback } from "../../../lib/useDebouncedCallback";
import type { Settings } from "../../../lib/types";
import { HotkeyRecorder } from "./HotkeyRecorder";
import styles from "../SettingsDialog.module.css";

interface Props {
    settings: Settings;
}

/**
 * Default ports for new SSH/RDP hosts. Live-save with a short debounce
 * so the user can type a 4-digit port without 4 IPC calls.
 *
 * We use local form state mirrored from settings so the input doesn't
 * fight the user mid-typing. On blur (or after debounce), the value
 * is clamped to 1..65535 and pushed to the backend.
 */
export function ConnectionsSection({ settings }: Props) {
    const { t } = useT();
    const update = useSettingsStore((s) => s.update);

    const [ssh, setSsh] = useState(String(settings.default_ssh_port));
    const [rdp, setRdp] = useState(String(settings.default_rdp_port));

    // Sync local form when the settings change from outside (event,
    // other window, etc.). Compares to current local string to avoid
    // overwriting in-progress typing.
    useEffect(() => {
        setSsh((v) => (Number(v) === settings.default_ssh_port ? v : String(settings.default_ssh_port)));
        setRdp((v) => (Number(v) === settings.default_rdp_port ? v : String(settings.default_rdp_port)));
    }, [settings.default_ssh_port, settings.default_rdp_port]);

    const commit = useDebouncedCallback(
        async (key: string, raw: string) => {
            const n = Number(raw);
            if (!Number.isInteger(n) || n < 1 || n > 65535) return; // silent skip
            await update({ [key]: n });
        },
        400,
    );

    return (
        <div className={styles.section}>
            <h3 className={styles.sectionTitle}>{t("settings.connections.title")}</h3>
            <p className={styles.sectionDescription}>
                {t("settings.connections.description")}
            </p>

            <div className={styles.field}>
                <label className={styles.fieldLabel}>
                    {t("settings.connections.defaultSshPort")}
                </label>
                <input
                    type="number"
                    min={1}
                    max={65535}
                    value={ssh}
                    onChange={(e) => {
                        setSsh(e.target.value);
                        commit.call("default_ssh_port", e.target.value);
                    }}
                    className={styles.numInput}
                />
            </div>

            <div className={styles.field}>
                <label className={styles.fieldLabel}>
                    {t("settings.connections.defaultRdpPort")}
                </label>
                <input
                    type="number"
                    min={1}
                    max={65535}
                    value={rdp}
                    onChange={(e) => {
                        setRdp(e.target.value);
                        commit.call("default_rdp_port", e.target.value);
                    }}
                    className={styles.numInput}
                />
            </div>

            <div className={styles.field}>
                <label className={styles.checkboxRow}>
                    <input
                        type="checkbox"
                        checked={settings.rdp_gfx}
                        onChange={(e) => void update({ rdp_gfx: e.target.checked })}
                    />
                    <span>{t("settings.connections.rdpGfx")}</span>
                </label>
                <div className={styles.fieldHint}>
                    {t("settings.connections.rdpGfxHint")}
                </div>
            </div>

            <HotkeyRecorder />
        </div>
    );
}
