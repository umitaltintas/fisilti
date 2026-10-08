import React from "react";
import { useTranslation } from "react-i18next";
import { SettingsGroup } from "../../ui/SettingsGroup";
import { useSettings } from "../../../hooks/useSettings";
import { PasteMethodSetting } from "../PasteMethod";
import { TypingToolSetting } from "../TypingTool";
import { ClipboardHandlingSetting } from "../ClipboardHandling";
import { AutoSubmit } from "../AutoSubmit";
import { AppendTrailingSpace } from "../AppendTrailingSpace";
import { CustomWords } from "../CustomWords";
import { ModelUnloadTimeoutSetting } from "../ModelUnloadTimeout";
import { ExperimentalToggle } from "../ExperimentalToggle";
import { AccelerationSelector } from "../AccelerationSelector";
import { LazyStreamClose } from "../LazyStreamClose";
import { AlwaysOnMicrophone } from "../AlwaysOnMicrophone";
import { KeyboardImplementationSelector } from "../debug/KeyboardImplementationSelector";
import { WordCorrectionThreshold } from "../debug/WordCorrectionThreshold";
import { LogLevelSelector } from "../debug/LogLevelSelector";
import { LogDirectory } from "../debug/LogDirectory";
import { PasteDelay } from "../debug/PasteDelay";
import { RecordingBuffer } from "../debug/RecordingBuffer";

// Set-once options. The old Debug section was folded in here as the
// "Developer" group so there is one place for low-level settings instead of
// two sidebar entries that overlapped.
export const AdvancedSettings: React.FC = () => {
  const { t } = useTranslation();
  const { getSetting } = useSettings();
  const experimentalEnabled = getSetting("experimental_enabled") || false;
  const debugEnabled = getSetting("debug_mode") || false;

  return (
    <div className="mx-auto w-full max-w-3xl space-y-6">
      <SettingsGroup title={t("settings.advanced.groups.output")}>
        <PasteMethodSetting descriptionMode="tooltip" grouped={true} />
        <TypingToolSetting descriptionMode="tooltip" grouped={true} />
        <ClipboardHandlingSetting descriptionMode="tooltip" grouped={true} />
        <AutoSubmit descriptionMode="tooltip" grouped={true} />
        <AppendTrailingSpace descriptionMode="tooltip" grouped={true} />
      </SettingsGroup>

      <SettingsGroup title={t("settings.advanced.groups.transcription")}>
        <CustomWords descriptionMode="tooltip" grouped />
        <WordCorrectionThreshold descriptionMode="tooltip" grouped={true} />
        <ModelUnloadTimeoutSetting descriptionMode="tooltip" grouped={true} />
        {/* A real latency/privacy trade-off, not a debugging aid: it used to
            sit in the debug-only Developer group, invisible to almost
            everyone. */}
        <AlwaysOnMicrophone descriptionMode="tooltip" grouped={true} />
      </SettingsGroup>

      <SettingsGroup title={t("settings.advanced.groups.experimental")}>
        <ExperimentalToggle descriptionMode="tooltip" grouped={true} />
        {experimentalEnabled && (
          <>
            <KeyboardImplementationSelector
              descriptionMode="tooltip"
              grouped={true}
            />
            <AccelerationSelector descriptionMode="tooltip" grouped={true} />
            <LazyStreamClose descriptionMode="tooltip" grouped={true} />
          </>
        )}
      </SettingsGroup>

      {debugEnabled && (
        <SettingsGroup title={t("settings.advanced.groups.developer")}>
          <LogLevelSelector grouped={true} />
          <PasteDelay descriptionMode="tooltip" grouped={true} />
          <RecordingBuffer descriptionMode="tooltip" grouped={true} />
          <LogDirectory grouped={true} />
        </SettingsGroup>
      )}
    </div>
  );
};
