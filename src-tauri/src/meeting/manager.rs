// Meeting mode (Step 3): continuous meeting session manager.
//
// Owns a long-running session that:
//   1. Captures mixed mic + system audio at 16 kHz mono (reusing the exact
//      capture/mix machinery proven in `commands::audio::capture_mixed_audio_test`).
//   2. Segments the mixed stream with a dedicated `SmoothedVad` instance in
//      480-sample (30 ms) frames.
//   3. Sends each completed speech segment to `TranscriptionManager::transcribe`.
//   4. Accumulates the returned text into a running transcript and emits a
//      `"meeting-transcript-update"` event per segment.
//
// This module is ADDITIVE and ISOLATED from the dictation flow. It never touches
// the `AudioRecordingManager` / `RecordingState` singletons. It uses a SEPARATE
// VAD instance (not shared with dictation) and an independent cpal mic stream.
//
// The capture loop is macOS-only (CoreAudio tap). On other platforms `start()`
// returns an "unsupported" error; the struct and commands still compile.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::AppHandle;

use crate::managers::transcription::TranscriptionManager;
use crate::meeting::store::{MeetingRecordInput, MeetingStore};

/// Which captured source a transcript segment came from.
///
/// Serializes as `"you"` (microphone / the local speaker) and `"others"`
/// (system audio / remote participants) so the frontend can label segments
/// directly without an extra mapping step.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum TranscriptSource {
    #[serde(rename = "you")]
    Mic,
    #[serde(rename = "others")]
    System,
}

/// A single transcribed speech segment with its (relative) start timestamp.
#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct TranscriptSegment {
    /// Cleaned transcript text for this segment.
    pub text: String,
    /// Milliseconds since the meeting session started.
    pub timestamp_ms: u64,
    /// Which captured source produced this segment (mic = "you",
    /// system = "others").
    #[serde(default = "default_transcript_source")]
    pub source: TranscriptSource,
}

/// Default source for segments deserialized from older records that predate the
/// per-source labeling feature: treat them as microphone ("you").
fn default_transcript_source() -> TranscriptSource {
    TranscriptSource::Mic
}

/// Event payload emitted on `"meeting-transcript-update"` after each segment.
#[derive(Clone, Debug, Serialize, Type)]
pub struct MeetingTranscriptUpdate {
    pub segment: TranscriptSegment,
    /// The full transcript so far (all segments joined).
    pub full_transcript: String,
}

/// Event payload emitted on `"meeting-finalizing"` to signal the on-stop
/// full-audio re-transcription pass. The UI keeps showing the live (rough)
/// transcript as a preview while `finalizing` is true, then receives the
/// replacement final transcript via the usual `"meeting-transcript-update"`
/// event once it is `false` again.
#[derive(Clone, Debug, Serialize, Type)]
pub struct MeetingFinalizing {
    /// True when the finalize pass starts, false when it completes.
    pub finalizing: bool,
}

/// Event payload emitted on `"meeting-audio-level"` (~20 fps) for a live UI
/// visualizer. This is SEPARATE from the dictation `"mic-level"` event so the
/// two visualizers never interfere.
#[derive(Clone, Debug, Serialize, Type)]
pub struct MeetingAudioLevel {
    /// ~16 normalized 0..1 frequency-bar levels (same shape as `mic-level`,
    /// produced by the shared `AudioVisualiser`).
    pub bars: Vec<f32>,
    /// ~96 downsampled samples in -1..1 for an oscilloscope trace of the most
    /// recent window. Flat (all zeros) when silent.
    pub wave: Vec<f32>,
    /// 0..1 peak absolute amplitude of the window.
    pub peak: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeetingState {
    Idle,
    Running,
}

/// Owns the state of a continuous meeting session.
///
/// Cloneable handle around shared state; the actual capture work runs on a
/// dedicated thread spawned in `start()`.
#[derive(Clone)]
pub struct MeetingManager {
    app_handle: AppHandle,
    transcription_manager: Arc<TranscriptionManager>,
    /// Idle/Running state guard. Held briefly to serialize start/stop.
    state: Arc<Mutex<MeetingState>>,
    /// Accumulated transcript segments.
    transcript: Arc<Mutex<Vec<TranscriptSegment>>>,
    /// Stop signal for the capture loop.
    stop_signal: Arc<AtomicBool>,
    /// Fast-path flag the TranscriptionManager idle-watcher checks to avoid
    /// unloading the model mid-meeting. Set true while a session runs.
    active: Arc<AtomicBool>,
    /// Handle of the running capture thread, joined on stop.
    worker: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,
    /// Persistence store for meeting sessions (same history.db as dictation).
    store: MeetingStore,
    /// Absolute epoch-ms timestamp of when the current session started.
    /// Set in `start()`; used to compute `started_at`/`duration_ms` on save.
    session_started_at_ms: Arc<Mutex<Option<i64>>>,
    /// Row id of the meeting persisted on the most recent `stop()`. Used by
    /// `summarize_meeting` to update the same row with the generated summary.
    last_saved_meeting_id: Arc<Mutex<Option<i64>>>,
    /// Absolute path to the persisted mixed WAV written on the most recent
    /// `stop()`, if any. Stored on the meeting row via `audio_path`.
    last_saved_audio_path: Arc<Mutex<Option<String>>>,
    /// Raw f32 (little-endian, 16 kHz mono) buffer files the capture loop
    /// streams the FULL per-source audio into for the on-stop finalize pass.
    /// Bounded-memory: written incrementally, read back once on stop. Set by the
    /// capture loop, consumed by `finalize_session`.
    buffer_paths: Arc<Mutex<Option<SessionBuffers>>>,
}

/// Temp-file paths holding the full session audio for the on-stop finalize
/// pass. All paths are raw little-endian f32 at 16 kHz mono.
#[derive(Clone, Debug)]
struct SessionBuffers {
    /// Full microphone ("you") audio.
    mic: std::path::PathBuf,
    /// Full system ("others") audio.
    system: std::path::PathBuf,
    /// Full mixed mono audio (used for the saved playback WAV).
    mixed: std::path::PathBuf,
}

impl MeetingManager {
    pub fn new(app_handle: &AppHandle, transcription_manager: Arc<TranscriptionManager>) -> Self {
        // Resolve the persistence store. If the app data dir cannot be resolved
        // (should not happen in practice), fall back to a store pointing at a
        // best-effort path; save errors are logged, not fatal.
        let store = MeetingStore::new(app_handle).unwrap_or_else(|e| {
            log::error!("Failed to initialize MeetingStore: {}", e);
            MeetingStore::with_db_path(std::path::PathBuf::from("history.db"))
        });
        Self {
            app_handle: app_handle.clone(),
            transcription_manager,
            state: Arc::new(Mutex::new(MeetingState::Idle)),
            transcript: Arc::new(Mutex::new(Vec::new())),
            stop_signal: Arc::new(AtomicBool::new(false)),
            active: Arc::new(AtomicBool::new(false)),
            worker: Arc::new(Mutex::new(None)),
            store,
            session_started_at_ms: Arc::new(Mutex::new(None)),
            last_saved_meeting_id: Arc::new(Mutex::new(None)),
            last_saved_audio_path: Arc::new(Mutex::new(None)),
            buffer_paths: Arc::new(Mutex::new(None)),
        }
    }

    /// Whether a meeting session is currently running. Consulted by the
    /// TranscriptionManager idle-watcher to keep the model loaded.
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }

    pub fn status(&self) -> MeetingState {
        *self.state.lock().unwrap()
    }

