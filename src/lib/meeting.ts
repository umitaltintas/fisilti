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
  /** Translation of `text`, present only for segments produced by the Gemini
   * Live translation path. */
  translation?: string | null;
  /** Display label of the speaker who said this ("Speaker 1"), present only for
   * segments produced by the Gemini batch finalize pass with diarization on. */
  speaker?: string | null;
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
  /** Tokens the Gemini paths reported, or null when no cloud model ran (or the
   * meeting predates usage tracking). Absent is NOT the same as zero. */
  usage?: MeetingUsage | null;
}

/** Tokens spent on one model. Mirrors Rust `ModelUsage`. */
export interface ModelUsage {
  model: string;
  input_tokens: number;
  output_tokens: number;
  /** Seconds of audio streamed. The only basis available for the Live API,
   * which reports no token counts for these models. */
  audio_seconds?: number;
}

/** Everything a meeting spent. Mirrors Rust `MeetingUsage`. */
export interface MeetingUsage {
  entries: ModelUsage[];
}

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
const MEETING_STATE_CHANGED = "meeting-state-changed";
const MEETING_ERROR = "meeting-error";

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

/** Epoch-ms start time of the running session, or `null` when idle. Lets the UI
 * show a truthful elapsed timer when it attaches to a session that was started
 * from the tray, a global shortcut, or the auto-detect prompt. */
