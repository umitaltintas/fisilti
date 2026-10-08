import React from "react";
import { useTranslation } from "react-i18next";
import { useShallow } from "zustand/react/shallow";
import { toast } from "sonner";
import { Loader2, Search, Sparkles, Trash2, X } from "lucide-react";

import { Button } from "../../ui/Button";
import { IconButton } from "../../ui/IconButton";
import { useConfirm } from "../../ui/ConfirmDialog";
import {
  InlineError,
  SectionHeading,
  formatDuration,
  formatMeetingTime,
} from "./shared";
import type { MeetingListItem } from "@/lib/meeting";
import { isLiveMeeting, useMeetingStore } from "@/stores/meetingStore";
import { errorMessage } from "@/lib/utils/errors";

interface MeetingHistoryProps {
  onOpen: (id: number) => void;
  /** Called after a meeting was deleted (e.g. to close its detail view). */
  onDeleted?: (id: number) => void;
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

const EmptyCard: React.FC<{ children: React.ReactNode }> = ({ children }) => (
  <div className="bg-background border border-mid-gray/20 rounded-lg px-4 py-8 text-center text-text/60 text-sm">
    {children}
  </div>
);

// The "History" tab: search + past meetings grouped by day.
export const MeetingHistory: React.FC<MeetingHistoryProps> = ({
  onOpen,
  onDeleted,
}) => {
  const { t, i18n } = useTranslation();
  const {
    meetings,
    meetingsState,
    meetingsError,
    query,
    setQuery,
    loadMeetings,
    deleteMeeting,
    status,
    meetingId,
  } = useMeetingStore(
    useShallow((s) => ({
      meetings: s.meetings,
      meetingsState: s.meetingsState,
      meetingsError: s.meetingsError,
      query: s.query,
      setQuery: s.setQuery,
      loadMeetings: s.loadMeetings,
      deleteMeeting: s.deleteMeeting,
      status: s.status,
      meetingId: s.meetingId,
    })),
  );
  const { confirm, dialog } = useConfirm();
  const groups = groupByDay(meetings, i18n.language, t);

  const handleDelete = async (meeting: MeetingListItem) => {
    const ok = await confirm({
      title: t("meeting.confirmDelete"),
      description: t("meeting.confirmDeleteText", {
        title: meeting.title.trim() || t("meeting.untitledMeeting"),
      }),
      destructive: true,
    });
    if (!ok) return;
    try {
      await deleteMeeting(meeting.id);
      onDeleted?.(meeting.id);
    } catch (error) {
      toast.error(t("meeting.deleteError"), {
        description: errorMessage(error),
      });
    }
  };

  let content: React.ReactNode;
  if (meetingsState === "loading" && meetings.length === 0) {
    content = (
      <EmptyCard>
        <span className="inline-flex items-center gap-2" role="status">
          <Loader2 width={14} height={14} className="animate-spin" />
          {t("meeting.loading")}
        </span>
      </EmptyCard>
    );
  } else if (meetingsState === "error") {
    content = (
      <div className="bg-background border border-mid-gray/20 rounded-lg px-4 py-6 flex flex-col items-center gap-3 text-center">
        <InlineError>
          {t("meeting.pastMeetingsError")}
          {meetingsError ? ` (${meetingsError})` : ""}
        </InlineError>
        <Button
          variant="secondary"
          size="sm"
          onClick={() => void loadMeetings()}
        >
          {t("common.retry")}
        </Button>
      </div>
    );
  } else if (meetings.length === 0) {
    content = (
      <EmptyCard>
        {query.trim().length > 0
          ? t("meeting.searchNoResults")
          : t("meeting.pastMeetingsEmpty")}
      </EmptyCard>
    );
  } else {
    content = groups.map((group) => (
      <section key={group.key} className="space-y-2">
        <SectionHeading className="px-1">{group.label}</SectionHeading>
        <ul className="bg-background border border-mid-gray/20 rounded-lg divide-y divide-mid-gray/20">
          {group.items.map((m) => (
            <PastMeetingRow
              key={m.id}
              meeting={m}
              locale={i18n.language}
              live={isLiveMeeting(m.id, { status, meetingId })}
              onOpen={() => onOpen(m.id)}
              onDelete={() => void handleDelete(m)}
            />
          ))}
        </ul>
      </section>
    ));
  }

  return (
    <div className="space-y-4">
      {dialog}
      <div className="relative">
        <Search
          width={15}
          height={15}
          className="absolute start-3 top-1/2 -translate-y-1/2 text-text/40 pointer-events-none"
          aria-hidden
        />
        <input
          type="search"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder={t("meeting.searchPlaceholder")}
          aria-label={t("meeting.search")}
          className="w-full rounded-md border border-mid-gray/20 bg-mid-gray/5 py-2 ps-9 pe-8 text-sm text-text placeholder:text-text/40 focus:border-logo-primary focus:outline-none focus:ring-1 focus:ring-logo-primary [&::-webkit-search-cancel-button]:hidden"
        />
        {query.length > 0 && (
          <IconButton
            onClick={() => setQuery("")}
            label={t("common.clear")}
            size="sm"
            className="absolute end-2 top-1/2 -translate-y-1/2"
          >
            <X width={14} height={14} />
          </IconButton>
        )}
      </div>

      {content}
    </div>
  );
};

interface PastMeetingRowProps {
  meeting: MeetingListItem;
  locale: string;
  /** The session recording right now: it cannot be deleted from here. */
  live: boolean;
  onOpen: () => void;
  onDelete: () => void;
}

const PastMeetingRow: React.FC<PastMeetingRowProps> = ({
  meeting,
  locale,
  live,
  onOpen,
  onDelete,
}) => {
  const { t } = useTranslation();
  const title = meeting.title.trim() || t("meeting.untitledMeeting");
  // A row that is still transcribing (or was interrupted, or is live) stays in
  // the list rather than vanishing — a session that takes minutes to finalize
  // should not look like it was lost. It cannot be deleted while it is in
  // that state: the backend still owns it.
  const processing = meeting.status !== "completed";

  return (
    <li className="px-4 py-3 flex items-start justify-between gap-3">
      <button
        type="button"
        onClick={onOpen}
        className="flex-1 min-w-0 text-start cursor-pointer group rounded focus:outline-none focus-visible:ring-1 focus-visible:ring-logo-primary"
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
          {live ? (
            <span className="shrink-0 inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] font-medium uppercase tracking-wide bg-red-500/15 text-red-400">
              <span
                className="h-1.5 w-1.5 rounded-full bg-red-500"
                aria-hidden
              />
              {t("meeting.recording")}
            </span>
          ) : (
            processing && (
              <span className="shrink-0 rounded px-1.5 py-0.5 text-[10px] font-medium uppercase tracking-wide bg-logo-primary/15 text-logo-primary">
                {t("meeting.stillProcessing")}
              </span>
            )
          )}
        </div>
        <div className="mt-0.5 flex items-center gap-2 text-xs text-text/50">
          <span className="tabular-nums">
            {formatMeetingTime(meeting.started_at, locale)}
          </span>
          <span aria-hidden>•</span>
          <span className="tabular-nums">
            {formatDuration(meeting.duration_ms, t)}
          </span>
        </div>
        {meeting.transcript_preview.trim().length > 0 && (
          <p className="mt-1 text-xs text-text/60 line-clamp-2 break-words">
            {meeting.transcript_preview}
          </p>
        )}
      </button>
      {!live && !processing && (
        <IconButton
          tone="danger"
          onClick={onDelete}
          label={t("meeting.deleteNamed", { title })}
          className="shrink-0"
        >
          <Trash2 width={16} height={16} />
        </IconButton>
      )}
    </li>
  );
};