    /// Return the full accumulated transcript text. Segments are sorted by their
    /// relative timestamp first (segments now arrive from two independent
    /// per-source VAD pipelines, so insertion order is not chronological) and
    /// joined by spaces.
    pub fn full_transcript(&self) -> String {
        let segs = self.transcript.lock().unwrap();
        let mut ordered: Vec<&TranscriptSegment> = segs.iter().collect();
        ordered.sort_by_key(|s| s.timestamp_ms);
        ordered
            .iter()
            .map(|s| s.text.as_str())
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Start a meeting session. Ensures the transcription model is loaded
    /// (using the same path dictation uses), resets the transcript, and spawns
    /// the capture+mix+VAD+transcribe loop.
    pub fn start(&self) -> Result<(), String> {
        // Serialize against concurrent start/stop.
        let mut state = self.state.lock().unwrap();
        if *state == MeetingState::Running {
            return Err("Meeting already running".to_string());
        }

        #[cfg(not(target_os = "macos"))]
        {
            let _ = &self.transcription_manager;
            return Err("Meeting mode capture is only supported on macOS".to_string());
        }

        #[cfg(target_os = "macos")]
        {
            // Ensure the model is loaded (background load, same path as dictation).
            // The capture loop's transcribe() calls will also block-wait on the
            // loading condvar if needed, so this is a best-effort kickstart.
            self.transcription_manager.initiate_model_load();

            // Reset session state.
            {
                let mut segs = self.transcript.lock().unwrap();
                segs.clear();
            }
            // Record the ABSOLUTE session start time (epoch ms). Segment
            // timestamps remain relative to this.
            *self.session_started_at_ms.lock().unwrap() = Some(now_epoch_ms());
            *self.last_saved_meeting_id.lock().unwrap() = None;
            *self.last_saved_audio_path.lock().unwrap() = None;
            *self.buffer_paths.lock().unwrap() = None;
            self.stop_signal.store(false, Ordering::Relaxed);
            self.active.store(true, Ordering::Relaxed);

            let manager = self.clone();
            let handle = std::thread::spawn(move || {
                if let Err(e) = manager.run_capture_loop() {
                    log::error!("Meeting capture loop ended with error: {}", e);
                }
                manager.active.store(false, Ordering::Relaxed);
            });
            *self.worker.lock().unwrap() = Some(handle);

            *state = MeetingState::Running;
            log::info!("Meeting session started");
            Ok(())
        }
    }

    /// Stop the meeting session, join the capture thread, and return the final
    /// accumulated transcript text.
    pub fn stop(&self) -> Result<String, String> {
        let mut state = self.state.lock().unwrap();
        if *state == MeetingState::Idle {
            // Idempotent: return whatever transcript exists.
            return Ok(self.full_transcript());
        }

        self.stop_signal.store(true, Ordering::Relaxed);

        // Take the worker handle out and join it WITHOUT holding the state lock
        // for the join duration would be ideal, but we hold `state` to serialize
        // start/stop. The join completes promptly because the loop polls the
        // stop signal each iteration.
        let handle = self.worker.lock().unwrap().take();
        if let Some(handle) = handle {
            // Release the state lock during join to avoid blocking status reads.
            drop(state);
            if let Err(e) = handle.join() {
                log::warn!("Failed to join meeting capture thread: {:?}", e);
            }
            state = self.state.lock().unwrap();
        }

        self.active.store(false, Ordering::Relaxed);
        *state = MeetingState::Idle;
        // Release the state lock before persisting (DB I/O shouldn't block
        // status reads).
        drop(state);

        // Hybrid transcription: re-transcribe the FULL per-source audio for a
        // higher-quality, labeled transcript that REPLACES the live preview.
        // macOS-only; on other platforms this is a no-op (no buffers written).
        #[cfg(target_os = "macos")]
        self.finalize_session();

        // Persist the session. Failures are logged, never propagated, so a
        // stop always succeeds and returns the transcript.
        self.persist_session();

        // Save the mixed audio WAV for playback (needs the persisted row id).
        #[cfg(target_os = "macos")]
        self.save_session_audio();

        // Best-effort auto-summarize (behind a setting). Runs after persistence
        // so it can update the saved row. Never blocks/fails the stop.
        self.maybe_auto_summarize();

        log::info!("Meeting session stopped");
        Ok(self.full_transcript())
    }

    /// Build a `MeetingRecordInput` from the accumulated session and save it via
    /// `MeetingStore`. Skips empty meetings (no transcript). Remembers the saved
    /// row id so a later summary can update the same record. Never panics or
    /// returns an error to the caller.
    fn persist_session(&self) {
        let segments: Vec<TranscriptSegment> = { self.transcript.lock().unwrap().clone() };
        let transcript = self.full_transcript();
        if transcript.trim().is_empty() {
            log::debug!("Meeting transcript empty; skipping persistence");
            return;
        }

        let ended_at = now_epoch_ms();
        let started_at = self
            .session_started_at_ms
            .lock()
            .unwrap()
            .unwrap_or(ended_at);
        let duration_ms = (ended_at - started_at).max(0);
        let title = default_meeting_title(started_at);

        let record = MeetingRecordInput {
            started_at,
            ended_at,
            duration_ms,
            title,
            transcript,
            segments,
            summary: None,
            audio_path: None,
        };

        match self.store.save_meeting(&record) {
            Ok(id) => {
                log::info!("Persisted meeting session as row {}", id);
                *self.last_saved_meeting_id.lock().unwrap() = Some(id);
            }
            Err(e) => {
                log::error!("Failed to persist meeting session: {}", e);
            }
        }
    }

    /// Returns the row id of the most recently persisted meeting (set on stop),
    /// or `None` if the last session was empty/unsaved.
    pub fn last_saved_meeting_id(&self) -> Option<i64> {
        *self.last_saved_meeting_id.lock().unwrap()
    }

    /// Update the summary of the persisted meeting row, if one exists.
    pub fn update_saved_summary(&self, summary: &str) -> Result<(), String> {
        let id = match self.last_saved_meeting_id() {
            Some(id) => id,
            None => return Ok(()),
        };
        self.store
            .update_summary(id, summary)
            .map_err(|e| format!("Failed to update meeting summary: {}", e))
    }

    /// Access the persistence store (used by list/get/delete commands).
    pub fn store(&self) -> &MeetingStore {
        &self.store
    }

    /// HYBRID TRANSCRIPTION (Feature 2). On stop, re-transcribe the FULL mic and
    /// system audio buffered to temp files during the session, producing a
    /// higher-quality labeled transcript that REPLACES the live rough preview.
    ///
    /// Rather than one blob per source (which would collapse chronology to a
    /// single t=0 block), each source is split into TIME-ORDERED ~25-30 s
    /// windows that close preferentially at VAD silence boundaries (so words
    /// aren't cut). Each window keeps its real start offset → `timestamp_ms`.
    /// Mic + system windows are merged and sorted by timestamp, yielding an
    /// interleaved, speaker-labeled transcript with near-full-context quality.
    ///
    /// Emits `"meeting-finalizing"` (true) before and (false) after. If no
    /// buffers were captured (e.g. capture failed early), leaves the live
    /// transcript untouched.
    #[cfg(target_os = "macos")]
    fn finalize_session(&self) {
        let buffers = match self.buffer_paths.lock().unwrap().clone() {
            Some(b) => b,
            None => return,
        };

        self.emit_finalizing(true);

        // Swap in the stronger FINAL model (default "turbo") for the duration of
        // the finalize pass, then restore the user's normal selected model. The
        // LIVE path already used whatever model was loaded; we never hold two
        // models resident. If the final model isn't downloaded / fails to load,
        // we fall back gracefully to the currently-loaded model. This is
        // invisible to the user (the "finalizing" spinner is already showing).
        let restore_model = self.swap_in_final_model();

        // Resolve the VAD model once for the silence-boundary chunker.
        let vad_path = {
            use tauri::Manager;
            self.app_handle.path().resolve(
                "resources/models/silero_vad_v4.onnx",
                tauri::path::BaseDirectory::Resource,
            )
        };

        let mut final_segments: Vec<TranscriptSegment> = Vec::new();
        for (path, source) in [
            (&buffers.mic, TranscriptSource::Mic),
            (&buffers.system, TranscriptSource::System),
        ] {
            match read_f32_raw(path) {
                Ok(audio) if !audio.is_empty() => {
                    let windows = match &vad_path {
                        Ok(p) => chunk_for_finalize(&audio, p),
                        Err(e) => {
                            log::warn!(
                                "meeting finalize: VAD path unresolved ({}); using fixed windows",
                                e
                            );
                            chunk_fixed(&audio)
                        }
                    };
                    self.transcribe_windows(&audio, &windows, source, &mut final_segments);
                }
                Ok(_) => {}
                Err(e) => log::warn!("meeting finalize: failed to read {:?}: {}", path, e),
            }
        }

        // Only replace the live transcript if the finalize pass produced
        // something; otherwise keep the live preview as-is.
        let has_text = final_segments.iter().any(|s| !s.text.trim().is_empty());
        if has_text {
            final_segments.sort_by_key(|s| s.timestamp_ms);
            self.replace_transcript(final_segments);
        } else {
            log::warn!(
                "meeting finalize: full re-transcription yielded no text; keeping live transcript"
            );
        }

        // Restore the user's normal model so dictation / subsequent meetings use
        // the expected model again.
        self.restore_model(restore_model);

        self.emit_finalizing(false);
    }

    /// Load the configured `meeting_final_model` (default "turbo") for the
    /// finalize pass, returning the model id that should be restored afterwards
    /// (the model that was loaded before, or the user's `selected_model`).
    /// Returns `None` if no swap happened (already on the final model, or the
    /// final model couldn't be loaded — in which case the loaded model is kept).
    #[cfg(target_os = "macos")]
    fn swap_in_final_model(&self) -> Option<String> {
        let settings = crate::settings::get_settings(&self.app_handle);
        let final_model = settings.meeting_final_model.trim().to_string();
        if final_model.is_empty() {
            return None;
        }

        // What is loaded right now (used by the LIVE pass). Fall back to the
        // user's configured selected_model if nothing is loaded.
        let current = self
            .transcription_manager
            .get_current_model()
            .unwrap_or_else(|| settings.selected_model.clone());

        if current == final_model {
            // Already on the final model; nothing to swap or restore.
            return None;
        }

        match self.transcription_manager.load_model(&final_model) {
            Ok(()) => {
                log::info!(
                    "meeting finalize: swapped model {} -> {} for final pass",
                    current,
                    final_model
                );
                Some(current)
            }
            Err(e) => {
                // Graceful fallback: keep using whatever is loaded (the live
                // model). Don't crash the finalize pass.
                log::warn!(
                    "meeting finalize: could not load final model '{}' ({}); using loaded model",
                    final_model,
                    e
                );
                None
            }
        }
    }

    /// Restore the model recorded by `swap_in_final_model`. No-op when `None`.
    #[cfg(target_os = "macos")]
    fn restore_model(&self, restore: Option<String>) {
        if let Some(model_id) = restore {
            if let Err(e) = self.transcription_manager.load_model(&model_id) {
                log::warn!(
                    "meeting finalize: failed to restore model '{}': {}",
                    model_id,
                    e
                );
            }
        }
    }

    /// Transcribe each `[start, end)` window of `audio` into one timestamped,
    /// labeled segment via the meeting path (`transcribe_meeting`: forced
    /// meeting language + Turkish style prompt + higher no_speech_thold).
    /// ~25-30 s windows give whisper plenty of context per call.
    #[cfg(target_os = "macos")]
    fn transcribe_windows(
        &self,
        audio: &[f32],
        windows: &[(usize, usize)],
        source: TranscriptSource,
        out: &mut Vec<TranscriptSegment>,
    ) {
        use crate::audio_toolkit::constants::WHISPER_SAMPLE_RATE;
        for &(start, end) in windows {
            let slice = &audio[start..end];
            match self.transcription_manager.transcribe_meeting(slice.to_vec()) {
                Ok(text) => {
                    let text = text.trim().to_string();
                    if !text.is_empty() {
                        let timestamp_ms =
                            (start as u64).saturating_mul(1000) / WHISPER_SAMPLE_RATE as u64;
                        out.push(TranscriptSegment {
                            text,
                            timestamp_ms,
                            source,
                        });
                    }
                }
                Err(e) => log::warn!(
                    "meeting finalize: transcription failed for {:?} window [{}..{}]: {}",
                    source,
                    start,
                    end,
                    e
                ),
            }
        }
    }

    /// AUDIO SAVE (Feature 4). Persist the mixed 16 kHz mono audio to
    /// `{app_data_dir}/meetings/{id}.wav` and record its path on the meeting
    /// row. Requires a saved row id (set by `persist_session`). Best-effort:
    /// failures are logged, never propagated.
    #[cfg(target_os = "macos")]
    fn save_session_audio(&self) {
        let id = match self.last_saved_meeting_id() {
            Some(id) => id,
            None => return,
        };
        let buffers = match self.buffer_paths.lock().unwrap().clone() {
            Some(b) => b,
            None => return,
        };
        let mixed = match read_f32_raw(&buffers.mixed) {
            Ok(m) if !m.is_empty() => m,
            Ok(_) => return,
            Err(e) => {
                log::warn!("meeting: failed to read mixed buffer: {}", e);
                return;
            }
        };

        let dir = match crate::portable::app_data_dir(&self.app_handle) {
            Ok(d) => d.join("meetings"),
            Err(e) => {
                log::error!("meeting: cannot resolve app data dir for audio save: {}", e);
                return;
            }
        };
        if let Err(e) = std::fs::create_dir_all(&dir) {
            log::error!("meeting: failed to create meetings dir {:?}: {}", dir, e);
            return;
        }
        let wav_path = dir.join(format!("{}.wav", id));
        use crate::audio_toolkit::constants::WHISPER_SAMPLE_RATE;
        if let Err(e) = write_f32_wav(&wav_path, &mixed, WHISPER_SAMPLE_RATE) {
            log::error!("meeting: failed to write audio WAV {:?}: {}", wav_path, e);
            return;
        }
        let path_str = wav_path.to_string_lossy().to_string();
        if let Err(e) = self.store.update_audio_path(id, &path_str) {
            log::error!("meeting: failed to record audio path: {}", e);
            return;
        }
        *self.last_saved_audio_path.lock().unwrap() = Some(path_str);
        // Clean up the temp buffer files now that everything is persisted.
        let _ = std::fs::remove_file(&buffers.mic);
        let _ = std::fs::remove_file(&buffers.system);
        let _ = std::fs::remove_file(&buffers.mixed);
        log::info!("meeting: saved playback audio to {:?}", wav_path);
    }

    /// AUTO-SUMMARIZE (Feature 3). If `meeting_auto_summarize` is enabled, run
    /// the same summary path as the `summarize_meeting` command and persist +
    /// emit the result. Best-effort: spawned async, never blocks/fails stop.
    fn maybe_auto_summarize(&self) {
        let settings = crate::settings::get_settings(&self.app_handle);
        if !settings.meeting_auto_summarize {
            return;
        }
        let transcript = self.full_transcript();
        if transcript.trim().is_empty() {
            return;
        }
        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            match crate::commands::meeting::summarize_transcript(&manager.app_handle, &transcript)
                .await
            {
                Ok(summary) => {
                    if let Err(e) = manager.update_saved_summary(&summary) {
                        log::error!("meeting auto-summarize: failed to persist: {}", e);
                    }
                    use tauri::Emitter;
                    let _ = manager.app_handle.emit("meeting-summary-update", summary);
                }
                Err(e) => log::error!("meeting auto-summarize failed: {}", e),
            }
        });
    }

    /// Append a transcribed segment and emit an update event.
    fn push_segment(&self, text: String, timestamp_ms: u64, source: TranscriptSource) {
        if text.trim().is_empty() {
            return;
        }
        let segment = TranscriptSegment {
            text,
            timestamp_ms,
            source,
        };
        {
            let mut segs = self.transcript.lock().unwrap();
            segs.push(segment.clone());
        }
        let full = self.full_transcript();
        use tauri::Emitter;
        let _ = self.app_handle.emit(
            "meeting-transcript-update",
            MeetingTranscriptUpdate {
                segment,
                full_transcript: full,
            },
        );
    }

    /// Emit a `"meeting-finalizing"` signal so the UI can show progress while
    /// the on-stop full-audio re-transcription runs.
    fn emit_finalizing(&self, finalizing: bool) {
        use tauri::Emitter;
        let _ = self
            .app_handle
            .emit("meeting-finalizing", MeetingFinalizing { finalizing });
    }

    /// Replace the entire accumulated transcript with `segments` (the FINAL
    /// labeled transcript) and emit a synthetic update so the UI swaps the live
    /// preview for the final result. The emitted `segment` is the last one for
    /// payload-shape compatibility; the authoritative content is
    /// `full_transcript` plus the persisted record.
    fn replace_transcript(&self, segments: Vec<TranscriptSegment>) {
        {
            let mut segs = self.transcript.lock().unwrap();
            *segs = segments;
        }
        let full = self.full_transcript();
        let last = { self.transcript.lock().unwrap().last().cloned() };
        if let Some(segment) = last {
            use tauri::Emitter;
            let _ = self.app_handle.emit(
                "meeting-transcript-update",
                MeetingTranscriptUpdate {
                    segment,
                    full_transcript: full,
                },
            );
        }
    }

    /// Emit a throttled `"meeting-audio-level"` event for the live visualizer.
    /// Cheap: caller passes precomputed bars/wave/peak. Separate from the
    /// dictation `mic-level` path.
    fn emit_audio_level(&self, bars: Vec<f32>, wave: Vec<f32>, peak: f32) {
        use tauri::Emitter;
        let _ = self.app_handle.emit(
            "meeting-audio-level",
            MeetingAudioLevel { bars, wave, peak },
        );
    }

    /// The capture + mix + VAD + transcribe loop. macOS-only.
    ///
    /// Mirrors the capture/mix machinery of `capture_mixed_audio_test`: an
    /// independent cpal mic worker resamples to 16 kHz and forwards frames over a
    /// channel; the Step-1 system-audio stream is resampled to 16 kHz; a
    /// `MeetingMixer` produces mixed 16 kHz mono samples. Those samples are then
    /// fed through a dedicated `SmoothedVad` in 480-sample frames to segment
    /// speech, and each completed segment is transcribed.
    #[cfg(target_os = "macos")]
    fn run_capture_loop(&self) -> Result<(), String> {
        use crate::audio_toolkit::audio::{
            AudioVisualiser, FrameResampler, MeetingMixer, MixSource, SystemAudioCapture,
        };
        use crate::audio_toolkit::constants::WHISPER_SAMPLE_RATE;
        use crate::audio_toolkit::vad::SmoothedVad;
        use crate::audio_toolkit::SileroVad;
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        use futures_util::StreamExt;
        use std::sync::mpsc;
        use std::time::{Duration, Instant};
        use tauri::Manager;

        // --- VAD setup (dedicated instance, NOT shared with dictation) ---
        const VAD_FRAME_SAMPLES: usize = (WHISPER_SAMPLE_RATE as usize * 30) / 1000; // 480 samples / 30 ms

        // ----- Meeting VAD segmentation tuning (WIDER / fewer segments) -----
        // These are intentionally more generous than dictation's SmoothedVad so a
        // speaker's brief pauses don't shatter an utterance into many tiny blocks.
        //
        // Pre-roll captured before speech onset (~450 ms). Same as dictation.
        const VAD_PREFILL_FRAMES: usize = 15;
        // Silence tail tolerated before a segment ends: 40 frames * 30 ms = 1200 ms.
        // (Dictation uses 15 ≈ 450 ms.) Pauses shorter than this stay in one segment.
        const VAD_HANGOVER_FRAMES: usize = 40;
        // Consecutive voice frames required to (re)enter speech. Same as dictation.
        const VAD_ONSET_FRAMES: usize = 2;
        // Max samples per segment before forced flush (~22 s) so a continuous
        // talker still gets periodic transcription.
        const MAX_SEGMENT_SAMPLES: usize = WHISPER_SAMPLE_RATE as usize * 22;
        // Minimum segment length worth transcribing (~400 ms) to skip blips. Raised
        // from ~200 ms so very short noise bursts don't become standalone segments.
        const MIN_SEGMENT_SAMPLES: usize = (WHISPER_SAMPLE_RATE as usize * 2) / 5;
        // If a new segment starts within this gap of the previous one's END, merge
        // them into a single transcript block instead of pushing separately
        // (~800 ms). Belt-and-suspenders on top of the longer hangover, and also
        // bridges the gap created by MAX_SEGMENT_SAMPLES forced flushes.
        const SEGMENT_MERGE_GAP_SAMPLES: u64 = (WHISPER_SAMPLE_RATE as u64 * 4) / 5;

        // --- Live audio-level visualizer (SEPARATE from dictation mic-level) ---
        // Reuse the same AudioVisualiser config dictation uses for `bars`.
        const VIS_BUCKETS: usize = 16;
        const VIS_WINDOW_SIZE: usize = 512;
        // Oscilloscope trace length sent to the frontend.
        const WAVE_POINTS: usize = 96;
        // Throttle: emit at most one event every 50 ms (~20 fps), accumulating
        // mixed samples across the faster 30 ms VAD frames.
        const LEVEL_EMIT_INTERVAL: Duration = Duration::from_millis(50);
        let mut visualizer = AudioVisualiser::new(
            WHISPER_SAMPLE_RATE,
            VIS_WINDOW_SIZE,
            VIS_BUCKETS,
            80.0,
            6000.0,
        );
        // Most recent mixed samples awaiting the next emit (raw, for the wave +
        // peak). Capped to roughly one emit window so it stays cheap.
        let mut level_accum: Vec<f32> = Vec::with_capacity(WHISPER_SAMPLE_RATE as usize / 10);
        // Latest bar levels from the visualiser; reused if no new bars this tick.
        let mut last_bars: Vec<f32> = vec![0.0; VIS_BUCKETS];
        let mut last_level_emit = Instant::now();

        let vad_path = self
            .app_handle
            .path()
            .resolve(
                "resources/models/silero_vad_v4.onnx",
                tauri::path::BaseDirectory::Resource,
            )
            .map_err(|e| format!("Failed to resolve VAD path: {}", e))?;

        // Build a fresh SmoothedVad tuned for WIDER meeting segments. We build
        // TWO independent instances (mic + system) so each source is segmented
        // and labeled separately (Feature 1). NOT shared with dictation.
        let make_vad = || -> Result<SmoothedVad, String> {
            let silero = SileroVad::new(&vad_path, 0.3)
                .map_err(|e| format!("Failed to create SileroVad for meeting: {}", e))?;
            Ok(SmoothedVad::new(
                Box::new(silero),
                VAD_PREFILL_FRAMES,
                VAD_HANGOVER_FRAMES,
                VAD_ONSET_FRAMES,
            ))
        };
        let mic_vad = make_vad()?;
        let system_vad = make_vad()?;

        // --- Per-source full-audio buffer files (Feature 2: hybrid finalize) ---
        // Stream each source's full 16 kHz mono audio (raw little-endian f32) to
        // a temp file so a 2h meeting doesn't hold ~230 MB/source in RAM. Read
        // back once on stop for the high-quality finalize pass.
        let buf_dir = std::env::temp_dir();
        let pid = std::process::id();
        let stamp = now_epoch_ms();
        let mic_buf_path = buf_dir.join(format!("handy_meeting_{}_{}_mic.f32", pid, stamp));
        let system_buf_path = buf_dir.join(format!("handy_meeting_{}_{}_sys.f32", pid, stamp));
        let mixed_buf_path = buf_dir.join(format!("handy_meeting_{}_{}_mix.f32", pid, stamp));
        let mut mic_buf_writer = RawF32Writer::create(&mic_buf_path)
            .map_err(|e| format!("Failed to create mic buffer: {}", e))?;
        let mut system_buf_writer = RawF32Writer::create(&system_buf_path)
            .map_err(|e| format!("Failed to create system buffer: {}", e))?;
        let mut mixed_buf_writer = RawF32Writer::create(&mixed_buf_path)
            .map_err(|e| format!("Failed to create mixed buffer: {}", e))?;
        // Publish the buffer paths so stop()'s finalize/audio-save can read them.
        *self.buffer_paths.lock().unwrap() = Some(SessionBuffers {
            mic: mic_buf_path.clone(),
            system: system_buf_path.clone(),
            mixed: mixed_buf_path.clone(),
        });

        // Per-source live segmentation state (mirrors the old single-VAD state).
        let mut mic_proc = SourceProcessor::new(mic_vad, TranscriptSource::Mic);
        let mut system_proc = SourceProcessor::new(system_vad, TranscriptSource::System);

        // --- Mic capture on a dedicated thread (independent cpal stream) ---
        let (mic_tx, mic_rx) = mpsc::channel::<Vec<f32>>();
        let mic_stop = Arc::new(AtomicBool::new(false));
        let mic_stop_worker = mic_stop.clone();
        let (mic_init_tx, mic_init_rx) = mpsc::sync_channel::<Result<(), String>>(1);

        let mic_handle = std::thread::spawn(move || {
            let host = crate::audio_toolkit::get_cpal_host();
            let device = match host.default_input_device() {
                Some(d) => d,
                None => {
                    let _ = mic_init_tx.send(Err("No default input device".into()));
                    return;
                }
            };
            let config = match device.default_input_config() {
                Ok(c) => c,
                Err(e) => {
                    let _ = mic_init_tx.send(Err(format!("No default input config: {e}")));
                    return;
                }
            };
            let in_rate = config.sample_rate().0;
            let channels = config.channels() as usize;
            let sample_format = config.sample_format();
            log::info!(
                "meeting: mic '{:?}' {} Hz {} ch {:?}",
                device.name(),
                in_rate,
                channels,
                sample_format
            );

            let (raw_tx, raw_rx) = mpsc::channel::<Vec<f32>>();

            macro_rules! build {
                ($t:ty) => {{
                    let raw_tx = raw_tx.clone();
                    device.build_input_stream(
                        &config.clone().into(),
                        move |data: &[$t], _: &cpal::InputCallbackInfo| {
                            let mono: Vec<f32> = if channels <= 1 {
                                data.iter()
                                    .map(|&s| cpal::Sample::to_sample::<f32>(s))
                                    .collect()
                            } else {
                                data.chunks_exact(channels)
                                    .map(|f| {
                                        f.iter()
                                            .map(|&s| cpal::Sample::to_sample::<f32>(s))
                                            .sum::<f32>()
                                            / channels as f32
                                    })
                                    .collect()
                            };
                            let _ = raw_tx.send(mono);
                        },
                        |err| log::error!("meeting mic stream error: {err}"),
                        None,
                    )
                }};
            }

            let stream = match sample_format {
                cpal::SampleFormat::F32 => build!(f32),
                cpal::SampleFormat::I16 => build!(i16),
                cpal::SampleFormat::I32 => build!(i32),
                cpal::SampleFormat::U8 => build!(u8),
                other => {
                    let _ = mic_init_tx.send(Err(format!("Unsupported mic format: {other:?}")));
                    return;
                }
            };
            let stream = match stream {
                Ok(s) => s,
                Err(e) => {
                    let _ = mic_init_tx.send(Err(format!("Failed to build mic stream: {e}")));
                    return;
                }
            };
            if let Err(e) = stream.play() {
                let _ = mic_init_tx.send(Err(format!("Failed to start mic stream: {e}")));
                return;
            }
            let _ = mic_init_tx.send(Ok(()));

            let mut resampler = FrameResampler::new(
                in_rate as usize,
                WHISPER_SAMPLE_RATE as usize,
                Duration::from_millis(30),
            );

            loop {
                if mic_stop_worker.load(Ordering::Relaxed) {
                    break;
                }
                match raw_rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(raw) => {
                        resampler.push(&raw, &mut |frame: &[f32]| {
                            let _ = mic_tx.send(frame.to_vec());
                        });
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
            resampler.finish(&mut |frame: &[f32]| {
                let _ = mic_tx.send(frame.to_vec());
            });
            drop(stream);
        });

        // Wait for mic init (or fail).
        match mic_init_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                mic_stop.store(true, Ordering::Relaxed);
                let _ = mic_handle.join();
                return Err(format!("Mic init failed: {e}"));
            }
            Err(e) => {
                mic_stop.store(true, Ordering::Relaxed);
                let _ = mic_handle.join();
                return Err(format!("Mic worker died: {e}"));
            }
        }

        // --- System audio capture (Step 1) on Tauri's async runtime ---
        // The CoreAudio stream is a `Stream<Item = f32>`; drain it on a spawned
        // async task and forward batched samples to this (sync) loop over a
        // std mpsc channel, mirroring the mic worker. This avoids a per-sample
        // `block_on` and keeps the capture loop purely synchronous.
        let sys_stream = match SystemAudioCapture::start() {
            Ok(s) => s,
            Err(e) => {
                mic_stop.store(true, Ordering::Relaxed);
                let _ = mic_handle.join();
                return Err(format!("Failed to start system capture: {e}"));
            }
        };
        let sys_rate = sys_stream.sample_rate();
        // Live system sample-rate handle (Item 4): the CoreAudio IO-proc updates
        // this when the device rate changes (e.g. AirPods switching profiles).
        // We poll it in the loop and rebuild `sys_resampler` on change so the
        // ratio stays correct (else pitch/speed corruption).
        let sys_rate_handle = sys_stream.sample_rate_handle();
        let mut sys_resampler = FrameResampler::new(
            sys_rate as usize,
            WHISPER_SAMPLE_RATE as usize,
            Duration::from_millis(30),
        );
        let mut sys_resampler_rate = sys_rate;

        let (sys_tx, sys_rx) = mpsc::channel::<Vec<f32>>();
        let sys_stop = Arc::new(AtomicBool::new(false));
        let sys_stop_task = sys_stop.clone();
        let sys_task = tauri::async_runtime::spawn(async move {
            let mut stream = sys_stream;
            let mut batch: Vec<f32> = Vec::with_capacity(1024);
            loop {
                if sys_stop_task.load(Ordering::Relaxed) {
                    break;
                }
                match stream.next().await {
                    Some(s) => {
                        batch.push(s);
                        if batch.len() >= 1024 {
                            if sys_tx.send(std::mem::take(&mut batch)).is_err() {
                                break;
                            }
                        }
                    }
                    None => break,
                }
            }
            if !batch.is_empty() {
                let _ = sys_tx.send(batch);
            }
            drop(stream);
        });

        log::info!(
            "meeting: capture loop running at {} Hz (system in {} Hz)",
            WHISPER_SAMPLE_RATE,
            sys_rate
        );

        // --- Mixer (kept ONLY for the level meter + saved playback WAV) ---
        let mut mixer = MeetingMixer::new();
        let mut mixed: Vec<f32> = Vec::new();
        // Per-source 16 kHz frames pulled this tick, fed to each VAD + buffer.
        let mut mic_frames: Vec<f32> = Vec::new();
        let mut sys_frames: Vec<f32> = Vec::new();

        // Segmentation tuning shared by both source processors.
        let seg_cfg = SegConfig {
            frame_samples: VAD_FRAME_SAMPLES,
            min_samples: MIN_SEGMENT_SAMPLES,
            max_samples: MAX_SEGMENT_SAMPLES,
            merge_gap_samples: SEGMENT_MERGE_GAP_SAMPLES,
        };

        // --- Mic conditioning + echo mitigation (Items 3 & 5), mic frames only ---
        // Echo duck: detect the output route once at start (built-in speakers vs
        // headphones). A mid-meeting headphone plug/unplug isn't re-detected here
        // to keep the hot loop allocation-free; the common case (fixed route for
        // the session) is handled.
        let mut echo_duck = EchoDuck::new(crate::audio_toolkit::audio::detect_output_route());
        let mut mic_highpass = HighPass::new(MIC_HIGHPASS_HZ, WHISPER_SAMPLE_RATE as f32);
        let mut mic_norm = MicLoudnessNorm::new(WHISPER_SAMPLE_RATE);

        loop {
            if self.stop_signal.load(Ordering::Relaxed) {
                break;
            }

            mic_frames.clear();
            sys_frames.clear();

            // Item 4: rebuild the system resampler if the device rate changed
            // (AirPods/Bluetooth profile switch). Without this the fixed initial
            // ratio would pitch/speed-corrupt all subsequent system audio.
            let live_sys_rate = sys_rate_handle.load(Ordering::Acquire);
            if live_sys_rate != 0 && live_sys_rate != sys_resampler_rate {
                log::info!(
                    "meeting: system sample rate changed {} -> {} Hz; rebuilding resampler",
                    sys_resampler_rate,
                    live_sys_rate
                );
                // Flush any tail of the old resampler into the mixer so samples
                // aren't lost across the rebuild.
                sys_resampler.finish(&mut |frame: &[f32]| {
                    mixer.push(MixSource::System, frame);
                    sys_frames.extend_from_slice(frame);
                });
                sys_resampler = FrameResampler::new(
                    live_sys_rate as usize,
                    WHISPER_SAMPLE_RATE as usize,
                    Duration::from_millis(30),
                );
                sys_resampler_rate = live_sys_rate;
            }

            // Drain any available mic frames (already 16 kHz mono, non-blocking).
            while let Ok(frame) = mic_rx.try_recv() {
                mixer.push(MixSource::Microphone, &frame);
                mic_frames.extend_from_slice(&frame);
            }

            // Pull a system-audio batch (blocks up to 100 ms to pace the loop).
            match sys_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(batch) => {
                    sys_resampler.push(&batch, &mut |frame: &[f32]| {
                        mixer.push(MixSource::System, frame);
                        sys_frames.extend_from_slice(frame);
                    });
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // No system audio yet; keep looping so mic/stop are serviced.
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }

            // --- Mic conditioning + echo mitigation (Items 3 & 5) ---
            // Update the smoothed system loudness every tick (so the duck
            // releases promptly when system audio stops), then condition the
            // mic. Order: echo duck -> high-pass -> loudness normalize. Applied
            // to mic_frames BEFORE the buffer write + VAD feed, so BOTH the live
            // pass and the on-stop finalize benefit. System audio + the saved
            // playback mix (already pushed with raw mic above) are untouched.
            echo_duck.observe_system(&sys_frames);
            if !mic_frames.is_empty() {
                let ducked = echo_duck.apply(&mut mic_frames);
                mic_highpass.process(&mut mic_frames);
                if let Some(norm) = mic_norm.as_mut() {
                    norm.process(&mut mic_frames);
                }
                let _ = mic_buf_writer.write(&mic_frames);
                // While the mic is ducked (remote audio loud on speakers), skip
                // feeding the mic VAD: the attenuated leakage shouldn't open a
                // mic segment that duplicates what the system tap already has.
                // Double-talk tradeoff: a quiet local interjection over loud
                // playback may be missed — accepted to avoid duplicated text.
                if !ducked {
                    mic_proc.feed(&mic_frames, &seg_cfg, self);
                }
            }
            if !sys_frames.is_empty() {
                let _ = system_buf_writer.write(&sys_frames);
                system_proc.feed(&sys_frames, &seg_cfg, self);
            }

            // --- Mixed stream: level meter + playback buffer ONLY ---
            mixer.drain_into(&mut mixed);
            if !mixed.is_empty() {
                let _ = mixed_buf_writer.write(&mixed);
                level_accum.extend_from_slice(&mixed);
                if let Some(bars) = visualizer.feed(&mixed) {
                    last_bars = bars;
                }
                if last_level_emit.elapsed() >= LEVEL_EMIT_INTERVAL {
                    let (wave, peak) = downsample_wave(&level_accum, WAVE_POINTS);
                    self.emit_audio_level(last_bars.clone(), wave, peak);
                    level_accum.clear();
                    last_level_emit = Instant::now();
                }
                mixed.clear();
            }
        }

        // --- Teardown: stop mic + system, flush resamplers + mixer + final ---
        mic_stop.store(true, Ordering::Relaxed);
        let _ = mic_handle.join();
        sys_stop.store(true, Ordering::Relaxed);
        // The async task tears down the CoreAudio tap on drop; wait for it.
        let _ = tauri::async_runtime::block_on(sys_task);

        // Drain trailing mic frames.
        let mut tail_mic: Vec<f32> = Vec::new();
        while let Ok(frame) = mic_rx.try_recv() {
            mixer.push(MixSource::Microphone, &frame);
            tail_mic.extend_from_slice(&frame);
        }
        if !tail_mic.is_empty() {
            // Apply the same mic conditioning to the trailing tail (no ducking
            // here — the session is ending and there's no fresh system RMS).
            mic_highpass.process(&mut tail_mic);
            if let Some(norm) = mic_norm.as_mut() {
                norm.process(&mut tail_mic);
            }
            let _ = mic_buf_writer.write(&tail_mic);
            mic_proc.feed(&tail_mic, &seg_cfg, self);
        }

        // Drain + flush trailing system audio.
        let mut tail_sys: Vec<f32> = Vec::new();
        while let Ok(batch) = sys_rx.try_recv() {
            sys_resampler.push(&batch, &mut |frame: &[f32]| {
                mixer.push(MixSource::System, frame);
                tail_sys.extend_from_slice(frame);
            });
        }
        sys_resampler.finish(&mut |frame: &[f32]| {
            mixer.push(MixSource::System, frame);
            tail_sys.extend_from_slice(frame);
        });
        if !tail_sys.is_empty() {
            let _ = system_buf_writer.write(&tail_sys);
            system_proc.feed(&tail_sys, &seg_cfg, self);
        }

        // Flush any remaining mixed audio to the playback buffer.
        mixer.flush_into(&mut mixed);
        if !mixed.is_empty() {
            let _ = mixed_buf_writer.write(&mixed);
        }

        // Flush each source's final in-progress + pending segments.
        mic_proc.finish(&seg_cfg, self);
        system_proc.finish(&seg_cfg, self);

        // Ensure buffers are fully written to disk before stop() reads them.
        let _ = mic_buf_writer.flush();
        let _ = system_buf_writer.flush();
        let _ = mixed_buf_writer.flush();

        // Emit one final flat level so the visualiser settles to a flat line.
        self.emit_audio_level(vec![0.0; VIS_BUCKETS], vec![0.0; WAVE_POINTS], 0.0);

        Ok(())
    }

    /// Transcribe a completed live segment and append the result with its source
    /// label. Clears `segment`. This is the LIVE rough pass; the on-stop
    /// finalize replaces these with full-audio re-transcription.
    #[cfg(target_os = "macos")]
    fn flush_segment(&self, segment: &mut Vec<f32>, start_samples: u64, source: TranscriptSource) {
        use crate::audio_toolkit::constants::WHISPER_SAMPLE_RATE;

        if segment.is_empty() {
            return;
        }
        let audio = std::mem::take(segment);
        let timestamp_ms = start_samples.saturating_mul(1000) / WHISPER_SAMPLE_RATE as u64;

        match self.transcription_manager.transcribe_meeting(audio) {
            Ok(text) => {
                if !text.trim().is_empty() {
                    log::info!(
                        "meeting segment [{:?}] @ {}ms: {}",
                        source,
                        timestamp_ms,
                        text
                    );
                    self.push_segment(text, timestamp_ms, source);
                }
            }
            Err(e) => {
                log::warn!("meeting segment transcription failed: {}", e);
            }
        }
    }
}

