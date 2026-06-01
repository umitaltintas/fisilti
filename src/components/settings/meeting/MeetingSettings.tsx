import React, { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, Copy, Mic, Sparkles, Square } from "lucide-react";

import { Button } from "../../ui/Button";
import {
  getMeetingStatus,
  getMeetingTranscript,
  listenMeetingTranscript,
  startMeeting,
  stopMeeting,
  summarizeMeeting,
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

export const MeetingSettings: React.FC = () => {
  const { t } = useTranslation();

  const [status, setStatus] = useState<MeetingStatus>("idle");
  const [transcript, setTranscript] = useState("");
  const [elapsed, setElapsed] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [notes, setNotes] = useState("");
  const [summarizing, setSummarizing] = useState(false);
  const [summaryError, setSummaryError] = useState<string | null>(null);

  const transcriptRef = useRef<HTMLDivElement>(null);
  const timerRef = useRef<ReturnType<typeof setInterval> | null>(null);

  const isRunning = status === "running";

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
  }, [startTimer, stopTimer]);

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
              {summarizing ? t("meeting.summarizing") : t("meeting.summarize")}
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
    </div>
  );
};
