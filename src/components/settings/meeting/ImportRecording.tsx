import React, { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { open } from "@tauri-apps/plugin-dialog";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { FileAudio, Loader2, Upload, X } from "lucide-react";

import { Button } from "../../ui/Button";
import {
  MEETING_IMPORT_CANCELLED,
  cancelMeetingImport,
  getMeetingImportProgress,
  getSupportedImportExtensions,
  importMeetingRecording,
  listenMeetingImportFinished,
  listenMeetingImportProgress,
  type MeetingImportProgress,
} from "@/lib/meeting";
import { errorMessage } from "@/lib/utils/errors";
import { InlineError } from "./shared";

interface ImportRecordingProps {
  /** A live meeting owns the engine; importing waits until it ends. */
  disabled: boolean;
  /** Called with the new meeting id once an import is saved. */
  onImported: (id: number) => void;
}

const extensionOf = (path: string) =>
  path.split(".").pop()?.toLowerCase() ?? "";

// Transcribe a recording made elsewhere — a voice memo from a phone, a
// conference recording — into a regular meeting. Pick a file or drop one on
// the window; progress survives switching tabs because it is read back from
// the backend on mount.
export const ImportRecording: React.FC<ImportRecordingProps> = ({
  disabled,
  onImported,
}) => {
  const { t } = useTranslation();
  const [progress, setProgress] = useState<MeetingImportProgress | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [dragging, setDragging] = useState(false);
  const [extensions, setExtensions] = useState<string[]>([]);
  const [cancelling, setCancelling] = useState(false);

  // The drop listener is registered once; it reads the latest values here.
  const stateRef = useRef({ disabled, busy: false, extensions });
  stateRef.current = { disabled, busy: progress !== null, extensions };
  const onImportedRef = useRef(onImported);
  onImportedRef.current = onImported;
  const tRef = useRef(t);
  tRef.current = t;

  const startImport = useCallback(
    async (path: string) => {
      if (!stateRef.current.extensions.includes(extensionOf(path))) {
        setError(t("meeting.import.unsupported"));
        return;
      }
      setError(null);
      setCancelling(false);
      setProgress({
        file_name: path.split(/[\\/]/).pop() ?? path,
        stage: "decoding",
        progress: 0,
      });
      try {
        // The outcome is handled by the "finished" event, which also reaches
        // a window that did not start the import.
        await importMeetingRecording(path);
      } catch (e) {
        // A refusal before the import starts (a meeting is running, another
        // import is under way) sends no "finished" event; resync with
        // whatever is actually running so the card does not hang at 0%.
        const message = errorMessage(e);
        if (message !== MEETING_IMPORT_CANCELLED) {
          setError(t("meeting.import.failed", { error: message }));
        }
        setProgress(await getMeetingImportProgress().catch(() => null));
      }
    },
    [t],
  );

  useEffect(() => {
    let cancelled = false;
    const unlisteners: UnlistenFn[] = [];
    const register = (p: Promise<UnlistenFn>) => {
      p.then((fn) => {
        if (cancelled) fn();
        else unlisteners.push(fn);
      }).catch((e: unknown) => {
        console.warn("Failed to listen for import events:", e);
      });
    };

    getSupportedImportExtensions()
      .then((exts) => {
        if (!cancelled) setExtensions(exts);
      })
      .catch((e: unknown) => {
        console.warn("Failed to read supported import formats:", e);
      });
    getMeetingImportProgress()
      .then((p) => {
        if (!cancelled && p) setProgress(p);
      })
      .catch(() => undefined);

    register(listenMeetingImportProgress((p) => setProgress(p)));
    register(
      listenMeetingImportFinished(({ id, error: failure }) => {
        setProgress(null);
        setCancelling(false);
        if (failure && failure !== MEETING_IMPORT_CANCELLED) {
          setError(tRef.current("meeting.import.failed", { error: failure }));
        }
        if (id != null) onImportedRef.current(id);
      }),
    );
    register(
      getCurrentWebview().onDragDropEvent((event) => {
        const { disabled: off, busy } = stateRef.current;
        const { type } = event.payload;
        if (type === "over" || type === "enter") {
          setDragging(!off && !busy);
        } else if (type === "leave") {
          setDragging(false);
        } else if (type === "drop") {
          setDragging(false);
          const path = event.payload.paths[0];
          if (path && !off && !busy) void startImport(path);
        }
      }),
    );

    return () => {
      cancelled = true;
      for (const fn of unlisteners) fn();
    };
  }, [startImport]);

  const handlePick = async () => {
    try {
      const picked = await open({
        multiple: false,
        directory: false,
        filters: [{ name: t("meeting.import.fileFilter"), extensions }],
      });
      if (typeof picked === "string") void startImport(picked);
    } catch (e) {
      setError(
        t("meeting.errors.filePickerFailed", { error: errorMessage(e) }),
      );
    }
  };

  const handleCancel = async () => {
    setCancelling(true);
    try {
      await cancelMeetingImport();
    } catch (e) {
      setCancelling(false);
      setError(`${t("meeting.errors.cancelFailed")} (${errorMessage(e)})`);
    }
  };

  if (progress) {
    const pct =
      progress.progress != null ? Math.round(progress.progress * 100) : null;
    return (
      <div className="bg-background border border-mid-gray/20 rounded-lg px-4 py-3 space-y-2">
        <div className="flex items-center gap-3">
          <Loader2
            width={16}
            height={16}
            className="shrink-0 animate-spin text-logo-primary"
          />
          <div className="flex-1 min-w-0">
            <p className="text-sm font-medium text-text truncate">
              {progress.file_name}
            </p>
            <p className="text-xs text-text/50">
              {t(`meeting.import.stage.${progress.stage}`)}
              {pct != null && ` · ${pct}%`}
            </p>
          </div>
          <Button
            onClick={() => void handleCancel()}
            variant="secondary"
            size="sm"
            disabled={cancelling}
            className="flex items-center gap-1 shrink-0"
          >
            <X width={13} height={13} />
            <span>{t("meeting.cancel")}</span>
          </Button>
        </div>
        <div className="h-1 w-full overflow-hidden rounded-full bg-mid-gray/15">
          <div
            className={`h-full bg-logo-primary transition-[width] duration-300 ${
              pct == null ? "w-1/3 animate-pulse" : ""
            }`}
            style={pct != null ? { width: `${Math.max(pct, 2)}%` } : undefined}
          />
        </div>
      </div>
    );
  }

  return (
    <div
      className={`rounded-lg border border-dashed px-4 py-3 flex items-center gap-3 transition-colors ${
        dragging
          ? "border-logo-primary bg-logo-primary/10"
          : "border-mid-gray/30 bg-mid-gray/5"
      }`}
    >
      <FileAudio width={18} height={18} className="shrink-0 text-text/50" />
      <div className="flex-1 min-w-0">
        <p className="text-sm text-text">
          {dragging ? t("meeting.import.dropHere") : t("meeting.import.title")}
        </p>
        <p className="text-xs text-text/50">
          {disabled
            ? t("meeting.import.busyMeeting")
            : t("meeting.import.description")}
        </p>
        {error && <InlineError className="mt-1 text-xs">{error}</InlineError>}
      </div>
      <Button
        onClick={() => void handlePick()}
        variant="secondary"
        size="sm"
        disabled={disabled || extensions.length === 0}
        className="flex items-center gap-1.5 shrink-0"
      >
        <Upload width={13} height={13} />
        <span>{t("meeting.import.choose")}</span>
      </Button>
    </div>
  );
};
