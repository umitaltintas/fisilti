import React from "react";
import { useTranslation } from "react-i18next";
import { useSettingsStore } from "@/stores/settingsStore";
import type { ModelUnloadTimeout } from "@/bindings";
import { Dropdown } from "../ui/Dropdown";
import { SettingContainer } from "../ui/SettingContainer";

interface ModelUnloadTimeoutProps {
  descriptionMode?: "tooltip" | "inline";
  grouped?: boolean;
}

// Values are what serde sends (`rename_all = "snake_case"` turns `Min2` into
// `min2`); the generated binding type spells them `min_2`, so they are cast.
const OPTIONS: { value: string; labelKey: string }[] = [
  { value: "never", labelKey: "never" },
  { value: "immediately", labelKey: "immediately" },
  { value: "min2", labelKey: "min2" },
  { value: "min5", labelKey: "min5" },
  { value: "min10", labelKey: "min10" },
  { value: "min15", labelKey: "min15" },
  { value: "hour1", labelKey: "hour1" },
];
const DEBUG_OPTIONS: typeof OPTIONS = [
  ...OPTIONS,
  { value: "sec15", labelKey: "sec15" },
];

export const ModelUnloadTimeoutSetting: React.FC<ModelUnloadTimeoutProps> = ({
  descriptionMode = "inline",
  grouped = false,
}) => {
  const { t } = useTranslation();
  const debugMode = useSettingsStore((s) => s.settings?.debug_mode === true);
  const currentValue = useSettingsStore(
    (s) => s.settings?.model_unload_timeout ?? "never",
  );
  const updateSetting = useSettingsStore((s) => s.updateSetting);
  const updating = useSettingsStore(
    (s) => s.isUpdating["model_unload_timeout"] === true,
  );

  const options = (debugMode ? DEBUG_OPTIONS : OPTIONS).map((option) => ({
    value: option.value,
    label: t(`settings.advanced.modelUnload.options.${option.labelKey}`),
  }));

  return (
    <SettingContainer
      title={t("settings.advanced.modelUnload.title")}
      description={t("settings.advanced.modelUnload.description")}
      descriptionMode={descriptionMode}
      grouped={grouped}
    >
      <Dropdown
        options={options}
        selectedValue={currentValue}
        onSelect={(value) =>
          void updateSetting(
            "model_unload_timeout",
            value as ModelUnloadTimeout,
          )
        }
        disabled={updating}
      />
    </SettingContainer>
  );
};
