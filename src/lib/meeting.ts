// Typed helpers for the meeting-mode backend commands and event stream.
//
// Every command goes through the generated `commands.*` and is unwrapped here
// (a backend error becomes a rejected promise, which is what every caller
// expects).

import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  commands,
  type InterruptedMeeting,
  type MeetingImportProgress,
  type MeetingImportStage,
  type MeetingListItem,
  type MeetingRecord,
  type MeetingSessionInfo,
  type MeetingSummaryTemplate,
  type MeetingUsage,
  type ModelUsage,
  type Result,
  type StopMeetingResult,
  type TranscriptSegment,
  type TranscriptSource,
} from "@/bindings";

export type {
  InterruptedMeeting,
  MeetingImportProgress,
  MeetingImportStage,
  MeetingListItem,
  MeetingRecord,
  MeetingSessionInfo,
  MeetingSummaryTemplate,
  MeetingUsage,
  ModelUsage,
  StopMeetingResult,
  TranscriptSegment,
  TranscriptSource,
};

/** Resolve a specta `Result`, rejecting with the backend's error string. */
async function unwrap<T>(promise: Promise<Result<T, string>>): Promise<T> {
  const result = await promise;
  if (result.status === "error") throw result.error;
  return result.data;
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

/** Session lifecycle. `finalizing` = Stop was requested: capture has ended and
 * the backend is transcribing/saving. */
export type MeetingStatus = "idle" | "running" | "finalizing";

export const toMeetingStatus = (value: unknown): MeetingStatus =>
  value === "running" || value === "finalizing" ? value : "idle";

/** US dollars per million tokens, mirroring the Rust price table.
 *
 * Kept as an ESTIMATE and labelled as one in the UI: these models are previews,
 * prices move, and free-tier quota and billing discounts are invisible here. */
const PRICES: Array<{
  prefix: string;
  input: number;
  output: number;
  inputPerMin: number;
  outputPerMin: number;
}> = [
  // Longest prefix first: "…-transcribe-live" must not be priced by the
  // cheaper "…-transcribe" entry.
  {
    prefix: "gemini-3.5-live-translate",
    input: 3.5,
    output: 21.0,
    inputPerMin: 0.0053,
    outputPerMin: 0.0315,
  },
  {
    prefix: "gemini-3.5-transcribe-live",
    input: 3.5,
    output: 21.0,
    inputPerMin: 0.005,
    outputPerMin: 0.004,
  },
  {
    prefix: "gemini-3.5-transcribe",
    input: 2.0,
    output: 12.0,
    inputPerMin: 0.003,
    outputPerMin: 0.002,
  },
];

/** Estimated cost of a meeting in USD, and whether every model was priced.
 * Returns null when there is nothing to price at all. */
export function estimateMeetingCost(
  usage: MeetingUsage | null | undefined,
): { usd: number; complete: boolean } | null {
  if (!usage || usage.entries.length === 0) return null;
  let usd = 0;
  let complete = true;
  for (const entry of usage.entries) {
    const model = entry.model.replace(/^models\//, "");
    const price = PRICES.find((p) => model.startsWith(p.prefix));
    if (!price) {
      // Contribute nothing rather than a guess, but say the total is partial —
      // a confidently low number is worse than an openly incomplete one.
      complete = false;
      continue;
    }
    // Prefer reported tokens; fall back to the audio we measured ourselves
    // when the model reports none. Never both, or the same audio is billed
    // twice.
    if (entry.input_tokens > 0 || entry.output_tokens > 0) {
      usd += (entry.input_tokens / 1_000_000) * price.input;
      usd += (entry.output_tokens / 1_000_000) * price.output;
    } else {
      const minutes = (entry.audio_seconds ?? 0) / 60;
      usd += minutes * (price.inputPerMin + price.outputPerMin);
    }
  }
  return { usd, complete };
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

/** Where the summary provider runs, from the loaded app settings. */
export function summaryProviderInfo(
  settings:
    | {
        post_process_provider_id?: string;
        post_process_providers?: {
          id: string;
          label?: string;
          base_url?: string;
        }[];
      }
    | null
    | undefined,
): SummaryProviderInfo | null {
  if (!settings) return null;
  const id = settings.post_process_provider_id ?? "";
  const provider = (settings.post_process_providers ?? []).find(
    (p) => p.id === id,
  );
  if (!provider) return { label: "", location: "none" };
  const base = (provider.base_url ?? "").toLowerCase();
  // Local: Apple Intelligence or a localhost endpoint (e.g. Ollama).
  const isLocal =
    provider.id === "apple_intelligence" ||
    base.includes("apple-intelligence://") ||
    base.includes("localhost") ||
    base.includes("127.0.0.1");
  return {
    label: provider.label ?? provider.id,
    location: isLocal ? "local" : "cloud",
  };
}

/** Payload of the `"meeting-finalizing"` event. Mirrors Rust
 * `MeetingFinalizing`. Emitted `true` right after Stop while the high-quality
 * full-audio re-transcription runs, then `false` once the polished final
 * transcript has replaced the live preview. */
export interface MeetingFinalizing {
  finalizing: boolean;
}

/** Payload of `"meeting-summary-update"`: the summary of meeting `id`. */
export interface MeetingSummaryUpdate {
  /** Row id the summary belongs to; `null` only from older backends that sent
   * a bare string. */
  id: number | null;
  summary: string;
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

export const MEETING_EVENTS = {
  transcript: "meeting-transcript-update",
  audioLevel: "meeting-audio-level",
  finalizing: "meeting-finalizing",
  summary: "meeting-summary-update",
  title: "meeting-title-update",
  state: "meeting-state-changed",
  session: "meeting-session-changed",
  error: "meeting-error",
  importProgress: "meeting-import-progress",
  importFinished: "meeting-import-finished",
} as const;

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

/** Begin a capture + mix + VAD + transcribe meeting session (macOS). */
export function startMeeting(): Promise<void> {
  return unwrap(commands.startMeeting()).then(() => undefined);
}

/** Stop the meeting session. Resolves once the finalize pass has saved the
 * row, with that row's id (`null` when nothing was saved) and the final
 * transcript. */
export function stopMeeting(): Promise<StopMeetingResult> {
  return unwrap(commands.stopMeeting()).then((result) =>
    // Tolerate a backend that still returns the bare transcript string.
    typeof result === "string"
      ? { meeting_id: null, transcript: result }
      : result,
  );
}

/** Get the transcript accumulated so far. */
export function getMeetingTranscript(): Promise<string> {
  return unwrap(commands.getMeetingTranscript());
}

/** Snapshot of the session: state, in-progress row id, start time. Falls back
 * to the older status/started-at pair on a backend without
 * `get_meeting_session`. */
export async function getMeetingSession(): Promise<MeetingSessionInfo> {
  try {
    return await unwrap(commands.getMeetingSession());
  } catch {
    const [state, started] = await Promise.all([
      unwrap(commands.getMeetingStatus()),
      unwrap(commands.getMeetingStartedAt()).catch(() => null),
    ]);
    return { state, meeting_id: null, started_at_ms: started };
  }
}

// ---------------------------------------------------------------------------
// Saved meetings
// ---------------------------------------------------------------------------

/** List persisted meetings, newest-first. An optional `query` filters by a
 * case-insensitive substring match against title, transcript, or summary;
 * omit or pass an empty string for all meetings. */
export function listMeetings(query?: string): Promise<MeetingListItem[]> {
  const trimmed = query?.trim();
  return unwrap(
    commands.listMeetings(trimmed && trimmed.length > 0 ? trimmed : null),
  );
}

/** Fetch a single full meeting record by id. */
export function getMeeting(id: number): Promise<MeetingRecord> {
  return unwrap(commands.getMeeting(id));
}

/** Produce an LLM summary of the last session, using an optional template
 * selector (a configured template id) OR a raw custom prompt. The backend
 * merges any stored user notes as context. Empty/undefined → the default
 * prompt. */
export function summarizeMeetingWith(template?: string): Promise<string> {
  const trimmed = template?.trim();
  return unwrap(
    commands.summarizeMeetingWith(
      trimmed && trimmed.length > 0 ? trimmed : null,
    ),
  );
}

/** Regenerate the summary for an already-saved meeting `id` (optionally with a
 * template id or raw custom prompt). Persists + returns the new summary. */
export function regenerateMeetingSummary(
  id: number,
  template?: string,
): Promise<string> {
  const trimmed = template?.trim();
  return unwrap(
    commands.regenerateMeetingSummary(
      id,
      trimmed && trimmed.length > 0 ? trimmed : null,
    ),
  );
}

/** Persist the meeting's title (manual rename). */
export function updateMeetingTitle(id: number, title: string): Promise<void> {
  return unwrap(commands.updateMeetingTitle(id, title)).then(() => undefined);
}

/** Persist the user's own notes for a meeting (distinct from the AI summary).
 * Works on the in-progress row too, which is how live notes autosave. */
export function updateMeetingNotes(id: number, notes: string): Promise<void> {
  return unwrap(commands.updateMeetingNotes(id, notes)).then(() => undefined);
}

/** Render a meeting as a clean Markdown document (title, metadata, notes,
 * summary, labeled transcript). Returns the markdown string. */
export function exportMeetingMarkdown(id: number): Promise<string> {
  return unwrap(commands.exportMeetingMarkdown(id));
}

/** List meetings interrupted by a crash (still in `recording` status). The
 * live session is never included. */
export function listInterruptedMeetings(): Promise<InterruptedMeeting[]> {
  return unwrap(commands.listInterruptedMeetings());
}

/** Recover an interrupted meeting (re-finalizes from temp buffers if present,
 * else keeps the partial transcript). Resolves with the recovered transcript.
 * Async / potentially slow; show a spinner. */
export function recoverMeeting(id: number): Promise<string> {
  return unwrap(commands.recoverMeeting(id));
}

/** Delete a persisted meeting by id. */
export function deleteMeeting(id: number): Promise<void> {
  return unwrap(commands.deleteMeeting(id)).then(() => undefined);
}

/** Discard an interrupted meeting: delete the row AND the capture buffers only
 * it referenced. Distinct from `deleteMeeting`, which leaves those files. */
export function discardInterruptedMeeting(id: number): Promise<void> {
  return unwrap(commands.discardInterruptedMeeting(id)).then(() => undefined);
}

/** Get the absolute path to a meeting's saved audio file. Rejects if the
 * meeting has no saved audio. Pass the result through Tauri's
 * `convertFileSrc()` before using it as a media `src`. */
export function getMeetingAudioPath(id: number): Promise<string> {
  return unwrap(commands.getMeetingAudioPath(id));
}

/** Where meeting transcription would actually run. An empty `cloudProviders`
 * means fully on-device. */
export interface TranscriptionLocation {
  cloudProviders: string[];
}

/** Ask the backend where transcription runs. Computed there because only the
 * backend knows whether the selected model is a cloud engine. Falls back to
 * claiming cloud on error: over-claiming privacy is the one failure mode worth
 * avoiding. */
export function getTranscriptionLocation(): Promise<TranscriptionLocation> {
  return unwrap(commands.getTranscriptionLocation())
    .then((r) => ({ cloudProviders: r.cloud_providers }))
    .catch(() => ({ cloudProviders: ["?"] }));
}

// ---------------------------------------------------------------------------
// Settings & permissions
// ---------------------------------------------------------------------------

/** What a running meeting streams to the Gemini Live API, if anything. */
export type MeetingLiveMode = "off" | "translate" | "transcribe";

export const toLiveMode = (value: unknown): MeetingLiveMode =>
  value === "translate" || value === "transcribe" ? value : "off";

/** Status of macOS calendar access for meeting naming. */
export type CalendarAccessStatus =
  | "authorized"
  | "denied"
  | "notDetermined"
  | "unavailable";

/** Read the current Calendars permission status for meeting naming. */
export function getCalendarAccessStatus(): Promise<CalendarAccessStatus> {
  return unwrap(commands.getCalendarAccessStatus()).then(
    (s) => s as CalendarAccessStatus,
  );
}

/** Request Calendars access (shows the system prompt on first call). Resolves
 * `true` when full access is granted. May take as long as the user leaves the
 * prompt open. */
export function requestCalendarAccess(): Promise<boolean> {
  return unwrap(commands.requestCalendarAccess());
}

/** Persist the folder completed meetings are also written to as Markdown
 * (`meeting_export_dir`); an empty string turns the copy off. Rejects when the
 * folder does not exist. */
export function changeMeetingExportDir(dir: string): Promise<void> {
  return unwrap(commands.changeMeetingExportDirSetting(dir)).then(
    () => undefined,
  );
}

// ---------------------------------------------------------------------------
// Recording import & re-transcription
// ---------------------------------------------------------------------------

/** Payload of `"meeting-import-finished"`. Mirrors Rust
 * `MeetingImportFinished`. */
export interface MeetingImportFinished {
  id: number | null;
  error: string | null;
}

/** Error text the backend uses for a cancelled import (`import::CANCELLED`);
 * a cancel is not shown as a failure. */
export const MEETING_IMPORT_CANCELLED = "Import cancelled.";

/** Import a recording file (voice memo, conference recording) as a meeting.
 * Slow: resolves with the new meeting id once it is transcribed and saved. */
export function importMeetingRecording(path: string): Promise<number> {
  return unwrap(commands.importMeetingRecording(path));
}

/** Ask the running import (or re-transcription) to stop at its next
 * checkpoint. */
export function cancelMeetingImport(): Promise<void> {
  return commands.cancelMeetingImport();
}

/** Progress of the running import, or `null` when nothing is importing. */
export function getMeetingImportProgress(): Promise<MeetingImportProgress | null> {
  return commands.getMeetingImportProgress();
}

/** File extensions the import accepts (lower-case, no dot). */
export function getSupportedImportExtensions(): Promise<string[]> {
  return commands.getSupportedImportExtensions();
}

/** Transcribe a saved meeting again from its stored audio, replacing its
 * transcript (and summary, when it had one). Slow; progress arrives on the
 * import events. Resolves with the meeting id. */
export function retranscribeMeeting(id: number): Promise<number> {
  return unwrap(commands.retranscribeMeeting(id));
}

// ---------------------------------------------------------------------------
// Event subscriptions used by components (the session itself is tracked by
// `stores/meetingStore.ts`)
// ---------------------------------------------------------------------------

/** Subscribe to live audio-level updates (oscilloscope wave + level bars +
 * peak), emitted throttled (~20 fps) while a meeting is recording. */
export function listenMeetingAudioLevel(
  cb: (lvl: MeetingAudioLevel) => void,
): Promise<UnlistenFn> {
  return listen<MeetingAudioLevel>(MEETING_EVENTS.audioLevel, (event) => {
    cb(event.payload);
  });
}

export function listenMeetingImportProgress(
  cb: (progress: MeetingImportProgress) => void,
): Promise<UnlistenFn> {
  return listen<MeetingImportProgress>(MEETING_EVENTS.importProgress, (event) =>
    cb(event.payload),
  );
}

export function listenMeetingImportFinished(
  cb: (result: MeetingImportFinished) => void,
): Promise<UnlistenFn> {
  return listen<MeetingImportFinished>(MEETING_EVENTS.importFinished, (event) =>
    cb(event.payload),
  );
}