/// Segmentation tuning shared by both per-source processors.
#[cfg(target_os = "macos")]
struct SegConfig {
    frame_samples: usize,
    min_samples: usize,
    max_samples: usize,
    merge_gap_samples: u64,
}

/// Owns the live VAD segmentation state for a SINGLE capture source (mic or
/// system). Each source has its own `SmoothedVad`, segment buffer, and pending
/// merge state, producing source-labeled segments (Feature 1). The previous
/// implementation ran one VAD on the mix; this splits it in two.
#[cfg(target_os = "macos")]
struct SourceProcessor {
    vad: crate::audio_toolkit::vad::SmoothedVad,
    source: TranscriptSource,
    /// Carry-over of samples not yet aligned to a VAD frame.
    frame_accum: Vec<f32>,
    /// Current in-progress speech segment.
    segment: Vec<f32>,
    /// Total samples seen for this source (relative timestamps).
    total_samples: u64,
    /// Sample index where the current segment began.
    segment_start: u64,
    /// Already-finished segment held back for possible merge with the next.
    pending: Option<(Vec<f32>, u64)>,
    /// Sample index where the pending segment's audio ended.
    pending_end: u64,
}

#[cfg(target_os = "macos")]
impl SourceProcessor {
    fn new(vad: crate::audio_toolkit::vad::SmoothedVad, source: TranscriptSource) -> Self {
        Self {
            vad,
            source,
            frame_accum: Vec::new(),
            segment: Vec::new(),
            total_samples: 0,
            segment_start: 0,
            pending: None,
            pending_end: 0,
        }
    }

