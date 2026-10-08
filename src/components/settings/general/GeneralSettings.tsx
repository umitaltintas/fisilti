import React from "react";
import { useTranslation } from "react-i18next";
import { type } from "@tauri-apps/plugin-os";
import { SettingsGroup } from "../../ui/SettingsGroup";
import { useSettings } from "../../../hooks/useSettings";
import { ShortcutInput } from "../ShortcutInput";
import { PushToTalk } from "../PushToTalk";
import { ShowOverlay } from "../ShowOverlay";
import { MicrophoneSelector } from "../MicrophoneSelector";
import { ClamshellMicrophoneSelector } from "../ClamshellMicrophoneSelector";
import { MuteWhileRecording } from "../MuteWhileRecording";
import { AudioFeedback } from "../AudioFeedback";
import { OutputDeviceSelector } from "../OutputDeviceSelector";
import { VolumeSlider } from "../VolumeSlider";
import { SoundPicker } from "../SoundPicker";
import { AppLanguageSelector } from "../AppLanguageSelector";
import { AutostartToggle } from "../AutostartToggle";
import { StartHidden } from "../StartHidden";
import { ShowTrayIcon } from "../ShowTrayIcon";
import { UpdateChecksToggle } from "../UpdateChecksToggle";

// The everyday page: how you record, what it sounds like, and how the app
// behaves. Model-specific options live on the Models page; anything you set
// once and forget lives under Advanced.
export const GeneralSettings: React.FC = () => {
  const { t } = useTranslation();
  const { audioFeedbackEnabled, getSetting } = useSettings();
  const pushToTalk = getSetting("push_to_talk");
  // Dynamic shortcut registration is unstable on Linux, so the cancel binding
  // stays hidden there.
  const isLinux = type() === "linux";

  return (
    <div className="mx-auto w-full max-w-3xl space-y-6">
      <SettingsGroup title={t("settings.general.groups.recording")}>
        <ShortcutInput shortcutId="transcribe" grouped={true} />
        {!isLinux && (
          <ShortcutInput
            shortcutId="cancel"
            grouped={true}
            disabled={pushToTalk}
          />
        )}
        <PushToTalk descriptionMode="tooltip" grouped={true} />
        <ShowOverlay descriptionMode="tooltip" grouped={true} />
      </SettingsGroup>

      <SettingsGroup title={t("settings.sound.title")}>
        <MicrophoneSelector descriptionMode="tooltip" grouped={true} />
        {/* Which microphone to use with the lid closed. Laptops only (the
            component renders nothing elsewhere); it lives next to the main
            microphone choice it overrides. */}
        <ClamshellMicrophoneSelector descriptionMode="tooltip" grouped={true} />
        <MuteWhileRecording descriptionMode="tooltip" grouped={true} />
        <AudioFeedback descriptionMode="tooltip" grouped={true} />
        <OutputDeviceSelector
          descriptionMode="tooltip"
          grouped={true}
          disabled={!audioFeedbackEnabled}
        />
        <SoundPicker
          label={t("settings.sound.soundTheme.label")}
          description={t("settings.sound.soundTheme.description")}
        />
        <VolumeSlider disabled={!audioFeedbackEnabled} />
      </SettingsGroup>

      <SettingsGroup title={t("settings.general.groups.app")}>
        <AppLanguageSelector descriptionMode="tooltip" grouped={true} />
        <AutostartToggle descriptionMode="tooltip" grouped={true} />
        <StartHidden descriptionMode="tooltip" grouped={true} />
        <ShowTrayIcon descriptionMode="tooltip" grouped={true} />
        <UpdateChecksToggle descriptionMode="tooltip" grouped={true} />
      </SettingsGroup>
    </div>
  );
};
