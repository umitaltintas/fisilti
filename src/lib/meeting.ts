// Typed helpers for the meeting-mode backend commands and event stream.
//
// The meeting commands are registered in Rust but are NOT (yet) present in the
// auto-generated `src/bindings.ts`. To avoid a typecheck dependency on
// regenerating bindings, we call them through the raw Tauri api here and expose
// a small typed surface so components stay clean.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** Which captured source produced a transcript segment. `"you"` is the local
 * microphone / local speaker; `"others"` is system / remote audio. */
export type TranscriptSource = "you" | "others";

/** A single transcribed speech segment. Mirrors Rust `TranscriptSegment`. */
export interface TranscriptSegment {
  /** Cleaned transcript text for this segment. */
  text: string;
  /** Milliseconds since the meeting session started. */
  timestamp_ms: number;
  /** Which captured source produced this segment (mic = "you",
   * system = "others"). */
  source: TranscriptSource;
}

/** Payload of the `"meeting-transcript-update"` event. Mirrors Rust
 * `MeetingTranscriptUpdate`. */
export interface MeetingTranscriptUpdate {
  segment: TranscriptSegment;
  /** The full transcript so far (all segments joined). */
  full_transcript: string;
}

/** Payload of the `"meeting-audio-level"` event. Mirrors Rust
 * `MeetingAudioLevel`. Emitted throttled (~20 fps) while a meeting records;
 * a flat (zeros) event is emitted when silent or stopped. */
export interface MeetingAudioLevel {
  /** 16 level-bar values, each in 0..1. */
  bars: number[];
  /** 96 oscilloscope waveform samples, each in -1..1. */
  wave: number[];
  /** Overall peak amplitude, 0..1. */
  peak: number;
}

export type MeetingStatus = "idle" | "running";

/** Lightweight row for the past-meetings list. Mirrors Rust `MeetingListItem`
 * (`src-tauri/src/meeting/store.rs`). */
export interface MeetingListItem {
  id: number;
  /** Epoch milliseconds. */
  started_at: number;
  /** Epoch milliseconds. */
  ended_at: number;
  duration_ms: number;
  title: string;
  has_summary: boolean;
  /** Short preview of the transcript (first ~200 chars). */
  transcript_preview: string;
  /** Lifecycle status: `"recording"` (interrupted) or `"completed"`. */
  status: string;
}

/** Full meeting record. Mirrors Rust `MeetingRecord`
 * (`src-tauri/src/meeting/store.rs`). */
export interface MeetingRecord {
  id: number;
  /** Epoch milliseconds. */
  started_at: number;
  /** Epoch milliseconds. */
  ended_at: number;
  duration_ms: number;
  title: string;
  transcript: string;
  segments: TranscriptSegment[];
  summary: string | null;
  /** Epoch milliseconds. */
  created_at: number;
  /** Absolute path to the saved mixed-audio file, if one exists. Older
   * meetings recorded before audio persistence have no audio. */
  audio_path?: string | null;
  /** The user's own editable notes, distinct from the AI `summary`. */
  notes?: string | null;
  /** Lifecycle status: `"recording"` or `"completed"`. */
  status?: string;
}

/** A preset summary prompt template. Mirrors Rust `MeetingSummaryTemplate`
 * (`src-tauri/src/settings.rs`). The `id` is passed to `summarizeMeetingWith`
 * / `regenerateMeetingSummary`; `name` is the display label. */
export interface MeetingSummaryTemplate {
  id: string;
  name: string;
  prompt: string;
}

/** An interrupted meeting (status still `"recording"`) detected at startup.
 * Mirrors Rust `InterruptedMeeting` (`src-tauri/src/meeting/store.rs`). */
export interface InterruptedMeeting {
  id: number;
  /** Epoch milliseconds. */
  started_at: number;
  title: string;
  transcript: string;
  /** True if the per-source temp audio buffers still exist on disk, so a
   * re-finalize can recover a high-quality transcript. */
  has_buffers: boolean;
}

/** Where the summary LLM runs, derived from the active post-process provider.
 * `local` → on-device (Apple Intelligence / localhost Ollama); `cloud` → the
 * request leaves the device; `none` → no provider configured. */
export type SummaryLocation = "local" | "cloud" | "none";

/** Resolved info about the summary provider for the trust indicator. */
export interface SummaryProviderInfo {
  /** Display label of the active provider, if any. */
  label: string;
  location: SummaryLocation;
}

/** Payload of the `"meeting-finalizing"` event. Mirrors Rust
 * `MeetingFinalizing`. Emitted `true` right after Stop while the high-quality
 * full-audio re-transcription runs, then `false` once the polished final
 * transcript has replaced the live preview. */
export interface MeetingFinalizing {
  finalizing: boolean;
}

