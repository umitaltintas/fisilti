import { emit, listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import i18n, { getLanguageDirection, syncLanguageFromSettings } from "@/i18n";
import {
  MEETING_PROMPT_UPDATE_EVENT,
  MEETING_PROMPT_READY_EVENT,
} from "./events";

/** Payload pushed by the backend, mirrors `MeetingPromptPayload` in Rust. */
type MeetingPromptPayload =
  | { kind: "start"; appName: string }
  | { kind: "end"; reason: "silence" | "app_closed"; graceSecs: number };

/** Fire a backend command and swallow/log errors so a rejected promise never
 * leaves the prompt in a half-disabled state. */
const runCommand = (command: string, args?: Record<string, unknown>): void => {
  invoke(command, args).catch((error) => {
    console.error(`Meeting prompt command "${command}" failed:`, error);
  });
};

const MeetingPrompt: React.FC = () => {
  const { t } = useTranslation();
  const [payload, setPayload] = useState<MeetingPromptPayload | null>(null);
  const [seconds, setSeconds] = useState(0);
  // Disables the buttons after a click; the backend hides the window in
  // response, so no client-side hide is needed.
  const [submitted, setSubmitted] = useState(false);
  const direction = getLanguageDirection(i18n.language);

  useEffect(() => {
    const unlistenPromise = listen<MeetingPromptPayload>(
      MEETING_PROMPT_UPDATE_EVENT,
      async (event) => {
        // Match the app's current language each time a prompt is shown.
        await syncLanguageFromSettings();
        setSubmitted(false);
        setPayload(event.payload);
      },
    );

    // Signal the backend we are mounted so it can (re)send the current payload,
    // closing the race where the first emit happens before this listener binds.
    emit(MEETING_PROMPT_READY_EVENT);

    return () => {
      unlistenPromise.then((unlisten) => unlisten());
    };
  }, []);

  // Client-side countdown for the "end" prompt. Display only — the backend owns
  // the real auto-end timer and hides the window when it fires.
  useEffect(() => {
    if (payload?.kind !== "end") return;
    setSeconds(payload.graceSecs);
    const id = setInterval(() => {
      setSeconds((value) => (value > 0 ? value - 1 : 0));
    }, 1000);
    return () => clearInterval(id);
  }, [payload]);

  if (!payload) return null;

  const primaryClass =
    "bg-violet-600 hover:bg-violet-500 text-white focus:ring-violet-400";
  const secondaryClass =
    "bg-neutral-200 hover:bg-neutral-300 text-neutral-800 dark:bg-neutral-700 dark:hover:bg-neutral-600 dark:text-neutral-100 focus:ring-neutral-400";
  const buttonClass =
    "flex-1 rounded-md px-3 py-1.5 text-xs font-medium transition-colors outline-none focus:ring-2 disabled:opacity-50 disabled:cursor-not-allowed";

  let title: string;
  let body: string;
  let primaryLabel: string;
  let secondaryLabel: string;
  let onPrimary: () => void;
  let onSecondary: () => void;

  if (payload.kind === "start") {
    title = t("meetingPrompt.startTitle");
    body = payload.appName
      ? t("meetingPrompt.startBody", { app: payload.appName })
      : t("meetingPrompt.startBodyGeneric");
    primaryLabel = t("meetingPrompt.start");
    secondaryLabel = t("meetingPrompt.dismiss");
    onPrimary = () => runCommand("accept_meeting_prompt");
    onSecondary = () => runCommand("dismiss_meeting_prompt");
  } else {
    title =
      payload.reason === "silence"
        ? t("meetingPrompt.endTitleSilence")
        : t("meetingPrompt.endTitleAppClosed");
    body =
      payload.reason === "silence"
        ? t("meetingPrompt.endBodySilence")
        : t("meetingPrompt.endBodyAppClosed");
    // Keep recording is the safe, non-destructive action → make it primary.
    primaryLabel = t("meetingPrompt.continue");
    secondaryLabel = t("meetingPrompt.end");
    onPrimary = () =>
      runCommand("respond_meeting_auto_end", { continueMeeting: true });
    onSecondary = () =>
      runCommand("respond_meeting_auto_end", { continueMeeting: false });
  }

  const handle = (action: () => void) => () => {
    if (submitted) return;
    setSubmitted(true);
    action();
  };

  return (
    <div
      dir={direction}
      className="flex h-full w-full flex-col justify-between rounded-2xl border border-neutral-200 bg-white/95 p-4 text-neutral-900 shadow-lg backdrop-blur dark:border-neutral-700 dark:bg-neutral-900/95 dark:text-neutral-100"
    >
      <div className="min-h-0">
        <h1 className="text-sm font-semibold">{title}</h1>
        <p className="mt-1 text-xs text-neutral-600 dark:text-neutral-400">
          {body}
        </p>
        {payload.kind === "end" && (
          <p className="mt-1 text-[11px] text-neutral-500 dark:text-neutral-500">
            {t("meetingPrompt.autoEndIn", { seconds })}
          </p>
        )}
      </div>

      <div className="mt-3 flex gap-2">
        <button
          type="button"
          disabled={submitted}
          onClick={handle(onPrimary)}
          className={`${buttonClass} ${primaryClass}`}
        >
          {primaryLabel}
        </button>
        <button
          type="button"
          disabled={submitted}
          onClick={handle(onSecondary)}
          className={`${buttonClass} ${secondaryClass}`}
        >
          {secondaryLabel}
        </button>
      </div>
    </div>
  );
};

export default MeetingPrompt;
