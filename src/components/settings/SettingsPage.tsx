import React from "react";
import { useTranslation } from "react-i18next";
import { SegmentedControl } from "../ui/SegmentedControl";
import { GeneralSettings } from "./general/GeneralSettings";
import { ModelsSettings } from "./models/ModelsSettings";
import { PostProcessingSettings } from "./post-processing/PostProcessingSettings";

export type SettingsTab = "general" | "models" | "ai";

interface SettingsPageProps {
  tab: SettingsTab;
  onTabChange: (tab: SettingsTab) => void;
}

/** All configuration, in three tabs, the way a macOS Settings window is laid
 * out. The sidebar holds only content (meetings, dictations). */
export const SettingsPage: React.FC<SettingsPageProps> = ({
  tab,
  onTabChange,
}) => {
  const { t } = useTranslation();

  return (
    <div className="mx-auto w-full max-w-2xl space-y-6">
      <header className="flex flex-col items-center gap-4">
        <h1 className="text-[22px] font-semibold tracking-tight">
          {t("sidebar.settings")}
        </h1>
        <SegmentedControl
          label={t("sidebar.settings")}
          value={tab}
          onChange={onTabChange}
          segments={[
            { id: "general", label: t("sidebar.general") },
            { id: "models", label: t("sidebar.models") },
            { id: "ai", label: t("sidebar.postProcessing") },
          ]}
        />
      </header>
      <div role="tabpanel">
        {tab === "general" && <GeneralSettings />}
        {tab === "models" && <ModelsSettings />}
        {tab === "ai" && <PostProcessingSettings />}
      </div>
    </div>
  );
};