    /// Feed newly-captured 16 kHz mono samples for this source, segmenting them
    /// and transcribing completed segments (live rough pass) via `manager`.
    fn feed(&mut self, samples: &[f32], cfg: &SegConfig, manager: &MeetingManager) {
        use crate::audio_toolkit::vad::{VadFrame, VoiceActivityDetector};

        self.frame_accum.extend_from_slice(samples);
        while self.frame_accum.len() >= cfg.frame_samples {
            let frame: Vec<f32> = self.frame_accum.drain(0..cfg.frame_samples).collect();
            self.total_samples += cfg.frame_samples as u64;

            match self.vad.push_frame(&frame) {
                Ok(VadFrame::Speech(speech)) => {
                    if self.segment.is_empty() {
                        let prelen = speech.len() as u64;
                        self.segment_start = self.total_samples.saturating_sub(prelen);
                    }
                    self.segment.extend_from_slice(speech);
                    if self.segment.len() >= cfg.max_samples {
                        self.finish_current(cfg, manager);
                    }
                }
                Ok(VadFrame::Noise) => {
                    if !self.segment.is_empty() {
                        if self.segment.len() >= cfg.min_samples {
                            self.finish_current(cfg, manager);
                        } else {
                            self.segment.clear();
                        }
                    }
                }
                Err(e) => log::warn!("meeting VAD frame error [{:?}]: {}", self.source, e),
            }
        }
    }

