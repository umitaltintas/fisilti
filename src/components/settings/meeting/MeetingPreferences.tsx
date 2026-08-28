import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { Input } from "../../ui/Input";
import { Select, type SelectOption } from "../../ui/Select";
import { Textarea } from "../../ui/Textarea";
import { ShortcutInput } from "../ShortcutInput";
import { MeetingToggle, SectionHeading } from "./shared";
import {
  changeGeminiApiKey,
  changeMeetingAutoDetect,
  changeMeetingAutoEnd,
  changeMeetingAutoEndGrace,
  changeMeetingAutoSummarize,
  changeMeetingCalendarNames,
  changeMeetingCustomVocabulary,
  changeMeetingGeminiDiarize,
  changeMeetingGeminiFinalize,
  changeMeetingGeminiSmart,
  changeMeetingLiveMode,
  changeMeetingLiveTranslateTarget,
  changeMeetingSubtitles,
  changeMeetingSilenceTimeout,
  getMeetingAutoDetectSettings,
  getMeetingAutoSummarize,
  getMeetingCalendarNames,
  getMeetingGeminiSettings,
  requestCalendarAccess,
  type MeetingLiveMode,
} from "@/lib/meeting";

// Languages offered for live translation. BCP-47 codes, matching what the
// Live API expects; it supports many more, so the field stays editable via the
// dropdown's list rather than being an exhaustive catalogue.
const LIVE_TRANSLATE_LANGUAGES: SelectOption[] = [
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

// The "Settings" tab: everything you configure once and rarely revisit —
// the global shortcut, auto-summarize, and automatic meeting detection.
export const MeetingPreferences: React.FC = () => {
  const { t } = useTranslation();

  const [autoSummarize, setAutoSummarize] = useState(false);
  const [calendarNames, setCalendarNames] = useState(false);
  const [autoDetect, setAutoDetect] = useState(false);
  const [autoEnd, setAutoEnd] = useState(true);
  const [silenceTimeoutSecs, setSilenceTimeoutSecs] = useState(180);
  const [autoEndGraceSecs, setAutoEndGraceSecs] = useState(60);
  const [liveMode, setLiveMode] = useState<MeetingLiveMode>("off");
  const [liveTarget, setLiveTarget] = useState("en");
  const [geminiFinalize, setGeminiFinalize] = useState(false);
  const [geminiDiarize, setGeminiDiarize] = useState(true);
  const [geminiSmart, setGeminiSmart] = useState(true);
  const [vocabulary, setVocabulary] = useState("");
  const [subtitles, setSubtitles] = useState(true);
  const [hasGeminiKey, setHasGeminiKey] = useState(false);
  // Kept separate from `hasGeminiKey`: a stored key is never read back, so the
  // field starts empty and only its edits are persisted.
  const [geminiKeyDraft, setGeminiKeyDraft] = useState("");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    void getMeetingAutoSummarize().then((v) => {
      if (!cancelled) setAutoSummarize(v);
    });
    void getMeetingCalendarNames().then((v) => {
      if (!cancelled) setCalendarNames(v);
    });
    void getMeetingAutoDetectSettings().then((s) => {
      if (cancelled) return;
      setAutoDetect(s.autoDetect);
      setAutoEnd(s.autoEnd);
      setSilenceTimeoutSecs(s.silenceTimeoutSecs);
      setAutoEndGraceSecs(s.autoEndGraceSecs);
    });
    void getMeetingGeminiSettings().then((s) => {
      if (cancelled) return;
      setLiveMode(s.liveMode);
      setLiveTarget(s.targetLanguage);
      setGeminiFinalize(s.finalizeWithGemini);
      setGeminiDiarize(s.diarize);
      setGeminiSmart(s.smart);
      setVocabulary(s.customVocabulary);
      setSubtitles(s.subtitles);
      setHasGeminiKey(s.hasApiKey);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  const handleToggleAutoSummarize = async () => {
    const next = !autoSummarize;
    setAutoSummarize(next);
    setError(null);
    try {
      await changeMeetingAutoSummarize(next);
    } catch (e) {
      // Revert the optimistic toggle on failure.
      setAutoSummarize(!next);
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

  const handleToggleAutoDetect = async () => {
    const next = !autoDetect;
    setAutoDetect(next);
    setError(null);
    try {
      await changeMeetingAutoDetect(next);
    } catch (e) {
      setAutoDetect(!next);
      setError(String(e));
    }
  };

  const handleToggleAutoEnd = async () => {
    const next = !autoEnd;
    setAutoEnd(next);
    setError(null);
    try {
      await changeMeetingAutoEnd(next);
    } catch (e) {
      setAutoEnd(!next);
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

  const silenceTimeoutOptions: SelectOption[] = [60, 120, 180, 300, 600].map(
    (secs) => ({
      value: String(secs),
      label: t("meeting.durationMinutes", { count: secs / 60 }),
    }),
  );
  const autoEndGraceOptions: SelectOption[] = [30, 60, 120].map((secs) => ({
    value: String(secs),
    label: t("meeting.durationSeconds", { count: secs }),
  }));

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

  // All four Gemini toggles revert optimistically the same way, so they share
  // one helper rather than repeating the try/catch four times.
  const toggleGeminiSetting = async (
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

  // Persisted on blur, like the API key: vocabulary is pasted in bulk and
  // writing settings per keystroke is wasteful.
  const handleVocabularyCommit = async () => {
    setError(null);
    try {
      await changeMeetingCustomVocabulary(vocabulary);
    } catch (e) {
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

  // Persist on blur rather than per keystroke: an API key is pasted, not typed,
  // and writing settings on every character is wasteful.
  const handleGeminiKeyCommit = async () => {
    const key = geminiKeyDraft.trim();
    if (key.length === 0) return;
    setError(null);
    try {
      await changeGeminiApiKey(key);
      setHasGeminiKey(true);
      setGeminiKeyDraft("");
    } catch (e) {
      setError(String(e));
    }
  };

  const handleClearGeminiKey = async () => {
    setError(null);
    try {
      await changeGeminiApiKey("");
      setHasGeminiKey(false);
      setGeminiKeyDraft("");
    } catch (e) {
      setError(String(e));
    }
  };

  // A key is only actually required once something is switched on; before that
  // the empty field is normal, not an error.
  const needsKey = !hasGeminiKey && (geminiFinalize || liveMode !== "off");

  const liveModeOptions: SelectOption[] = [
    { value: "off", label: t("meeting.liveModeOff") },
    { value: "transcribe", label: t("meeting.liveModeTranscribe") },
    { value: "translate", label: t("meeting.liveModeTranslate") },
  ];

  return (
    <div className="space-y-6">
      {/* General: shortcut + auto-summarize */}
      <div className="space-y-2">
        <SectionHeading className="px-1">
          {t("meeting.generalSection")}
        </SectionHeading>
        <div className="bg-background border border-mid-gray/20 rounded-lg p-4 space-y-4">
          {/* Optional global shortcut to start/stop a meeting without opening
              the window. Unbound by default; mirrors the tray quick-start. */}
          <ShortcutInput shortcutId="toggle_meeting" descriptionMode="inline" />

          <MeetingToggle
            checked={autoSummarize}
            onToggle={handleToggleAutoSummarize}
            label={t("meeting.autoSummarize")}
            description={t("meeting.autoSummarizeDescription")}
          />

          <MeetingToggle
            checked={calendarNames}
            onToggle={handleToggleCalendarNames}
            label={t("meeting.calendarNamesToggle")}
            description={t("meeting.calendarNamesDescription")}
          />
        </div>
      </div>

      {/* Gemini (cloud transcription, translation and speaker attribution) */}
      <div className="space-y-2">
        <SectionHeading className="px-1">
          {t("meeting.geminiSection")}
        </SectionHeading>
        <div className="bg-background border border-mid-gray/20 rounded-lg p-4 space-y-4">
          {/* The API key comes FIRST and is deliberately never gated on any of
              the toggles below: you need somewhere to paste it before turning
              anything on, and enabling a Gemini path without a key just makes
              the backend skip it silently. */}
          <div className="space-y-1">
            <label className="text-[11px] font-medium uppercase tracking-wide text-mid-gray">
              {t("meeting.geminiApiKeyLabel")}
            </label>
            <div className="flex items-center gap-2">
              <Input
                type="password"
                value={geminiKeyDraft}
                onChange={(e) => setGeminiKeyDraft(e.target.value)}
                onBlur={() => void handleGeminiKeyCommit()}
                placeholder={
                  hasGeminiKey
                    ? t("meeting.geminiApiKeyStored")
                    : t("meeting.geminiApiKeyPlaceholder")
                }
                className="flex-1"
              />
              {hasGeminiKey && (
                <button
                  type="button"
                  onClick={() => void handleClearGeminiKey()}
                  className="text-xs text-text/50 hover:text-red-400 transition-colors"
                >
                  {t("meeting.geminiApiKeyClear")}
                </button>
              )}
            </div>
            {needsKey ? (
              <p className="text-xs text-red-400">
                {t("meeting.geminiNeedsKey")}
              </p>
            ) : (
              <p className="text-xs text-text/50">
                {t("meeting.geminiKeyNote")}
              </p>
            )}
          </div>

          {/* On-stop pass: the only path that can tell participants apart. */}
          <MeetingToggle
            checked={geminiFinalize}
            onToggle={() =>
              void toggleGeminiSetting(
                geminiFinalize,
                changeMeetingGeminiFinalize,
                setGeminiFinalize,
              )
            }
            label={t("meeting.geminiFinalizeToggle")}
            description={t("meeting.geminiFinalizeDescription")}
          />
          {geminiFinalize && (
            <MeetingToggle
              checked={geminiDiarize}
              onToggle={() =>
                void toggleGeminiSetting(
                  geminiDiarize,
                  changeMeetingGeminiDiarize,
                  setGeminiDiarize,
                )
              }
              label={t("meeting.geminiDiarizeToggle")}
              description={t("meeting.geminiDiarizeDescription")}
            />
          )}

          {/* Live streaming: off, translate, or transcribe. */}
          <div className="space-y-1">
            <label className="text-[11px] font-medium uppercase tracking-wide text-mid-gray">
              {t("meeting.liveModeLabel")}
            </label>
            <Select
              value={liveMode}
              options={liveModeOptions}
              onChange={(v) => {
                if (v != null) void handleLiveModeChange(v as MeetingLiveMode);
              }}
              isClearable={false}
            />
            <p className="text-xs text-text/50">
              {t(`meeting.liveModeNote.${liveMode}`)}
            </p>
          </div>

          {liveMode !== "off" && (
            <MeetingToggle
              checked={subtitles}
              onToggle={() =>
                void toggleGeminiSetting(
                  subtitles,
                  changeMeetingSubtitles,
                  setSubtitles,
                )
              }
              label={t("meeting.subtitlesToggle")}
              description={t("meeting.subtitlesDescription")}
            />
          )}

          {liveMode === "translate" && (
            <div className="space-y-1">
              <label className="text-[11px] font-medium uppercase tracking-wide text-mid-gray">
                {t("meeting.liveTranslateTargetLabel")}
              </label>
              <Select
                value={liveTarget}
                options={LIVE_TRANSLATE_LANGUAGES}
                onChange={(v) => {
                  if (v != null) void handleLiveTargetChange(String(v));
                }}
                isClearable={false}
              />
            </div>
          )}

          {/* Shared by every Gemini path. */}
          <MeetingToggle
            checked={geminiSmart}
            onToggle={() =>
              void toggleGeminiSetting(
                geminiSmart,
                changeMeetingGeminiSmart,
                setGeminiSmart,
              )
            }
            label={t("meeting.geminiSmartToggle")}
            description={t("meeting.geminiSmartDescription")}
          />

          <div className="space-y-1">
            <label className="text-[11px] font-medium uppercase tracking-wide text-mid-gray">
              {t("meeting.customVocabularyLabel")}
            </label>
            <Textarea
              variant="compact"
              value={vocabulary}
              onChange={(e) => setVocabulary(e.target.value)}
              onBlur={() => void handleVocabularyCommit()}
              placeholder={t("meeting.customVocabularyPlaceholder")}
              className="w-full"
            />
            <p className="text-xs text-text/50">
              {t("meeting.customVocabularyDescription")}
            </p>
          </div>
        </div>
      </div>

      {/* Automatic detection */}
      <div className="space-y-2">
        <SectionHeading className="px-1">
          {t("meeting.autoDetectSection")}
        </SectionHeading>
        <div className="bg-background border border-mid-gray/20 rounded-lg p-4 space-y-4">
          <MeetingToggle
            checked={autoDetect}
            onToggle={handleToggleAutoDetect}
            label={t("meeting.autoDetectToggle")}
            description={t("meeting.autoDetectDescription")}
          />
          <MeetingToggle
            checked={autoEnd}
            onToggle={handleToggleAutoEnd}
            label={t("meeting.autoEndToggle")}
            description={t("meeting.autoEndDescription")}
          />

          <div
            className={`grid grid-cols-1 gap-3 sm:grid-cols-2 ${
              autoEnd ? "" : "opacity-50"
            }`}
          >
            <div className="space-y-1">
              <label className="text-[11px] font-medium uppercase tracking-wide text-mid-gray">
                {t("meeting.silenceTimeoutLabel")}
              </label>
              <Select
                value={String(silenceTimeoutSecs)}
                options={silenceTimeoutOptions}
                onChange={(v) => {
                  if (v != null) void handleSilenceTimeoutChange(Number(v));
                }}
                isClearable={false}
                disabled={!autoEnd}
              />
            </div>
            <div className="space-y-1">
              <label className="text-[11px] font-medium uppercase tracking-wide text-mid-gray">
                {t("meeting.autoEndGraceLabel")}
              </label>
              <Select
                value={String(autoEndGraceSecs)}
                options={autoEndGraceOptions}
                onChange={(v) => {
                  if (v != null) void handleAutoEndGraceChange(Number(v));
                }}
                isClearable={false}
                disabled={!autoEnd}
              />
            </div>
          </div>
        </div>
      </div>

      {error && (
        <p className="text-sm text-red-400 whitespace-pre-wrap break-words">
          {error}
        </p>
      )}
    </div>
  );
};
