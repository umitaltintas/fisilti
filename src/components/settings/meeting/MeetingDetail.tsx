import React, { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { convertFileSrc } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import { writeTextFile } from "@tauri-apps/plugin-fs";
import {
  ArrowLeft,
  AudioLines,
  Download,
  Loader2,
  Pencil,
  RefreshCw,
} from "lucide-react";

import { Button } from "../../ui/Button";
import type { SelectOption } from "../../ui/Select";
import { Markdown } from "./Markdown";
import {
  CopyButton,
  NOTES_AUTOSAVE_MS,
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
  type SummaryProviderInfo,
} from "@/lib/meeting";

interface MeetingDetailProps {
  detail: MeetingRecord | null;
  loading: boolean;
  error: string | null;
  templates: MeetingSummaryTemplate[];
  providerInfo: SummaryProviderInfo | null;
  onBack: () => void;
  onCopy: (text: string) => void;
  onRefreshList: () => void;
  setDetail: React.Dispatch<React.SetStateAction<MeetingRecord | null>>;
}

// Full-page detail view of a saved meeting (takes over the History tab).
/** Sub-cent meetings are the common case for a short transcription, and
 * "$0.00" reads as "free" rather than "too small to show". */
const formatCost = (usd: number): string =>
  usd > 0 && usd < 0.01 ? "<0.01" : usd.toFixed(2);

export const MeetingDetail: React.FC<MeetingDetailProps> = ({
  detail,
  loading,
  error,
  templates,
  providerInfo,
  onBack,
  onCopy,
  onRefreshList,
  setDetail,
}) => {
  const { t, i18n } = useTranslation();
  const locale = i18n.language;

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
  const hasLabeledSegments = labeledSegments.length > 0;
  const summary = detail?.summary?.trim() ?? "";
  // Null when no cloud model ran, which must read differently from "$0.00" —
  // the latter would claim a paid path was free.
  const cost = estimateMeetingCost(detail?.usage);

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

  // Transcribe again from the saved audio (for meetings that came back
  // empty or garbled). Asks for confirmation: it replaces the transcript.
  const [confirmRetranscribe, setConfirmRetranscribe] = useState(false);
  const [retranscribing, setRetranscribing] = useState(false);
  const [retranscribeProgress, setRetranscribeProgress] =
    useState<MeetingImportProgress | null>(null);
  const [retranscribeError, setRetranscribeError] = useState<string | null>(
    null,
  );

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
    setConfirmRetranscribe(false);
    setRetranscribeError(null);
  }, [detailId]);

  useEffect(() => {
    if (!retranscribing) return;
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void listenMeetingImportProgress((p) => setRetranscribeProgress(p)).then(
      (fn) => (cancelled ? fn() : (unlisten = fn)),
    );
    return () => {
      cancelled = true;
      unlisten?.();
      setRetranscribeProgress(null);
    };
  }, [retranscribing]);

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

  const handleRetranscribe = async () => {
    if (detailId == null) return;
    setConfirmRetranscribe(false);
    setRetranscribeError(null);
    setRetranscribing(true);
    try {
      await retranscribeMeeting(detailId);
      setDetail(await getMeeting(detailId));
      onRefreshList();
    } catch (e) {
      setRetranscribeError(String(e));
    } finally {
      setRetranscribing(false);
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
      <div className="px-1 flex items-center justify-between gap-2">
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
              {cost && (
                <>
                  <span aria-hidden>•</span>
                  {/* Labelled as an estimate on purpose: the price table is a
                      snapshot of preview pricing, and free-tier quota and
                      billing discounts are invisible from here. Someone
                      comparing this to an invoice should already know it will
                      not match to the cent. */}
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

        {detail && detailHasAudioPath && detail.status === "completed" && (
          <RetranscribePanel
            looksFailed={transcriptLooksFailed}
            confirming={confirmRetranscribe}
            running={retranscribing}
            progress={retranscribeProgress}
            error={retranscribeError}
            onRequest={() => setConfirmRetranscribe(true)}
            onCancel={() => setConfirmRetranscribe(false)}
            onConfirm={() => void handleRetranscribe()}
          />
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
                <SectionHeading>{t("meeting.transcript")}</SectionHeading>
                <CopyButton
                  onCopy={() =>
                    onCopy(
                      plainTranscriptText(labeledSegments, detail.transcript),
                    )
                  }
                  disabled={!hasTranscript}
                  title={t("meeting.copyTranscript")}
                  copiedTitle={t("meeting.copied")}
                />
              </div>
              {hasLabeledSegments ? (
                <PlainTranscript segments={labeledSegments} />
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
              <SectionHeading>{t("meeting.audio")}</SectionHeading>
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
                <SectionHeading>{t("meeting.myNotes")}</SectionHeading>
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
                <SectionHeading>{t("meeting.summary")}</SectionHeading>
                {summary.length > 0 && (
                  <CopyButton
                    onCopy={() => onCopy(summary)}
                    title={t("meeting.copySummary")}
                    copiedTitle={t("meeting.copied")}
                  />
                )}
              </div>

              <div className="flex flex-wrap items-center gap-3">
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

interface RetranscribePanelProps {
  looksFailed: boolean;
  confirming: boolean;
  running: boolean;
  progress: MeetingImportProgress | null;
  error: string | null;
  onRequest: () => void;
  onCancel: () => void;
  onConfirm: () => void;
}

// One row offering to transcribe the meeting again from its saved audio.
// Prominent when the transcript looks failed (empty or far too short),
// otherwise a quiet secondary action.
const RetranscribePanel: React.FC<RetranscribePanelProps> = ({
  looksFailed,
  confirming,
  running,
  progress,
  error,
  onRequest,
  onCancel,
  onConfirm,
}) => {
  const { t } = useTranslation();
  const pct =
    progress?.progress != null ? Math.round(progress.progress * 100) : null;

  return (
    <div
      className={`rounded-md px-3 py-2 space-y-1 ${
        looksFailed
          ? "border border-logo-primary/40 bg-logo-primary/10"
          : "border border-mid-gray/20 bg-mid-gray/5"
      }`}
    >
      <div className="flex items-center gap-3">
        <AudioLines width={15} height={15} className="shrink-0 text-text/50" />
        <p className="flex-1 min-w-0 text-xs text-text/70">
          {running
            ? `${t(`meeting.import.stage.${progress?.stage ?? "decoding"}`)}${
                pct != null ? ` · ${pct}%` : ""
              }`
            : confirming
              ? t("meeting.retranscribe.confirmText")
              : looksFailed
                ? t("meeting.retranscribe.failedHint")
                : t("meeting.retranscribe.hint")}
        </p>
        {running ? (
          <Loader2
            width={15}
            height={15}
            className="shrink-0 animate-spin text-logo-primary"
          />
        ) : confirming ? (
          <div className="flex items-center gap-1 shrink-0">
            <Button onClick={onConfirm} variant="primary-soft" size="sm">
              {t("meeting.confirm")}
            </Button>
            <Button onClick={onCancel} variant="secondary" size="sm">
              {t("meeting.cancel")}
            </Button>
          </div>
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
      {error && (
        <p className="text-xs text-red-400 whitespace-pre-wrap break-words">
          {error}
        </p>
      )}
    </div>
  );
};
