import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { emit } from "@tauri-apps/api/event";
import { ArrowRight, Loader2, Mic, Radio } from "lucide-react";
import { commands, type HistoryEntry } from "@/bindings";
import { useMeetingStore } from "@/stores/meetingStore";
import { useModelStore } from "@/stores/modelStore";
import { useSettingsStore } from "@/stores/settingsStore";
import { formatKeyCombination } from "@/lib/utils/keyboard";
import { Button } from "../../ui/Button";
import {
  formatDuration,
  formatElapsed,
  formatMeetingTime,
} from "../meeting/shared";

const RECENT_COUNT = 3;

const goTo = (section: string) => {
  void emit("navigate-section", section);
};

/** "Good morning" style greeting for the current hour. */
const greetingKey = (hour: number) =>
  hour < 5
    ? "home.greeting.evening"
    : hour < 12
      ? "home.greeting.morning"
      : hour < 18
        ? "home.greeting.afternoon"
        : "home.greeting.evening";

/** Today, yesterday, or a short date, for list rows. */
const useRelativeDay = () => {
  const { t, i18n } = useTranslation();
  return (epochMs: number) => {
    const date = new Date(epochMs);
    const today = new Date();
    const startOf = (d: Date) =>
      new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
    const days = Math.round((startOf(today) - startOf(date)) / 86_400_000);
    if (days === 0) return t("home.today");
    if (days === 1) return t("home.yesterday");
    return new Intl.DateTimeFormat(i18n.language, {
      day: "numeric",
      month: "short",
    }).format(date);
  };
};

const SectionHeader: React.FC<{
  title: string;
  onShowAll: () => void;
}> = ({ title, onShowAll }) => {
  const { t } = useTranslation();
  return (
    <div className="flex items-baseline justify-between px-1 pb-1.5">
      <h2 className="text-xs font-semibold text-sub">{title}</h2>
      <button
        type="button"
        onClick={onShowAll}
        className="flex cursor-pointer items-center gap-1 rounded text-xs font-medium text-brand-text hover:underline focus:outline-none focus-visible:ring-2 focus-visible:ring-logo-primary/50"
      >
        {t("home.showAll")}
        <ArrowRight className="h-3 w-3 rtl:rotate-180" aria-hidden />
      </button>
    </div>
  );
};

/** Leading icon tile shared by the two rows of the "now" card. */
const RowIcon: React.FC<{ children: React.ReactNode }> = ({ children }) => (
  <span className="grid h-8 w-8 shrink-0 place-items-center rounded-lg bg-brand-soft text-brand-text">
    {children}
  </span>
);

const DictationRow: React.FC = () => {
  const { t } = useTranslation();
  const binding = useSettingsStore(
    (s) => s.settings?.bindings?.transcribe?.current_binding ?? "",
  );
  const keys = binding
    ? formatKeyCombination(binding, "macos").split(" + ")
    : [];

  return (
    <div className="flex items-center gap-3 px-4 py-3">
      <RowIcon>
        <Mic className="h-4 w-4" aria-hidden />
      </RowIcon>
      <div className="min-w-0 flex-1">
        <h2 className="text-[13px] font-semibold">
          {t("home.dictation.title")}
        </h2>
        <p className="truncate text-xs text-sub">
          {t("home.dictation.description")}
        </p>
      </div>
      {keys.length > 0 && (
        <button
          type="button"
          onClick={() => goTo("general")}
          title={t("home.dictation.change")}
          aria-label={`${t("home.dictation.shortcut")}: ${keys.join(" + ")}. ${t("home.dictation.change")}`}
          className="flex cursor-pointer gap-1 rounded-md p-0.5 focus:outline-none focus-visible:ring-2 focus-visible:ring-logo-primary/50"
        >
          {keys.map((key) => (
            <kbd
              key={key}
              className="min-w-6 rounded-[5px] bg-chip px-1.5 py-0.5 text-center font-sans text-xs font-medium shadow-[inset_0_-1px_0_var(--color-line)]"
            >
              {key}
            </kbd>
          ))}
        </button>
      )}
    </div>
  );
};

const MeetingRow: React.FC = () => {
  const { t } = useTranslation();
  const status = useMeetingStore((s) => s.status);
  const startedAtMs = useMeetingStore((s) => s.startedAtMs);
  const sessionTitle = useMeetingStore((s) => s.sessionTitle);
  const busy = useMeetingStore((s) => s.busy);
  const start = useMeetingStore((s) => s.start);
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    if (status !== "running") return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [status]);

  let detail: React.ReactNode;
  let action: React.ReactNode;
  if (status === "running") {
    const elapsed = startedAtMs ? (now - startedAtMs) / 1000 : 0;
    detail = (
      <p className="flex min-w-0 items-center gap-1.5 text-xs">
        <span className="rec-dot shrink-0" aria-hidden />
        <span className="font-medium text-rec tabular-nums">
          {formatElapsed(elapsed)}
        </span>
        <span className="truncate text-sub">
          {sessionTitle || t("home.meeting.untitled")}
        </span>
      </p>
    );
    action = (
      <Button variant="secondary" size="sm" onClick={() => goTo("meeting")}>
        {t("home.meeting.open")}
      </Button>
    );
  } else if (status === "finalizing") {
    detail = (
      <p className="flex items-center gap-1.5 text-xs text-sub">
        <Loader2 className="h-3 w-3 animate-spin" aria-hidden />
        {t("home.meeting.finalizing")}
      </p>
    );
  } else {
    detail = (
      <p className="truncate text-xs text-sub">
        {t("home.meeting.description")}
      </p>
    );
    action = (
      <div className="flex shrink-0 gap-1.5">
        <Button variant="ghost" size="sm" onClick={() => goTo("meeting")}>
          {t("home.meeting.import")}
        </Button>
        <Button
          variant="primary"
          size="sm"
          disabled={busy}
          onClick={() => {
            void start();
            goTo("meeting");
          }}
        >
          {t("home.meeting.start")}
        </Button>
      </div>
    );
  }

  return (
    <div className="flex items-center gap-3 px-4 py-3">
      <RowIcon>
        <Radio className="h-4 w-4" aria-hidden />
      </RowIcon>
      <div className="min-w-0 flex-1">
        <h2 className="text-[13px] font-semibold">{t("home.meeting.title")}</h2>
        {detail}
      </div>
      {action}
    </div>
  );
};

