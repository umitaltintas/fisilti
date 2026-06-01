import React, { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { convertFileSrc } from "@tauri-apps/api/core";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { save } from "@tauri-apps/plugin-dialog";
import { writeTextFile } from "@tauri-apps/plugin-fs";
import {
  ArrowLeft,
  Check,
  Copy,
  Download,
  Loader2,
  Lock,
  Mic,
  Pencil,
  RefreshCw,
  Search,
  Sparkles,
  Square,
  Trash2,
  X,
} from "lucide-react";

import { Button } from "../../ui/Button";
import { Select, type SelectOption } from "../../ui/Select";
import { ShortcutInput } from "../ShortcutInput";
import { MeetingSignal } from "./MeetingSignal";
import { Markdown } from "./Markdown";
import {
  changeMeetingAutoSummarize,
  deleteMeeting,
  exportMeetingMarkdown,
  getMeeting,
  getMeetingAudioPath,
  getMeetingAutoSummarize,
  getMeetingStatus,
  getMeetingSummaryTemplates,
  getMeetingTranscript,
  getSummaryProviderInfo,
  listInterruptedMeetings,
  listMeetings,
  listenMeetingFinalizing,
  listenMeetingSummary,
  listenMeetingTitle,
  listenMeetingTranscript,
  recoverMeeting,
  regenerateMeetingSummary,
  startMeeting,
  stopMeeting,
  summarizeMeetingWith,
  updateMeetingNotes,
  updateMeetingTitle,
  type InterruptedMeeting,
  type MeetingListItem,
  type MeetingRecord,
  type MeetingStatus,
  type MeetingSummaryTemplate,
  type SummaryProviderInfo,
  type TranscriptSegment,
  type TranscriptSource,
} from "@/lib/meeting";

const NOTES_AUTOSAVE_MS = 800;
const SEARCH_DEBOUNCE_MS = 300;

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

// Persistent "100% on-device transcription" trust badge.
const OnDeviceBadge: React.FC<{ t: (key: string) => string }> = ({ t }) => (
  <span className="inline-flex items-center gap-1.5 rounded-full bg-logo-primary/10 px-2.5 py-1 text-[11px] font-medium text-logo-primary">
    <Lock width={12} height={12} />
    {t("meeting.onDeviceBadge")}
  </span>
);

// Honest indicator for where the SUMMARY runs (local vs a cloud provider).
const SummaryLocationNote: React.FC<{
  info: SummaryProviderInfo | null;
  t: (key: string, opts?: Record<string, unknown>) => string;
}> = ({ info, t }) => {
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

function formatElapsed(seconds: number): string {
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

// Build a safe-ish default export filename from a meeting title.
function exportFilename(title: string): string {
  const base = title.trim() || "meeting";
  const slug = base
    .replace(/[\\/:*?"<>|]/g, "")
    .replace(/\s+/g, "-")
    .slice(0, 60);
  return `${slug || "meeting"}.md`;
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

  // The user's own editable notes for the in-progress / just-finished meeting.
  const [userNotes, setUserNotes] = useState("");
  const [notesSaving, setNotesSaving] = useState(false);
  // The AI summary for the live/just-finished meeting.
  const [summary, setSummary] = useState("");
  const [summarizing, setSummarizing] = useState(false);
  const [summaryError, setSummaryError] = useState<string | null>(null);
  const [autoSummarize, setAutoSummarize] = useState(false);

  // Summary template picker + custom prompt (shared by live + detail flows).
  const [templates, setTemplates] = useState<MeetingSummaryTemplate[]>([]);
  const [selectedTemplate, setSelectedTemplate] = useState<string | null>(null);
  const [customPrompt, setCustomPrompt] = useState("");

  // Trust indicator: where the summary provider runs.
  const [providerInfo, setProviderInfo] = useState<SummaryProviderInfo | null>(
    null,
  );

  // The id of the most recently saved meeting (so live notes/summary edits can
  // be persisted to the right row).
  const [currentMeetingId, setCurrentMeetingId] = useState<number | null>(null);

  // Past meetings list + detail view.
  const [pastMeetings, setPastMeetings] = useState<MeetingListItem[]>([]);
  const [pastError, setPastError] = useState<string | null>(null);
  const [confirmDeleteId, setConfirmDeleteId] = useState<number | null>(null);
  const [detail, setDetail] = useState<MeetingRecord | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);
  const [detailError, setDetailError] = useState<string | null>(null);

  // Search box (debounced -> list_meetings({query})).
  const [searchQuery, setSearchQuery] = useState("");

  // Crash-recovery banner.
  const [interrupted, setInterrupted] = useState<InterruptedMeeting[]>([]);
  const [recoveringId, setRecoveringId] = useState<number | null>(null);
  const [recoverError, setRecoverError] = useState<string | null>(null);

  const transcriptRef = useRef<HTMLDivElement>(null);
  const timerRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const notesSaveRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const searchRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const isRunning = status === "running";

  const loadPastMeetings = useCallback(async (query?: string) => {
    try {
      const items = await listMeetings(query);
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

    // Load summary templates for the picker + the provider trust info.
    void getMeetingSummaryTemplates().then((tpl) => {
      if (cancelled) return;
      setTemplates(tpl);
      setSelectedTemplate((cur) => cur ?? tpl[0]?.id ?? null);
    });
    void getSummaryProviderInfo().then((info) => {
      if (!cancelled) setProviderInfo(info);
    });

    // Crash-recovery: surface any interrupted meetings as a banner.
    void listInterruptedMeetings()
      .then((items) => {
        if (!cancelled) setInterrupted(items);
      })
      .catch(() => {
        // Best-effort; no banner on failure.
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
      listenMeetingSummary((s) => {
        setSummary(s);
      }),
    );

    // Auto-title pass updates the title after a meeting finishes; reflect it
    // live in the detail view + refresh the list.
    register(
      listenMeetingTitle((title) => {
        setDetail((prev) => (prev ? { ...prev, title } : prev));
        setPastMeetings((prev) =>
          prev.map((m, i) => (i === 0 ? { ...m, title } : m)),
        );
      }),
    );

    return () => {
      cancelled = true;
      for (const fn of unlisteners) fn();
      stopTimer();
    };
  }, [startTimer, stopTimer, loadPastMeetings]);

  // Debounced search: empty query -> all meetings.
  useEffect(() => {
    if (searchRef.current) clearTimeout(searchRef.current);
    searchRef.current = setTimeout(() => {
      void loadPastMeetings(searchQuery);
    }, SEARCH_DEBOUNCE_MS);
    return () => {
      if (searchRef.current) clearTimeout(searchRef.current);
    };
  }, [searchQuery, loadPastMeetings]);

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
      setUserNotes("");
      setSummary("");
      setSummaryError(null);
      setTranscript("");
      setLiveSegments([]);
      setFinalizing(false);
      setElapsed(0);
      setCurrentMeetingId(null);
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
      const items = await listMeetings(searchQuery).catch(
        () => [] as MeetingListItem[],
      );
      setPastMeetings(items);
      const newest = items[0];
      if (newest) {
        setCurrentMeetingId(newest.id);
        try {
          const record = await getMeeting(newest.id);
          if (record.segments.length > 0) {
            setLiveSegments(record.segments);
          }
          if (record.notes) setUserNotes(record.notes);
          if (record.summary) setSummary(record.summary);
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

  // Resolve the template argument to pass: a free-text custom prompt overrides
  // the dropdown selection when present.
  const resolveTemplateArg = useCallback((): string | undefined => {
    const custom = customPrompt.trim();
    if (custom.length > 0) return custom;
    return selectedTemplate ?? undefined;
  }, [customPrompt, selectedTemplate]);

  const handleSummarize = async () => {
    setSummaryError(null);
    setSummarizing(true);
    try {
      const result = await summarizeMeetingWith(resolveTemplateArg());
      setSummary(result);
      // The summary is persisted onto the last-saved row by the backend;
      // refresh the list so the "has summary" marker appears.
      void loadPastMeetings(searchQuery);
    } catch (e) {
      setSummaryError(String(e));
    } finally {
      setSummarizing(false);
    }
  };

  // Debounced autosave for the live user-notes textarea.
  const handleUserNotesChange = (value: string) => {
    setUserNotes(value);
    if (currentMeetingId == null) return;
    const id = currentMeetingId;
    if (notesSaveRef.current) clearTimeout(notesSaveRef.current);
    setNotesSaving(true);
    notesSaveRef.current = setTimeout(() => {
      updateMeetingNotes(id, value)
        .catch((e) => setSummaryError(String(e)))
        .finally(() => setNotesSaving(false));
    }, NOTES_AUTOSAVE_MS);
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
      await loadPastMeetings(searchQuery);
    } catch (e) {
      setPastError(String(e));
    }
  };

  const handleRecover = async (id: number) => {
    setRecoverError(null);
    setRecoveringId(id);
    try {
      await recoverMeeting(id);
      setInterrupted((prev) => prev.filter((m) => m.id !== id));
      await loadPastMeetings(searchQuery);
    } catch (e) {
      setRecoverError(String(e));
    } finally {
      setRecoveringId(null);
    }
  };

  const handleDiscardInterrupted = (id: number) => {
    // Discarding just dismisses the banner item; the row stays for now and
    // delete is available from the list once it appears as completed.
    setInterrupted((prev) => prev.filter((m) => m.id !== id));
  };

  const copyText = (text: string) => {
    navigator.clipboard.writeText(text).catch((e) => {
      console.error("Failed to copy:", e);
    });
  };

  const copyTranscript = () => copyText(transcript);
  const copyNotes = () => copyText(summary);
  const copyUserNotes = () => copyText(userNotes);

  const hasTranscript = transcript.trim().length > 0;

  const templateOptions: SelectOption[] = templates.map((tpl) => ({
    value: tpl.id,
    label: tpl.name,
  }));

  return (
    <div className="max-w-3xl w-full mx-auto space-y-6">
      {/* Crash-recovery banner */}
      {interrupted.length > 0 && (
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
                  onClick={() => handleRecover(m.id)}
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
                  onClick={() => handleDiscardInterrupted(m.id)}
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
      )}

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
          <div className="flex items-center justify-between gap-3 flex-wrap">
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
                {isRunning
                  ? t("meeting.stopMeeting")
                  : t("meeting.startMeeting")}
              </span>
            </Button>
            <OnDeviceBadge t={t} />
          </div>

          {error && (
            <p className="text-sm text-red-400 whitespace-pre-wrap break-words">
              {error}
            </p>
          )}

          {/* Optional global shortcut to start/stop a meeting without opening
              the window. Unbound by default; mirrors the tray quick-start. */}
          <ShortcutInput shortcutId="toggle_meeting" descriptionMode="inline" />


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
          templates={templates}
          providerInfo={providerInfo}
          onBack={closeDetail}
          onCopy={copyText}
          onRefreshList={() => loadPastMeetings(searchQuery)}
          setDetail={setDetail}
          t={t}
        />
      ) : (
        <>
          {/* Editable user notes (primary panel during recording) */}
          <div className="space-y-2">
            <div className="px-4 flex items-center justify-between">
              <h2 className="text-xs font-medium text-mid-gray uppercase tracking-wide">
                {t("meeting.myNotes")}
              </h2>
              <div className="flex items-center gap-2">
                {currentMeetingId != null && (
                  <span className="text-[10px] font-medium uppercase tracking-wide text-text/40">
                    {notesSaving
                      ? t("meeting.notesSaving")
                      : t("meeting.notesSaved")}
                  </span>
                )}
                <CopyButton
                  onCopy={copyUserNotes}
                  disabled={userNotes.trim().length === 0}
                  title={t("meeting.copyMyNotes")}
                  copiedTitle={t("meeting.copied")}
                />
              </div>
            </div>
            <div className="bg-background border border-mid-gray/20 rounded-lg p-4">
              <textarea
                value={userNotes}
                onChange={(e) => handleUserNotesChange(e.target.value)}
                placeholder={t("meeting.myNotesPlaceholder")}
                className="w-full min-h-[8rem] resize-y bg-transparent text-sm text-text/90 placeholder:text-text/40 focus:outline-none"
              />
            </div>
          </div>

          {/* Live transcript (secondary, collapsible) */}
          <details className="group space-y-2" open={isRunning}>
            <summary className="px-4 flex items-center justify-between cursor-pointer list-none">
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
            </summary>
            <div className="relative mt-2">
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
          </details>

          {/* AI summary */}
          <div className="space-y-2">
            <div className="px-4 flex items-center justify-between">
              <h2 className="text-xs font-medium text-mid-gray uppercase tracking-wide">
                {t("meeting.summary")}
              </h2>
              {summary.trim().length > 0 && (
                <CopyButton
                  onCopy={copyNotes}
                  title={t("meeting.copyNotes")}
                  copiedTitle={t("meeting.copied")}
                />
              )}
            </div>
            <div className="bg-background border border-mid-gray/20 rounded-lg p-4 space-y-3">
              <SummaryControls
                templateOptions={templateOptions}
                selectedTemplate={selectedTemplate}
                onSelectTemplate={setSelectedTemplate}
                customPrompt={customPrompt}
                onCustomPromptChange={setCustomPrompt}
                disabled={!hasTranscript || isRunning || summarizing}
                t={t}
              />

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
                    : t("meeting.generateFromNotes")}
                </span>
              </Button>

              <SummaryLocationNote info={providerInfo} t={t} />

              {summaryError && (
                <p className="text-sm text-red-400 whitespace-pre-wrap break-words">
                  {summaryError}
                </p>
              )}

              {summary.trim().length > 0 && <Markdown>{summary}</Markdown>}
            </div>
          </div>
        </>
      )}

      {/* Past meetings */}
      <div className="space-y-2">
        <div className="px-4 flex items-center justify-between gap-3">
          <h2 className="text-xs font-medium text-mid-gray uppercase tracking-wide">
            {t("meeting.pastMeetings")}
          </h2>
        </div>
        <div className="px-4">
          <div className="relative">
            <Search
              width={15}
              height={15}
              className="absolute left-3 top-1/2 -translate-y-1/2 text-text/40 pointer-events-none"
            />
            <input
              type="text"
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              placeholder={t("meeting.searchPlaceholder")}
              className="w-full rounded-md border border-mid-gray/20 bg-mid-gray/5 py-2 pl-9 pr-8 text-sm text-text placeholder:text-text/40 focus:border-logo-primary focus:outline-none focus:ring-1 focus:ring-logo-primary"
            />
            {searchQuery.length > 0 && (
              <button
                onClick={() => setSearchQuery("")}
                className="absolute right-2 top-1/2 -translate-y-1/2 p-1 text-text/40 hover:text-logo-primary cursor-pointer"
                title={t("meeting.dismiss")}
              >
                <X width={14} height={14} />
              </button>
            )}
          </div>
        </div>
        <div className="bg-background border border-mid-gray/20 rounded-lg overflow-visible">
          {pastError && (
            <p className="px-4 py-3 text-sm text-red-400 whitespace-pre-wrap break-words">
              {pastError}
            </p>
          )}
          {pastMeetings.length === 0 ? (
            <div className="px-4 py-3 text-center text-text/60 text-sm">
              {searchQuery.trim().length > 0
                ? t("meeting.searchNoResults")
                : t("meeting.pastMeetingsEmpty")}
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

// Shared summary template picker + custom-prompt field. Used by both the live
// summary panel and the detail view's regenerate controls.
interface SummaryControlsProps {
  templateOptions: SelectOption[];
  selectedTemplate: string | null;
  onSelectTemplate: (value: string | null) => void;
  customPrompt: string;
  onCustomPromptChange: (value: string) => void;
  disabled?: boolean;
  t: (key: string) => string;
}

const SummaryControls: React.FC<SummaryControlsProps> = ({
  templateOptions,
  selectedTemplate,
  onSelectTemplate,
  customPrompt,
  onCustomPromptChange,
  disabled,
  t,
}) => (
  <div className="space-y-2">
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
);

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
  templates: MeetingSummaryTemplate[];
  providerInfo: SummaryProviderInfo | null;
  onBack: () => void;
  onCopy: (text: string) => void;
  onRefreshList: () => void;
  setDetail: React.Dispatch<React.SetStateAction<MeetingRecord | null>>;
  t: (key: string, opts?: Record<string, unknown>) => string;
}

const MeetingDetailView: React.FC<MeetingDetailViewProps> = ({
  detail,
  loading,
  error,
  locale,
  templates,
  providerInfo,
  onBack,
  onCopy,
  onRefreshList,
  setDetail,
  t,
}) => {
  const title = detail
    ? detail.title.trim() || t("meeting.untitledMeeting")
    : "";
  const hasTranscript = !!detail && detail.transcript.trim().length > 0;
  const labeledSegments = detail?.segments ?? [];
  const hasLabeledSegments = labeledSegments.length > 0;
  const summary = detail?.summary?.trim() ?? "";

  // Inline title rename.
  const [editingTitle, setEditingTitle] = useState(false);
  const [titleDraft, setTitleDraft] = useState("");

  // Editable user notes in the detail view (debounced autosave).
  const [notes, setNotes] = useState("");
  const [notesSaving, setNotesSaving] = useState(false);
  const notesSaveRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Regenerate controls.
  const [selectedTemplate, setSelectedTemplate] = useState<string | null>(null);
  const [customPrompt, setCustomPrompt] = useState("");
  const [regenerating, setRegenerating] = useState(false);
  const [regenError, setRegenError] = useState<string | null>(null);

  // Export.
  const [exporting, setExporting] = useState(false);
  const [exportErr, setExportErr] = useState<string | null>(null);

  const detailId = detail?.id;

  // Sync local editable state when the detail record loads/changes.
  useEffect(() => {
    setNotes(detail?.notes ?? "");
    setTitleDraft(detail?.title ?? "");
    setEditingTitle(false);
    setRegenError(null);
    setExportErr(null);
  }, [detailId, detail?.notes, detail?.title]);

  useEffect(() => {
    setSelectedTemplate((cur) => cur ?? templates[0]?.id ?? null);
  }, [templates]);

  // Resolve a playable audio URL for the saved recording, if any. Older
  // meetings have no `audio_path`; we ask the backend for the absolute path
  // and wrap it with convertFileSrc so the asset protocol can serve it.
  const [audioSrc, setAudioSrc] = useState<string | null>(null);
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

  const handleSaveTitle = async () => {
    if (detailId == null) return;
    const next = titleDraft.trim();
    setEditingTitle(false);
    if (next === (detail?.title ?? "").trim()) return;
    try {
      await updateMeetingTitle(detailId, next);
      setDetail((prev) => (prev ? { ...prev, title: next } : prev));
      onRefreshList();
    } catch (e) {
      setRegenError(String(e));
    }
  };

  const handleNotesChange = (value: string) => {
    setNotes(value);
    if (detailId == null) return;
    const id = detailId;
    if (notesSaveRef.current) clearTimeout(notesSaveRef.current);
    setNotesSaving(true);
    notesSaveRef.current = setTimeout(() => {
      updateMeetingNotes(id, value)
        .catch((e) => setRegenError(String(e)))
        .finally(() => setNotesSaving(false));
    }, NOTES_AUTOSAVE_MS);
  };

  const handleRegenerate = async () => {
    if (detailId == null) return;
    setRegenError(null);
    setRegenerating(true);
    const custom = customPrompt.trim();
    const arg = custom.length > 0 ? custom : (selectedTemplate ?? undefined);
    try {
      const result = await regenerateMeetingSummary(detailId, arg);
      setDetail((prev) => (prev ? { ...prev, summary: result } : prev));
      onRefreshList();
    } catch (e) {
      setRegenError(String(e));
    } finally {
      setRegenerating(false);
    }
  };

  const handleExport = async () => {
    if (detailId == null) return;
    setExportErr(null);
    setExporting(true);
    try {
      const markdown = await exportMeetingMarkdown(detailId);
      const path = await save({
        defaultPath: exportFilename(title),
        filters: [{ name: "Markdown", extensions: ["md"] }],
      });
      if (!path) return; // user cancelled
      await writeTextFile(path, markdown);
    } catch (e) {
      setExportErr(String(e));
    } finally {
      setExporting(false);
    }
  };

  const templateOptions: SelectOption[] = templates.map((tpl) => ({
    value: tpl.id,
    label: tpl.name,
  }));

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
        <div className="flex items-center gap-2">
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
          {detail && (
            <Button
              onClick={handleExport}
              variant="secondary"
              size="sm"
              disabled={exporting}
              className="flex items-center gap-1.5"
            >
              {exporting ? (
                <Loader2 width={14} height={14} className="animate-spin" />
              ) : (
                <Download width={14} height={14} />
              )}
              <span>
                {exporting ? t("meeting.exporting") : t("meeting.export")}
              </span>
            </Button>
          )}
        </div>
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

        {exportErr && (
          <p className="text-sm text-red-400 whitespace-pre-wrap break-words">
            {t("meeting.exportError")}
          </p>
        )}

        {detail && (
          <>
            {editingTitle ? (
              <div className="flex items-center gap-2">
                <input
                  type="text"
                  value={titleDraft}
                  onChange={(e) => setTitleDraft(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") void handleSaveTitle();
                    if (e.key === "Escape") setEditingTitle(false);
                  }}
                  autoFocus
                  className="flex-1 rounded-md border border-mid-gray/20 bg-mid-gray/5 px-2 py-1 text-base text-text focus:border-logo-primary focus:outline-none focus:ring-1 focus:ring-logo-primary"
                />
                <Button
                  onClick={handleSaveTitle}
                  variant="primary-soft"
                  size="sm"
                >
                  {t("meeting.save")}
                </Button>
                <Button
                  onClick={() => setEditingTitle(false)}
                  variant="secondary"
                  size="sm"
                >
                  {t("meeting.cancel")}
                </Button>
              </div>
            ) : (
              <div className="flex items-center gap-2 group">
                <h3 className="text-base font-medium text-text break-words">
                  {title}
                </h3>
                <button
                  onClick={() => setEditingTitle(true)}
                  title={t("meeting.rename")}
                  className="p-1 rounded-md text-text/40 hover:text-logo-primary transition-colors cursor-pointer"
                >
                  <Pencil width={14} height={14} />
                </button>
              </div>
            )}

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

            {/* Editable user notes */}
            <div className="space-y-2">
              <div className="flex items-center justify-between">
                <h2 className="text-xs font-medium text-mid-gray uppercase tracking-wide">
                  {t("meeting.myNotes")}
                </h2>
                <div className="flex items-center gap-2">
                  <span className="text-[10px] font-medium uppercase tracking-wide text-text/40">
                    {notesSaving
                      ? t("meeting.notesSaving")
                      : t("meeting.notesSaved")}
                  </span>
                  <CopyButton
                    onCopy={() => onCopy(notes)}
                    disabled={notes.trim().length === 0}
                    title={t("meeting.copyMyNotes")}
                    copiedTitle={t("meeting.copied")}
                  />
                </div>
              </div>
              <textarea
                value={notes}
                onChange={(e) => handleNotesChange(e.target.value)}
                placeholder={t("meeting.myNotesPlaceholder")}
                className="w-full min-h-[6rem] resize-y rounded-md border border-mid-gray/20 bg-mid-gray/5 p-2 text-sm text-text/90 placeholder:text-text/40 focus:border-logo-primary focus:outline-none focus:ring-1 focus:ring-logo-primary"
              />
            </div>

            {/* AI summary + regenerate */}
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

              <SummaryControls
                templateOptions={templateOptions}
                selectedTemplate={selectedTemplate}
                onSelectTemplate={setSelectedTemplate}
                customPrompt={customPrompt}
                onCustomPromptChange={setCustomPrompt}
                disabled={!hasTranscript || regenerating}
                t={t}
              />

              <Button
                onClick={handleRegenerate}
                variant="primary-soft"
                size="md"
                disabled={!hasTranscript || regenerating}
                className="flex items-center gap-2"
              >
                <RefreshCw
                  width={16}
                  height={16}
                  className={regenerating ? "animate-spin" : ""}
                />
                <span>
                  {regenerating
                    ? t("meeting.regenerating")
                    : t("meeting.regenerate")}
                </span>
              </Button>

              <SummaryLocationNote info={providerInfo} t={t} />

              {regenError && (
                <p className="text-sm text-red-400 whitespace-pre-wrap break-words">
                  {regenError}
                </p>
              )}

              {summary.length > 0 ? (
                <Markdown>{summary}</Markdown>
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
