import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

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
import { useModelStore } from "@/stores/modelStore";
import { useSettingsStore } from "@/stores/settingsStore";
import { cloudProviderOf } from "@/lib/utils/model";
import { emit } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import {
  changeMeetingAutoDetect,
  changeMeetingAutoEnd,
  changeMeetingAutoEndGrace,
  changeMeetingAutoSummarize,
  changeMeetingCalendarNames,
  changeMeetingExportDir,
  changeMeetingGeminiDiarize,
  changeMeetingGeminiSmart,
  changeMeetingLiveMode,
  changeMeetingLiveTranslateTarget,
  changeMeetingSubtitles,
  changeMeetingSilenceTimeout,
  getMeetingAutoDetectSettings,
  getMeetingAutoSummarize,
  getMeetingCalendarNames,
  getMeetingExportDir,
  getMeetingGeminiSettings,
  requestCalendarAccess,
  type MeetingLiveMode,
} from "@/lib/meeting";

// Languages offered for live translation. BCP-47 codes, matching what the
// Live API expects; it supports many more, so the field stays editable via the
// dropdown's list rather than being an exhaustive catalogue.
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
// It is built from the same primitives as every other settings page
// (SettingsGroup / SettingContainer / ToggleSwitch) rather than its own local
// widgets, so a toggle here looks and behaves exactly like a toggle on General
// or Advanced. The two cloud/automation blocks are collapsed unless they are
// actually in use, which keeps the page down to three visible rows for someone
// who only records meetings locally.
export const MeetingPreferences: React.FC = () => {
  const { t } = useTranslation();
  // Read-only here: the meeting model is chosen on the Models page. This page
  // only needs to know whether it is Gemini, to decide which options apply.
  const { models, currentModel } = useModelStore();
  const { settings } = useSettingsStore();

  // Every default below is overwritten by the backend in the effect. Until that
  // lands we render nothing, because the collapsed/expanded state of the groups
  // is derived from the loaded values and must not flip after the first paint.
  const [loaded, setLoaded] = useState(false);
  const [autoSummarize, setAutoSummarize] = useState(false);
  const [calendarNames, setCalendarNames] = useState(false);
  const [exportDir, setExportDir] = useState("");
  const [autoDetect, setAutoDetect] = useState(false);
  const [autoEnd, setAutoEnd] = useState(true);
  const [silenceTimeoutSecs, setSilenceTimeoutSecs] = useState(180);
  const [autoEndGraceSecs, setAutoEndGraceSecs] = useState(60);
  const [liveMode, setLiveMode] = useState<MeetingLiveMode>("off");
  const [liveTarget, setLiveTarget] = useState("en");
  const [geminiDiarize, setGeminiDiarize] = useState(true);
  const [geminiSmart, setGeminiSmart] = useState(true);
  const [subtitles, setSubtitles] = useState(true);
  const [hasGeminiKey, setHasGeminiKey] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    void Promise.all([
      getMeetingAutoSummarize().then((v) => {
        if (!cancelled) setAutoSummarize(v);
      }),
      getMeetingCalendarNames().then((v) => {
        if (!cancelled) setCalendarNames(v);
      }),
      getMeetingExportDir().then((v) => {
        if (!cancelled) setExportDir(v);
      }),
      getMeetingAutoDetectSettings().then((s) => {
        if (cancelled) return;
        setAutoDetect(s.autoDetect);
        setAutoEnd(s.autoEnd);
        setSilenceTimeoutSecs(s.silenceTimeoutSecs);
        setAutoEndGraceSecs(s.autoEndGraceSecs);
      }),
      getMeetingGeminiSettings().then((s) => {
        if (cancelled) return;
        setLiveMode(s.liveMode);
        setLiveTarget(s.targetLanguage);
        setGeminiDiarize(s.diarize);
        setGeminiSmart(s.smart);
        setSubtitles(s.subtitles);
        setHasGeminiKey(s.hasApiKey);
      }),
      // A failed read must not leave the tab blank forever; the groups still
      // render with their defaults and the error is surfaced at the bottom.
    ])
      .catch((e) => {
        if (!cancelled) setError(String(e));
      })
      .finally(() => {
        if (!cancelled) setLoaded(true);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // Every toggle on this page persists optimistically and reverts on failure,
  // so they all share one helper instead of repeating the try/catch.
  const applyToggle = async (
    current: boolean,
    apply: (next: boolean) => Promise<void>,
    set: (value: boolean) => void,
  ) => {
    const next = !current;
    set(next);
    setError(null);
    try {
      await apply(next);
    } catch (e) {
      set(current);
      setError(String(e));
    }
  };

  const handleToggleCalendarNames = async () => {
    setError(null);
    if (!calendarNames) {
      // Enabling: get calendar access first; only persist once granted.
      setCalendarNames(true);
      try {
        const granted = await requestCalendarAccess();
        if (!granted) {
          setCalendarNames(false);
          setError(t("meeting.calendarAccessDenied"));
          return;
        }
        await changeMeetingCalendarNames(true);
      } catch (e) {
        setCalendarNames(false);
        setError(String(e));
      }
      return;
    }
    setCalendarNames(false);
    try {
      await changeMeetingCalendarNames(false);
    } catch (e) {
      setCalendarNames(true);
      setError(String(e));
    }
  };

  const handleExportDir = async (pick: boolean) => {
    setError(null);
    let next = "";
    if (pick) {
      const picked = await open({ directory: true, multiple: false });
      if (typeof picked !== "string") return;
      next = picked;
    }
    try {
      await changeMeetingExportDir(next);
      setExportDir(next);
    } catch (e) {
      setError(String(e));
    }
  };

  const handleSilenceTimeoutChange = async (secs: number) => {
    const prev = silenceTimeoutSecs;
    setSilenceTimeoutSecs(secs);
    setError(null);
    try {
      await changeMeetingSilenceTimeout(secs);
    } catch (e) {
      setSilenceTimeoutSecs(prev);
      setError(String(e));
    }
  };

  const handleAutoEndGraceChange = async (secs: number) => {
    const prev = autoEndGraceSecs;
    setAutoEndGraceSecs(secs);
    setError(null);
    try {
      await changeMeetingAutoEndGrace(secs);
    } catch (e) {
      setAutoEndGraceSecs(prev);
      setError(String(e));
    }
  };

  const handleLiveModeChange = async (next: MeetingLiveMode) => {
    const previous = liveMode;
    setLiveMode(next);
    setError(null);
    try {
      await changeMeetingLiveMode(next);
    } catch (e) {
      setLiveMode(previous);
      setError(String(e));
    }
  };

  const handleLiveTargetChange = async (language: string) => {
    const previous = liveTarget;
    setLiveTarget(language);
    setError(null);
    try {
      await changeMeetingLiveTranslateTarget(language);
    } catch (e) {
      setLiveTarget(previous);
      setError(String(e));
    }
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
  const meetingModelIsGemini =
    cloudProviderOf(
      models.find((m) => m.id === (settings?.meeting_selected_model || ""))
        ?.engine_type ?? models.find((m) => m.id === currentModel)?.engine_type,
    ) === "gemini";

  // A key is only actually required once something needs it; before that an
  // empty key is normal, not an error.
  const needsKey =
    !hasGeminiKey && (meetingModelIsGemini || liveMode !== "off");
  // Open the cloud block for anyone already using it, and leave it folded for
  // the on-device-only case it does not apply to.
  const geminiInUse =
    meetingModelIsGemini || liveMode !== "off" || hasGeminiKey;

  if (!loaded) {
    // Reserve roughly the collapsed height so switching tabs does not jump.
    return <div className="h-48" />;
  }

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
          onChange={() =>
            void applyToggle(
              autoSummarize,
              changeMeetingAutoSummarize,
              setAutoSummarize,
            )
          }
          label={t("meeting.autoSummarize")}
          description={t("meeting.autoSummarizeDescription")}
          grouped
        />

        <ToggleSwitch
          checked={calendarNames}
          onChange={() => void handleToggleCalendarNames()}
          label={t("meeting.calendarNamesToggle")}
          description={t("meeting.calendarNamesDescription")}
          grouped
        />

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
              >
                {t("meeting.exportDir.off")}
              </Button>
            )}
          </div>
        </SettingContainer>
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
              onChange={() =>
                void applyToggle(
                  geminiDiarize,
                  changeMeetingGeminiDiarize,
                  setGeminiDiarize,
                )
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
              onSelect={(v) => void handleLiveModeChange(v as MeetingLiveMode)}
            />
          </SettingContainer>

          {liveMode !== "off" && (
            <ToggleSwitch
              checked={subtitles}
              onChange={() =>
                void applyToggle(
                  subtitles,
                  changeMeetingSubtitles,
                  setSubtitles,
                )
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
                onSelect={(v) => void handleLiveTargetChange(v)}
              />
            </SettingContainer>
          )}

          {/* Shared by every Gemini path. */}
          <ToggleSwitch
            checked={geminiSmart}
            onChange={() =>
              void applyToggle(
                geminiSmart,
                changeMeetingGeminiSmart,
                setGeminiSmart,
              )
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
            onChange={() =>
              void applyToggle(
                autoDetect,
                changeMeetingAutoDetect,
                setAutoDetect,
              )
            }
            label={t("meeting.autoDetectToggle")}
            description={t("meeting.autoDetectDescription")}
            grouped
          />
          <ToggleSwitch
            checked={autoEnd}
            onChange={() =>
              void applyToggle(autoEnd, changeMeetingAutoEnd, setAutoEnd)
            }
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
              onSelect={(v) => void handleSilenceTimeoutChange(Number(v))}
              disabled={!autoEnd}
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
              onSelect={(v) => void handleAutoEndGraceChange(Number(v))}
              disabled={!autoEnd}
            />
          </SettingContainer>
        </SettingsGroup>
      </CollapsibleGroup>

      {error && (
        <p className="text-sm text-red-400 whitespace-pre-wrap break-words">
          {error}
        </p>
      )}
    </div>
  );
};
