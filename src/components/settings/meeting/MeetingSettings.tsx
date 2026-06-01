import React, { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { convertFileSrc } from "@tauri-apps/api/core";
import type { UnlistenFn } from "@tauri-apps/api/event";
import {
  ArrowLeft,
  Check,
  Copy,
  Loader2,
  Mic,
  Sparkles,
  Square,
  Trash2,
} from "lucide-react";

import { Button } from "../../ui/Button";
import { MeetingSignal } from "./MeetingSignal";
import {
  changeMeetingAutoSummarize,
  deleteMeeting,
  getMeeting,
  getMeetingAudioPath,
  getMeetingAutoSummarize,
  getMeetingStatus,
  getMeetingTranscript,
  listMeetings,
  listenMeetingFinalizing,
  listenMeetingSummary,
  listenMeetingTranscript,
  startMeeting,
  stopMeeting,
  summarizeMeeting,
  type MeetingListItem,
  type MeetingRecord,
  type MeetingStatus,
  type TranscriptSegment,
  type TranscriptSource,
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

// A speaker "chip" + body for a single labeled transcript segment. "You"
// (mic / local speaker) is accented and aligned right; "others" (system /
// remote) is neutral and aligned left, so the two sides read like a chat.
const SpeakerSegment: React.FC<{
  source: TranscriptSource;
  text: string;
  t: (key: string) => string;
}> = ({ source, text, t }) => {
  const isYou = source === "you";
  const label = isYou ? t("meeting.speakerYou") : t("meeting.speakerOthers");
  return (
    <div className={`flex ${isYou ? "justify-end" : "justify-start"}`}>
      <div
        className={`max-w-[85%] min-w-0 flex flex-col gap-1 ${
          isYou ? "items-end" : "items-start"
        }`}
      >
        <span
          className={`text-[10px] font-semibold uppercase tracking-wide px-1.5 py-0.5 rounded ${
            isYou
              ? "bg-logo-primary/15 text-logo-primary"
              : "bg-mid-gray/15 text-mid-gray"
          }`}
        >
          {label}
        </span>
        <p
          className={`text-sm whitespace-pre-wrap break-words select-text rounded-lg px-3 py-2 ${
            isYou
              ? "bg-logo-primary/10 text-text/90"
              : "bg-mid-gray/10 text-text/90"
          }`}
        >
          {text}
        </p>
      </div>
    </div>
  );
};

// Render a list of labeled segments as a chat-like transcript. Falls back to
// nothing when there are no segments (callers handle the empty/plain case).
const SpeakerTranscript: React.FC<{
  segments: TranscriptSegment[];
  t: (key: string) => string;
}> = ({ segments, t }) => (
  <div className="space-y-3">
    {segments.map((seg, i) => (
      <SpeakerSegment key={i} source={seg.source} text={seg.text} t={t} />
    ))}
  </div>
);

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
  // Accumulated labeled segments for the live (and final) transcript so we can
  // render per-source speaker labels. Replaced wholesale by the polished list
  // when the finalize pass completes.
  const [liveSegments, setLiveSegments] = useState<TranscriptSegment[]>([]);
  const [finalizing, setFinalizing] = useState(false);
  const [elapsed, setElapsed] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [notes, setNotes] = useState("");
  const [summarizing, setSummarizing] = useState(false);
  const [summaryError, setSummaryError] = useState<string | null>(null);
  const [autoSummarize, setAutoSummarize] = useState(false);

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
    let cancelled = false;

    void loadPastMeetings();

    // Read the persisted auto-summarize setting (raw get_app_settings).
    void getMeetingAutoSummarize().then((v) => {
      if (!cancelled) setAutoSummarize(v);
    });

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

    const unlisteners: UnlistenFn[] = [];
    const register = (p: Promise<UnlistenFn>) => {
      p.then((fn) => {
        if (cancelled) fn();
        else unlisteners.push(fn);
      }).catch((e) => {
        if (!cancelled) setError(String(e));
      });
    };

    register(
      listenMeetingTranscript((update) => {
        setTranscript(update.full_transcript);
        // Accumulate labeled segments for the live preview. The on-stop
        // finalize pass replaces the full transcript text but only re-emits
        // the LAST segment, so the polished labeled list is re-fetched from
        // the saved record in handleStop; here we just append live segments.
        setLiveSegments((prev) => [...prev, update.segment]);
      }),
    );

    register(
      listenMeetingFinalizing((value) => {
        setFinalizing(value);
      }),
    );

    register(
      listenMeetingSummary((summary) => {
        setNotes(summary);
      }),
    );

    return () => {
      cancelled = true;
      for (const fn of unlisteners) fn();
      stopTimer();
    };
  }, [startTimer, stopTimer, loadPastMeetings]);

  // Auto-scroll the transcript panel to the newest line.
  useEffect(() => {
    const el = transcriptRef.current;
    if (el) {
      el.scrollTop = el.scrollHeight;
    }
  }, [transcript, liveSegments]);

  const handleStart = async () => {
    setError(null);
    setBusy(true);
    try {
      await startMeeting();
      setNotes("");
      setSummaryError(null);
      setTranscript("");
      setLiveSegments([]);
      setFinalizing(false);
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
      // stopMeeting() resolves only after the finalize pass + persistence,
      // returning the polished full transcript. The finalize pass re-emits
      // only the last labeled segment, so to render the polished, interleaved
      // labeled transcript we re-fetch the just-saved record's segments.
      const finalTranscript = await stopMeeting();
      setTranscript(finalTranscript);
      setStatus("idle");
      stopTimer();
      // A new meeting was just saved; refresh the past-meetings list and pull
      // its polished labeled segments to replace the live preview.
      const items = await listMeetings().catch(() => [] as MeetingListItem[]);
      setPastMeetings(items);
      const newest = items[0];
      if (newest) {
        try {
          const record = await getMeeting(newest.id);
          if (record.segments.length > 0) {
            setLiveSegments(record.segments);
          }
        } catch {
          // Best-effort: keep the accumulated live segments as a fallback.
        }
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleToggleAutoSummarize = async () => {
    const next = !autoSummarize;
    setAutoSummarize(next);
    try {
      await changeMeetingAutoSummarize(next);
    } catch (e) {
      // Revert the optimistic toggle on failure.
      setAutoSummarize(!next);
      setSummaryError(String(e));
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

          <label className="flex items-center gap-2.5 cursor-pointer select-none">
            <button
              type="button"
              role="switch"
              aria-checked={autoSummarize}
              onClick={handleToggleAutoSummarize}
              className={`relative inline-flex h-5 w-9 shrink-0 items-center rounded-full transition-colors ${
                autoSummarize ? "bg-logo-primary" : "bg-mid-gray/30"
              }`}
            >
              <span
                className={`inline-block h-4 w-4 transform rounded-full bg-white shadow transition-transform ${
                  autoSummarize ? "translate-x-4" : "translate-x-0.5"
                }`}
              />
            </button>
            <span
              className="text-sm text-text/80"
              onClick={handleToggleAutoSummarize}
            >
              {t("meeting.autoSummarize")}
            </span>
          </label>

          {isRunning && <MeetingSignal active={isRunning} />}
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
              <div className="flex items-center gap-2">
                {isRunning && (
                  <span className="text-[10px] font-medium uppercase tracking-wide text-text/40">
                    {t("meeting.livePreview")}
                  </span>
                )}
                <CopyButton
                  onCopy={copyTranscript}
                  disabled={!hasTranscript}
                  title={t("meeting.copyTranscript")}
                  copiedTitle={t("meeting.copied")}
                />
              </div>
            </div>
            <div className="relative">
              <div
                ref={transcriptRef}
                className="bg-background border border-mid-gray/20 rounded-lg p-4 h-64 overflow-y-auto"
              >
                {liveSegments.length > 0 ? (
                  <SpeakerTranscript segments={liveSegments} t={t} />
                ) : hasTranscript ? (
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
              {finalizing && (
                <div className="absolute inset-x-0 bottom-0 flex items-center justify-center gap-2 rounded-b-lg border-t border-mid-gray/20 bg-background/95 py-2 text-sm text-text/70 backdrop-blur-sm">
                  <Loader2 width={15} height={15} className="animate-spin" />
                  <span>{t("meeting.finalizing")}</span>
                </div>
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
  const labeledSegments = detail?.segments ?? [];
  const hasLabeledSegments = labeledSegments.length > 0;
  const summary = detail?.summary?.trim() ?? "";

  // Resolve a playable audio URL for the saved recording, if any. Older
  // meetings have no `audio_path`; we ask the backend for the absolute path
  // and wrap it with convertFileSrc so the asset protocol can serve it.
  const [audioSrc, setAudioSrc] = useState<string | null>(null);
  const detailId = detail?.id;
  const detailHasAudioPath = !!detail?.audio_path;
  useEffect(() => {
    let cancelled = false;
    setAudioSrc(null);
    if (detailId == null || !detailHasAudioPath) return;
    getMeetingAudioPath(detailId)
      .then((path) => {
        if (!cancelled) setAudioSrc(convertFileSrc(path));
      })
      .catch(() => {
        // Audio missing or unreadable; fall back to the no-audio hint.
      });
    return () => {
      cancelled = true;
    };
  }, [detailId, detailHasAudioPath]);

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
              {hasLabeledSegments ? (
                <SpeakerTranscript segments={labeledSegments} t={t} />
              ) : hasTranscript ? (
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
              <h2 className="text-xs font-medium text-mid-gray uppercase tracking-wide">
                {t("meeting.audio")}
              </h2>
              {audioSrc ? (
                <audio
                  controls
                  src={audioSrc}
                  className="w-full"
                  preload="metadata"
                />
              ) : (
                <p className="text-sm text-text/40">{t("meeting.noAudio")}</p>
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
