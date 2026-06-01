import React, { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  ArrowLeft,
  Check,
  Copy,
  Mic,
  Sparkles,
  Square,
  Trash2,
} from "lucide-react";

import { Button } from "../../ui/Button";
import {
  deleteMeeting,
  getMeeting,
  getMeetingStatus,
  getMeetingTranscript,
  listMeetings,
  listenMeetingTranscript,
  startMeeting,
  stopMeeting,
  summarizeMeeting,
  type MeetingListItem,
  type MeetingRecord,
  type MeetingStatus,
} from "@/lib/meeting";

const CopyButton: React.FC<{
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

function formatElapsed(seconds: number): string {
  const mins = Math.floor(seconds / 60);
  const secs = seconds % 60;
  return `${mins.toString().padStart(2, "0")}:${secs.toString().padStart(2, "0")}`;
}

// Format an epoch-ms timestamp using the user's locale (no hardcoded format).
function formatMeetingDate(epochMs: number, locale: string): string {
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

// Format a duration in ms as `Hh Mm` (>= 1h) or `mm:ss` otherwise.
function formatDuration(durationMs: number): string {
  const totalSeconds = Math.max(0, Math.floor(durationMs / 1000));
  const hours = Math.floor(totalSeconds / 3600);
  const mins = Math.floor((totalSeconds % 3600) / 60);
  const secs = totalSeconds % 60;
  if (hours > 0) {
    return `${hours}h ${mins}m`;
  }
  return `${mins.toString().padStart(2, "0")}:${secs.toString().padStart(2, "0")}`;
}

export const MeetingSettings: React.FC = () => {
  const { t, i18n } = useTranslation();

  const [status, setStatus] = useState<MeetingStatus>("idle");
  const [transcript, setTranscript] = useState("");
  const [elapsed, setElapsed] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [notes, setNotes] = useState("");
  const [summarizing, setSummarizing] = useState(false);
  const [summaryError, setSummaryError] = useState<string | null>(null);

  // Past meetings list + detail view.
  const [pastMeetings, setPastMeetings] = useState<MeetingListItem[]>([]);
  const [pastError, setPastError] = useState<string | null>(null);
  const [confirmDeleteId, setConfirmDeleteId] = useState<number | null>(null);
  const [detail, setDetail] = useState<MeetingRecord | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);
  const [detailError, setDetailError] = useState<string | null>(null);

  const transcriptRef = useRef<HTMLDivElement>(null);
  const timerRef = useRef<ReturnType<typeof setInterval> | null>(null);

  const isRunning = status === "running";

  const loadPastMeetings = useCallback(async () => {
    try {
      const items = await listMeetings();
      setPastMeetings(items);
      setPastError(null);
    } catch (e) {
      setPastError(String(e));
    }
  }, []);

  const stopTimer = useCallback(() => {
    if (timerRef.current !== null) {
      clearInterval(timerRef.current);
      timerRef.current = null;
    }
  }, []);

  const startTimer = useCallback(() => {
    stopTimer();
    timerRef.current = setInterval(() => {
      setElapsed((e) => e + 1);
    }, 1000);
  }, [stopTimer]);

  // Reflect an already-running session on mount and subscribe to updates.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;

    void loadPastMeetings();

    (async () => {
      try {
        const current = await getMeetingStatus();
        if (cancelled) return;
        setStatus(current);
        if (current === "running") {
          try {
            setTranscript(await getMeetingTranscript());
          } catch {
            // ignore: transcript fetch is best-effort
          }
          startTimer();
        }
      } catch (e) {
        if (!cancelled) setError(String(e));
      }
    })();

    listenMeetingTranscript((update) => {
      setTranscript(update.full_transcript);
    })
      .then((fn) => {
        if (cancelled) {
          fn();
        } else {
          unlisten = fn;
        }
      })
      .catch((e) => {
        if (!cancelled) setError(String(e));
      });

    return () => {
      cancelled = true;
      if (unlisten) unlisten();
      stopTimer();
    };
  }, [startTimer, stopTimer, loadPastMeetings]);

  // Auto-scroll the transcript panel to the newest line.
  useEffect(() => {
    const el = transcriptRef.current;
    if (el) {
      el.scrollTop = el.scrollHeight;
    }
  }, [transcript]);

  const handleStart = async () => {
    setError(null);
    setBusy(true);
    try {
      await startMeeting();
      setNotes("");
      setSummaryError(null);
      setTranscript("");
      setElapsed(0);
      setStatus("running");
      startTimer();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleStop = async () => {
    setError(null);
    setBusy(true);
    try {
      const finalTranscript = await stopMeeting();
      setTranscript(finalTranscript);
      setStatus("idle");
      stopTimer();
      // A new meeting was just saved; refresh the past-meetings list.
      void loadPastMeetings();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleSummarize = async () => {
    setSummaryError(null);
    setSummarizing(true);
    try {
      const result = await summarizeMeeting();
      setNotes(result);
    } catch (e) {
      setSummaryError(String(e));
    } finally {
      setSummarizing(false);
    }
  };

  const openDetail = async (id: number) => {
    setDetail(null);
    setDetailError(null);
    setDetailLoading(true);
    try {
      const record = await getMeeting(id);
      setDetail(record);
    } catch (e) {
      setDetailError(String(e));
    } finally {
      setDetailLoading(false);
    }
  };

  const closeDetail = () => {
    setDetail(null);
    setDetailError(null);
    setDetailLoading(false);
  };

  const handleDelete = async (id: number) => {
    setPastError(null);
    try {
      await deleteMeeting(id);
      setConfirmDeleteId(null);
      if (detail?.id === id) closeDetail();
      await loadPastMeetings();
    } catch (e) {
      setPastError(String(e));
    }
  };

  const copyText = (text: string) => {
    navigator.clipboard.writeText(text).catch((e) => {
      console.error("Failed to copy:", e);
    });
  };

  const copyTranscript = () => {
    navigator.clipboard.writeText(transcript).catch((e) => {
      console.error("Failed to copy transcript:", e);
    });
  };

  const copyNotes = () => {
    navigator.clipboard.writeText(notes).catch((e) => {
      console.error("Failed to copy notes:", e);
    });
  };

  const hasTranscript = transcript.trim().length > 0;

  return (
    <div className="max-w-3xl w-full mx-auto space-y-6">
      {/* Controls + status */}
      <div className="space-y-2">
        <div className="px-4 flex items-center justify-between">
          <h2 className="text-xs font-medium text-mid-gray uppercase tracking-wide">
            {t("meeting.title")}
          </h2>
          {isRunning && (
            <div className="flex items-center gap-2 text-sm text-text/80">
              <span className="relative flex h-2.5 w-2.5">
                <span className="absolute inline-flex h-full w-full rounded-full bg-red-500/70 animate-ping" />
                <span className="relative inline-flex h-2.5 w-2.5 rounded-full bg-red-500" />
              </span>
              <span>{t("meeting.recording")}</span>
              <span className="tabular-nums font-medium">
                {formatElapsed(elapsed)}
              </span>
            </div>
          )}
        </div>

        <div className="bg-background border border-mid-gray/20 rounded-lg p-4 space-y-3">
          <Button
            onClick={isRunning ? handleStop : handleStart}
            variant={isRunning ? "danger" : "primary"}
            size="md"
            disabled={busy}
            className="flex items-center gap-2"
          >
            {isRunning ? (
              <Square width={16} height={16} />
            ) : (
              <Mic width={16} height={16} />
            )}
            <span>
              {isRunning ? t("meeting.stopMeeting") : t("meeting.startMeeting")}
            </span>
          </Button>

          {error && (
            <p className="text-sm text-red-400 whitespace-pre-wrap break-words">
              {error}
            </p>
          )}
        </div>
      </div>

      {detail || detailLoading || detailError ? (
        <MeetingDetailView
          detail={detail}
          loading={detailLoading}
          error={detailError}
          locale={i18n.language}
          onBack={closeDetail}
          onCopy={copyText}
          t={t}
        />
      ) : (
        <>
          {/* Live transcript */}
          <div className="space-y-2">
            <div className="px-4 flex items-center justify-between">
              <h2 className="text-xs font-medium text-mid-gray uppercase tracking-wide">
                {t("meeting.transcript")}
              </h2>
              <CopyButton
                onCopy={copyTranscript}
                disabled={!hasTranscript}
                title={t("meeting.copyTranscript")}
                copiedTitle={t("meeting.copied")}
              />
            </div>
            <div
              ref={transcriptRef}
              className="bg-background border border-mid-gray/20 rounded-lg p-4 h-64 overflow-y-auto"
            >
              {hasTranscript ? (
                <p className="text-sm text-text/90 whitespace-pre-wrap break-words select-text">
                  {transcript}
                </p>
              ) : (
                <p className="text-sm text-text/40">
                  {isRunning
                    ? t("meeting.listening")
                    : t("meeting.transcriptEmpty")}
                </p>
              )}
            </div>
          </div>

          {/* Summarize + notes */}
          <div className="space-y-2">
            <div className="px-4 flex items-center justify-between">
              <h2 className="text-xs font-medium text-mid-gray uppercase tracking-wide">
                {t("meeting.notes")}
              </h2>
              {notes.trim().length > 0 && (
                <CopyButton
                  onCopy={copyNotes}
                  title={t("meeting.copyNotes")}
                  copiedTitle={t("meeting.copied")}
                />
              )}
            </div>
            <div className="bg-background border border-mid-gray/20 rounded-lg p-4 space-y-3">
              <Button
                onClick={handleSummarize}
                variant="primary-soft"
                size="md"
                disabled={!hasTranscript || isRunning || summarizing}
                className="flex items-center gap-2"
              >
                <Sparkles
                  width={16}
                  height={16}
                  className={summarizing ? "animate-pulse" : ""}
                />
                <span>
                  {summarizing
                    ? t("meeting.summarizing")
                    : t("meeting.summarize")}
                </span>
              </Button>

              {summaryError && (
                <p className="text-sm text-red-400 whitespace-pre-wrap break-words">
                  {summaryError}
                </p>
              )}

              {notes.trim().length > 0 && (
                <p className="text-sm text-text/90 whitespace-pre-wrap break-words select-text">
                  {notes}
                </p>
              )}
            </div>
          </div>
        </>
      )}

      {/* Past meetings */}
      <div className="space-y-2">
        <div className="px-4">
          <h2 className="text-xs font-medium text-mid-gray uppercase tracking-wide">
            {t("meeting.pastMeetings")}
          </h2>
        </div>
        <div className="bg-background border border-mid-gray/20 rounded-lg overflow-visible">
          {pastError && (
            <p className="px-4 py-3 text-sm text-red-400 whitespace-pre-wrap break-words">
              {pastError}
            </p>
          )}
          {pastMeetings.length === 0 ? (
            <div className="px-4 py-3 text-center text-text/60 text-sm">
              {t("meeting.pastMeetingsEmpty")}
            </div>
          ) : (
            <div className="divide-y divide-mid-gray/20">
              {pastMeetings.map((m) => (
                <PastMeetingRow
                  key={m.id}
                  meeting={m}
                  locale={i18n.language}
                  confirming={confirmDeleteId === m.id}
                  onOpen={() => openDetail(m.id)}
                  onRequestDelete={() => setConfirmDeleteId(m.id)}
                  onCancelDelete={() => setConfirmDeleteId(null)}
                  onConfirmDelete={() => handleDelete(m.id)}
                  t={t}
                />
              ))}
            </div>
          )}
        </div>
      </div>
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
  t: (key: string) => string;
}

const PastMeetingRow: React.FC<PastMeetingRowProps> = ({
  meeting,
  locale,
  confirming,
  onOpen,
  onRequestDelete,
  onCancelDelete,
  onConfirmDelete,
  t,
}) => {
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
          <span>{formatMeetingDate(meeting.started_at, locale)}</span>
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

interface MeetingDetailViewProps {
  detail: MeetingRecord | null;
  loading: boolean;
  error: string | null;
  locale: string;
  onBack: () => void;
  onCopy: (text: string) => void;
  t: (key: string) => string;
}

const MeetingDetailView: React.FC<MeetingDetailViewProps> = ({
  detail,
  loading,
  error,
  locale,
  onBack,
  onCopy,
  t,
}) => {
  const title = detail
    ? detail.title.trim() || t("meeting.untitledMeeting")
    : "";
  const hasTranscript = !!detail && detail.transcript.trim().length > 0;
  const summary = detail?.summary?.trim() ?? "";

  return (
    <div className="space-y-2">
      <div className="px-4 flex items-center justify-between gap-2">
        <button
          onClick={onBack}
          className="flex items-center gap-1.5 text-sm text-text/70 hover:text-logo-primary transition-colors cursor-pointer"
        >
          <ArrowLeft width={16} height={16} />
          <span>{t("meeting.back")}</span>
        </button>
        {detail && (
          <div className="flex items-center gap-2 text-xs text-text/50 min-w-0">
            <span className="truncate">
              {formatMeetingDate(detail.started_at, locale)}
            </span>
            <span aria-hidden>•</span>
            <span className="tabular-nums">
              {formatDuration(detail.duration_ms)}
            </span>
          </div>
        )}
      </div>

      <div className="bg-background border border-mid-gray/20 rounded-lg p-4 space-y-4">
        {loading && (
          <p className="text-sm text-text/60">{t("meeting.loading")}</p>
        )}

        {error && (
          <p className="text-sm text-red-400 whitespace-pre-wrap break-words">
            {error}
          </p>
        )}

        {detail && (
          <>
            <h3 className="text-base font-medium text-text break-words">
              {title}
            </h3>

            <div className="space-y-2">
              <div className="flex items-center justify-between">
                <h2 className="text-xs font-medium text-mid-gray uppercase tracking-wide">
                  {t("meeting.transcript")}
                </h2>
                <CopyButton
                  onCopy={() => onCopy(detail.transcript)}
                  disabled={!hasTranscript}
                  title={t("meeting.copyTranscript")}
                  copiedTitle={t("meeting.copied")}
                />
              </div>
              {hasTranscript ? (
                <p className="text-sm text-text/90 whitespace-pre-wrap break-words select-text">
                  {detail.transcript}
                </p>
              ) : (
                <p className="text-sm text-text/40">
                  {t("meeting.transcriptEmpty")}
                </p>
              )}
            </div>

            <div className="space-y-2">
              <div className="flex items-center justify-between">
                <h2 className="text-xs font-medium text-mid-gray uppercase tracking-wide">
                  {t("meeting.summary")}
                </h2>
                {summary.length > 0 && (
                  <CopyButton
                    onCopy={() => onCopy(summary)}
                    title={t("meeting.copySummary")}
                    copiedTitle={t("meeting.copied")}
                  />
                )}
              </div>
              {summary.length > 0 ? (
                <p className="text-sm text-text/90 whitespace-pre-wrap break-words select-text">
                  {summary}
                </p>
              ) : (
                <p className="text-sm text-text/40">{t("meeting.noSummary")}</p>
              )}
            </div>
          </>
        )}
      </div>
    </div>
  );
};
