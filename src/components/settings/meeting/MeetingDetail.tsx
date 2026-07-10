import React, { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { convertFileSrc } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import { writeTextFile } from "@tauri-apps/plugin-fs";
import { ArrowLeft, Download, Loader2, Pencil, RefreshCw } from "lucide-react";

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
  exportMeetingMarkdown,
  getMeetingAudioPath,
  regenerateMeetingSummary,
  updateMeetingNotes,
  updateMeetingTitle,
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