const MEETING_TRANSCRIPT_UPDATE = "meeting-transcript-update";
const MEETING_AUDIO_LEVEL = "meeting-audio-level";
const MEETING_FINALIZING = "meeting-finalizing";
const MEETING_SUMMARY_UPDATE = "meeting-summary-update";
const MEETING_TITLE_UPDATE = "meeting-title-update";

/** Begin a capture + mix + VAD + transcribe meeting session (macOS). */
export function startMeeting(): Promise<void> {
  return invoke<void>("start_meeting");
}

/** Stop the meeting session; resolves with the final transcript text. */
export function stopMeeting(): Promise<string> {
  return invoke<string>("stop_meeting");
}

/** Get the transcript accumulated so far. */
export function getMeetingTranscript(): Promise<string> {
  return invoke<string>("get_meeting_transcript");
}

/** Get the current session status. */
export function getMeetingStatus(): Promise<MeetingStatus> {
  return invoke<string>("get_meeting_status").then((s) =>
    s === "running" ? "running" : "idle",
  );
}

/** Produce an LLM summary of the accumulated transcript (markdown-ish notes). */
export function summarizeMeeting(): Promise<string> {
  return invoke<string>("summarize_meeting");
}

/** List persisted meetings, newest-first. An optional `query` filters by a
 * case-insensitive substring match against title, transcript, or summary;
 * omit or pass an empty string for all meetings. */
export function listMeetings(query?: string): Promise<MeetingListItem[]> {
  const trimmed = query?.trim();
  return invoke<MeetingListItem[]>("list_meetings", {
    query: trimmed && trimmed.length > 0 ? trimmed : null,
  });
}

/** Produce an LLM summary using an optional template selector (a configured
 * template id) OR a raw custom prompt. The backend merges any stored user
 * notes as context. Empty/undefined → the default prompt. */
export function summarizeMeetingWith(template?: string): Promise<string> {
  const trimmed = template?.trim();
  return invoke<string>("summarize_meeting_with", {
    template: trimmed && trimmed.length > 0 ? trimmed : null,
  });
}

/** Regenerate the summary for an already-saved meeting `id` (optionally with a
 * template id or raw custom prompt). Persists + returns the new summary. */
export function regenerateMeetingSummary(
  id: number,
  template?: string,
): Promise<string> {
  const trimmed = template?.trim();
  return invoke<string>("regenerate_meeting_summary", {
    id,
    template: trimmed && trimmed.length > 0 ? trimmed : null,
  });
}

/** Persist the meeting's title (manual rename). */
export function updateMeetingTitle(id: number, title: string): Promise<void> {
  return invoke<void>("update_meeting_title", { id, title });
}

/** Persist the user's own editable notes for a meeting (distinct from the AI
 * summary). */
export function updateMeetingNotes(id: number, notes: string): Promise<void> {
  return invoke<void>("update_meeting_notes", { id, notes });
}

/** Render a meeting as a clean Markdown document (title, metadata, notes,
 * summary, labeled transcript). Returns the markdown string. */
export function exportMeetingMarkdown(id: number): Promise<string> {
  return invoke<string>("export_meeting_markdown", { id });
}

/** List meetings interrupted by a crash (still in `recording` status). */
export function listInterruptedMeetings(): Promise<InterruptedMeeting[]> {
  return invoke<InterruptedMeeting[]>("list_interrupted_meetings");
}

/** Recover an interrupted meeting (re-finalizes from temp buffers if present,
 * else keeps the partial transcript). Resolves with the recovered transcript.
 * Async / potentially slow; show a spinner. */
export function recoverMeeting(id: number): Promise<string> {
  return invoke<string>("recover_meeting", { id });
}

/** Read the configured meeting summary templates from persisted app settings.
 * Falls back to an empty list on error. */
export function getMeetingSummaryTemplates(): Promise<
  MeetingSummaryTemplate[]
> {
  return invoke<{ meeting_summary_templates?: MeetingSummaryTemplate[] }>(
    "get_app_settings",
  )
    .then((s) => s?.meeting_summary_templates ?? [])
    .catch(() => []);
}

/** Resolve the active post-process provider used for summaries and whether it
 * runs locally or in the cloud, for the honest trust indicator. */
export function getSummaryProviderInfo(): Promise<SummaryProviderInfo> {
  return invoke<{
    post_process_provider_id?: string;
    post_process_providers?: {
      id: string;
      label?: string;
      base_url?: string;
    }[];
  }>("get_app_settings")
    .then((s) => {
      const id = s?.post_process_provider_id ?? "";
      const provider = (s?.post_process_providers ?? []).find(
        (p) => p.id === id,
      );
      if (!provider) {
        return { label: "", location: "none" as SummaryLocation };
      }
      const base = (provider.base_url ?? "").toLowerCase();
      // Local: Apple Intelligence or a localhost endpoint (e.g. Ollama).
      const isLocal =
        provider.id === "apple_intelligence" ||
        base.includes("apple-intelligence://") ||
        base.includes("localhost") ||
        base.includes("127.0.0.1");
      return {
        label: provider.label ?? provider.id,
        location: (isLocal ? "local" : "cloud") as SummaryLocation,
      };
    })
    .catch(() => ({ label: "", location: "none" as SummaryLocation }));
}