    /// Finish the in-progress segment: merge into `pending` when close, else
    /// flush the previous pending and make this the new pending. Mirrors the
    /// previous free `finish_segment` function, scoped to one source.
    fn finish_current(&mut self, cfg: &SegConfig, manager: &MeetingManager) {
        if self.segment.is_empty() {
            return;
        }
        let audio = std::mem::take(&mut self.segment);
        let segment_start = self.segment_start;
        let segment_end = self.total_samples;

        match self.pending.take() {
            Some((mut prev_audio, prev_start)) => {
                let gap = segment_start.saturating_sub(self.pending_end);
                if gap <= cfg.merge_gap_samples {
                    prev_audio.extend(std::iter::repeat(0.0).take(gap as usize));
                    prev_audio.extend_from_slice(&audio);
                    self.pending = Some((prev_audio, prev_start));
                    self.pending_end = segment_end;
                } else {
                    manager.flush_segment(&mut prev_audio, prev_start, self.source);
                    self.pending = Some((audio, segment_start));
                    self.pending_end = segment_end;
                }
            }
            None => {
                self.pending = Some((audio, segment_start));
                self.pending_end = segment_end;
            }
        }
    }

    /// Flush any trailing in-progress + pending segment on teardown.
    fn finish(&mut self, cfg: &SegConfig, manager: &MeetingManager) {
        if !self.segment.is_empty() {
            self.finish_current(cfg, manager);
        }
        if let Some((mut audio, start)) = self.pending.take() {
            manager.flush_segment(&mut audio, start, self.source);
        }
    }
}

