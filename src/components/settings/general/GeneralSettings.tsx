import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { type } from "@tauri-apps/plugin-os";
import { getVersion } from "@tauri-apps/api/app";
import { openUrl } from "@tauri-apps/plugin-opener";
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
import { AppearanceSelector } from "../AppearanceSelector";
import { AutostartToggle } from "../AutostartToggle";
import { StartHidden } from "../StartHidden";
import { ShowTrayIcon } from "../ShowTrayIcon";
import { ShowMascot } from "../ShowMascot";
import { UpdateChecksToggle } from "../UpdateChecksToggle";
import { PasteMethodSetting } from "../PasteMethod";
import { TypingToolSetting } from "../TypingTool";
import { ClipboardHandlingSetting } from "../ClipboardHandling";
import { AutoSubmit } from "../AutoSubmit";
import { AppendTrailingSpace } from "../AppendTrailingSpace";
import { CustomWords } from "../CustomWords";
import { ModelUnloadTimeoutSetting } from "../ModelUnloadTimeout";
import { AccelerationSelector } from "../AccelerationSelector";
import { LazyStreamClose } from "../LazyStreamClose";
import { AlwaysOnMicrophone } from "../AlwaysOnMicrophone";
import { AppDataDirectory } from "../AppDataDirectory";
import { HistoryLimit } from "../HistoryLimit";
import { RecordingRetentionPeriodSelector } from "../RecordingRetentionPeriod";
import { KeyboardImplementationSelector } from "../debug/KeyboardImplementationSelector";
import { WordCorrectionThreshold } from "../debug/WordCorrectionThreshold";
import { LogLevelSelector } from "../debug/LogLevelSelector";
import { LogDirectory } from "../debug/LogDirectory";
import { PasteDelay } from "../debug/PasteDelay";
import { RecordingBuffer } from "../debug/RecordingBuffer";
import { MeetingPreferences } from "../meeting/MeetingPreferences";

const REPO_URL = "https://github.com/umitaltintas/fisilti";

/** Version and source link, as a quiet footer instead of an About page. */
const AboutFooter: React.FC = () => {
  const { t } = useTranslation();
  const [version, setVersion] = useState("");
  useEffect(() => {
    getVersion()
      .then(setVersion)
      .catch(() => setVersion(""));
  }, []);

  return (
    <footer className="flex flex-col items-center gap-1 pt-2 text-xs text-faint">
      {/* eslint-disable-next-line i18next/no-literal-string -- brand name */}
      <span>Fısıltı {version && `${version}`}</span>
      <button
        type="button"
        onClick={() => void openUrl(REPO_URL)}
        className="cursor-pointer rounded text-brand-text hover:underline focus:outline-none focus-visible:ring-2 focus-visible:ring-logo-primary/50"
      >
        {t("settings.about.sourceCode.button")}
      </button>
    </footer>
  );
};

// Everything most people set once: how dictation is triggered, the meeting
// switches and how the app behaves. Rarely touched options are not shown;
// they keep their defaults and appear only in the Developer group
// (Ctrl/Cmd+Shift+D) so nothing is out of reach.
export const GeneralSettings: React.FC = () => {
  const { t } = useTranslation();
  const { audioFeedbackEnabled: audioFeedback, getSetting } = useSettings();
  const pushToTalk = getSetting("push_to_talk");
  const debugEnabled = getSetting("debug_mode") || false;
  // Dynamic shortcut registration is unstable on Linux.
  const isLinux = type() === "linux";

  return (
    <div className="space-y-7">
      <SettingsGroup title={t("settings.general.groups.recording")}>
        <ShortcutInput shortcutId="transcribe" grouped={true} />
        <PushToTalk descriptionMode="inline" grouped={true} />
        <MicrophoneSelector descriptionMode="inline" grouped={true} />
        <AudioFeedback descriptionMode="inline" grouped={true} />
        <PasteMethodSetting descriptionMode="inline" grouped={true} />
        <CustomWords descriptionMode="inline" grouped />
      </SettingsGroup>

      <MeetingPreferences part="general" />

      <SettingsGroup title={t("settings.general.groups.app")}>
        <AppLanguageSelector descriptionMode="inline" grouped={true} />
        <AppearanceSelector descriptionMode="inline" grouped={true} />
        <AutostartToggle descriptionMode="inline" grouped={true} />
      </SettingsGroup>

      {debugEnabled && (
        <SettingsGroup title={t("settings.advanced.groups.developer")}>
          {!isLinux && !pushToTalk && (
            <ShortcutInput shortcutId="cancel" grouped={true} />
          )}
          <ShowOverlay descriptionMode="inline" grouped={true} />
          {audioFeedback && (
            <SoundPicker
              label={t("settings.sound.soundTheme.label")}
              description={t("settings.sound.soundTheme.description")}
            />
          )}
          {audioFeedback && <VolumeSlider />}
          {audioFeedback && (
            <OutputDeviceSelector descriptionMode="inline" grouped={true} />
          )}
          <ClamshellMicrophoneSelector
            descriptionMode="inline"
            grouped={true}
          />
          <MuteWhileRecording descriptionMode="inline" grouped={true} />
          <AlwaysOnMicrophone descriptionMode="inline" grouped={true} />
          <AutoSubmit descriptionMode="inline" grouped={true} />
          <TypingToolSetting descriptionMode="inline" grouped={true} />
          <ClipboardHandlingSetting descriptionMode="inline" grouped={true} />
          <AppendTrailingSpace descriptionMode="inline" grouped={true} />
          <WordCorrectionThreshold descriptionMode="inline" grouped={true} />
          <ModelUnloadTimeoutSetting descriptionMode="inline" grouped={true} />
          <KeyboardImplementationSelector
            descriptionMode="inline"
            grouped={true}
          />
          <AccelerationSelector descriptionMode="inline" grouped={true} />
          <LazyStreamClose descriptionMode="inline" grouped={true} />
          <ShowMascot descriptionMode="inline" grouped={true} />
          <StartHidden descriptionMode="inline" grouped={true} />
          <ShowTrayIcon descriptionMode="inline" grouped={true} />
          <UpdateChecksToggle descriptionMode="inline" grouped={true} />
          <HistoryLimit descriptionMode="inline" grouped={true} />
          <RecordingRetentionPeriodSelector
            descriptionMode="inline"
            grouped={true}
          />
          <LogLevelSelector grouped={true} />
          <PasteDelay descriptionMode="inline" grouped={true} />
          <RecordingBuffer descriptionMode="inline" grouped={true} />
          <LogDirectory grouped={true} />
          <AppDataDirectory descriptionMode="inline" grouped={true} />
        </SettingsGroup>
      )}

      <AboutFooter />
    </div>
  );
};
