import { User } from "lucide-react";

import { useT } from "../../../i18n";
import { Placeholder } from "./Placeholder";

export function ProfileSection() {
    const { t } = useT();
    return (
        <Placeholder
            icon={<User size={32} />}
            title={t("settings.profile.placeholderTitle")}
            description={t("settings.profile.placeholderDescription")}
            roadmap="Stage 5"
        />
    );
}
