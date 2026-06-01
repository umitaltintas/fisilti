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

export type MeetingStatus = "idle" | "running";

const MEETING_TRANSCRIPT_UPDATE = "meeting-transcript-update";

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

/** Subscribe to live transcript updates. Returns a promise resolving to the
 * unlisten function (call it to clean up). */
export function listenMeetingTranscript(
  cb: (update: MeetingTranscriptUpdate) => void,
): Promise<UnlistenFn> {
  return listen<MeetingTranscriptUpdate>(MEETING_TRANSCRIPT_UPDATE, (event) => {
    cb(event.payload);
  });
}
