import React, { useCallback, useEffect, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { readFile } from "@tauri-apps/plugin-fs";
import {
  Check,
  Copy,
  FolderOpen,
  Loader2,
  RotateCcw,
  Star,
  Trash2,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import {
  commands,
  events,
  type HistoryEntry,
  type HistoryUpdatePayload,
} from "@/bindings";
import { useOsType } from "@/hooks/useOsType";
import { formatDateTime } from "@/utils/dateFormat";
import { AudioPlayer } from "../../ui/AudioPlayer";
import { Button } from "../../ui/Button";
import { IconButton } from "../../ui/IconButton";
import { useConfirm } from "../../ui/ConfirmDialog";
import { useCopyToClipboard } from "@/hooks/useCopyToClipboard";
import { errorMessage } from "@/lib/utils/errors";
import { CollapsibleGroup } from "../../ui/CollapsibleGroup";
import { SettingsGroup } from "../../ui/SettingsGroup";
import { HistoryLimit } from "../HistoryLimit";
import { RecordingRetentionPeriodSelector } from "../RecordingRetentionPeriod";

const PAGE_SIZE = 30;

interface OpenRecordingsButtonProps {
  onClick: () => void;
  label: string;
}

const OpenRecordingsButton: React.FC<OpenRecordingsButtonProps> = ({
  onClick,
  label,
}) => (
  <Button
    onClick={onClick}
    variant="secondary"
    size="sm"
    className="flex items-center gap-2"
    title={label}
  >
    <FolderOpen className="w-4 h-4" />
    <span>{label}</span>
  </Button>
);

export const HistorySettings: React.FC = () => {
  const { t } = useTranslation();
  const osType = useOsType();
  const [entries, setEntries] = useState<HistoryEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [hasMore, setHasMore] = useState(true);
  const { confirm, dialog } = useConfirm();
  const sentinelRef = useRef<HTMLDivElement>(null);
  const entriesRef = useRef<HistoryEntry[]>([]);
  const loadingRef = useRef(false);

  // Keep ref in sync for use in IntersectionObserver callback
  useEffect(() => {
    entriesRef.current = entries;
  }, [entries]);

  const loadPage = useCallback(
    async (cursor?: number) => {
      const isFirstPage = cursor === undefined;
      if (!isFirstPage && loadingRef.current) return;
      loadingRef.current = true;

      if (isFirstPage) {
        setLoading(true);
        setLoadError(null);
      }

      try {
        const result = await commands.getHistoryEntries(
          cursor ?? null,
          PAGE_SIZE,
        );
        if (result.status === "error") throw result.error;
        const { entries: newEntries, has_more } = result.data;
        setEntries((prev) =>
          isFirstPage ? newEntries : [...prev, ...newEntries],
        );
        setHasMore(has_more);
      } catch (error) {
        console.error("Failed to load history entries:", error);
        if (isFirstPage) {
          setLoadError(errorMessage(error));
        } else {
          // Stop the infinite scroll from hammering a failing backend.
          setHasMore(false);
          toast.error(t("settings.history.loadMoreError"), {
            description: errorMessage(error),
          });
        }
      } finally {
        setLoading(false);
        loadingRef.current = false;
      }
    },
    [t],
  );

  // Initial load
  useEffect(() => {
    void loadPage();
  }, [loadPage]);

  // Infinite scroll via IntersectionObserver
  useEffect(() => {
    if (loading) return;

    const sentinel = sentinelRef.current;
    if (!sentinel || !hasMore) return;

    const observer = new IntersectionObserver(
      (observerEntries) => {
        const first = observerEntries[0];
        if (first.isIntersecting) {
          const lastEntry = entriesRef.current[entriesRef.current.length - 1];
          if (lastEntry) {
            void loadPage(lastEntry.id);
          }
        }
      },
      { threshold: 0 },
    );

    observer.observe(sentinel);
    return () => observer.disconnect();
  }, [loading, hasMore, loadPage]);

  // Listen for new entries added from the transcription pipeline
  useEffect(() => {
    const unlisten = events.historyUpdatePayload.listen((event) => {
      const payload: HistoryUpdatePayload = event.payload;
      if (payload.action === "added") {
        setEntries((prev) => [payload.entry, ...prev]);
      } else if (payload.action === "updated") {
        setEntries((prev) =>
          prev.map((e) => (e.id === payload.entry.id ? payload.entry : e)),
        );
      }
      // "deleted" and "toggled" are handled by optimistic updates only,
      // so we intentionally ignore them here to avoid double-mutation.
    });

    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  const toggleSaved = async (id: number) => {
    // Optimistic update
    setEntries((prev) =>
      prev.map((e) => (e.id === id ? { ...e, saved: !e.saved } : e)),
    );
    try {
      const result = await commands.toggleHistoryEntrySaved(id);
      if (result.status === "error") throw result.error;
    } catch (error) {
      console.error("Failed to toggle saved status:", error);
      // Revert on failure
      setEntries((prev) =>
        prev.map((e) => (e.id === id ? { ...e, saved: !e.saved } : e)),
      );
      toast.error(t("settings.history.saveError"), {
        description: errorMessage(error),
      });
    }
  };

  const getAudioUrl = useCallback(
    async (fileName: string) => {
      try {
        const result = await commands.getAudioFilePath(fileName);
        if (result.status === "ok") {
          if (osType === "linux") {
            const fileData = await readFile(result.data);
            const blob = new Blob([fileData], { type: "audio/wav" });
            return URL.createObjectURL(blob);
          }
          return convertFileSrc(result.data, "asset");
        }
        return null;
      } catch (error) {
        console.error("Failed to get audio file path:", error);
        return null;
      }
    },
    [osType],
  );

  /** Ask, then delete. Rejects when the backend refused, after restoring
   * the list, so the caller's error toast actually fires. */
  const deleteAudioEntry = async (id: number) => {
    const ok = await confirm({
      title: t("settings.history.deleteConfirmTitle"),
      description: t("settings.history.deleteConfirmText"),
      destructive: true,
    });
    if (!ok) return;
    // Optimistically remove
    setEntries((prev) => prev.filter((e) => e.id !== id));
    try {
      const result = await commands.deleteHistoryEntry(id);
      if (result.status === "error") throw result.error;
    } catch (error) {
      void loadPage();
      throw error;
    }
  };

  const retryHistoryEntry = async (id: number) => {
    const result = await commands.retryHistoryEntryTranscription(id);
    if (result.status === "error") throw result.error;
  };

  const openRecordingsFolder = async () => {
    try {
      const result = await commands.openRecordingsFolder();
      if (result.status === "error") throw result.error;
    } catch (error) {
      console.error("Failed to open recordings folder:", error);
      toast.error(t("settings.history.openFolderError"), {
        description: errorMessage(error),
      });
    }
  };

  let content: React.ReactNode;

  if (loading) {
    content = (
      <div
        className="px-4 py-6 flex items-center justify-center gap-2 text-sm text-text/60"
        role="status"
      >
        <Loader2 width={14} height={14} className="animate-spin" />
        {t("settings.history.loading")}
      </div>
    );
  } else if (loadError !== null) {
    content = (
      <div className="px-4 py-6 flex flex-col items-center gap-3 text-center">
        <p role="alert" className="text-sm text-red-400 break-words">
          {t("settings.history.loadError")} ({loadError})
        </p>
        <Button variant="secondary" size="sm" onClick={() => void loadPage()}>
          {t("common.retry")}
        </Button>
      </div>
    );
  } else if (entries.length === 0) {
    content = (
      <div className="px-4 py-8 text-center text-sm text-text/60">
        {t("settings.history.empty")}
      </div>
    );
  } else {
    content = (
      <>
        <div className="divide-y divide-mid-gray/20">
          {entries.map((entry) => (
            <HistoryEntryComponent
              key={entry.id}
              entry={entry}
              onToggleSaved={() => void toggleSaved(entry.id)}
              getAudioUrl={getAudioUrl}
              deleteAudio={deleteAudioEntry}
              retryTranscription={retryHistoryEntry}
            />
          ))}
        </div>
        {/* Sentinel for infinite scroll */}
        <div ref={sentinelRef} className="h-1" />
      </>
    );
  }

  return (
    <div className="max-w-3xl w-full mx-auto space-y-6">
      {dialog}
      {/* How much history is kept used to be configured under Advanced, two
          sections away from the list it governs. */}
      <CollapsibleGroup title={t("settings.history.retentionSettings")}>
        <SettingsGroup>
          <HistoryLimit descriptionMode="tooltip" grouped={true} />
          <RecordingRetentionPeriodSelector
            descriptionMode="tooltip"
            grouped={true}
          />
        </SettingsGroup>
      </CollapsibleGroup>

      <div className="space-y-2">
        <div className="px-4 flex items-center justify-between">
          <div>
            <h2 className="text-xs font-medium text-mid-gray uppercase tracking-wide">
              {t("settings.history.title")}
            </h2>
          </div>
          <OpenRecordingsButton
            onClick={() => void openRecordingsFolder()}
            label={t("settings.history.openFolder")}
          />
        </div>
        <div className="bg-background border border-mid-gray/20 rounded-lg overflow-visible">
          {content}
        </div>
      </div>
    </div>
  );
};

interface HistoryEntryProps {
  entry: HistoryEntry;
  onToggleSaved: () => void;
  getAudioUrl: (fileName: string) => Promise<string | null>;
  deleteAudio: (id: number) => Promise<void>;
  retryTranscription: (id: number) => Promise<void>;
}

const HistoryEntryComponent: React.FC<HistoryEntryProps> = ({
  entry,
  onToggleSaved,
  getAudioUrl,
  deleteAudio,
  retryTranscription,
}) => {
  const { t, i18n } = useTranslation();
  const { copied, copy } = useCopyToClipboard();
  const [retrying, setRetrying] = useState(false);

  const hasTranscription = entry.transcription_text.trim().length > 0;

  const handleLoadAudio = useCallback(
    () => getAudioUrl(entry.file_name),
    [getAudioUrl, entry.file_name],
  );

  const handleDeleteEntry = async () => {
    try {
      await deleteAudio(entry.id);
    } catch (error) {
      console.error("Failed to delete entry:", error);
      toast.error(t("settings.history.deleteError"), {
        description: errorMessage(error),
      });
    }
  };

  const handleRetranscribe = async () => {
    try {
      setRetrying(true);
      await retryTranscription(entry.id);
    } catch (error) {
      console.error("Failed to re-transcribe:", error);
      toast.error(t("settings.history.retranscribeError"), {
        description: errorMessage(error),
      });
    } finally {
      setRetrying(false);
    }
  };

  const formattedDate = formatDateTime(String(entry.timestamp), i18n.language);

  return (
    <div className="px-4 py-2 pb-5 flex flex-col gap-3">
      <div className="flex justify-between items-center">
        <p className="text-sm font-medium">{formattedDate}</p>
        <div className="flex items-center">
          <IconButton
            onClick={() => void copy(entry.transcription_text)}
            disabled={!hasTranscription || retrying}
            label={
              copied
                ? t("meeting.copied")
                : t("settings.history.copyToClipboard")
            }
          >
            {copied ? (
              <Check width={16} height={16} />
            ) : (
              <Copy width={16} height={16} />
            )}
          </IconButton>
          <IconButton
            onClick={onToggleSaved}
            disabled={retrying}
            tone={entry.saved ? "active" : "default"}
            aria-pressed={entry.saved}
            label={
              entry.saved
                ? t("settings.history.unsave")
                : t("settings.history.save")
            }
          >
            <Star
              width={16}
              height={16}
              fill={entry.saved ? "currentColor" : "none"}
            />
          </IconButton>
          <IconButton
            onClick={() => void handleRetranscribe()}
            disabled={retrying}
            label={t("settings.history.retranscribe")}
          >
            <RotateCcw
              width={16}
              height={16}
              style={
                retrying
                  ? { animation: "spin 1s linear infinite reverse" }
                  : undefined
              }
            />
          </IconButton>
          <IconButton
            onClick={() => void handleDeleteEntry()}
            disabled={retrying}
            tone="danger"
            label={t("settings.history.delete")}
          >
            <Trash2 width={16} height={16} />
          </IconButton>
        </div>
      </div>

      <p
        className={`italic text-sm pb-2 ${
          retrying
            ? ""
            : hasTranscription
              ? "text-text/90 select-text cursor-text whitespace-pre-wrap break-words"
              : "text-text/40"
        }`}
        style={
          retrying
            ? { animation: "transcribe-pulse 3s ease-in-out infinite" }
            : undefined
        }
      >
        {retrying && (
          <style>{`
            @keyframes transcribe-pulse {
              0%, 100% { color: color-mix(in srgb, var(--color-text) 40%, transparent); }
              50% { color: color-mix(in srgb, var(--color-text) 90%, transparent); }
            }
          `}</style>
        )}
        {retrying
          ? t("settings.history.transcribing")
          : hasTranscription
            ? entry.transcription_text
            : t("settings.history.transcriptionFailed")}
      </p>

      <AudioPlayer onLoadRequest={handleLoadAudio} className="w-full" />
    </div>
  );
};
