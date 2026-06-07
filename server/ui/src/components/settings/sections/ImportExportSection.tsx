import { ArrowDownToLine } from "lucide-react";

import { useT } from "../../../i18n";
import { Placeholder } from "./Placeholder";

export function ImportExportSection() {
    const { t } = useT();
    return (
        <Placeholder
            icon={<ArrowDownToLine size={32} />}
            title={t("settings.importExport.placeholderTitle")}
            description={t("settings.importExport.placeholderDescription")}
            roadmap="Stage 1.10 / 1.12"
        />
    );
}
