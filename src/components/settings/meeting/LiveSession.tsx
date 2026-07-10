import React, { useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import {
  ChevronRight,
  Loader2,
  Mic,
  RefreshCw,
  Sparkles,
  Square,
} from "lucide-react";

import { Button } from "../../ui/Button";
import type { SelectOption } from "../../ui/Select";
import { MeetingSignal } from "./MeetingSignal";
import { Markdown } from "./Markdown";
import {
  CopyButton,
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
  MeetingListItem,
  MeetingSummaryTemplate,
  SummaryProviderInfo,
  TranscriptSegment,
} from "@/lib/meeting";

const RECENT_MEETINGS_COUNT = 3;

interface LiveSessionProps {
  isRunning: boolean;
  busy: boolean;
  elapsed: number;
  error: string | null;
  finalizing: boolean;
  selectedIsCloud: boolean;
  transcript: string;
  liveSegments: TranscriptSegment[];
  userNotes: string;
  notesSaving: boolean;
  hasSavedMeeting: boolean;
  summary: string;
  summarizing: boolean;
  summaryError: string | null;
  templates: MeetingSummaryTemplate[];
  selectedTemplate: string | null;
  onSelectTemplate: (value: string | null) => void;
  customPrompt: string;
  onCustomPromptChange: (value: string) => void;
  providerInfo: SummaryProviderInfo | null;
  recentMeetings: MeetingListItem[];
  interrupted: InterruptedMeeting[];
  recoveringId: number | null;
  recoverError: string | null;
  onStart: () => void;
  onStop: () => void;
  onSummarize: () => void;
  onUserNotesChange: (value: string) => void;
  onOpenMeeting: (id: number) => void;
  onViewAllMeetings: () => void;
  onRecover: (id: number) => void;
  onDiscardInterrupted: (id: number) => void;
  onCopy: (text: string) => void;
}

// The "Session" tab: a state-driven workspace. Idle shows a start hero +
// recent meetings; recording shows the live notes/transcript workspace;
// just-finished keeps the workspace and adds the summary panel.
export const LiveSession: React.FC<LiveSessionProps> = (props) => {
  const { t, i18n } = useTranslation();
  const {
    isRunning,
    busy,
    elapsed,
    error,
    finalizing,
    transcript,
    summary,
    userNotes,
    interrupted,
    onStart,
    onStop,
  } = props;

  const hasTranscript = transcript.trim().length > 0;
  // Whether there is a live or just-finished session worth showing the full
  // workspace for; otherwise the idle hero takes over.
  const hasSession =
    isRunning ||
    finalizing ||
    hasTranscript ||
    summary.trim().length > 0 ||
    userNotes.trim().length > 0;

  return (
    <div className="space-y-6">
      {interrupted.length > 0 && (
        <InterruptedBanner
          interrupted={interrupted}
          recoveringId={props.recoveringId}
          recoverError={props.recoverError}
          onRecover={props.onRecover}
          onDiscard={props.onDiscardInterrupted}
        />
      )}

      {isRunning ? (
        <div className="space-y-4">
          {/* Status bar: everything needed mid-meeting, nothing else. */}
          <div className="bg-background border border-mid-gray/20 rounded-lg px-4 py-3 flex items-center gap-4">
            <div className="flex items-center gap-2 text-sm text-text/80 shrink-0">
              <span className="relative flex h-2.5 w-2.5">
                <span className="absolute inline-flex h-full w-full rounded-full bg-red-500/70 animate-ping" />
                <span className="relative inline-flex h-2.5 w-2.5 rounded-full bg-red-500" />
              </span>
              <span>{t("meeting.recording")}</span>
              <span className="tabular-nums font-medium">
                {formatElapsed(elapsed)}
              </span>
            </div>
            <div className="flex-1 min-w-0">
              <MeetingSignal active={isRunning} variant="compact" />
            </div>
            <Button
              onClick={onStop}
              variant="danger"
              size="md"
              disabled={busy}
              className="flex items-center gap-2 shrink-0"
            >
              <Square width={16} height={16} />
              <span>{t("meeting.stopMeeting")}</span>
            </Button>
          </div>

          {error && (
            <p className="text-sm text-red-400 whitespace-pre-wrap break-words">
              {error}
            </p>
          )}

          <Workspace {...props} />
        </div>
      ) : hasSession ? (
        <div className="space-y-4">
          <div className="px-1 flex items-center justify-between gap-3 flex-wrap">
            <SectionHeading>{t("meeting.lastSession")}</SectionHeading>
            <div className="flex items-center gap-2">
              <OnDeviceBadge />
              <Button
                onClick={onStart}
                variant="primary"
                size="sm"
                disabled={busy}
                className="flex items-center gap-1.5"
              >
                <Mic width={14} height={14} />
                <span>{t("meeting.newMeeting")}</span>
              </Button>
            </div>
          </div>

          {error && (
            <p className="text-sm text-red-400 whitespace-pre-wrap break-words">
              {error}
            </p>
          )}

          <Workspace {...props} />
          <SummaryPanel {...props} />
        </div>
      ) : (
        <div className="space-y-6">
          {/* Idle hero: one clear action. */}
          <div className="bg-background border border-mid-gray/20 rounded-lg px-6 py-10 flex flex-col items-center text-center gap-4">
            <div className="flex h-14 w-14 items-center justify-center rounded-full bg-logo-primary/10">
              <Mic width={26} height={26} className="text-logo-primary" />
            </div>
            <div className="space-y-1">
              <h3 className="text-base font-medium text-text">
                {t("meeting.idleTitle")}
              </h3>
              <p className="text-sm text-text/50 max-w-sm">
                {t("meeting.idleDescription")}
              </p>
            </div>
            <Button
              onClick={onStart}
              variant="primary"
              size="lg"
              disabled={busy}
              className="flex items-center gap-2"
            >
              <Mic width={17} height={17} />
              <span>{t("meeting.startMeeting")}</span>
            </Button>
            <OnDeviceBadge />
            {error && (
              <p className="text-sm text-red-400 whitespace-pre-wrap break-words">
                {error}
              </p>
            )}
          </div>

          {props.recentMeetings.length > 0 && (
            <div className="space-y-2">
              <div className="px-1 flex items-center justify-between">
                <SectionHeading>{t("meeting.recentMeetings")}</SectionHeading>
                <button
                  onClick={props.onViewAllMeetings}
                  className="flex items-center gap-0.5 text-xs text-text/50 hover:text-logo-primary transition-colors cursor-pointer"
                >
                  <span>{t("meeting.viewAllMeetings")}</span>
                  <ChevronRight width={13} height={13} />
                </button>
              </div>
              <div className="bg-background border border-mid-gray/20 rounded-lg divide-y divide-mid-gray/20">
                {props.recentMeetings
                  .slice(0, RECENT_MEETINGS_COUNT)
                  .map((m) => (
                    <button
                      key={m.id}
                      onClick={() => props.onOpenMeeting(m.id)}
                      className="w-full px-4 py-3 text-left cursor-pointer group"
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
                      <div className="mt-0.5 flex items-center gap-2 text-xs text-text/50">
                        <span>
                          {formatMeetingDate(m.started_at, i18n.language)}
                        </span>
                        <span aria-hidden>•</span>
                        <span className="tabular-nums">
                          {formatDuration(m.duration_ms)}
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
const Workspace: React.FC<LiveSessionProps> = ({
  isRunning,
  finalizing,
  selectedIsCloud,
  transcript,
  liveSegments,
  userNotes,
  notesSaving,
  hasSavedMeeting,
  onUserNotesChange,
  onCopy,
}) => {
  const { t } = useTranslation();
  const transcriptRef = useRef<HTMLDivElement>(null);
  const hasTranscript = transcript.trim().length > 0;

  // Auto-scroll the transcript panel to the newest line.
  useEffect(() => {
    const el = transcriptRef.current;
    if (el) {
      el.scrollTop = el.scrollHeight;
    }
  }, [transcript, liveSegments]);

  return (
    <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
      {/* My notes */}
      <div className="space-y-2 min-w-0">
        <div className="px-1 flex items-center justify-between">
          <SectionHeading>{t("meeting.myNotes")}</SectionHeading>
          <div className="flex items-center gap-2">
            {hasSavedMeeting && (
              <span className="text-[10px] font-medium uppercase tracking-wide text-text/40">
                {notesSaving
                  ? t("meeting.notesSaving")
                  : t("meeting.notesSaved")}
              </span>
            )}
            <CopyButton
              onCopy={() => onCopy(userNotes)}
              disabled={userNotes.trim().length === 0}
              title={t("meeting.copyMyNotes")}
              copiedTitle={t("meeting.copied")}
            />
          </div>
        </div>
        <div className="bg-background border border-mid-gray/20 rounded-lg p-4">
          <textarea
            value={userNotes}
            onChange={(e) => onUserNotesChange(e.target.value)}
            placeholder={t("meeting.myNotesPlaceholder")}
            className="w-full h-56 resize-none bg-transparent text-sm text-text/90 placeholder:text-text/40 focus:outline-none"
          />
        </div>
      </div>

      {/* Live transcript */}
      <div className="space-y-2 min-w-0">
        <div className="px-1 flex items-center justify-between">
          <SectionHeading>{t("meeting.transcript")}</SectionHeading>
          <div className="flex items-center gap-2">
            {isRunning && (
              <span className="text-[10px] font-medium uppercase tracking-wide text-text/40">
                {t("meeting.livePreview")}
              </span>
            )}
            <CopyButton
              onCopy={() =>
                onCopy(plainTranscriptText(liveSegments, transcript))
              }
              disabled={!hasTranscript}
              title={t("meeting.copyTranscript")}
              copiedTitle={t("meeting.copied")}
            />
          </div>
        </div>
        <div className="relative">
          <div
            ref={transcriptRef}
            className="bg-background border border-mid-gray/20 rounded-lg p-4 h-[15.5rem] overflow-y-auto"
          >
            {liveSegments.length > 0 ? (
              <PlainTranscript segments={liveSegments} />
            ) : hasTranscript ? (
              <p className="text-sm text-text/90 whitespace-pre-wrap break-words select-text">
                {transcript}
              </p>
            ) : (
              <p className="text-sm text-text/40">
                {isRunning
                  ? selectedIsCloud
                    ? t("meeting.cloudLivePreviewOff")
                    : t("meeting.listening")
                  : t("meeting.transcriptEmpty")}
              </p>
            )}
          </div>
          {finalizing && (
            <div className="absolute inset-x-0 bottom-0 flex items-center justify-center gap-2 rounded-b-lg border-t border-mid-gray/20 bg-background/95 py-2 text-sm text-text/70 backdrop-blur-sm">
              <Loader2 width={15} height={15} className="animate-spin" />
              <span>{t("meeting.finalizing")}</span>
            </div>
          )}
        </div>
      </div>
    </div>
  );
};

// One-click summary generation; template + custom prompt live behind a small
// "options" disclosure so the default flow stays a single button.
const SummaryPanel: React.FC<LiveSessionProps> = ({
  transcript,
  isRunning,
  summary,
  summarizing,
  summaryError,
  templates,
  selectedTemplate,
  onSelectTemplate,
  customPrompt,
  onCustomPromptChange,
  providerInfo,
  onSummarize,
  onCopy,
}) => {
  const { t } = useTranslation();
  const hasTranscript = transcript.trim().length > 0;
  const hasSummary = summary.trim().length > 0;

  const templateOptions: SelectOption[] = templates.map((tpl) => ({
    value: tpl.id,
    label: tpl.name,
  }));

  return (
    <div className="space-y-2">
      <div className="px-1 flex items-center justify-between">
        <SectionHeading>{t("meeting.summary")}</SectionHeading>
        {hasSummary && (
          <CopyButton
            onCopy={() => onCopy(summary)}
            title={t("meeting.copyNotes")}
            copiedTitle={t("meeting.copied")}
          />
        )}
      </div>
      <div className="bg-background border border-mid-gray/20 rounded-lg p-4 space-y-3">
        <div className="flex flex-wrap items-center gap-3">
          <Button
            onClick={onSummarize}
            variant="primary-soft"
            size="md"
            disabled={!hasTranscript || isRunning || summarizing}
            className="flex items-center gap-2"
          >
            {hasSummary ? (
              <RefreshCw
                width={16}
                height={16}
                className={summarizing ? "animate-spin" : ""}
              />
            ) : (
              <Sparkles
                width={16}
                height={16}
                className={summarizing ? "animate-pulse" : ""}
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

        <p className="text-[11px] text-text/40">
          {t("meeting.generateSummaryHint")}
        </p>

        <SummaryControls
          templateOptions={templateOptions}
          selectedTemplate={selectedTemplate}
          onSelectTemplate={onSelectTemplate}
          customPrompt={customPrompt}
          onCustomPromptChange={onCustomPromptChange}
          disabled={!hasTranscript || isRunning || summarizing}
        />

        {summaryError && (
          <p className="text-sm text-red-400 whitespace-pre-wrap break-words">
            {summaryError}
          </p>
        )}

        {hasSummary && <Markdown>{summary}</Markdown>}
      </div>
    </div>
  );
};

const InterruptedBanner: React.FC<{
  interrupted: InterruptedMeeting[];
  recoveringId: number | null;
  recoverError: string | null;
  onRecover: (id: number) => void;
  onDiscard: (id: number) => void;
}> = ({ interrupted, recoveringId, recoverError, onRecover, onDiscard }) => {
  const { t } = useTranslation();
  return (
    <div className="space-y-2">
      {interrupted.map((m) => (
        <div
          key={m.id}
          className="bg-logo-primary/5 border border-logo-primary/30 rounded-lg p-4 flex items-start justify-between gap-3"
        >
          <div className="min-w-0">
            <p className="text-sm font-medium text-text">
              {t("meeting.recoverTitle")}
            </p>
            <p className="mt-0.5 text-xs text-text/60">
              {t("meeting.recoverDescription")}
            </p>
            {recoverError && recoveringId === null && (
              <p className="mt-1 text-xs text-red-400 break-words">
                {recoverError}
              </p>
            )}
          </div>
          <div className="shrink-0 flex items-center gap-1.5">
            <Button
              onClick={() => onRecover(m.id)}
              variant="primary-soft"
              size="sm"
              disabled={recoveringId !== null}
              className="flex items-center gap-1.5"
            >
              {recoveringId === m.id ? (
                <Loader2 width={14} height={14} className="animate-spin" />
              ) : (
                <RefreshCw width={14} height={14} />
              )}
              <span>
                {recoveringId === m.id
                  ? t("meeting.recovering")
                  : t("meeting.recover")}
              </span>
            </Button>
            <Button
              onClick={() => onDiscard(m.id)}
              variant="secondary"
              size="sm"
              disabled={recoveringId !== null}
            >
              {t("meeting.discard")}
            </Button>
          </div>
        </div>
      ))}
    </div>
  );
};