// ---- Mic conditioning + echo mitigation (Items 3 & 5) ----------------------
//
// These run on the MIC frames ONLY (16 kHz mono), in this order each tick:
//   1. Echo duck  (Item 3): when output = speakers AND system audio is loud,
//      attenuate the mic so the remote party (already cleanly captured by the
//      system tap) isn't re-captured + duplicated in the transcript.
//   2. High-pass  (Item 5): ~80 Hz one-pole HPF to remove rumble/DC before
//      loudness measurement.
//   3. Loudness   (Item 5): EBU R128 shortterm normalization toward -23 LUFS.
// System audio and the saved mix are NOT touched by any of these.

/// Mic attenuation applied while ducking (echo-prone speaker output + loud
/// system audio). -15 dB ≈ ×0.178 linear. Chosen to strongly suppress leakage
/// without fully gating, so a person talking OVER the remote audio (double-talk)
/// is still partially captured rather than dropped entirely.
#[cfg(target_os = "macos")]
const ECHO_DUCK_GAIN_DB: f32 = -15.0;
/// System-audio running-RMS threshold above which we consider remote audio to
/// be "actively playing" and enable ducking. ~0.02 RMS on the 16 kHz system
/// stream — above ambient tap noise/silence, below normal speech level.
#[cfg(target_os = "macos")]
const ECHO_DUCK_SYS_RMS_THRESHOLD: f32 = 0.02;
/// Smoothing factor for the system running RMS (per mic-frame batch). Closer to
/// 1.0 = slower/steadier; 0.2 reacts within ~5 ticks (~0.5 s).
#[cfg(target_os = "macos")]
const ECHO_DUCK_RMS_SMOOTH: f32 = 0.2;
/// Target integrated loudness for mic normalization (EBU R128). -23 LUFS is the
/// EBU broadcast reference; Whisper was trained on roughly this level of speech.
#[cfg(target_os = "macos")]
const MIC_TARGET_LUFS: f64 = -23.0;
/// Clamp the normalization gain so a near-silent block isn't amplified into
/// noise (and a hot block isn't over-attenuated). ±12 dB.
#[cfg(target_os = "macos")]
const MIC_NORM_MAX_GAIN_DB: f64 = 12.0;
/// High-pass cutoff applied to the mic before normalization (Hz).
#[cfg(target_os = "macos")]
const MIC_HIGHPASS_HZ: f32 = 80.0;

