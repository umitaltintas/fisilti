// Typed helpers for the meeting-mode backend commands and event stream.
//
// The meeting commands are registered in Rust but are NOT (yet) present in the
// auto-generated `src/bindings.ts`. To avoid a typecheck dependency on
// regenerating bindings, we call them through the raw Tauri api here and expose
// a small typed surface so components stay clean.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** A single transcribed speech segment. Mirrors Rust `TranscriptSegment`. */
export interface TranscriptSegment {
  /** Cleaned transcript text for this segment. */
  text: string;
  /** Milliseconds since the meeting session started. */
  timestamp_ms: number;
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
}

const MEETING_TRANSCRIPT_UPDATE = "meeting-transcript-update";
const MEETING_AUDIO_LEVEL = "meeting-audio-level";

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