const RecentMeetings: React.FC = () => {
  const { t, i18n } = useTranslation();
  const relativeDay = useRelativeDay();
  const meetings = useMeetingStore((s) => s.meetings);
  const meetingsState = useMeetingStore((s) => s.meetingsState);
  const query = useMeetingStore((s) => s.query);
  const loadMeetings = useMeetingStore((s) => s.loadMeetings);
  const requestOpen = useMeetingStore((s) => s.requestOpen);

  useEffect(() => {
    void loadMeetings();
  }, [loadMeetings]);

  // A search typed on the Meetings page filters the shared list; Home only
  // shows the newest meetings when nothing is filtered.
  const recent = query ? [] : meetings.slice(0, RECENT_COUNT);

  return (
    <section>
      <SectionHeader
        title={t("home.recentMeetings")}
        onShowAll={() => goTo("meeting")}
      />
      {meetingsState === "loading" && recent.length === 0 ? (
        <div className="card h-16 animate-pulse" />
      ) : recent.length === 0 ? (
        <p className="card px-4 py-3 text-[13px] text-faint">
          {t("home.noMeetings")}
        </p>
      ) : (
        <ul className="card divide-y divide-line">
          {recent.map((meeting) => {
            const live = meeting.status === "recording";
            return (
              <li key={meeting.id}>
                <button
                  type="button"
                  onClick={() => {
                    requestOpen(meeting.id);
                    goTo("meeting");
                  }}
                  className="grid w-full cursor-pointer grid-cols-[1fr_auto] gap-x-3 gap-y-0.5 px-4 py-2.5 text-start first:rounded-t-[10px] last:rounded-b-[10px] hover:bg-chip/60 focus:outline-none focus-visible:ring-2 focus-visible:ring-logo-primary/50"
                >
                  <span className="truncate text-[13px] font-medium">
                    {meeting.title || t("home.meeting.untitled")}
                  </span>
                  <span className="self-center text-end text-xs whitespace-nowrap text-faint tabular-nums">
                    {live ? (
                      <span className="rounded-full bg-rec/10 px-2 py-0.5 font-semibold text-rec">
                        {t("home.meeting.live")}
                      </span>
                    ) : (
                      `${relativeDay(meeting.started_at)} ${formatMeetingTime(meeting.started_at, i18n.language)} · ${formatDuration(meeting.duration_ms, t)}`
                    )}
                  </span>
                  <span className="truncate text-xs text-sub">
                    {meeting.transcript_preview || " "}
                  </span>
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
};

const RecentDictations: React.FC = () => {
  const { t, i18n } = useTranslation();
  const relativeDay = useRelativeDay();
  const [entries, setEntries] = useState<HistoryEntry[] | null>(null);

  useEffect(() => {
    let cancelled = false;
    void commands.getHistoryEntries(null, RECENT_COUNT).then((result) => {
      if (!cancelled)
        setEntries(result.status === "ok" ? result.data.entries : []);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <section>
      <SectionHeader
        title={t("home.recentDictations")}
        onShowAll={() => goTo("history")}
      />
      {entries === null ? (
        <div className="card h-16 animate-pulse" />
      ) : entries.length === 0 ? (
        <p className="card px-4 py-3 text-[13px] text-faint">
          {t("home.noDictations")}
        </p>
      ) : (
        <ul className="card divide-y divide-line">
          {entries.map((entry) => (
            <li
              key={entry.id}
              className="grid grid-cols-[1fr_auto] gap-3 px-4 py-2.5"
            >
              <p className="line-clamp-2 text-[13px] leading-snug">
                {entry.post_processed_text || entry.transcription_text}
              </p>
              <span className="text-xs whitespace-nowrap text-faint tabular-nums">
                {relativeDay(entry.timestamp * 1000)}{" "}
                {formatMeetingTime(entry.timestamp * 1000, i18n.language)}
              </span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
};

/** The first page: what the app does, what is happening now, and what was
 * captured recently. Everything configurable lives on the other pages. */
export const HomePage: React.FC = () => {
  const { t } = useTranslation();
  const hour = new Date().getHours();

  return (
    <div className="w-full max-w-3xl space-y-7">
      <header>
        <h1 className="font-serif text-[26px] leading-tight font-semibold tracking-tight">
          {t(greetingKey(hour))}
        </h1>
        <p className="mt-1 text-[13px] text-sub">{t("home.subtitle")}</p>
      </header>
      <section className="card divide-y divide-line">
        <MeetingRow />
        <DictationRow />
      </section>
      <RecentMeetings />
      <RecentDictations />
    </div>
  );
};
