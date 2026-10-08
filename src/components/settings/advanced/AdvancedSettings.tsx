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
import { MoreOptions } from "../../ui/MoreOptions";
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
        <PasteMethodSetting descriptionMode="inline" grouped={true} />
        <AutoSubmit descriptionMode="inline" grouped={true} />
        <MoreOptions>
          <TypingToolSetting descriptionMode="inline" grouped={true} />
          <ClipboardHandlingSetting descriptionMode="inline" grouped={true} />
          <AppendTrailingSpace descriptionMode="inline" grouped={true} />
        </MoreOptions>
      </SettingsGroup>

      <SettingsGroup title={t("settings.advanced.groups.transcription")}>
        <CustomWords descriptionMode="inline" grouped />
        <MoreOptions>
          <WordCorrectionThreshold descriptionMode="inline" grouped={true} />
          <ModelUnloadTimeoutSetting descriptionMode="inline" grouped={true} />
          {/* A real latency/privacy trade-off, not a debugging aid. */}
          <AlwaysOnMicrophone descriptionMode="inline" grouped={true} />
        </MoreOptions>
      </SettingsGroup>

      <SettingsGroup title={t("settings.advanced.groups.experimental")}>
        <ExperimentalToggle descriptionMode="inline" grouped={true} />
        {experimentalEnabled && (
          <>
            <KeyboardImplementationSelector
              descriptionMode="inline"
              grouped={true}
            />
            <AccelerationSelector descriptionMode="inline" grouped={true} />
            <LazyStreamClose descriptionMode="inline" grouped={true} />
          </>
        )}
      </SettingsGroup>

      {debugEnabled && (
        <SettingsGroup title={t("settings.advanced.groups.developer")}>
          <LogLevelSelector grouped={true} />
          <PasteDelay descriptionMode="inline" grouped={true} />
          <RecordingBuffer descriptionMode="inline" grouped={true} />
          <LogDirectory grouped={true} />
        </SettingsGroup>
      )}
    </div>
  );
};
