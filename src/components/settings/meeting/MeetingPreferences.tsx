import React, { useState } from "react";
import { useTranslation } from "react-i18next";
import { emit } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";

import {
  CollapsibleGroup,
  Dropdown,
  SettingContainer,
  SettingsGroup,
  ToggleSwitch,
} from "../../ui";
import type { DropdownOption } from "../../ui/Dropdown";
import { ShortcutInput } from "../ShortcutInput";
import { Alert } from "../../ui/Alert";
import { Button } from "../../ui/Button";
import { InlineError } from "./shared";
import { useModelStore } from "@/stores/modelStore";
import { useSettingsStore, type Settings } from "@/stores/settingsStore";
import { cloudProviderOf, meetingModelId } from "@/lib/utils/model";
import { errorMessage } from "@/lib/utils/errors";
import { requestCalendarAccess, toLiveMode } from "@/lib/meeting";

// Languages offered for live translation. BCP-47 codes, matching what the
// Live API expects. Each is shown in its own language (an endonym), like the
// app-language picker, so it is not translated.
const LIVE_TRANSLATE_LANGUAGES: DropdownOption[] = [
  { value: "en", label: "English" },
  { value: "tr", label: "Türkçe" },
  { value: "de", label: "Deutsch" },
  { value: "es", label: "Español" },
  { value: "fr", label: "Français" },
  { value: "it", label: "Italiano" },
  { value: "pt", label: "Português" },
  { value: "nl", label: "Nederlands" },
  { value: "pl", label: "Polski" },
  { value: "ru", label: "Русский" },
  { value: "ar", label: "العربية" },
  { value: "hi", label: "हिन्दी" },
  { value: "ja", label: "日本語" },
  { value: "ko", label: "한국어" },
  { value: "zh", label: "中文" },
];