/// One-pole high-pass filter (DC/rumble removal). Stateful across frames.
#[cfg(target_os = "macos")]
struct HighPass {
    alpha: f32,
    prev_in: f32,
    prev_out: f32,
}

#[cfg(target_os = "macos")]
impl HighPass {
    fn new(cutoff_hz: f32, sample_rate: f32) -> Self {
        // Standard one-pole HPF coefficient.
        let rc = 1.0 / (2.0 * std::f32::consts::PI * cutoff_hz);
        let dt = 1.0 / sample_rate;
        let alpha = rc / (rc + dt);
        Self {
            alpha,
            prev_in: 0.0,
            prev_out: 0.0,
        }
    }

    fn process(&mut self, samples: &mut [f32]) {
        for s in samples.iter_mut() {
            let x = *s;
            let y = self.alpha * (self.prev_out + x - self.prev_in);
            self.prev_in = x;
            self.prev_out = y;
            *s = y;
        }
    }
}

/// EBU R128 shortterm loudness normalization toward `MIC_TARGET_LUFS`. We feed
/// every mic frame into the meter, read the shortterm (3 s) loudness, and apply
/// a clamped gain. Using shortterm keeps it adaptive to a moving talker without
/// pumping on every sample.
#[cfg(target_os = "macos")]
struct MicLoudnessNorm {
    meter: ebur128::EbuR128,
}

#[cfg(target_os = "macos")]
impl MicLoudnessNorm {
    fn new(sample_rate: u32) -> Option<Self> {
        match ebur128::EbuR128::new(1, sample_rate, ebur128::Mode::S) {
            Ok(meter) => Some(Self { meter }),
            Err(e) => {
                log::warn!("meeting: failed to init EBU R128 meter: {}; mic norm disabled", e);
                None
            }
        }
    }

    /// Feed + normalize a block of mic samples in place.
    fn process(&mut self, samples: &mut [f32]) {
        if samples.is_empty() {
            return;
        }
        if self.meter.add_frames_f32(samples).is_err() {
            return;
        }
        // shortterm loudness needs ~3 s of audio; returns -inf / error early on.
        let loudness = match self.meter.loudness_shortterm() {
            Ok(l) if l.is_finite() => l,
            _ => return,
        };
        // Gain (dB) to reach target, clamped, then linearized.
        let gain_db = (MIC_TARGET_LUFS - loudness).clamp(-MIC_NORM_MAX_GAIN_DB, MIC_NORM_MAX_GAIN_DB);
        let gain = 10f64.powf(gain_db / 20.0) as f32;
        for s in samples.iter_mut() {
            *s = (*s * gain).clamp(-1.0, 1.0);
        }
    }
}

/// Echo / double-capture mitigation. On speaker output, the mic re-captures the
/// remote party (already captured by the system tap) → duplicated transcript.
/// We track a smoothed RMS of the SYSTEM frames; when output = speakers and the
/// system is loud, we attenuate the mic by `ECHO_DUCK_GAIN_DB`.
///
/// Double-talk tradeoff: when both the local user and remote audio are loud at
/// once, the local user's mic is also attenuated (~15 dB), so very quiet local
/// interjections over loud playback may be missed. This is the accepted cost of
/// preventing the (worse) duplicated/echoed transcript. Headphone output is
/// detected separately and skips ducking entirely.
#[cfg(target_os = "macos")]
struct EchoDuck {
    /// Whether the current output route is echo-prone (speakers/unknown).
    enabled: bool,
    /// Smoothed system RMS.
    sys_rms: f32,
    duck_gain: f32,
}

#[cfg(target_os = "macos")]
impl EchoDuck {
    fn new(route: crate::audio_toolkit::audio::OutputRoute) -> Self {
        use crate::audio_toolkit::audio::OutputRoute;
        let enabled = !matches!(route, OutputRoute::Headphones);
        log::info!(
            "meeting: echo duck {} (output route: {:?})",
            if enabled { "ENABLED" } else { "disabled (headphones)" },
            route
        );
        Self {
            enabled,
            sys_rms: 0.0,
            duck_gain: 10f32.powf(ECHO_DUCK_GAIN_DB / 20.0),
        }
    }

    /// Update the smoothed system RMS from this tick's system frames.
    fn observe_system(&mut self, sys_frames: &[f32]) {
        if sys_frames.is_empty() {
            // Decay toward zero so a gap in system audio releases the duck.
            self.sys_rms *= 1.0 - ECHO_DUCK_RMS_SMOOTH;
            return;
        }
        let sum_sq: f32 = sys_frames.iter().map(|s| s * s).sum();
        let rms = (sum_sq / sys_frames.len() as f32).sqrt();
        self.sys_rms = ECHO_DUCK_RMS_SMOOTH * rms + (1.0 - ECHO_DUCK_RMS_SMOOTH) * self.sys_rms;
    }

    /// Whether the mic should currently be ducked.
    fn is_ducking(&self) -> bool {
        self.enabled && self.sys_rms > ECHO_DUCK_SYS_RMS_THRESHOLD
    }

    /// Attenuate mic samples in place if ducking is active. Returns whether the
    /// mic was ducked this tick (caller may skip feeding the mic VAD).
    fn apply(&self, mic_frames: &mut [f32]) -> bool {
        if self.is_ducking() {
            for s in mic_frames.iter_mut() {
                *s *= self.duck_gain;
            }
            true
        } else {
            false
        }
    }
}

/// Incrementally append raw little-endian f32 samples to a file (bounded
/// memory: full session audio lives on disk, not in RAM).
#[cfg(target_os = "macos")]
struct RawF32Writer {
    inner: std::io::BufWriter<std::fs::File>,
}

#[cfg(target_os = "macos")]
impl RawF32Writer {
    fn create(path: &std::path::Path) -> std::io::Result<Self> {
        Ok(Self {
            inner: std::io::BufWriter::new(std::fs::File::create(path)?),
        })
    }

