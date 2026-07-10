import React, { useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, ChevronRight, Copy, Lock } from "lucide-react";

import { Select, type SelectOption } from "../../ui/Select";
import type { SummaryProviderInfo, TranscriptSegment } from "@/lib/meeting";

export const NOTES_AUTOSAVE_MS = 800;
export const SEARCH_DEBOUNCE_MS = 300;

export const CopyButton: React.FC<{
  onCopy: () => void;
  disabled?: boolean;
  title: string;
  copiedTitle: string;
}> = ({ onCopy, disabled, title, copiedTitle }) => {
  const [copied, setCopied] = useState(false);

  const handleClick = () => {
    onCopy();
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  return (
    <button
      onClick={handleClick}
      disabled={disabled}
      title={copied ? copiedTitle : title}
      className="p-1.5 rounded-md flex items-center justify-center transition-colors cursor-pointer text-text/50 hover:text-logo-primary disabled:cursor-not-allowed disabled:text-text/20"
    >
      {copied ? (
        <Check width={16} height={16} />
      ) : (
        <Copy width={16} height={16} />
      )}
    </button>
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
}> = ({ segments }) => (
  <div className="space-y-2">
    {segments.map((seg, i) => (
      <p
        key={i}
        className="text-sm whitespace-pre-wrap break-words select-text text-text/90"
      >
        {seg.text}
      </p>
    ))}
  </div>
);

// Build a copy-friendly transcript: one line per segment, no speaker labels.
// Falls back to the plain joined transcript when no segments are available.
export const plainTranscriptText = (
  segments: TranscriptSegment[],
  fallback: string,
): string => {
  if (segments.length === 0) return fallback;
  return segments.map((seg) => seg.text).join("\n");
};

// Persistent "100% on-device transcription" trust badge.
export const OnDeviceBadge: React.FC = () => {
  const { t } = useTranslation();
  return (
    <span className="inline-flex items-center gap-1.5 rounded-full bg-logo-primary/10 px-2.5 py-1 text-[11px] font-medium text-logo-primary">
      <Lock width={12} height={12} />
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
      <p className="text-[11px] text-text/40">
        {t("meeting.summaryNoProvider")}
      </p>
    );
  }
  if (info.location === "local") {
    return (
      <p className="inline-flex items-center gap-1.5 text-[11px] text-emerald-500">
        <Lock width={11} height={11} />
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

// A labeled on/off switch with a supporting description.
export const MeetingToggle: React.FC<{
  checked: boolean;
  onToggle: () => void;
  label: string;
  description?: string;
}> = ({ checked, onToggle, label, description }) => (
  <div className="flex items-start gap-2.5">
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      onClick={onToggle}
      className={`relative mt-0.5 inline-flex h-5 w-9 shrink-0 items-center rounded-full transition-colors ${
        checked ? "bg-logo-primary" : "bg-mid-gray/30"
      }`}
    >
      <span
        className={`inline-block h-4 w-4 transform rounded-full bg-white shadow transition-transform ${
          checked ? "translate-x-4" : "translate-x-0.5"
        }`}
      />
    </button>
    <div className="min-w-0 cursor-pointer select-none" onClick={onToggle}>
      <p className="text-sm text-text/80">{label}</p>
      {description && (
        <p className="mt-0.5 text-xs text-text/50">{description}</p>
      )}
    </div>
  </div>
);

// Small uppercase section heading used across the meeting panels.
export const SectionHeading: React.FC<{
  children: React.ReactNode;
  className?: string;
}> = ({ children, className = "" }) => (
  <h2
    className={`text-xs font-medium text-mid-gray uppercase tracking-wide ${className}`}
  >
    {children}
  </h2>
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

// Format a duration in ms as `Hh Mm` (>= 1h) or `mm:ss` otherwise.
export function formatDuration(durationMs: number): string {
  const totalSeconds = Math.max(0, Math.floor(durationMs / 1000));
  const hours = Math.floor(totalSeconds / 3600);
  const mins = Math.floor((totalSeconds % 3600) / 60);
  const secs = totalSeconds % 60;
  if (hours > 0) {
    return `${hours}h ${mins}m`;
  }
  return `${mins.toString().padStart(2, "0")}:${secs.toString().padStart(2, "0")}`;
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
  return (
    <details className="group">
      <summary className="flex items-center gap-1 cursor-pointer list-none text-xs text-text/50 hover:text-logo-primary transition-colors select-none">
        <ChevronRight
          width={14}
          height={14}
          className="transition-transform group-open:rotate-90"
        />
        <span>{t("meeting.summaryOptions")}</span>
      </summary>
      <div className="mt-3 space-y-2">
        <div className="space-y-1">
          <label className="text-[11px] font-medium uppercase tracking-wide text-mid-gray">
            {t("meeting.template")}
          </label>
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
          <label className="text-[11px] font-medium uppercase tracking-wide text-mid-gray">
            {t("meeting.customPrompt")}
          </label>
          <textarea
            value={customPrompt}
            onChange={(e) => onCustomPromptChange(e.target.value)}
            placeholder={t("meeting.customPromptPlaceholder")}
            disabled={disabled}
            className="w-full min-h-[3rem] resize-y rounded-md border border-mid-gray/20 bg-mid-gray/5 p-2 text-sm text-text/90 placeholder:text-text/40 focus:border-logo-primary focus:outline-none focus:ring-1 focus:ring-logo-primary disabled:opacity-50"
          />
        </div>
      </div>
    </details>
  );
};
