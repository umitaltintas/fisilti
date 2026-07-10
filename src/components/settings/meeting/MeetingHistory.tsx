import React from "react";
import { useTranslation } from "react-i18next";
import { Search, Sparkles, Trash2, X } from "lucide-react";

import { Button } from "../../ui/Button";
import { SectionHeading, formatDuration, formatMeetingTime } from "./shared";
import type { MeetingListItem } from "@/lib/meeting";

interface MeetingHistoryProps {
  meetings: MeetingListItem[];
  error: string | null;
  searchQuery: string;
  onSearchChange: (value: string) => void;
  confirmDeleteId: number | null;
  onRequestDelete: (id: number) => void;
  onCancelDelete: () => void;
  onConfirmDelete: (id: number) => void;
  onOpen: (id: number) => void;
}

interface DayGroup {
  key: string;
  label: string;
  items: MeetingListItem[];
}

function dayKey(epochMs: number): string {
  const d = new Date(epochMs);
  return `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`;
}

function dayLabel(
  epochMs: number,
  locale: string,
  t: (key: string) => string,
): string {
  const date = new Date(epochMs);
  if (isNaN(date.getTime())) return "";
  const now = new Date();
  const startOfDay = (d: Date) =>
    new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const diffDays = Math.round(
    (startOfDay(now) - startOfDay(date)) / 86_400_000,
  );
  if (diffDays === 0) return t("meeting.today");
  if (diffDays === 1) return t("meeting.yesterday");
  try {
    return new Intl.DateTimeFormat(locale, {
      weekday: "short",
      month: "short",
      day: "numeric",
      ...(date.getFullYear() !== now.getFullYear()
        ? { year: "numeric" as const }
        : {}),
    }).format(date);
  } catch {
    return date.toDateString();
  }
}

// Chunk the (newest-first) list into consecutive same-day groups.
function groupByDay(
  meetings: MeetingListItem[],
  locale: string,
  t: (key: string) => string,
): DayGroup[] {
  const groups: DayGroup[] = [];
  for (const m of meetings) {
    const key = dayKey(m.started_at);
    const last = groups[groups.length - 1];
    if (last && last.key === key) {
      last.items.push(m);
    } else {
      groups.push({
        key,
        label: dayLabel(m.started_at, locale, t),
        items: [m],
      });
    }
  }
  return groups;
}

// The "History" tab: search + past meetings grouped by day.
export const MeetingHistory: React.FC<MeetingHistoryProps> = ({
  meetings,
  error,
  searchQuery,
  onSearchChange,
  confirmDeleteId,
  onRequestDelete,
  onCancelDelete,
  onConfirmDelete,
  onOpen,
}) => {
  const { t, i18n } = useTranslation();
  const groups = groupByDay(meetings, i18n.language, t);

  return (
    <div className="space-y-4">
      <div className="relative">
        <Search
          width={15}
          height={15}
          className="absolute left-3 top-1/2 -translate-y-1/2 text-text/40 pointer-events-none"
        />
        <input
          type="text"
          value={searchQuery}
          onChange={(e) => onSearchChange(e.target.value)}
          placeholder={t("meeting.searchPlaceholder")}
          className="w-full rounded-md border border-mid-gray/20 bg-mid-gray/5 py-2 pl-9 pr-8 text-sm text-text placeholder:text-text/40 focus:border-logo-primary focus:outline-none focus:ring-1 focus:ring-logo-primary"
        />
        {searchQuery.length > 0 && (
          <button
            onClick={() => onSearchChange("")}
            className="absolute right-2 top-1/2 -translate-y-1/2 p-1 text-text/40 hover:text-logo-primary cursor-pointer"
            title={t("meeting.dismiss")}
          >
            <X width={14} height={14} />
          </button>
        )}
      </div>

      {error && (
        <p className="text-sm text-red-400 whitespace-pre-wrap break-words">
          {error}
        </p>
      )}

      {meetings.length === 0 ? (
        <div className="bg-background border border-mid-gray/20 rounded-lg px-4 py-8 text-center text-text/60 text-sm">
          {searchQuery.trim().length > 0
            ? t("meeting.searchNoResults")
            : t("meeting.pastMeetingsEmpty")}
        </div>
      ) : (
        groups.map((group) => (
          <div key={group.key} className="space-y-2">
            <SectionHeading className="px-1">{group.label}</SectionHeading>
            <div className="bg-background border border-mid-gray/20 rounded-lg divide-y divide-mid-gray/20">
              {group.items.map((m) => (
                <PastMeetingRow
                  key={m.id}
                  meeting={m}
                  locale={i18n.language}
                  confirming={confirmDeleteId === m.id}
                  onOpen={() => onOpen(m.id)}
                  onRequestDelete={() => onRequestDelete(m.id)}
                  onCancelDelete={onCancelDelete}
                  onConfirmDelete={() => onConfirmDelete(m.id)}
                />
              ))}
            </div>
          </div>
        ))
      )}
    </div>
  );
};

interface PastMeetingRowProps {
  meeting: MeetingListItem;
  locale: string;
  confirming: boolean;
  onOpen: () => void;
  onRequestDelete: () => void;
  onCancelDelete: () => void;
  onConfirmDelete: () => void;
}

const PastMeetingRow: React.FC<PastMeetingRowProps> = ({
  meeting,
  locale,
  confirming,
  onOpen,
  onRequestDelete,
  onCancelDelete,
  onConfirmDelete,
}) => {
  const { t } = useTranslation();
  const title = meeting.title.trim() || t("meeting.untitledMeeting");
  return (
    <div className="px-4 py-3 flex items-start justify-between gap-3">
      <button
        onClick={onOpen}
        className="flex-1 min-w-0 text-left cursor-pointer group"
      >
        <div className="flex items-center gap-2">
          <p className="text-sm font-medium text-text group-hover:text-logo-primary transition-colors truncate">
            {title}
          </p>
          {meeting.has_summary && (
            <Sparkles
              width={14}
              height={14}
              className="shrink-0 text-logo-primary"
              aria-label={t("meeting.hasSummary")}
            />
          )}
        </div>
        <div className="mt-0.5 flex items-center gap-2 text-xs text-text/50">
          <span className="tabular-nums">
            {formatMeetingTime(meeting.started_at, locale)}
          </span>
          <span aria-hidden>•</span>
          <span className="tabular-nums">
            {formatDuration(meeting.duration_ms)}
          </span>
        </div>
        {meeting.transcript_preview.trim().length > 0 && (
          <p className="mt-1 text-xs text-text/60 line-clamp-2 break-words">
            {meeting.transcript_preview}
          </p>
        )}
      </button>
      <div className="shrink-0 flex items-center gap-1">
        {confirming ? (
          <>
            <Button
              onClick={onConfirmDelete}
              variant="danger"
              size="sm"
              title={t("meeting.confirmDelete")}
            >
              {t("meeting.confirm")}
            </Button>
            <Button onClick={onCancelDelete} variant="secondary" size="sm">
              {t("meeting.cancel")}
            </Button>
          </>
        ) : (
          <button
            onClick={onRequestDelete}
            title={t("meeting.delete")}
            className="p-1.5 rounded-md flex items-center justify-center transition-colors cursor-pointer text-text/50 hover:text-red-400"
          >
            <Trash2 width={16} height={16} />
          </button>
        )}
      </div>
    </div>
  );
};
