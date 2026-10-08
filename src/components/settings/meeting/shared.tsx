import i18n from "@/i18n";
import React, { useEffect, useId, useState } from "react";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import {
  AlertCircle,
  Check,
  ChevronRight,
  Cloud,
  Copy,
  Loader2,
  Lock,
} from "lucide-react";

import { Select, type SelectOption } from "../../ui/Select";
import { IconButton } from "../../ui/IconButton";
import { useCopyToClipboard } from "@/hooks/useCopyToClipboard";
import {
  getTranscriptionLocation,
  type SummaryProviderInfo,
  type TranscriptSegment,
} from "@/lib/meeting";
import type { NotesSaveState } from "@/stores/meetingStore";

/** Copy-to-clipboard icon button. Shows a check only after the copy
 * actually succeeded. */
export const CopyButton: React.FC<{
  text: string;
  disabled?: boolean;
  label: string;
}> = ({ text, disabled, label }) => {
  const { t } = useTranslation();
  const { copied, copy } = useCopyToClipboard();

  return (
    <IconButton
      onClick={() => void copy(text)}
      disabled={disabled}
      label={copied ? t("meeting.copied") : label}
    >
      {copied ? (
        <Check width={16} height={16} />
      ) : (
        <Copy width={16} height={16} />
      )}
    </IconButton>
  );
};

/** Truthful save indicator for notes: says "Saved" only once the backend
 * confirmed it, and says so plainly when a save failed or cannot happen. */
export const NotesSaveIndicator: React.FC<{
  state: NotesSaveState;
  error?: string | null;
}> = ({ state, error }) => {
  const { t } = useTranslation();
  if (state === "idle") return null;
  const base =
    "inline-flex items-center gap-1 text-[10px] font-medium uppercase tracking-wide";
  if (state === "saving") {
    return (
      <span className={`${base} text-faint`} role="status">
        <Loader2 width={10} height={10} className="animate-spin" aria-hidden />
        {t("meeting.notesSaving")}
      </span>
    );
  }
  if (state === "saved") {
    return (
      <span className={`${base} text-faint`} role="status">
        <Check width={10} height={10} aria-hidden />
        {t("meeting.notesSaved")}
      </span>
    );
  }
  if (state === "pending") {
    return (
      <span className={`${base} text-amber-500`} role="status">
        {t("meeting.notesNotSavedYet")}
      </span>
    );
  }
  return (
    <span
      className={`${base} text-rec`}
      role="alert"
      title={error ?? undefined}
    >
      <AlertCircle width={10} height={10} aria-hidden />
      {t("meeting.notesSaveFailed")}
    </span>
  );
};

// Render a transcript as a clean, chronological flow of plain text lines.
//
// Speaker attribution ("You" / "Others") was removed: it was source-based
// (mic vs system audio), not real diarization, and broke down on speaker
// output where the mic re-captures the remote voice and mislabels it. The
// backend still de-duplicates echoed segments, so this stays a single clean
// transcript without doubled lines.
export const PlainTranscript: React.FC<{
  segments: TranscriptSegment[];
  /** Small secondary text, for the live transcript rail. */
  compact?: boolean;
}> = ({ segments, compact = false }) => (
  <div
    className={`${compact ? "space-y-2.5 [&_p]:text-xs [&_p]:leading-relaxed" : "space-y-2"}`}
  >
    {segments.map((seg, i) => (
      <div key={i} className="space-y-0.5">
        {/* Only the Gemini finalize pass can attribute speech, so most
            transcripts have no label here at all. */}
        {seg.speaker && seg.speaker.trim().length > 0 && (
          <p className="text-[11px] font-medium uppercase tracking-wide text-mid-gray select-text">
            {seg.speaker}
          </p>
        )}
        <p className="text-sm whitespace-pre-wrap break-words select-text text-text/90">
          {seg.text}
        </p>
        {/* Live-translated segments carry both directions; the translation sits
            under the original so the original stays the primary reading. */}
        {seg.translation && seg.translation.trim().length > 0 && (
          <p className="text-sm whitespace-pre-wrap break-words select-text text-logo-primary/80 ps-3 border-s border-logo-primary/30">
            {seg.translation}
          </p>
        )}
      </div>
    ))}
  </div>
);

