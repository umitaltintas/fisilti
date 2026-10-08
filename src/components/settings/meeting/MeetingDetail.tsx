import React, { useCallback, useEffect, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { convertFileSrc } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import { writeTextFile } from "@tauri-apps/plugin-fs";
import { toast } from "sonner";
import {
  ArrowLeft,
  AudioLines,
  Download,
  Loader2,
  Pencil,
  RefreshCw,
  Sparkles,
  X,
} from "lucide-react";

import { Button } from "../../ui/Button";
import { IconButton } from "../../ui/IconButton";
import { AudioPlayer } from "../../ui/AudioPlayer";
import { useConfirm } from "../../ui/ConfirmDialog";
import type { SelectOption } from "../../ui/Select";
import { Markdown } from "./Markdown";
import {
  CopyButton,
  InlineError,
  NotesSaveIndicator,
  PlainTranscript,
  SectionHeading,
  SummaryControls,
  SummaryLocationNote,
  exportFilename,
  formatDuration,
  formatMeetingDate,
  plainTranscriptText,
} from "./shared";
import {
  MEETING_EVENTS,
  MEETING_IMPORT_CANCELLED,
  cancelMeetingImport,
  estimateMeetingCost,
  exportMeetingMarkdown,
  getMeeting,
  getMeetingAudioPath,
  listenMeetingImportProgress,
  regenerateMeetingSummary,
  retranscribeMeeting,
  updateMeetingNotes,
  updateMeetingTitle,
  type MeetingImportProgress,
  type MeetingRecord,
  type MeetingSummaryTemplate,
  type MeetingSummaryUpdate,
  type MeetingTitleUpdate,
  type SummaryProviderInfo,
} from "@/lib/meeting";
import {
  NOTES_AUTOSAVE_MS,
  useMeetingStore,
  type NotesSaveState,
} from "@/stores/meetingStore";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import { errorMessage } from "@/lib/utils/errors";

interface MeetingDetailProps {
  meetingId: number;
  templates: MeetingSummaryTemplate[];
  providerInfo: SummaryProviderInfo | null;
  onBack: () => void;
}

/** Sub-cent meetings are the common case for a short transcription, and
 * "$0.00" reads as "free" rather than "too small to show". */
const formatCost = (usd: number): string =>
  usd > 0 && usd < 0.01 ? "<0.01" : usd.toFixed(2);

// Full-page detail view of a saved meeting (takes over the History tab).
// Render with `key={meetingId}` so every piece of local state starts fresh for
// each meeting.
export const MeetingDetail: React.FC<MeetingDetailProps> = ({
  meetingId,
  templates,
  providerInfo,
  onBack,
}) => {
  const { t, i18n } = useTranslation();
  const locale = i18n.language;
  const patchMeeting = useMeetingStore((s) => s.patchMeeting);
  const loadMeetings = useMeetingStore((s) => s.loadMeetings);
  const { confirm, dialog } = useConfirm();

  const [detail, setDetail] = useState<MeetingRecord | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);

  // Editable notes, seeded once from the loaded record — never re-seeded from
  // it afterwards, which used to overwrite whatever was being typed.
  const [notes, setNotes] = useState("");
  const [notesSave, setNotesSave] = useState<NotesSaveState>("idle");
  const [notesError, setNotesError] = useState<string | null>(null);
  const notesTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const notesSeqRef = useRef(0);

  // Inline title rename.
  const [editingTitle, setEditingTitle] = useState(false);
  const [titleDraft, setTitleDraft] = useState("");
  const [titleError, setTitleError] = useState<string | null>(null);
  const titleInputRef = useRef<HTMLInputElement>(null);

  // Regenerate controls.
  const [selectedTemplate, setSelectedTemplate] = useState<string | null>(null);
  const [customPrompt, setCustomPrompt] = useState("");
  const [regenerating, setRegenerating] = useState(false);
  const [regenError, setRegenError] = useState<string | null>(null);

  // Transcribe again from the saved audio.
  const [retranscribing, setRetranscribing] = useState(false);
  const [cancellingRetranscribe, setCancellingRetranscribe] = useState(false);
  const [retranscribeProgress, setRetranscribeProgress] =
    useState<MeetingImportProgress | null>(null);
  const [retranscribeError, setRetranscribeError] = useState<string | null>(
    null,
  );

  const [exporting, setExporting] = useState(false);
  const notesId = useId();

  const load = useCallback(async () => {
    setLoading(true);
    setLoadError(null);
    try {
      const record = await getMeeting(meetingId);
      setDetail(record);
      return record;
    } catch (error) {
      setLoadError(errorMessage(error));
      return null;
    } finally {
      setLoading(false);
    }
  }, [meetingId]);

  useEffect(() => {
    void load().then((record) => {
      if (record) setNotes(record.notes ?? "");
    });
  }, [load]);

  useEffect(() => {
    setSelectedTemplate((cur) => cur ?? templates[0]?.id ?? null);
  }, [templates]);

  useEffect(() => {
    if (editingTitle) titleInputRef.current?.focus();
  }, [editingTitle]);

  // Automatic renames and summaries for this meeting land while it is open.
  useTauriEvent<MeetingTitleUpdate>(MEETING_EVENTS.title, ({ id, title }) => {
    if (id === meetingId) {
      setDetail((prev) => (prev ? { ...prev, title } : prev));
    }
  });
  useTauriEvent<MeetingSummaryUpdate>(MEETING_EVENTS.summary, (payload) => {
    if (typeof payload === "object" && payload.id === meetingId) {
      setDetail((prev) =>
        prev ? { ...prev, summary: payload.summary } : prev,
      );
    }
  });

  useEffect(() => {
    if (!retranscribing) return;
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void listenMeetingImportProgress((p) => setRetranscribeProgress(p)).then(
      (fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      },
    );
    return () => {
      cancelled = true;
      unlisten?.();
      setRetranscribeProgress(null);
    };
  }, [retranscribing]);

  // Flush a pending notes save when leaving the meeting.
  const notesRef = useRef(notes);
  notesRef.current = notes;
  useEffect(
    () => () => {
      if (notesTimerRef.current) {
        clearTimeout(notesTimerRef.current);
        void updateMeetingNotes(meetingId, notesRef.current).catch(
          (error: unknown) =>
            toast.error(
              t("meeting.errors.notesSaveFailed", {
                error: errorMessage(error),
              }),
            ),
        );
      }
    },
    [meetingId, t],
  );

  const title = detail
    ? detail.title.trim() || t("meeting.untitledMeeting")
    : "";
  const hasTranscript = !!detail && detail.transcript.trim().length > 0;
  // A few words for many minutes of audio means the transcription failed
  // (outage, wrong model) rather than that nobody spoke.
  const transcriptLooksFailed =
    !!detail &&
    (!hasTranscript ||
      (detail.duration_ms > 60_000 &&
        detail.transcript.trim().length < (detail.duration_ms / 60_000) * 30));
  const labeledSegments = detail?.segments ?? [];
  const summary = detail?.summary?.trim() ?? "";
  // Null when no cloud model ran, which must read differently from "$0.00" —
  // the latter would claim a paid path was free.
  const cost = estimateMeetingCost(detail?.usage);
  const hasAudio = !!detail?.audio_path;

  const loadAudio = useCallback(async () => {
    try {
      return convertFileSrc(await getMeetingAudioPath(meetingId));
    } catch (error) {
      toast.error(t("meeting.errors.audioFailed"), {
        description: errorMessage(error),
      });
      return null;
    }
  }, [meetingId, t]);

  const handleNotesChange = (value: string) => {
    setNotes(value);
    setNotesSave("saving");
    setNotesError(null);
    if (notesTimerRef.current) clearTimeout(notesTimerRef.current);
    notesTimerRef.current = setTimeout(() => {
      notesTimerRef.current = null;
      const seq = ++notesSeqRef.current;
      updateMeetingNotes(meetingId, value)
        .then(() => {
          if (seq !== notesSeqRef.current) return;
          setNotesSave("saved");
          // Keep the record in step, so nothing reads the stale copy later.
          setDetail((prev) => (prev ? { ...prev, notes: value } : prev));
        })
        .catch((error: unknown) => {
          if (seq !== notesSeqRef.current) return;
          setNotesSave("error");
          setNotesError(errorMessage(error));
        });
    }, NOTES_AUTOSAVE_MS);
  };

  const startEditingTitle = () => {
    setTitleDraft(detail?.title ?? "");
    setTitleError(null);
    setEditingTitle(true);
  };

  const handleSaveTitle = async () => {
    const next = titleDraft.trim();
    if (next === (detail?.title ?? "").trim()) {
      setEditingTitle(false);
      return;
    }
    try {
      await updateMeetingTitle(meetingId, next);
      setDetail((prev) => (prev ? { ...prev, title: next } : prev));
      patchMeeting(meetingId, { title: next });
      setEditingTitle(false);
    } catch (error) {
      // Stay in edit mode with the draft intact, error under the field.
      setTitleError(
        t("meeting.errors.renameFailed", { error: errorMessage(error) }),
      );
    }
  };

  const handleRetranscribe = async () => {
    const ok = await confirm({
      title: t("meeting.retranscribe.confirmTitle"),
      description: t("meeting.retranscribe.confirmText"),
      confirmLabel: t("meeting.retranscribe.button"),
    });
    if (!ok) return;
    setRetranscribeError(null);
    setCancellingRetranscribe(false);
    setRetranscribing(true);
    try {
      await retranscribeMeeting(meetingId);
      await load();
      void loadMeetings();
    } catch (error) {
      const message = errorMessage(error);
      if (message !== MEETING_IMPORT_CANCELLED) {
        setRetranscribeError(
          t("meeting.errors.retranscribeFailed", { error: message }),
        );
      }
    } finally {
      setRetranscribing(false);
      setCancellingRetranscribe(false);
    }
  };

  const handleCancelRetranscribe = async () => {
    setCancellingRetranscribe(true);
    try {
      await cancelMeetingImport();
    } catch (error) {
      setCancellingRetranscribe(false);
      toast.error(t("meeting.errors.cancelFailed"), {
        description: errorMessage(error),
      });
    }
  };

  const handleRegenerate = async () => {
    setRegenError(null);
    setRegenerating(true);
    const custom = customPrompt.trim();
    const arg = custom.length > 0 ? custom : (selectedTemplate ?? undefined);
    try {
      const result = await regenerateMeetingSummary(meetingId, arg);
      setDetail((prev) => (prev ? { ...prev, summary: result } : prev));
      patchMeeting(meetingId, { has_summary: result.trim().length > 0 });
    } catch (error) {
      setRegenError(
        t("meeting.errors.summaryFailed", { error: errorMessage(error) }),
      );
    } finally {
      setRegenerating(false);
    }
  };

  const handleExport = async () => {
    setExporting(true);
    try {
      const markdown = await exportMeetingMarkdown(meetingId);
      const path = await save({
        defaultPath: exportFilename(title),
        filters: [{ name: t("meeting.markdownFilter"), extensions: ["md"] }],
      });
      if (!path) return; // user cancelled
      await writeTextFile(path, markdown);
      toast.success(t("meeting.exported"));
    } catch (error) {
      toast.error(t("meeting.exportError"), {
        description: errorMessage(error),
      });
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
      {dialog}
      <div className="px-1 flex items-center justify-between gap-2">
        <button
          type="button"
          onClick={onBack}
          className="flex items-center gap-1.5 rounded text-sm text-sub hover:text-logo-primary transition-colors cursor-pointer focus:outline-none focus-visible:ring-1 focus-visible:ring-logo-primary"
        >
          <ArrowLeft width={16} height={16} className="rtl:rotate-180" />
          <span>{t("meeting.back")}</span>
        </button>
        <div className="flex items-center gap-2 min-w-0">
          {detail && (
            <div className="flex items-center gap-2 text-xs text-sub min-w-0">
              <span className="truncate">
                {formatMeetingDate(detail.started_at, locale)}
              </span>
              <span aria-hidden>•</span>
              <span className="tabular-nums">
                {formatDuration(detail.duration_ms, t)}
              </span>
              {cost && (
                <>
                  <span aria-hidden>•</span>
                  {/* Labelled as an estimate on purpose: the price table is a
                      snapshot of preview pricing, and free-tier quota and
                      billing discounts are invisible from here. */}
                  <span
                    className="tabular-nums"
                    title={t("meeting.costTooltip")}
                  >
                    {t("meeting.costEstimate", {
                      amount: formatCost(cost.usd),
                    })}
                    {!cost.complete && "+"}
                  </span>
                </>
              )}
            </div>
          )}
          {detail && (
            <Button
              onClick={() => void handleExport()}
              variant="secondary"
              size="sm"
              disabled={exporting}
              className="flex items-center gap-1.5 shrink-0"
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

      <div className="space-y-6">
        {loading && !detail && (
          <p className="flex items-center gap-2 text-sm text-sub" role="status">
            <Loader2 width={14} height={14} className="animate-spin" />
            {t("meeting.loading")}
          </p>
        )}

        {loadError && (
          <div className="flex flex-wrap items-center gap-3">
            <InlineError>
              {t("meeting.loadError")} ({loadError})
            </InlineError>
            <Button variant="secondary" size="sm" onClick={() => void load()}>
              {t("common.retry")}
            </Button>
          </div>
        )}

        {detail && hasAudio && detail.status === "completed" && (
          <RetranscribePanel
            looksFailed={transcriptLooksFailed}
            running={retranscribing}
            cancelling={cancellingRetranscribe}
            progress={retranscribeProgress}
            error={retranscribeError}
            onRequest={() => void handleRetranscribe()}
            onCancel={() => void handleCancelRetranscribe()}
          />
        )}

        {detail && (
          <>
            {editingTitle ? (
              <div className="space-y-1">
                <div className="flex items-center gap-2">
                  <input
                    ref={titleInputRef}
                    type="text"
                    value={titleDraft}
                    aria-label={t("meeting.renameTitle")}
                    aria-invalid={titleError ? true : undefined}
                    onChange={(e) => setTitleDraft(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") void handleSaveTitle();
                      if (e.key === "Escape") setEditingTitle(false);
                    }}
                    className="flex-1 rounded-md border border-line bg-surface px-2 py-1 font-serif text-xl text-text focus:border-logo-primary focus:ring-2 focus:ring-logo-primary/30 focus:outline-none"
                  />
                  <Button
                    onClick={() => void handleSaveTitle()}
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
                {titleError && (
                  <InlineError className="text-xs">{titleError}</InlineError>
                )}
              </div>
            ) : (
              <div className="flex items-center gap-2">
                <h2 className="font-serif text-[26px] leading-tight font-semibold tracking-tight break-words">
                  {title}
                </h2>
                <IconButton
                  onClick={startEditingTitle}
                  label={t("meeting.renameTitle")}
                  size="sm"
                >
                  <Pencil width={14} height={14} />
                </IconButton>
              </div>
            )}

            {/* AI summary + regenerate */}
            <section className="card space-y-3 p-4">
              <div className="flex items-center justify-between">
                <h3 className="flex items-center gap-1.5 text-xs font-semibold text-brand-text">
                  <Sparkles width={13} height={13} aria-hidden />
                  {t("meeting.summary")}
                </h3>
                {summary.length > 0 && (
                  <CopyButton text={summary} label={t("meeting.copySummary")} />
                )}
              </div>

              <div className="flex flex-wrap items-center gap-3">
                <Button
                  onClick={() => void handleRegenerate()}
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
                      : summary.length > 0
                        ? t("meeting.regenerate")
                        : t("meeting.generateSummary")}
                  </span>
                </Button>
                <SummaryLocationNote info={providerInfo} />
              </div>

              <SummaryControls
                templateOptions={templateOptions}
                selectedTemplate={selectedTemplate}
                onSelectTemplate={setSelectedTemplate}
                customPrompt={customPrompt}
                onCustomPromptChange={setCustomPrompt}
                disabled={!hasTranscript || regenerating}
              />

              {regenError && <InlineError>{regenError}</InlineError>}

              {summary.length > 0 ? (
                <Markdown>{summary}</Markdown>
              ) : (
                <p className="text-sm text-faint">{t("meeting.noSummary")}</p>
              )}
            </section>
            {/* Editable user notes */}
            <section className="space-y-2">
              <div className="flex items-center justify-between gap-2">
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
                aria-labelledby={`${notesId}-heading`}
                value={notes}
                onChange={(e) => handleNotesChange(e.target.value)}
                placeholder={t("meeting.myNotesPlaceholder")}
                className="min-h-[6rem] w-full resize-y border-s-2 border-line bg-transparent ps-3.5 font-serif text-[14.5px] leading-relaxed text-text placeholder:text-faint focus:border-logo-primary focus:outline-none"
              />
              {notesSave === "error" && notesError && (
                <InlineError className="text-xs">
                  {t("meeting.errors.notesSaveFailed", { error: notesError })}
                </InlineError>
              )}
            </section>

            <section className="space-y-2">
              <SectionHeading>{t("meeting.audio")}</SectionHeading>
              {hasAudio ? (
                <AudioPlayer onLoadRequest={loadAudio} className="w-full" />
              ) : (
                <p className="text-sm text-faint">{t("meeting.noAudio")}</p>
              )}
            </section>

            <section className="space-y-2">
              <div className="flex items-center justify-between">
                <SectionHeading>{t("meeting.transcript")}</SectionHeading>
                <CopyButton
                  text={plainTranscriptText(labeledSegments, detail.transcript)}
                  disabled={!hasTranscript}
                  label={t("meeting.copyTranscript")}
                />
              </div>
              {labeledSegments.length > 0 ? (
                <PlainTranscript segments={labeledSegments} compact />
              ) : hasTranscript ? (
                <p className="text-sm text-text/90 whitespace-pre-wrap break-words select-text">
                  {detail.transcript}
                </p>
              ) : (
                <p className="text-sm text-faint">
                  {t("meeting.transcriptEmpty")}
                </p>
              )}
            </section>
          </>
        )}
      </div>
    </div>
  );
};

interface RetranscribePanelProps {
  looksFailed: boolean;
  running: boolean;
  cancelling: boolean;
  progress: MeetingImportProgress | null;
  error: string | null;
  onRequest: () => void;
  onCancel: () => void;
}

// One row offering to transcribe the meeting again from its saved audio.
// Prominent when the transcript looks failed (empty or far too short),
// otherwise a quiet secondary action.
const RetranscribePanel: React.FC<RetranscribePanelProps> = ({
  looksFailed,
  running,
  cancelling,
  progress,
  error,
  onRequest,
  onCancel,
}) => {
  const { t } = useTranslation();
  const pct =
    progress?.progress != null ? Math.round(progress.progress * 100) : null;

  return (
    <div
      className={`rounded-md px-3 py-2 space-y-1 ${
        looksFailed
          ? "border border-logo-primary/40 bg-logo-primary/10"
          : "border border-line bg-mid-gray/5"
      }`}
    >
      <div className="flex items-center gap-3">
        {running ? (
          <Loader2
            width={15}
            height={15}
            className="shrink-0 animate-spin text-logo-primary"
            aria-hidden
          />
        ) : (
          <AudioLines
            width={15}
            height={15}
            className="shrink-0 text-sub"
            aria-hidden
          />
        )}
        <p
          className="flex-1 min-w-0 text-xs text-sub"
          role={running ? "status" : undefined}
        >
          {running
            ? cancelling
              ? t("meeting.retranscribe.cancelling")
              : `${t(`meeting.import.stage.${progress?.stage ?? "decoding"}`)}${
                  pct != null ? ` · ${pct}%` : ""
                }`
            : looksFailed
              ? t("meeting.retranscribe.failedHint")
              : t("meeting.retranscribe.hint")}
        </p>
        {running ? (
          <Button
            onClick={onCancel}
            variant="secondary"
            size="sm"
            disabled={cancelling}
            className="flex items-center gap-1 shrink-0"
          >
            <X width={13} height={13} aria-hidden />
            <span>{t("meeting.cancel")}</span>
          </Button>
        ) : (
          <Button
            onClick={onRequest}
            variant={looksFailed ? "primary-soft" : "secondary"}
            size="sm"
            className="shrink-0"
          >
            {t("meeting.retranscribe.button")}
          </Button>
        )}
      </div>
      {error && <InlineError className="text-xs">{error}</InlineError>}
    </div>
  );
};
