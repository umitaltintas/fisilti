import React, { useState } from "react";
import { useTranslation } from "react-i18next";
import { Dropdown } from "../ui/Dropdown";
import { SettingContainer } from "../ui/SettingContainer";
import {
  getThemePreference,
  setThemePreference,
  type ThemePreference,
} from "@/lib/utils/theme";

interface AppearanceSelectorProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

export const AppearanceSelector: React.FC<AppearanceSelectorProps> = ({
  descriptionMode = "inline",
  grouped = false,
}) => {
  const { t } = useTranslation();
  const [theme, setTheme] = useState<ThemePreference>(getThemePreference);

  const options = (["system", "light", "dark"] as const).map((value) => ({
    value,
    label: t(`settings.general.appearance.${value}`),
  }));

  return (
    <SettingContainer
      title={t("settings.general.appearance.title")}
      description={t("settings.general.appearance.description")}
      descriptionMode={descriptionMode}
      grouped={grouped}
    >
      <Dropdown
        options={options}
        selectedValue={theme}
        onSelect={(value) => {
          const next = value as ThemePreference;
          setTheme(next);
          setThemePreference(next);
        }}
      />
    </SettingContainer>
  );
};
