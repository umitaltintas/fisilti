import React, { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { History, Mic, Settings2 } from "lucide-react";

import { LiveSession } from "./LiveSession";
import { MeetingHistory } from "./MeetingHistory";
import { MeetingDetail } from "./MeetingDetail";
import { MeetingPreferences } from "./MeetingPreferences";
import { NOTES_AUTOSAVE_MS, SEARCH_DEBOUNCE_MS } from "./shared";
import {
  deleteMeeting,
  getMeeting,
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
  startMeeting,
  stopMeeting,
  summarizeMeetingWith,
  updateMeetingNotes,
  type InterruptedMeeting,
  type MeetingListItem,
  type MeetingRecord,
  type MeetingStatus,
  type MeetingSummaryTemplate,
  type SummaryProviderInfo,
  type TranscriptSegment,
} from "@/lib/meeting";
import { useModelStore } from "@/stores/modelStore";

type MeetingTab = "session" | "history" | "settings";

// The Meeting section, split into three tabs so each job gets its own space:
// "Session" (the live/last workspace), "History" (past meetings + detail) and
// "Settings" (one-time configuration). Shared session state and backend event
// subscriptions live here so switching tabs never drops a running meeting.
export const MeetingSettings: React.FC = () => {
  const { t } = useTranslation();

  const [tab, setTab] = useState<MeetingTab>("session");

  // Cloud models skip the live per-segment pass (it would be one request each),
  // so the live transcript stays empty until the on-stop finalize. Detect that
  // to show an accurate hint instead of "listening…".
  const { currentModel, models } = useModelStore();
  const selectedIsCloud = (() => {
    const engine = models.find((m) => m.id === currentModel)?.engine_type;
    return engine === "OpenRouter" || engine === "OpenRouterAsr";
  })();

  const [status, setStatus] = useState<MeetingStatus>("idle");
  const [transcript, setTranscript] = useState("");
  // Accumulated transcript segments for the live (and final) transcript,
  // rendered as a plain chronological flow. Replaced wholesale by the polished
  // list when the finalize pass completes.
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

  // Summary template picker + custom prompt for the live flow (the detail
  // view keeps its own local pair).
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

    // Automatic titles (calendar/window naming at start, LLM auto-title after
    // stop) arrive with the affected row id; rename only the matching meeting.
    register(
      listenMeetingTitle(({ id, title }) => {
        setDetail((prev) =>
          prev && prev.id === id ? { ...prev, title } : prev,
        );
        setPastMeetings((prev) =>
          prev.map((m) => (m.id === id ? { ...m, title } : m)),
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
    setTab("history");
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

  const goToHistoryList = () => {
    closeDetail();
    setTab("history");
  };

  // Re-clicking the active History tab pops back from a detail view.
  const handleTabChange = (next: MeetingTab) => {
    if (next === "history" && tab === "history") closeDetail();
    setTab(next);
  };

  const tabs: { id: MeetingTab; label: string; Icon: typeof Mic }[] = [
    { id: "session", label: t("meeting.tabSession"), Icon: Mic },
    { id: "history", label: t("meeting.tabHistory"), Icon: History },
    { id: "settings", label: t("meeting.tabSettings"), Icon: Settings2 },
  ];

  const detailOpen = detail !== null || detailLoading || detailError !== null;

  return (
    <div className="max-w-3xl w-full mx-auto space-y-6">
      {/* Tab bar */}
      <div
        role="tablist"
        className="flex items-center gap-1 rounded-lg border border-mid-gray/20 bg-mid-gray/5 p-1"
      >
        {tabs.map(({ id, label, Icon }) => (
          <button
            key={id}
            role="tab"
            aria-selected={tab === id}
            onClick={() => handleTabChange(id)}
            className={`flex-1 flex items-center justify-center gap-1.5 rounded-md px-3 py-1.5 text-sm transition-colors cursor-pointer ${
              tab === id
                ? "bg-background text-text border border-mid-gray/20 shadow-sm"
                : "border border-transparent text-text/60 hover:text-text"
            }`}
          >
            <Icon width={15} height={15} />
            <span>{label}</span>
            {id === "session" && isRunning && (
              <span className="relative flex h-2 w-2">
                <span className="absolute inline-flex h-full w-full rounded-full bg-red-500/70 animate-ping" />
                <span className="relative inline-flex h-2 w-2 rounded-full bg-red-500" />
              </span>
            )}
          </button>
        ))}
      </div>

      {tab === "session" && (
        <LiveSession
          isRunning={isRunning}
          busy={busy}
          elapsed={elapsed}
          error={error}
          finalizing={finalizing}
          selectedIsCloud={selectedIsCloud}
          transcript={transcript}
          liveSegments={liveSegments}
          userNotes={userNotes}
          notesSaving={notesSaving}
          hasSavedMeeting={currentMeetingId != null}
          summary={summary}
          summarizing={summarizing}
          summaryError={summaryError}
          templates={templates}
          selectedTemplate={selectedTemplate}
          onSelectTemplate={setSelectedTemplate}
          customPrompt={customPrompt}
          onCustomPromptChange={setCustomPrompt}
          providerInfo={providerInfo}
          recentMeetings={pastMeetings}
          interrupted={interrupted}
          recoveringId={recoveringId}
          recoverError={recoverError}
          onStart={handleStart}
          onStop={handleStop}
          onSummarize={handleSummarize}
          onUserNotesChange={handleUserNotesChange}
          onOpenMeeting={openDetail}
          onViewAllMeetings={goToHistoryList}
          onRecover={handleRecover}
          onDiscardInterrupted={handleDiscardInterrupted}
          onCopy={copyText}
        />
      )}

      {tab === "history" &&
        (detailOpen ? (
          <MeetingDetail
            detail={detail}
            loading={detailLoading}
            error={detailError}
            templates={templates}
            providerInfo={providerInfo}
            onBack={closeDetail}
            onCopy={copyText}
            onRefreshList={() => loadPastMeetings(searchQuery)}
            setDetail={setDetail}
          />
        ) : (
          <MeetingHistory
            meetings={pastMeetings}
            error={pastError}
            searchQuery={searchQuery}
            onSearchChange={setSearchQuery}
            confirmDeleteId={confirmDeleteId}
            onRequestDelete={setConfirmDeleteId}
            onCancelDelete={() => setConfirmDeleteId(null)}
            onConfirmDelete={handleDelete}
            onOpen={openDetail}
          />
        ))}

      {tab === "settings" && <MeetingPreferences />}
    </div>
  );
};
