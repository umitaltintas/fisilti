import React from "react";
import { useTranslation } from "react-i18next";
import { IconButton } from "../ui/IconButton";
import { Dropdown, DropdownOption } from "../ui/Dropdown";
import { PlayIcon } from "lucide-react";
import { SettingContainer } from "../ui/SettingContainer";
import { useSettingsStore } from "../../stores/settingsStore";
import { useSettings } from "../../hooks/useSettings";

interface SoundPickerProps {
  label: string;
  description: string;
}

export const SoundPicker: React.FC<SoundPickerProps> = ({
  label,
  description,
}) => {
  const { t } = useTranslation();
  const { getSetting, updateSetting } = useSettings();
  const playTestSound = useSettingsStore((state) => state.playTestSound);
  const customSounds = useSettingsStore((state) => state.customSounds);

  const selectedTheme = getSetting("sound_theme") ?? "marimba";

  const options: DropdownOption[] = [
    { value: "marimba", label: "Marimba" },
    { value: "pop", label: "Pop" },
  ];

  // Only add Custom option if both custom sound files exist
  if (customSounds.start && customSounds.stop) {
    options.push({
      value: "custom",
      label: t("settings.sound.soundTheme.custom"),
    });
  }

  const handlePlayBothSounds = async () => {
    await playTestSound("start");
    await playTestSound("stop");
  };

  return (
    <SettingContainer
      title={label}
      description={description}
      grouped
      layout="horizontal"
    >
      <div className="flex items-center gap-2">
        <Dropdown
          selectedValue={selectedTheme}
          onSelect={(value) =>
            void updateSetting(
              "sound_theme",
              value as "marimba" | "pop" | "custom",
            )
          }
          options={options}
        />
        <IconButton
          onClick={() => void handlePlayBothSounds()}
          label={t("settings.sound.soundTheme.preview")}
        >
          <PlayIcon className="h-4 w-4" />
        </IconButton>
      </div>
    </SettingContainer>
  );
};
