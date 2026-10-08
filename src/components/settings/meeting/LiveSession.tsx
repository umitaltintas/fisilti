import React, { useEffect, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useShallow } from "zustand/react/shallow";
import { toast } from "sonner";
import {
  ChevronRight,
  Loader2,
  Mic,
  RefreshCw,
  Sparkles,
  Square,
} from "lucide-react";

import { Button } from "../../ui/Button";
import { useConfirm } from "../../ui/ConfirmDialog";
import type { SelectOption } from "../../ui/Select";
import { MeetingSignal } from "./MeetingSignal";
import { Markdown } from "./Markdown";
import {
  CopyButton,
  InlineError,
  NotesSaveIndicator,
  OnDeviceBadge,
  PlainTranscript,
  SectionHeading,
  SummaryControls,
  SummaryLocationNote,
  formatElapsed,
  formatMeetingDate,
  formatDuration,
  plainTranscriptText,
} from "./shared";
import type {
  InterruptedMeeting,
  MeetingSummaryTemplate,
  SummaryProviderInfo,
} from "@/lib/meeting";
import { useMeetingStore } from "@/stores/meetingStore";
import { errorMessage } from "@/lib/utils/errors";

const RECENT_MEETINGS_COUNT = 3;

interface LiveSessionProps {
  /** The meeting model is a cloud engine: there is no live preview. */
  selectedIsCloud: boolean;
  templates: MeetingSummaryTemplate[];
  providerInfo: SummaryProviderInfo | null;
  onOpenMeeting: (id: number) => void;
  onViewAllMeetings: () => void;
  /** The "import a recording" card, shown under the idle hero as the other
   * way to get a meeting transcript. */
  importSlot?: React.ReactNode;
}

/** Seconds since `startedAtMs`, ticking while `active`. Derived from the
 * timestamp rather than counted, so it stays right after the window was
 * hidden or opened mid-meeting. */
const useElapsedSeconds = (startedAtMs: number | null, active: boolean) => {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    setNow(Date.now());
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, [active]);
  if (startedAtMs == null) return 0;
  return Math.max(0, Math.floor((now - startedAtMs) / 1000));
};

