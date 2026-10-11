import React from "react";
import { useTranslation } from "react-i18next";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { useSettings } from "../../hooks/useSettings";
import { useTauriEvent } from "../../hooks/useTauriEvent";

interface ShowMascotProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

export const ShowMascot: React.FC<ShowMascotProps> = React.memo(
  ({ descriptionMode = "inline", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating, refreshSettings } =
      useSettings();

    // The mascot can hide itself from its own menu.
    useTauriEvent<boolean>("mascot-visibility-changed", () => {
      void refreshSettings();
    });

    return (
      <ToggleSwitch
        checked={getSetting("show_mascot") ?? false}
        onChange={(enabled) => updateSetting("show_mascot", enabled)}
        isUpdating={isUpdating("show_mascot")}
        label={t("settings.advanced.showMascot.label")}
        description={t("settings.advanced.showMascot.description")}
        descriptionMode={descriptionMode}
        grouped={grouped}
        tooltipPosition="bottom"
      />
    );
  },
);
