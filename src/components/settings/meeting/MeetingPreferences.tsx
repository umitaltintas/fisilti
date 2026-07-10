import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { Select, type SelectOption } from "../../ui/Select";
import { ShortcutInput } from "../ShortcutInput";
import { MeetingToggle, SectionHeading } from "./shared";
import {
  changeMeetingAutoDetect,
  changeMeetingAutoEnd,
  changeMeetingAutoEndGrace,
  changeMeetingAutoSummarize,
  changeMeetingCalendarNames,
  changeMeetingSilenceTimeout,
  getMeetingAutoDetectSettings,
  getMeetingAutoSummarize,
  getMeetingCalendarNames,
  requestCalendarAccess,
} from "@/lib/meeting";

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
