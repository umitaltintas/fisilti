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

/** List all persisted meetings, newest-first. */
export function listMeetings(): Promise<MeetingListItem[]> {
  return invoke<MeetingListItem[]>("list_meetings");
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