/** Fetch a single full meeting record by id. */
export function getMeeting(id: number): Promise<MeetingRecord> {
  return invoke<MeetingRecord>("get_meeting", { id });
}

/** Delete a persisted meeting by id. */
export function deleteMeeting(id: number): Promise<void> {
  return invoke<void>("delete_meeting", { id });
}

/** Get the absolute path to a meeting's saved mixed-audio file. Rejects if the
 * meeting has no saved audio. Pass the result through Tauri's
 * `convertFileSrc()` before using it as an `<audio>` `src`. */
export function getMeetingAudioPath(id: number): Promise<string> {
  return invoke<string>("get_meeting_audio_path", { id });
}

/** Persist the meeting auto-summarize setting (`meeting_auto_summarize`). When
 * enabled, a summary is produced automatically after Stop and pushed via the
 * `"meeting-summary-update"` event. */
export function changeMeetingAutoSummarize(enabled: boolean): Promise<void> {
  return invoke<void>("change_meeting_auto_summarize_setting", { enabled });
}

/** Read the current meeting auto-summarize setting from persisted app
 * settings. Falls back to `false` if the setting cannot be read. */
export function getMeetingAutoSummarize(): Promise<boolean> {
  return invoke<{ meeting_auto_summarize?: boolean }>("get_app_settings")
    .then((s) => s?.meeting_auto_summarize ?? false)
    .catch(() => false);
}

/** The four automatic-detection settings, read together for the settings UI.
 * Mirrors the corresponding `meeting_*` fields in persisted app settings. */
export interface MeetingAutoDetectSettings {
  /** Watch for meeting apps grabbing the mic and offer to start transcribing. */
  autoDetect: boolean;
  /** Offer to end (and auto-end) the meeting after prolonged silence. */
  autoEnd: boolean;
  /** Seconds of silence before the end-of-meeting prompt appears. */
  silenceTimeoutSecs: number;
  /** Seconds the end prompt waits, unanswered, before auto-ending. */
  autoEndGraceSecs: number;
}

/** Defaults for the automatic-detection settings, matching the Rust
 * `AppSettings` defaults. Used as the fallback for `getMeetingAutoDetectSettings`. */
const MEETING_AUTO_DETECT_DEFAULTS: MeetingAutoDetectSettings = {
  autoDetect: false,
  autoEnd: true,
  silenceTimeoutSecs: 180,
  autoEndGraceSecs: 60,
};

/** Persist the meeting auto-detect setting (`meeting_auto_detect`). When
 * enabled, a meeting app grabbing the microphone triggers a prompt offering to
 * start transcription. */
export function changeMeetingAutoDetect(enabled: boolean): Promise<void> {
  return invoke<void>("change_meeting_auto_detect_setting", { enabled });
}

/** Persist the meeting auto-end setting (`meeting_auto_end`). When enabled, a
 * running meeting that goes silent for `silenceTimeoutSecs` prompts to end and
 * auto-ends after `autoEndGraceSecs` if unanswered. */
export function changeMeetingAutoEnd(enabled: boolean): Promise<void> {
  return invoke<void>("change_meeting_auto_end_setting", { enabled });
}

/** Persist the silence-before-asking duration (`meeting_silence_timeout_secs`).
 * The backend clamps to 30–3600 seconds. */
export function changeMeetingSilenceTimeout(secs: number): Promise<void> {
  return invoke<void>("change_meeting_silence_timeout_setting", { secs });
}

/** Persist the auto-end grace duration (`meeting_auto_end_grace_secs`) — how
 * long the end prompt waits before auto-ending. The backend clamps to 10–600
 * seconds. */
export function changeMeetingAutoEndGrace(secs: number): Promise<void> {
  return invoke<void>("change_meeting_auto_end_grace_setting", { secs });
}

/** Read all four automatic-detection settings from persisted app settings in a
 * single call. Any field that cannot be read falls back to its default
 * (`autoDetect: false`, `autoEnd: true`, `silenceTimeoutSecs: 180`,
 * `autoEndGraceSecs: 60`); a failed read returns all defaults. */