// The "Session" tab: a state-driven workspace. Idle shows a start hero +
// recent meetings; recording shows the live notes/transcript workspace;
// just-finished keeps the workspace and adds the summary panel.
//
// All session state comes from the meeting store, which lives for the whole
// app — switching pages mid-meeting loses nothing.
export const LiveSession: React.FC<LiveSessionProps> = (props) => {
  const { t, i18n } = useTranslation();
  const session = useMeetingStore(
    useShallow((s) => ({
      status: s.status,
      busy: s.busy,
      startedAtMs: s.startedAtMs,
      error: s.error,
      transcript: s.transcript,
      summary: s.summary,
      notes: s.notes,
      meetingId: s.meetingId,
      interrupted: s.interrupted,
      meetings: s.meetings,
      start: s.start,
      stop: s.stop,
    })),
  );

  const isRunning = session.status === "running";
  const finalizing = session.status === "finalizing";
  const elapsed = useElapsedSeconds(session.startedAtMs, isRunning);

  // Whether there is a live or just-finished session worth showing the full
  // workspace for; otherwise the idle hero takes over.
  const hasSession =
    isRunning ||
    finalizing ||
    session.meetingId != null ||
    session.transcript.trim().length > 0 ||
    session.summary.trim().length > 0 ||
    session.notes.trim().length > 0;

  const errorLine = session.error && <InlineError>{session.error}</InlineError>;

  return (
    <div className="space-y-6">
      {session.interrupted.length > 0 && (
        <InterruptedBanner interrupted={session.interrupted} />
      )}

      {isRunning || finalizing ? (
        <div className="relative">
          <SessionHeader />
          {errorLine}
          <Workspace selectedIsCloud={props.selectedIsCloud} />

          {/* Recording controls float over the page, out of the notes' way. */}
          <div className="sticky bottom-3 z-10 mx-auto mt-4 flex w-fit items-center gap-3 rounded-full bg-surface py-1.5 ps-4 pe-1.5 shadow-[0_0_0_1px_var(--color-line),0_8px_24px_-6px_rgb(0_0_0/0.18)]">
            <div className="flex items-center gap-2" role="status">
              {finalizing ? (
                <>
                  <Loader2
                    width={14}
                    height={14}
                    className="animate-spin text-logo-primary"
                    aria-hidden
                  />
                  <span className="text-[13px] text-sub">
                    {t("meeting.finalizing")}
                  </span>
                </>
              ) : (
                <>
                  <span className="rec-dot" aria-hidden />
                  <span className="sr-only">{t("meeting.recording")}</span>
                  <span className="text-[13px] font-semibold tabular-nums">
                    {formatElapsed(elapsed)}
                  </span>
                </>
              )}
            </div>
            {isRunning && (
              <div className="w-20">
                <MeetingSignal active={isRunning} variant="compact" />
              </div>
            )}
            <button
              type="button"
              onClick={() => void session.stop()}
              disabled={session.busy || finalizing}
              className="flex cursor-pointer items-center gap-1.5 rounded-full bg-text px-3.5 py-1.5 text-[13px] font-medium text-background transition-opacity hover:opacity-90 focus:outline-none focus-visible:ring-2 focus-visible:ring-logo-primary/50 disabled:cursor-not-allowed disabled:opacity-50"
            >
              <Square width={11} height={11} fill="currentColor" aria-hidden />
              <span>{t("meeting.stopMeeting")}</span>
            </button>
          </div>
        </div>
      ) : hasSession ? (
        <div className="space-y-4">
          <div className="px-1 flex items-center justify-between gap-3 flex-wrap">
            <SectionHeading>{t("meeting.lastSession")}</SectionHeading>
            <div className="flex items-center gap-2">
              <OnDeviceBadge />
              <Button
                onClick={() => void session.start()}
                variant="primary"
                size="sm"
                disabled={session.busy}
                className="flex items-center gap-1.5"
              >
                <Mic width={14} height={14} aria-hidden />
                <span>{t("meeting.newMeeting")}</span>
              </Button>
            </div>
          </div>

          {errorLine}

          <SessionHeader />
          <Workspace selectedIsCloud={props.selectedIsCloud} />
          <SummaryPanel
            templates={props.templates}
            providerInfo={props.providerInfo}
          />
        </div>
      ) : (
        <div className="space-y-6">
          {/* Idle hero: one clear action. */}
          <div className="card px-6 py-10 flex flex-col items-center text-center gap-4">
            <div className="flex h-14 w-14 items-center justify-center rounded-full bg-logo-primary/10">
              <Mic
                width={26}
                height={26}
                className="text-logo-primary"
                aria-hidden
              />
            </div>
            <div className="space-y-1">
              <h3 className="text-base font-medium text-text">
                {t("meeting.idleTitle")}
              </h3>
              <p className="text-sm text-sub max-w-sm">
                {t("meeting.idleDescription")}
              </p>
            </div>
            <Button
              onClick={() => void session.start()}
              variant="primary"
              size="lg"
              disabled={session.busy}
              className="flex items-center gap-2"
            >
              {session.busy ? (
                <Loader2
                  width={17}
                  height={17}
                  className="animate-spin"
                  aria-hidden
                />
              ) : (
                <Mic width={17} height={17} aria-hidden />
              )}
              <span>{t("meeting.startMeeting")}</span>
            </Button>
            <OnDeviceBadge />
            {errorLine}
          </div>

          {props.importSlot}

          {session.meetings.length > 0 && (
            <div className="space-y-2">
              <div className="px-1 flex items-center justify-between">
                <SectionHeading>{t("meeting.recentMeetings")}</SectionHeading>
                <button
                  type="button"
                  onClick={props.onViewAllMeetings}
                  className="flex items-center gap-0.5 rounded text-xs text-sub hover:text-logo-primary transition-colors cursor-pointer focus:outline-none focus-visible:ring-1 focus-visible:ring-logo-primary"
                >
                  <span>{t("meeting.viewAllMeetings")}</span>
                  <ChevronRight
                    width={13}
                    height={13}
                    className="rtl:rotate-180"
                    aria-hidden
                  />
                </button>
              </div>
              <div className="card divide-y divide-line">
                {session.meetings.slice(0, RECENT_MEETINGS_COUNT).map((m) => (
                  <button
                    type="button"
                    key={m.id}
                    onClick={() => props.onOpenMeeting(m.id)}
                    className="w-full px-4 py-3 text-start cursor-pointer group focus:outline-none focus-visible:bg-mid-gray/10"
                  >
                    <div className="flex items-center gap-2">
                      <p className="text-sm font-medium text-text group-hover:text-logo-primary transition-colors truncate">
                        {m.title.trim() || t("meeting.untitledMeeting")}
                      </p>
                      {m.has_summary && (
                        <Sparkles
                          width={14}
                          height={14}
                          className="shrink-0 text-logo-primary"
                          aria-label={t("meeting.hasSummary")}
                        />
                      )}
                    </div>
                    <div className="mt-0.5 flex items-center gap-2 text-xs text-sub">
                      <span>
                        {formatMeetingDate(m.started_at, i18n.language)}
                      </span>
                      <span aria-hidden>•</span>
                      <span className="tabular-nums">
                        {formatDuration(m.duration_ms, t)}
                      </span>
                    </div>
                  </button>
                ))}
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  );
};

// Notes + live transcript, side by side on wide windows.
const Workspace: React.FC<{ selectedIsCloud: boolean }> = ({
  selectedIsCloud,
}) => {
  const { t } = useTranslation();
  const notesId = useId();
  const {
    status,
    transcript,
    segments,
    notes,
    notesSave,
    notesError,
    setNotes,
  } = useMeetingStore(
    useShallow((s) => ({
      status: s.status,
      transcript: s.transcript,
      segments: s.segments,
      notes: s.notes,
      notesSave: s.notesSave,
      notesError: s.notesError,
      setNotes: s.setNotes,
    })),
  );
  const isRunning = status === "running";
  const finalizing = status === "finalizing";
  const transcriptRef = useRef<HTMLDivElement>(null);
  const hasTranscript = transcript.trim().length > 0;

  // Auto-scroll the transcript panel to the newest line.
  useEffect(() => {
    const el = transcriptRef.current;
    if (el) {
      el.scrollTop = el.scrollHeight;
    }
  }, [transcript, segments]);

  return (
    <div className="grid grid-cols-1 gap-6 md:grid-cols-[minmax(0,1fr)_14rem]">
      {/* My notes: the page itself, not a boxed field. */}
      <div className="min-w-0">
        <div className="flex items-center justify-between gap-2 pb-1">
          <SectionHeading id={`${notesId}-heading`}>
            {t("meeting.myNotes")}
          </SectionHeading>
          <div className="flex items-center gap-2">
            <NotesSaveIndicator state={notesSave} error={notesError} />
            <CopyButton
              text={notes}
              disabled={notes.trim().length === 0}
              label={t("meeting.copyMyNotes")}
            />
          </div>
        </div>
        <textarea
          id={notesId}
          aria-labelledby={`${notesId}-heading`}
          value={notes}
          onChange={(e) => setNotes(e.target.value)}
          placeholder={t("meeting.myNotesPlaceholder")}
          className="min-h-[18rem] w-full resize-none bg-transparent font-serif text-[15.5px] leading-7 text-text placeholder:text-faint focus:outline-none"
        />
        {notesSave === "error" && notesError && (
          <InlineError className="text-xs">
            {t("meeting.errors.notesSaveFailed", { error: notesError })}
          </InlineError>
        )}
      </div>

      {/* Live transcript: a quiet rail beside the notes. */}
      <div className="min-w-0 md:border-s md:border-line md:ps-4">
        <div className="flex items-center justify-between pb-2">
          <SectionHeading>{t("meeting.transcript")}</SectionHeading>
          <div className="flex items-center gap-1.5">
            {isRunning && (
              <span className="text-[11px] text-faint">
                {t("meeting.livePreview")}
              </span>
            )}
            <CopyButton
              text={plainTranscriptText(segments, transcript)}
              disabled={!hasTranscript && segments.length === 0}
              label={t("meeting.copyTranscript")}
            />
          </div>
        </div>
        <div
          ref={transcriptRef}
          // A scrollable region must be reachable by keyboard.
          // eslint-disable-next-line jsx-a11y/no-noninteractive-tabindex
          tabIndex={0}
          aria-label={t("meeting.transcript")}
          aria-live={isRunning ? "polite" : undefined}
          className="max-h-[22rem] overflow-y-auto rounded focus:outline-none focus-visible:ring-2 focus-visible:ring-logo-primary/40"
        >
          {segments.length > 0 ? (
            <PlainTranscript segments={segments} compact />
          ) : hasTranscript ? (
            <p className="text-xs leading-relaxed whitespace-pre-wrap break-words text-sub select-text">
              {transcript}
            </p>
          ) : (
            <p className="text-xs text-faint">
              {isRunning
                ? selectedIsCloud
                  ? t("meeting.cloudLivePreviewOff")
                  : t("meeting.listening")
                : finalizing
                  ? t("meeting.finalizing")
                  : t("meeting.transcriptEmpty")}
            </p>
          )}
        </div>
      </div>
    </div>
  );
};

/** Date and title of the current (or just-finished) meeting, set like the top
 * of a notebook page. */
const SessionHeader: React.FC = () => {
  const { t, i18n } = useTranslation();
  const { startedAtMs, sessionTitle } = useMeetingStore(
    useShallow((s) => ({
      startedAtMs: s.startedAtMs,
      sessionTitle: s.sessionTitle,
    })),
  );
  return (
    <header className="pb-4">
      {startedAtMs != null && (
        <p className="text-xs text-faint">
          {formatMeetingDate(startedAtMs, i18n.language)}
        </p>
      )}
      <h2 className="mt-1 font-serif text-[26px] leading-tight font-semibold tracking-tight">
        {sessionTitle?.trim() || t("meeting.untitledMeeting")}
      </h2>
    </header>
  );
};

// One-click summary generation; template + custom prompt live behind a small
// "options" disclosure so the default flow stays a single button.
const SummaryPanel: React.FC<{
  templates: MeetingSummaryTemplate[];
  providerInfo: SummaryProviderInfo | null;
}> = ({ templates, providerInfo }) => {
  const { t } = useTranslation();
  const { transcript, status, summary, summarizing, summaryError, summarize } =
    useMeetingStore(
      useShallow((s) => ({
        transcript: s.transcript,
        status: s.status,
        summary: s.summary,
        summarizing: s.summarizing,
        summaryError: s.summaryError,
        summarize: s.summarize,
      })),
    );
  const [selectedTemplate, setSelectedTemplate] = useState<string | null>(null);
  const [customPrompt, setCustomPrompt] = useState("");

  useEffect(() => {
    setSelectedTemplate((cur) => cur ?? templates[0]?.id ?? null);
  }, [templates]);

  const hasTranscript = transcript.trim().length > 0;
  const hasSummary = summary.trim().length > 0;
  const disabled = !hasTranscript || status !== "idle" || summarizing;

  const templateOptions: SelectOption[] = templates.map((tpl) => ({
    value: tpl.id,
    label: tpl.name,
  }));

  const handleSummarize = () => {
    // A free-text custom prompt overrides the dropdown selection.
    const custom = customPrompt.trim();
    void summarize(
      custom.length > 0 ? custom : (selectedTemplate ?? undefined),
    );
  };

  return (
    <div className="space-y-2">
      <div className="px-1 flex items-center justify-between">
        <SectionHeading>{t("meeting.summary")}</SectionHeading>
        {hasSummary && (
          <CopyButton text={summary} label={t("meeting.copySummary")} />
        )}
      </div>
      <div className="card p-4 space-y-3">
        <div className="flex flex-wrap items-center gap-3">
          <Button
            onClick={handleSummarize}
            variant="primary-soft"
            size="md"
            disabled={disabled}
            className="flex items-center gap-2"
          >
            {hasSummary ? (
              <RefreshCw
                width={16}
                height={16}
                className={summarizing ? "animate-spin" : ""}
                aria-hidden
              />
            ) : (
              <Sparkles
                width={16}
                height={16}
                className={summarizing ? "animate-pulse" : ""}
                aria-hidden
              />
            )}
            <span>
              {summarizing
                ? t("meeting.summarizing")
                : hasSummary
                  ? t("meeting.regenerate")
                  : t("meeting.generateSummary")}
            </span>
          </Button>
          <SummaryLocationNote info={providerInfo} />
        </div>

        <p className="text-[11px] text-faint">
          {t("meeting.generateSummaryHint")}
        </p>

        <SummaryControls
          templateOptions={templateOptions}
          selectedTemplate={selectedTemplate}
          onSelectTemplate={setSelectedTemplate}
          customPrompt={customPrompt}
          onCustomPromptChange={setCustomPrompt}
          disabled={disabled}
        />

        {summaryError && <InlineError>{summaryError}</InlineError>}

        {hasSummary && <Markdown>{summary}</Markdown>}
      </div>
    </div>
  );
};

/** One card per meeting that never finished. Says which kind of unfinished it
 * is, when it was and how long, and keeps errors next to the card they
 * belong to. */
const InterruptedBanner: React.FC<{
  interrupted: InterruptedMeeting[];
}> = ({ interrupted }) => {
  const { t, i18n } = useTranslation();
  const { recover, discard, meetings } = useMeetingStore(
    useShallow((s) => ({
      recover: s.recover,
      discard: s.discard,
      meetings: s.meetings,
    })),
  );
  const { confirm, dialog } = useConfirm();
  const [busyId, setBusyId] = useState<number | null>(null);
  const [errors, setErrors] = useState<Record<number, string>>({});

  const setError = (id: number, message: string | null) =>
    setErrors((prev) => {
      const next = { ...prev };
      if (message) next[id] = message;
      else delete next[id];
      return next;
    });

  const handleRecover = async (id: number) => {
    setError(id, null);
    setBusyId(id);
    try {
      await recover(id);
      toast.success(t("meeting.recovered"));
    } catch (error) {
      setError(
        id,
        t("meeting.errors.recoverFailed", { error: errorMessage(error) }),
      );
    } finally {
      setBusyId(null);
    }
  };

  const handleDiscard = async (m: InterruptedMeeting) => {
    const ok = await confirm({
      title: t("meeting.discardConfirmTitle"),
      description: m.has_buffers
        ? t("meeting.discardConfirmWithAudio")
        : t("meeting.discardConfirmText"),
      confirmLabel: t("meeting.discard"),
      destructive: true,
    });
    if (!ok) return;
    setError(m.id, null);
    setBusyId(m.id);
    try {
      await discard(m.id);
    } catch (error) {
      setError(
        m.id,
        t("meeting.errors.discardFailed", { error: errorMessage(error) }),
      );
    } finally {
      setBusyId(null);
    }
  };

  return (
    <div className="space-y-2">
      {dialog}
      {interrupted.map((m) => {
        // The interrupted record carries no duration; the list row does.
        const duration = meetings.find((x) => x.id === m.id)?.duration_ms;
        const error = errors[m.id];
        return (
          <div
            key={m.id}
            className="bg-logo-primary/5 border border-logo-primary/30 rounded-lg p-4 flex items-start justify-between gap-3"
          >
            <div className="min-w-0 space-y-0.5">
              <p className="text-sm font-medium text-text">
                {m.has_buffers
                  ? t("meeting.recoverTitleUnfinished")
                  : t("meeting.recoverTitleInterrupted")}
              </p>
              <p className="flex flex-wrap items-center gap-x-2 text-xs text-sub">
                <span className="truncate">
                  {m.title.trim() || t("meeting.untitledMeeting")}
                </span>
                <span aria-hidden>•</span>
                <span>{formatMeetingDate(m.started_at, i18n.language)}</span>
                {duration != null && duration > 0 && (
                  <>
                    <span aria-hidden>•</span>
                    <span className="tabular-nums">
                      {formatDuration(duration, t)}
                    </span>
                  </>
                )}
              </p>
              <p className="text-xs text-sub">
                {m.has_buffers
                  ? t("meeting.recoverDescriptionWithAudio")
                  : t("meeting.recoverDescriptionPartial")}
              </p>
              {error && <InlineError className="text-xs">{error}</InlineError>}
            </div>
            <div className="shrink-0 flex items-center gap-1.5">
              <Button
                onClick={() => void handleRecover(m.id)}
                variant="primary-soft"
                size="sm"
                disabled={busyId !== null}
                className="flex items-center gap-1.5"
              >
                {busyId === m.id ? (
                  <Loader2
                    width={14}
                    height={14}
                    className="animate-spin"
                    aria-hidden
                  />
                ) : (
                  <RefreshCw width={14} height={14} aria-hidden />
                )}
                <span>
                  {busyId === m.id
                    ? t("meeting.recovering")
                    : t("meeting.recover")}
                </span>
              </Button>
              <Button
                onClick={() => void handleDiscard(m)}
                variant="secondary"
                size="sm"
                disabled={busyId !== null}
              >
                {t("meeting.discard")}
              </Button>
            </div>
          </div>
        );
      })}
    </div>
  );
};