export function getMeetingStartedAt(): Promise<number | null> {
  return invoke<number | null>("get_meeting_started_at");
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

/** Where meeting transcription would actually run. Mirrors Rust
 * `TranscriptionLocation`. An empty `cloudProviders` means fully on-device. */
export interface TranscriptionLocation {
  cloudProviders: string[];
}

/** Ask the backend where transcription runs. Computed there because only the
 * backend knows whether the selected model is a cloud engine. Falls back to
 * claiming cloud on error: over-claiming privacy is the one failure mode worth
 * avoiding. */
export function getTranscriptionLocation(): Promise<TranscriptionLocation> {
  return invoke<{ cloud_providers?: string[] }>("get_transcription_location")
    .then((r) => ({ cloudProviders: r?.cloud_providers ?? [] }))
    .catch(() => ({ cloudProviders: ["?"] }));
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

/** Discard an interrupted meeting: delete the row AND the capture buffers only
 * it referenced. Distinct from `deleteMeeting`, which leaves those files. */
export function discardInterruptedMeeting(id: number): Promise<void> {
  return invoke<void>("discard_interrupted_meeting", { id });
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

/** What a running meeting streams to the Gemini Live API, if anything. */
export type MeetingLiveMode = "off" | "translate" | "transcribe";

/** Every Gemini meeting setting, read together for the settings UI. */
export interface MeetingGeminiSettings {
  /** Live streaming mode for a running meeting. */
  liveMode: MeetingLiveMode;
  /** BCP-47 code to translate INTO, e.g. "en". Only used in "translate" mode. */
  targetLanguage: string;
  /** Ask the finalize pass to attribute speech to individual speakers.
   * Only has an effect when the meeting model is Gemini. */
  diarize: boolean;
  /** Clean disfluencies and format, rather than transcribe verbatim. */
  smart: boolean;
  /** Float live subtitles near the bottom of the screen during a meeting. */
  subtitles: boolean;
  /** Whether a Gemini API key is stored. The key itself is never read back. */
  hasApiKey: boolean;
}

const MEETING_GEMINI_DEFAULTS: MeetingGeminiSettings = {
  liveMode: "off",
  targetLanguage: "en",
  diarize: true,
  smart: true,
  subtitles: true,
  hasApiKey: false,
};

/** Read the Gemini meeting settings. The API key is reported only as
 * "present or not" — the UI never displays a stored key. */
export function getMeetingGeminiSettings(): Promise<MeetingGeminiSettings> {
  return invoke<{
    meeting_live_mode?: string;
    meeting_live_translate_target?: string;
    meeting_gemini_diarize?: boolean;
    meeting_gemini_smart?: boolean;
    meeting_subtitles?: boolean;
    gemini_api_key?: string;
  }>("get_app_settings")
    .then((s) => ({
      liveMode: isLiveMode(s?.meeting_live_mode)
        ? s.meeting_live_mode
        : MEETING_GEMINI_DEFAULTS.liveMode,
      targetLanguage:
        s?.meeting_live_translate_target ||
        MEETING_GEMINI_DEFAULTS.targetLanguage,
      diarize: s?.meeting_gemini_diarize ?? MEETING_GEMINI_DEFAULTS.diarize,
      smart: s?.meeting_gemini_smart ?? MEETING_GEMINI_DEFAULTS.smart,
      subtitles: s?.meeting_subtitles ?? MEETING_GEMINI_DEFAULTS.subtitles,
      hasApiKey: (s?.gemini_api_key ?? "").trim().length > 0,
    }))
    .catch(() => ({ ...MEETING_GEMINI_DEFAULTS }));
}

function isLiveMode(value: unknown): value is MeetingLiveMode {
  return value === "off" || value === "translate" || value === "transcribe";
}

/** Pick what a running meeting streams to Gemini (`meeting_live_mode`). */
export function changeMeetingLiveMode(mode: MeetingLiveMode): Promise<void> {
  return invoke<void>("change_meeting_live_mode_setting", { mode });
}

/** Set the language live translation translates into (BCP-47). */
export function changeMeetingLiveTranslateTarget(
  language: string,
): Promise<void> {
  return invoke<void>("change_meeting_live_translate_target_setting", {
    language,
  });
}

/** Turn speaker attribution on or off for the Gemini finalize pass. */
export function changeMeetingGeminiDiarize(enabled: boolean): Promise<void> {
  return invoke<void>("change_meeting_gemini_diarize_setting", { enabled });
}

/** Cleaned-up ("smart") vs. verbatim Gemini transcription. */
export function changeMeetingGeminiSmart(enabled: boolean): Promise<void> {
  return invoke<void>("change_meeting_gemini_smart_setting", { enabled });
}

/** Show or hide the live subtitle strip. */
export function changeMeetingSubtitles(enabled: boolean): Promise<void> {
  return invoke<void>("change_meeting_subtitles_setting", { enabled });
}

/** Store (or, with an empty string, clear) the Gemini API key. */
export function changeGeminiApiKey(apiKey: string): Promise<void> {
  return invoke<void>("change_gemini_api_key_setting", { apiKey });
}

/**
 * Choose which model transcribes meetings. An empty string means "follow the
 * dictation model", which is the default.
 */
export function changeMeetingSelectedModel(modelId: string): Promise<void> {
  return invoke<void>("change_meeting_selected_model_setting", { modelId });
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

/** Subscribe to session start/stop, whatever triggered it — the UI command, the
 * tray item, a global shortcut, or the meeting auto-detect prompt. Without this
 * a window that is already open never learns about a session started elsewhere.
 * Returns a promise resolving to the unlisten function. */
export function listenMeetingState(
  cb: (status: MeetingStatus) => void,
): Promise<UnlistenFn> {
  return listen<string>(MEETING_STATE_CHANGED, (event) => {
    cb(event.payload === "running" ? "running" : "idle");
  });
}

/** Subscribe to transcription failures reported by the on-stop finalize pass
 * (every window failed: no API balance, network down, model unavailable). The
 * payload is the underlying error message. Returns a promise resolving to the
 * unlisten function. */
export function listenMeetingError(
  cb: (message: string) => void,
): Promise<UnlistenFn> {
  return listen<string>(MEETING_ERROR, (event) => {
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

/** Persist the folder completed meetings are also written to as Markdown
 * (`meeting_export_dir`); an empty string turns the copy off. Rejects when the
 * folder does not exist. */
export function changeMeetingExportDir(dir: string): Promise<void> {
  return invoke<void>("change_meeting_export_dir_setting", { dir });
}

/** Read the Markdown export folder; `""` when the copy is off. */
export function getMeetingExportDir(): Promise<string> {
  return invoke<{ meeting_export_dir?: string }>("get_app_settings")
    .then((s) => s?.meeting_export_dir ?? "")
    .catch(() => "");
}

/** Which step of a recording import is running. Mirrors Rust
 * `MeetingImportStage`. */
export type MeetingImportStage = "decoding" | "transcribing" | "summarizing";

/** Payload of `"meeting-import-progress"`. Mirrors Rust
 * `MeetingImportProgress`. */
export interface MeetingImportProgress {
  file_name: string;
  stage: MeetingImportStage;
  /** 0..1 within the stage; `null` when the stage cannot measure itself. */
  progress: number | null;
}

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
  return invoke<number>("import_meeting_recording", { path });
}

/** Ask the running import to stop at its next checkpoint. */
export function cancelMeetingImport(): Promise<void> {
  return invoke<void>("cancel_meeting_import");
}

/** Progress of the running import, or `null` when nothing is importing. */
export function getMeetingImportProgress(): Promise<MeetingImportProgress | null> {
  return invoke<MeetingImportProgress | null>("get_meeting_import_progress");
}

/** File extensions the import accepts (lower-case, no dot). */
export function getSupportedImportExtensions(): Promise<string[]> {
  return invoke<string[]>("get_supported_import_extensions");
}

export function listenMeetingImportProgress(
  cb: (progress: MeetingImportProgress) => void,
): Promise<UnlistenFn> {
  return listen<MeetingImportProgress>("meeting-import-progress", (event) => {
    cb(event.payload);
  });
}

export function listenMeetingImportFinished(
  cb: (result: MeetingImportFinished) => void,
): Promise<UnlistenFn> {
  return listen<MeetingImportFinished>("meeting-import-finished", (event) => {
    cb(event.payload);
  });
}

/** Transcribe a saved meeting again from its stored audio, replacing its
 * transcript (and summary, when it had one). Slow; progress arrives on the
 * import events. Resolves with the meeting id. */
export function retranscribeMeeting(id: number): Promise<number> {
  return invoke<number>("retranscribe_meeting", { id });
}
