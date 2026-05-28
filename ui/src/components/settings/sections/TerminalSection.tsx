import { Terminal } from "lucide-react";

import { useT } from "../../../i18n";
import { Placeholder } from "./Placeholder";

export function TerminalSection() {
    const { t } = useT();
    return (
        <Placeholder
            icon={<Terminal size={32} />}
            title={t("settings.terminal.placeholderTitle")}
            description={t("settings.terminal.placeholderDescription")}
            roadmap="Stage 2"
        />
    );
}