// The "Settings" tab: everything you configure once and rarely revisit.
//
// Built from the same primitives as every other settings page, and — like
// them — reads and writes through the settings store, so a failed write rolls
// back that one value and says so, and the rest of the app sees changes made
// here. The two cloud/automation blocks are collapsed unless they are actually
// in use.
export const MeetingPreferences: React.FC = () => {
  const { t } = useTranslation();
  const models = useModelStore((s) => s.models);
  const settings = useSettingsStore((s) => s.settings);
  const isLoading = useSettingsStore((s) => s.isLoading);
  const loadError = useSettingsStore((s) => s.loadError);
  const refreshSettings = useSettingsStore((s) => s.refreshSettings);
  const updateSetting = useSettingsStore((s) => s.updateSetting);
  const isUpdating = useSettingsStore((s) => s.isUpdating);

  const [calendarError, setCalendarError] = useState<string | null>(null);
  const [requestingCalendar, setRequestingCalendar] = useState(false);
  const [exportDirError, setExportDirError] = useState<string | null>(null);

  // The groups' open/closed state is derived from the loaded values and must
  // not flip after the first paint, so nothing renders until settings exist.
  if (!settings) {
    if (isLoading) {
      // Reserve roughly the collapsed height so switching tabs does not jump.
      return <div className="h-48" />;
    }
    return (
      <div className="flex flex-col items-center gap-3 rounded-lg border border-mid-gray/20 px-4 py-8 text-center">
        <InlineError>
          {t("meeting.settingsLoadError")}
          {loadError ? ` (${loadError})` : ""}
        </InlineError>
        <Button
          variant="secondary"
          size="sm"
          onClick={() => void refreshSettings()}
        >
          {t("common.retry")}
        </Button>
      </div>
    );
  }

  const busy = (key: keyof Settings) => isUpdating[key] === true;
  const autoSummarize = settings.meeting_auto_summarize ?? false;
  const calendarNames = settings.meeting_calendar_names ?? false;
  const exportDir = settings.meeting_export_dir ?? "";
  const autoDetect = settings.meeting_auto_detect ?? false;
  const autoEnd = settings.meeting_auto_end ?? true;
  const silenceTimeoutSecs = settings.meeting_silence_timeout_secs ?? 180;
  const autoEndGraceSecs = settings.meeting_auto_end_grace_secs ?? 60;
  const liveMode = toLiveMode(settings.meeting_live_mode);
  const liveTarget = settings.meeting_live_translate_target || "en";
  const geminiDiarize = settings.meeting_gemini_diarize ?? true;
  const geminiSmart = settings.meeting_gemini_smart ?? true;
  const subtitles = settings.meeting_subtitles ?? true;
  const hasGeminiKey = (settings.gemini_api_key ?? "").trim().length > 0;

  const handleToggleCalendarNames = async (enabled: boolean) => {
    setCalendarError(null);
    if (!enabled) {
      await updateSetting("meeting_calendar_names", false);
      return;
    }
    // Enabling: get calendar access first; only persist once granted.
    setRequestingCalendar(true);
    try {
      const granted = await requestCalendarAccess();
      if (!granted) {
        setCalendarError(t("meeting.calendarAccessDenied"));
        return;
      }
      await updateSetting("meeting_calendar_names", true);
    } catch (error) {
      setCalendarError(
        t("meeting.errors.calendarFailed", { error: errorMessage(error) }),
      );
    } finally {
      setRequestingCalendar(false);
    }
  };

  const handleExportDir = async (pick: boolean) => {
    setExportDirError(null);
    let next = "";
    if (pick) {
      try {
        const picked = await open({ directory: true, multiple: false });
        if (typeof picked !== "string") return; // cancelled
        next = picked;
      } catch (error) {
        setExportDirError(
          t("meeting.errors.folderPickerFailed", {
            error: errorMessage(error),
          }),
        );
        return;
      }
    }
    await updateSetting("meeting_export_dir", next);
  };

  const silenceTimeoutOptions: DropdownOption[] = [60, 120, 180, 300, 600].map(
    (secs) => ({
      value: String(secs),
      label: t("meeting.durationMinutes", { count: secs / 60 }),
    }),
  );
  const autoEndGraceOptions: DropdownOption[] = [30, 60, 120].map((secs) => ({
    value: String(secs),
    label: t("meeting.durationSeconds", { count: secs }),
  }));
  const liveModeOptions: DropdownOption[] = [
    { value: "off", label: t("meeting.liveModeOff") },
    { value: "transcribe", label: t("meeting.liveModeTranscribe") },
    { value: "translate", label: t("meeting.liveModeTranslate") },
  ];

  // Whether THIS meeting's model is Gemini. It decides whether the on-stop pass
  // can attribute speakers, so the diarize toggle only makes sense then.
  const meetingModel = models.find((m) => m.id === meetingModelId(settings));
  const meetingModelIsGemini =
    cloudProviderOf(meetingModel?.engine_type) === "gemini";

  // A key is only actually required once something needs it; before that an
  // empty key is normal, not an error.
  const needsKey =
    !hasGeminiKey && (meetingModelIsGemini || liveMode !== "off");
  // Open the cloud block for anyone already using it, and leave it folded for
  // the on-device-only case it does not apply to.
  const geminiInUse =
    meetingModelIsGemini || liveMode !== "off" || hasGeminiKey;

  return (
    <div className="space-y-6">
      <SettingsGroup title={t("meeting.generalSection")}>
        {/* Optional global shortcut to start/stop a meeting without opening
            the window. Unbound by default; mirrors the tray quick-start. */}
        <ShortcutInput
          shortcutId="toggle_meeting"
          descriptionMode="tooltip"
          grouped
        />

        <ToggleSwitch
          checked={autoSummarize}
          isUpdating={busy("meeting_auto_summarize")}
          onChange={(value) =>
            void updateSetting("meeting_auto_summarize", value)
          }
          label={t("meeting.autoSummarize")}
          description={t("meeting.autoSummarizeDescription")}
          grouped
        />

        <ToggleSwitch
          checked={calendarNames}
          isUpdating={requestingCalendar || busy("meeting_calendar_names")}
          onChange={(value) => void handleToggleCalendarNames(value)}
          label={t("meeting.calendarNamesToggle")}
          description={t("meeting.calendarNamesDescription")}
          grouped
        />
        {calendarError && (
          <InlineError className="px-4 pb-2 text-xs">
            {calendarError}
          </InlineError>
        )}

        <SettingContainer
          title={t("meeting.exportDir.title")}
          description={t("meeting.exportDir.description")}
          descriptionMode="tooltip"
          grouped
        >
          <div className="flex items-center gap-2 min-w-0">
            {exportDir && (
              <span
                className="max-w-56 truncate text-xs font-mono text-text/60"
                title={exportDir}
                dir="rtl"
              >
                {exportDir}
              </span>
            )}
            <Button
              onClick={() => void handleExportDir(true)}
              variant="secondary"
              size="sm"
              disabled={busy("meeting_export_dir")}
            >
              {exportDir
                ? t("meeting.exportDir.change")
                : t("meeting.exportDir.choose")}
            </Button>
            {exportDir && (
              <Button
                onClick={() => void handleExportDir(false)}
                variant="ghost"
                size="sm"
                disabled={busy("meeting_export_dir")}
              >
                {t("meeting.exportDir.off")}
              </Button>
            )}
          </div>
        </SettingContainer>
        {exportDirError && (
          <InlineError className="px-4 pb-2 text-xs">
            {exportDirError}
          </InlineError>
        )}
      </SettingsGroup>

      <CollapsibleGroup
        title={t("meeting.geminiSection")}
        defaultOpen={geminiInUse}
      >
        <SettingsGroup>
          {/* The key lives on the Models page next to every other credential.
              Live streaming needs it even when the meeting model is local, so
              say so here and offer one click to get there rather than growing a
              second field for the same setting. */}
          {needsKey && (
            <div className="px-4 py-3">
              <Alert variant="warning">
                <div className="flex flex-wrap items-center gap-3">
                  <span>{t("meeting.geminiNeedsKey")}</span>
                  <Button
                    variant="secondary"
                    size="sm"
                    onClick={() => void emit("navigate-section", "models")}
                  >
                    {t("meeting.geminiOpenModels")}
                  </Button>
                </div>
              </Alert>
            </div>
          )}

          {/* Speaker attribution only exists on the Gemini on-stop pass. */}
          {meetingModelIsGemini && (
            <ToggleSwitch
              checked={geminiDiarize}
              isUpdating={busy("meeting_gemini_diarize")}
              onChange={(value) =>
                void updateSetting("meeting_gemini_diarize", value)
              }
              label={t("meeting.geminiDiarizeToggle")}
              description={t("meeting.geminiDiarizeDescription")}
              grouped
            />
          )}

          {/* Live streaming: off, translate, or transcribe. */}
          <SettingContainer
            title={t("meeting.liveModeLabel")}
            description={t(`meeting.liveModeNote.${liveMode}`)}
            descriptionMode="tooltip"
            grouped
          >
            <Dropdown
              options={liveModeOptions}
              selectedValue={liveMode}
              disabled={busy("meeting_live_mode")}
              onSelect={(v) => void updateSetting("meeting_live_mode", v)}
            />
          </SettingContainer>

          {liveMode !== "off" && (
            <ToggleSwitch
              checked={subtitles}
              isUpdating={busy("meeting_subtitles")}
              onChange={(value) =>
                void updateSetting("meeting_subtitles", value)
              }
              label={t("meeting.subtitlesToggle")}
              description={t("meeting.subtitlesDescription")}
              grouped
            />
          )}

          {liveMode === "translate" && (
            <SettingContainer
              title={t("meeting.liveTranslateTargetLabel")}
              description={t("meeting.liveTranslateTargetDescription")}
              descriptionMode="tooltip"
              grouped
            >
              <Dropdown
                options={LIVE_TRANSLATE_LANGUAGES}
                selectedValue={liveTarget}
                disabled={busy("meeting_live_translate_target")}
                onSelect={(v) =>
                  void updateSetting("meeting_live_translate_target", v)
                }
              />
            </SettingContainer>
          )}

          {/* Shared by every Gemini path. */}
          <ToggleSwitch
            checked={geminiSmart}
            isUpdating={busy("meeting_gemini_smart")}
            onChange={(value) =>
              void updateSetting("meeting_gemini_smart", value)
            }
            label={t("meeting.geminiSmartToggle")}
            description={t("meeting.geminiSmartDescription")}
            grouped
          />
        </SettingsGroup>
      </CollapsibleGroup>

      <CollapsibleGroup
        title={t("meeting.autoDetectSection")}
        defaultOpen={autoDetect}
      >
        <SettingsGroup>
          <ToggleSwitch
            checked={autoDetect}
            isUpdating={busy("meeting_auto_detect")}
            onChange={(value) =>
              void updateSetting("meeting_auto_detect", value)
            }
            label={t("meeting.autoDetectToggle")}
            description={t("meeting.autoDetectDescription")}
            grouped
          />
          <ToggleSwitch
            checked={autoEnd}
            isUpdating={busy("meeting_auto_end")}
            onChange={(value) => void updateSetting("meeting_auto_end", value)}
            label={t("meeting.autoEndToggle")}
            description={t("meeting.autoEndDescription")}
            grouped
          />
          <SettingContainer
            title={t("meeting.silenceTimeoutLabel")}
            description={t("meeting.silenceTimeoutDescription")}
            descriptionMode="tooltip"
            grouped
            disabled={!autoEnd}
          >
            <Dropdown
              options={silenceTimeoutOptions}
              selectedValue={String(silenceTimeoutSecs)}
              onSelect={(v) =>
                void updateSetting("meeting_silence_timeout_secs", Number(v))
              }
              disabled={!autoEnd || busy("meeting_silence_timeout_secs")}
            />
          </SettingContainer>
          <SettingContainer
            title={t("meeting.autoEndGraceLabel")}
            description={t("meeting.autoEndGraceDescription")}
            descriptionMode="tooltip"
            grouped
            disabled={!autoEnd}
          >
            <Dropdown
              options={autoEndGraceOptions}
              selectedValue={String(autoEndGraceSecs)}
              onSelect={(v) =>
                void updateSetting("meeting_auto_end_grace_secs", Number(v))
              }
              disabled={!autoEnd || busy("meeting_auto_end_grace_secs")}
            />
          </SettingContainer>
        </SettingsGroup>
      </CollapsibleGroup>
    </div>
  );
};