// Build a copy-friendly transcript: one line per segment.
// Falls back to the plain joined transcript when no segments are available.
export const plainTranscriptText = (
  segments: TranscriptSegment[],
  fallback: string,
): string => {
  if (segments.length === 0) return fallback;
  return segments
    .map((seg) => {
      const speaker = seg.speaker?.trim();
      const line = speaker ? `${speaker}: ${seg.text}` : seg.text;
      return seg.translation && seg.translation.trim().length > 0
        ? `${line}\n${seg.translation}`
        : line;
    })
    .join("\n");
};

// Honest indicator for where TRANSCRIPTION runs. This used to claim
// "100% on-device" unconditionally, which stopped being true the moment a cloud
// transcription model or a Gemini path could be selected. A privacy claim that
// is only usually right is worse than none: it is exactly when someone stops
// checking that it misleads them.
export const OnDeviceBadge: React.FC = () => {
  const { t } = useTranslation();
  const [cloudProviders, setCloudProviders] = useState<string[] | null>(null);

  useEffect(() => {
    let cancelled = false;
    void getTranscriptionLocation().then((info) => {
      if (!cancelled) setCloudProviders(info.cloudProviders);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  // Say nothing until we know. Showing the on-device claim optimistically and
  // correcting it a moment later is the same lie, just briefer.
  if (cloudProviders === null) return null;

  if (cloudProviders.length > 0) {
    return (
      <span className="inline-flex items-center gap-1.5 rounded-full bg-mid-gray/15 px-2.5 py-1 text-[11px] font-medium text-sub">
        <Cloud width={12} height={12} aria-hidden />
        {t("meeting.cloudBadge", { providers: cloudProviders.join(", ") })}
      </span>
    );
  }

  return (
    <span className="inline-flex items-center gap-1.5 rounded-full bg-logo-primary/10 px-2.5 py-1 text-[11px] font-medium text-logo-primary">
      <Lock width={12} height={12} aria-hidden />
      {t("meeting.onDeviceBadge")}
    </span>
  );
};

// Honest indicator for where the SUMMARY runs (local vs a cloud provider).
export const SummaryLocationNote: React.FC<{
  info: SummaryProviderInfo | null;
}> = ({ info }) => {
  const { t } = useTranslation();
  if (!info) return null;
  if (info.location === "none") {
    return (
      <p className="text-[11px] text-faint">{t("meeting.summaryNoProvider")}</p>
    );
  }
  if (info.location === "local") {
    return (
      <p className="inline-flex items-center gap-1.5 text-[11px] text-emerald-500">
        <Lock width={11} height={11} aria-hidden />
        {t("meeting.summaryLocal")}
      </p>
    );
  }
  return (
    <p className="text-[11px] text-amber-500">
      {t("meeting.summaryCloud", { provider: info.label })}
    </p>
  );
};

// Small uppercase section heading used across the meeting panels.
export const SectionHeading: React.FC<{
  children: React.ReactNode;
  className?: string;
  id?: string;
}> = ({ children, className = "", id }) => (
  <h2 id={id} className={`text-xs font-semibold text-sub ${className}`}>
    {children}
  </h2>
);

/** Inline error line, the same everywhere on the meeting pages. */
export const InlineError: React.FC<{
  children: React.ReactNode;
  className?: string;
}> = ({ children, className = "" }) => (
  <p
    role="alert"
    className={`text-sm text-rec whitespace-pre-wrap break-words ${className}`}
  >
    {children}
  </p>
);

export function formatElapsed(seconds: number): string {
  const total = Math.max(0, Math.floor(seconds));
  const hours = Math.floor(total / 3600);
  const mins = Math.floor((total % 3600) / 60);
  const secs = total % 60;
  const mm = mins.toString().padStart(2, "0");
  const ss = secs.toString().padStart(2, "0");
  // Past one hour, render h:mm:ss so the minutes don't roll over past 59.
  return hours > 0 ? `${hours}:${mm}:${ss}` : `${mm}:${ss}`;
}

// Format an epoch-ms timestamp using the user's locale (no hardcoded format).
export function formatMeetingDate(epochMs: number, locale: string): string {
  try {
    const date = new Date(epochMs);
    if (isNaN(date.getTime())) return "";
    return new Intl.DateTimeFormat(locale, {
      year: "numeric",
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    }).format(date);
  } catch {
    return "";
  }
}

// Format only the time-of-day portion (used under day group headers where the
// date would be redundant).
export function formatMeetingTime(epochMs: number, locale: string): string {
  try {
    const date = new Date(epochMs);
    if (isNaN(date.getTime())) return "";
    return new Intl.DateTimeFormat(locale, {
      hour: "2-digit",
      minute: "2-digit",
    }).format(date);
  } catch {
    return "";
  }
}

// Format a duration in ms as a localized "1 h 5 min" (>= 1h) or `mm:ss`.
export function formatDuration(durationMs: number, t: TFunction): string {
  const totalSeconds = Math.max(0, Math.floor(durationMs / 1000));
  const hours = Math.floor(totalSeconds / 3600);
  const mins = Math.floor((totalSeconds % 3600) / 60);
  const secs = totalSeconds % 60;
  if (hours > 0) {
    return t("meeting.durationHoursMinutes", { hours, minutes: mins });
  }
  const unit = (value: number, name: "minute" | "second") =>
    new Intl.NumberFormat(i18n.language, {
      style: "unit",
      unit: name,
      unitDisplay: "short",
    }).format(value);
  return mins > 0 ? unit(mins, "minute") : unit(secs, "second");
}

// Build a safe-ish default export filename from a meeting title.
export function exportFilename(title: string): string {
  const base = title.trim() || "meeting";
  const slug = base
    .replace(/[\\/:*?"<>|]/g, "")
    .replace(/\s+/g, "-")
    .slice(0, 60);
  return `${slug || "meeting"}.md`;
}

// Summary template picker + custom-prompt field, tucked behind a disclosure so
// the default flow stays a single "Generate" button. Used by both the live
// summary panel and the detail view's regenerate controls.
interface SummaryControlsProps {
  templateOptions: SelectOption[];
  selectedTemplate: string | null;
  onSelectTemplate: (value: string | null) => void;
  customPrompt: string;
  onCustomPromptChange: (value: string) => void;
  disabled?: boolean;
}

export const SummaryControls: React.FC<SummaryControlsProps> = ({
  templateOptions,
  selectedTemplate,
  onSelectTemplate,
  customPrompt,
  onCustomPromptChange,
  disabled,
}) => {
  const { t } = useTranslation();
  const promptId = useId();
  return (
    <details className="group">
      <summary className="flex items-center gap-1 cursor-pointer list-none text-xs text-sub hover:text-logo-primary transition-colors select-none">
        <ChevronRight
          width={14}
          height={14}
          className="transition-transform group-open:rotate-90 rtl:rotate-180 rtl:group-open:rotate-90"
          aria-hidden
        />
        <span>{t("meeting.summaryOptions")}</span>
      </summary>
      <div className="mt-3 space-y-2">
        <div className="space-y-1">
          <p className="text-[11px] font-medium uppercase tracking-wide text-mid-gray">
            {t("meeting.template")}
          </p>
          <Select
            value={selectedTemplate}
            options={templateOptions}
            onChange={onSelectTemplate}
            isClearable={false}
            disabled={disabled}
            placeholder={t("meeting.template")}
          />
        </div>
        <div className="space-y-1">
          <label
            htmlFor={promptId}
            className="text-[11px] font-medium uppercase tracking-wide text-mid-gray"
          >
            {t("meeting.customPrompt")}
          </label>
          <textarea
            id={promptId}
            value={customPrompt}
            onChange={(e) => onCustomPromptChange(e.target.value)}
            placeholder={t("meeting.customPromptPlaceholder")}
            disabled={disabled}
            className="w-full min-h-[3rem] resize-y rounded-md border border-line bg-mid-gray/5 p-2 text-sm text-text/90 placeholder:text-faint focus:border-logo-primary focus:outline-none focus:ring-1 focus:ring-logo-primary disabled:opacity-50"
          />
        </div>
      </div>
    </details>
  );
};