export function getMeetingAutoDetectSettings(): Promise<MeetingAutoDetectSettings> {
  return invoke<{
    meeting_auto_detect?: boolean;
    meeting_auto_end?: boolean;
    meeting_silence_timeout_secs?: number;
    meeting_auto_end_grace_secs?: number;
  }>("get_app_settings")
    .then((s) => ({
      autoDetect:
        s?.meeting_auto_detect ?? MEETING_AUTO_DETECT_DEFAULTS.autoDetect,
      autoEnd: s?.meeting_auto_end ?? MEETING_AUTO_DETECT_DEFAULTS.autoEnd,
      silenceTimeoutSecs:
        s?.meeting_silence_timeout_secs ??
        MEETING_AUTO_DETECT_DEFAULTS.silenceTimeoutSecs,
      autoEndGraceSecs:
        s?.meeting_auto_end_grace_secs ??
        MEETING_AUTO_DETECT_DEFAULTS.autoEndGraceSecs,
    }))
    .catch(() => ({ ...MEETING_AUTO_DETECT_DEFAULTS }));
}

/** Subscribe to live transcript updates. Returns a promise resolving to the
 * unlisten function (call it to clean up). */
export function listenMeetingTranscript(
  cb: (update: MeetingTranscriptUpdate) => void,
): Promise<UnlistenFn> {
  return listen<MeetingTranscriptUpdate>(MEETING_TRANSCRIPT_UPDATE, (event) => {
    cb(event.payload);
  });
}

/** Subscribe to live audio-level updates (oscilloscope wave + level bars +
 * peak), emitted throttled (~20 fps) while a meeting is recording. Returns a
 * promise resolving to the unlisten function (call it to clean up). */
export function listenMeetingAudioLevel(
  cb: (lvl: MeetingAudioLevel) => void,
): Promise<UnlistenFn> {
  return listen<MeetingAudioLevel>(MEETING_AUDIO_LEVEL, (event) => {
    cb(event.payload);
  });
}

/** Subscribe to the on-stop "finalizing" signal. Fires `true` while the
 * high-quality re-transcription runs after Stop, then `false` once the
 * polished final transcript has been emitted. Returns a promise resolving to
 * the unlisten function (call it to clean up). */
export function listenMeetingFinalizing(
  cb: (finalizing: boolean) => void,
): Promise<UnlistenFn> {
  return listen<MeetingFinalizing>(MEETING_FINALIZING, (event) => {
    cb(event.payload.finalizing);
  });
}

/** Subscribe to automatic summary updates. Fires only when auto-summarize is
 * enabled; the payload is the summary string. Returns a promise resolving to
 * the unlisten function (call it to clean up). */
export function listenMeetingSummary(
  cb: (summary: string) => void,
): Promise<UnlistenFn> {
  return listen<string>(MEETING_SUMMARY_UPDATE, (event) => {
    cb(event.payload);
  });
}

/** Payload of the `"meeting-title-update"` event. Mirrors Rust
 * `MeetingTitleUpdate`. Fired whenever a meeting's title changes
 * automatically: at session start when the calendar/window naming resolves,
 * or after stop when the LLM auto-title lands. */
export interface MeetingTitleUpdate {
  /** Row id of the renamed meeting. */
  id: number;
  /** The new title. */
  title: string;
}

/** Subscribe to automatic title updates. Returns a promise resolving to the
 * unlisten function. */
export function listenMeetingTitle(
  cb: (update: MeetingTitleUpdate) => void,
): Promise<UnlistenFn> {
  return listen<MeetingTitleUpdate>(MEETING_TITLE_UPDATE, (event) => {
    cb(event.payload);
  });
}

/** Status of macOS calendar access for meeting naming. */
export type CalendarAccessStatus =
  | "authorized"
  | "denied"
  | "notDetermined"
  | "unavailable";

/** Read the current Calendars permission status for meeting naming. */
export function getCalendarAccessStatus(): Promise<CalendarAccessStatus> {
  return invoke<string>("get_calendar_access_status").then(
    (s) => s as CalendarAccessStatus,
  );
}

/** Request Calendars access (shows the system prompt on first call). Resolves
 * `true` when full access is granted. May take as long as the user leaves the
 * prompt open. */
export function requestCalendarAccess(): Promise<boolean> {
  return invoke<boolean>("request_calendar_access");
}

/** Persist the calendar-naming setting (`meeting_calendar_names`). When
 * enabled, a new session is named after the calendar event in progress. */
export function changeMeetingCalendarNames(enabled: boolean): Promise<void> {
  return invoke<void>("change_meeting_calendar_names_setting", { enabled });
}

/** Read the calendar-naming setting from persisted app settings. Falls back
 * to `false` if the setting cannot be read. */
export function getMeetingCalendarNames(): Promise<boolean> {
  return invoke<{ meeting_calendar_names?: boolean }>("get_app_settings")
    .then((s) => s?.meeting_calendar_names ?? false)
    .catch(() => false);
}