    fn write(&mut self, samples: &[f32]) -> std::io::Result<()> {
        use std::io::Write;
        // f32 is plain-old-data; write the little-endian byte view directly.
        let mut buf = Vec::with_capacity(samples.len() * 4);
        for &s in samples {
            buf.extend_from_slice(&s.to_le_bytes());
        }
        self.inner.write_all(&buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        use std::io::Write;
        self.inner.flush()
    }
}

/// Read a raw little-endian f32 buffer file back into a Vec<f32>.
#[cfg(target_os = "macos")]
fn read_f32_raw(path: &std::path::Path) -> std::io::Result<Vec<f32>> {
    let bytes = std::fs::read(path)?;
    let mut out = Vec::with_capacity(bytes.len() / 4);
    for chunk in bytes.chunks_exact(4) {
        out.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }
    Ok(out)
}

/// Write mono f32 samples as a 32-bit float PCM WAV (format tag 3). Mirrors the
/// helper in `commands::audio` so meeting playback audio is in the same format.
#[cfg(target_os = "macos")]
fn write_f32_wav(path: &std::path::Path, samples: &[f32], sample_rate: u32) -> std::io::Result<()> {
    use std::io::Write;

    let channels: u16 = 1;
    let bits_per_sample: u16 = 32;
    let block_align: u16 = channels * (bits_per_sample / 8);
    let byte_rate: u32 = sample_rate * block_align as u32;
    let data_bytes: u32 = (samples.len() * std::mem::size_of::<f32>()) as u32;
    let riff_chunk_size: u32 = 36 + data_bytes;

    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    f.write_all(b"RIFF")?;
    f.write_all(&riff_chunk_size.to_le_bytes())?;
    f.write_all(b"WAVE")?;
    f.write_all(b"fmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&3u16.to_le_bytes())?; // IEEE float
    f.write_all(&channels.to_le_bytes())?;
    f.write_all(&sample_rate.to_le_bytes())?;
    f.write_all(&byte_rate.to_le_bytes())?;
    f.write_all(&block_align.to_le_bytes())?;
    f.write_all(&bits_per_sample.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&data_bytes.to_le_bytes())?;
    for &s in samples {
        f.write_all(&s.to_le_bytes())?;
    }
    f.flush()?;
    Ok(())
}

// ---- Finalize-pass windowing (Feature 2 fix: preserve chronology + labels) --
//
// Target window length the chunker aims for before it starts looking for a
// silence boundary to close on (~25 s).
#[cfg(target_os = "macos")]
const FINALIZE_TARGET_SAMPLES: usize =
    crate::audio_toolkit::constants::WHISPER_SAMPLE_RATE as usize * 25;
// Hard cap: force-close a window here even mid-speech (~30 s, whisper's native
// window) so a continuous talker can't produce an unbounded window.
#[cfg(target_os = "macos")]
const FINALIZE_MAX_SAMPLES: usize =
    crate::audio_toolkit::constants::WHISPER_SAMPLE_RATE as usize * 30;
// 30 ms frame for the VAD silence scan.
#[cfg(target_os = "macos")]
const FINALIZE_FRAME_SAMPLES: usize =
    (crate::audio_toolkit::constants::WHISPER_SAMPLE_RATE as usize * 30) / 1000;
// Consecutive silent frames that mark a "safe" split point once past target
// (~150 ms). Long enough to be an inter-word/sentence gap, not a glottal stop.
#[cfg(target_os = "macos")]
const FINALIZE_SILENCE_SPLIT_FRAMES: usize = 5;

/// Split `audio` into time-ordered `[start, end)` windows for the finalize
/// re-transcription. Uses the raw SileroVad to classify 30 ms frames as
/// voice/silence and closes a window once it is past `FINALIZE_TARGET_SAMPLES`
/// AND a run of `FINALIZE_SILENCE_SPLIT_FRAMES` silent frames is seen (so we
/// split at a natural pause), or unconditionally at `FINALIZE_MAX_SAMPLES`.
/// Windows containing no speech at all are dropped. Falls back to fixed-size
/// windows if the VAD can't be constructed.
#[cfg(target_os = "macos")]
fn chunk_for_finalize(audio: &[f32], vad_path: &std::path::Path) -> Vec<(usize, usize)> {
    use crate::audio_toolkit::vad::VoiceActivityDetector;
    use crate::audio_toolkit::SileroVad;

    let mut vad = match SileroVad::new(vad_path, 0.3) {
        Ok(v) => v,
        Err(e) => {
            log::warn!(
                "meeting finalize: SileroVad init failed ({}); fixed windows",
                e
            );
            return chunk_fixed(audio);
        }
    };

    let mut windows: Vec<(usize, usize)> = Vec::new();
    let n = audio.len();
    let mut win_start = 0usize; // start of the current window
    let mut pos = 0usize; // current frame start offset
    let mut silence_run = 0usize; // consecutive silent frames seen
    let mut win_has_speech = false; // whether the current window contains speech

    while pos < n {
        let end = (pos + FINALIZE_FRAME_SAMPLES).min(n);
        let frame = &audio[pos..end];
        // is_voice on the raw VAD is a per-frame decision (no hangover).
        let is_voice = vad.is_voice(frame).unwrap_or(false);
        if is_voice {
            win_has_speech = true;
            silence_run = 0;
        } else {
            silence_run += 1;
        }

        let win_len = end - win_start;
        let past_target = win_len >= FINALIZE_TARGET_SAMPLES;
        let safe_split = past_target && silence_run >= FINALIZE_SILENCE_SPLIT_FRAMES;
        let force_split = win_len >= FINALIZE_MAX_SAMPLES;

        if safe_split || force_split {
            if win_has_speech {
                windows.push((win_start, end));
            }
            win_start = end;
            win_has_speech = false;
            silence_run = 0;
        }
        pos = end;
    }

    // Close the trailing window.
    if win_start < n && win_has_speech {
        windows.push((win_start, n));
    }
    windows
}

/// Fallback chunker: fixed `FINALIZE_MAX_SAMPLES`-sized windows, no VAD.
#[cfg(target_os = "macos")]
fn chunk_fixed(audio: &[f32]) -> Vec<(usize, usize)> {
    let n = audio.len();
    let mut windows = Vec::new();
    let mut start = 0usize;
    while start < n {
        let end = (start + FINALIZE_MAX_SAMPLES).min(n);
        windows.push((start, end));
        start = end;
    }
    windows
}

/// Downsample a window of mixed samples into a fixed-length oscilloscope trace
/// (averaging strided chunks, values in -1..1) and the peak absolute amplitude
/// (0..1). Returns a flat zero trace when there are no samples.
fn downsample_wave(samples: &[f32], points: usize) -> (Vec<f32>, f32) {
    if samples.is_empty() || points == 0 {
        return (vec![0.0; points], 0.0);
    }
    let mut wave = Vec::with_capacity(points);
    let mut peak = 0.0f32;
    let len = samples.len();
    for p in 0..points {
        let start = p * len / points;
        let end = ((p + 1) * len / points).max(start + 1).min(len);
        let mut sum = 0.0f32;
        let mut count = 0u32;
        for &s in &samples[start..end] {
            sum += s;
            let a = s.abs();
            if a > peak {
                peak = a;
            }
            count += 1;
        }
        let avg = if count > 0 { sum / count as f32 } else { 0.0 };
        wave.push(avg.clamp(-1.0, 1.0));
    }
    (wave, peak.min(1.0))
}

/// Current time as epoch milliseconds.
fn now_epoch_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Default human-readable title for a meeting, derived from its absolute start
/// time. Uses `chrono` (already a dependency) to format the local datetime.
fn default_meeting_title(started_at_ms: i64) -> String {
    use chrono::{DateTime, Local};
    match DateTime::from_timestamp_millis(started_at_ms) {
        Some(utc) => {
            let local = utc.with_timezone(&Local);
            format!("Meeting {}", local.format("%B %e, %Y - %l:%M %p"))
        }
        None => "Meeting".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{downsample_wave, TranscriptSegment, TranscriptSource};

    #[test]
    fn transcript_source_serializes_as_you_and_others() {
        assert_eq!(
            serde_json::to_string(&TranscriptSource::Mic).unwrap(),
            "\"you\""
        );
        assert_eq!(
            serde_json::to_string(&TranscriptSource::System).unwrap(),
            "\"others\""
        );
        // Round-trip.
        let back: TranscriptSource = serde_json::from_str("\"others\"").unwrap();
        assert_eq!(back, TranscriptSource::System);
    }

    #[test]
    fn transcript_segment_defaults_source_for_legacy_records() {
        // Older persisted segments have no `source` field; they default to Mic.
        let legacy = r#"{"text":"hello","timestamp_ms":1200}"#;
        let seg: TranscriptSegment = serde_json::from_str(legacy).unwrap();
        assert_eq!(seg.source, TranscriptSource::Mic);
        assert_eq!(seg.text, "hello");
        assert_eq!(seg.timestamp_ms, 1200);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn chunk_fixed_splits_into_max_windows_covering_all_samples() {
        use super::FINALIZE_MAX_SAMPLES;
        // 2.5 windows worth of audio.
        let n = FINALIZE_MAX_SAMPLES * 2 + FINALIZE_MAX_SAMPLES / 2;
        let audio = vec![0.1f32; n];
        let windows = super::chunk_fixed(&audio);
        assert_eq!(windows.len(), 3);
        // Contiguous, non-overlapping, fully covering.
        assert_eq!(windows[0].0, 0);
        assert_eq!(windows[0].1, FINALIZE_MAX_SAMPLES);
        assert_eq!(windows[1].0, FINALIZE_MAX_SAMPLES);
        assert_eq!(windows[2].1, n);
        for w in &windows {
            assert!(w.1 > w.0);
            assert!(w.1 - w.0 <= FINALIZE_MAX_SAMPLES);
        }
    }

    #[test]
    fn downsample_wave_empty_is_flat() {
        let (wave, peak) = downsample_wave(&[], 96);
        assert_eq!(wave.len(), 96);
        assert!(wave.iter().all(|&v| v == 0.0));
        assert_eq!(peak, 0.0);
    }

    #[test]
    fn downsample_wave_fixed_length_and_peak() {
        // 480 samples -> 96 points, peak should be the max abs amplitude.
        let samples: Vec<f32> = (0..480)
            .map(|i| if i % 2 == 0 { 0.5 } else { -0.5 })
            .collect();
        let (wave, peak) = downsample_wave(&samples, 96);
        assert_eq!(wave.len(), 96);
        assert!((peak - 0.5).abs() < 1e-6);
        // Averaging alternating +/-0.5 over each chunk -> near zero.
        assert!(wave.iter().all(|&v| v.abs() <= 0.5));
    }

    #[test]
    fn downsample_wave_clamps_and_bounds_peak() {
        let samples = vec![5.0f32, -5.0, 2.0, -2.0];
        let (wave, peak) = downsample_wave(&samples, 4);
        assert_eq!(wave.len(), 4);
        assert!(wave.iter().all(|&v| (-1.0..=1.0).contains(&v)));
        assert_eq!(peak, 1.0); // clamped to 1.0
    }

    #[test]
    fn downsample_wave_more_points_than_samples() {
        // Should not panic when points > samples.
        let samples = vec![0.1f32, 0.2, 0.3];
        let (wave, peak) = downsample_wave(&samples, 96);
        assert_eq!(wave.len(), 96);
        assert!(peak > 0.0);
    }
}
