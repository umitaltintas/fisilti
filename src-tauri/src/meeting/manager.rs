// Meeting mode: the continuous meeting session manager.
//
// Owns the meeting slot (idle / running / finalizing) and the lifecycle of a
// session:
//   1. `start()` creates a fresh `Session` and spawns the capture thread
//      (`capture.rs`): mic + system audio at 16 kHz, per-source VAD, raw
//      capture buffers on disk, and a session worker that runs the rough live
//      transcription and incremental persistence.
//   2. `stop()` moves the slot to `Finalizing`, joins capture, re-transcribes
//      the full audio (`finalize.rs`), persists the row, saves the playback
//      audio, then kicks off the LLM title/summary — and only then returns to
//      `Idle`. The session object is owned by that stop, so nothing a later
//      start does can reach it.
//
// Imports, re-transcription and recovery live in `exclusive.rs`; Gemini Live
// streaming in `live.rs`; the LLM prompts in `summarize.rs`.
//
// This module is ADDITIVE and ISOLATED from the dictation flow. It never
// touches the `AudioRecordingManager` / `RecordingState` singletons.
//
// The capture loop is macOS-only (CoreAudio tap). On other platforms `start()`
// returns an "unsupported" error; the struct and commands still compile.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::AppHandle;

use crate::managers::transcription::TranscriptionManager;
use crate::meeting::session::{MeetingSessionInfo, MeetingState, Session, StopMeetingResult};
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
    /// Translation of `text`, when the segment came from the Gemini Live
    /// translation path. `None` for the normal transcription paths and for
    /// records saved before live translation existed.
    #[serde(default)]
    pub translation: Option<String>,
    /// Which speaker said this, when the transcript came from a path that can
    /// tell participants apart (the Gemini batch finalize pass with diarization
    /// on). Holds a display label like `"Speaker 1"`, already resolved from the
    /// API's raw `spk_1`. `None` everywhere else — `source` remains the only
    /// attribution the local paths can offer.
    #[serde(default)]
    pub speaker: Option<String>,
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

/// Event payload emitted on `"meeting-title-update"` whenever a meeting's
/// title changes automatically — at session start when `meeting_naming`
/// resolves a calendar/window title, or after stop when the LLM auto-title
/// lands. Carries the row id so the UI only renames the matching meeting.
#[derive(Clone, Debug, Serialize, Type)]
pub struct MeetingTitleUpdate {
    pub id: i64,
    pub title: String,
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

/// Which step of an import is running, for `"meeting-import-progress"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum MeetingImportStage {
    Decoding,
    Transcribing,
    Summarizing,
}

/// Event payload emitted on `"meeting-import-progress"` while a recording file
/// is imported. `progress` is 0..1 within the current stage, `None` when the
/// stage cannot measure itself (a single cloud request).
#[derive(Clone, Debug, Serialize, Type)]
pub struct MeetingImportProgress {
    pub file_name: String,
    pub stage: MeetingImportStage,
    pub progress: Option<f32>,
}

/// Event payload emitted on `"meeting-import-finished"` when an import ends,
/// however it ends. Lets a window that did not start the import (or was
/// reopened mid-way) settle its progress card.
#[derive(Clone, Debug, Serialize, Type)]
pub struct MeetingImportFinished {
    /// Row id of the saved meeting on success.
    pub id: Option<i64>,
    /// Why it failed; `import::CANCELLED` when the user cancelled.
    pub error: Option<String>,
}

/// Event payload emitted on `"meeting-summary-update"` when a summary is
/// saved. Carries the row id so the UI only updates the matching meeting —
/// the summary of a meeting that finished earlier may land while the user is
/// already in the next one.
#[derive(Clone, Debug, Serialize, Type)]
pub struct MeetingSummaryUpdate {
    pub id: i64,
    pub summary: String,
}

/// Error returned by `stop()` while a stop is already in progress (tray and
/// UI clicked at once, or the auto-end timer raced a click).
pub const ALREADY_STOPPING: &str = "The meeting is already being stopped.";

/// The meeting slot: what state it is in, and the session it holds (the live
/// one, or the most recent one once idle — its transcript stays readable).
pub(super) struct Slot {
    pub state: MeetingState,
    pub session: Option<Arc<Session>>,
}

/// Owns the meeting slot and everything shared across sessions.
///
/// Cloneable handle around shared state; the capture work runs on a dedicated
/// thread spawned in `start()`.
#[derive(Clone)]
pub struct MeetingManager {
    pub(super) app_handle: AppHandle,
    pub(super) transcription_manager: Arc<TranscriptionManager>,
    /// Persistence store for meeting sessions (same history.db as dictation).
    pub(super) store: MeetingStore,
    /// State + current session, serializing start/stop/exclusive jobs.
    pub(super) slot: Arc<Mutex<Slot>>,
    /// Lock-free mirror of "a meeting owns the engine" (running OR
    /// finalizing). The TranscriptionManager idle-watcher and dictation read
    /// it; it stays set until `stop()` has fully finished, so dictation cannot
    /// grab or unload the engine in the middle of the finalize pass.
    pub(super) active: Arc<AtomicBool>,
    /// Source of session generations.
    next_generation: Arc<AtomicU64>,
    /// True while an exclusive job (import, re-transcription, recovery) runs.
    /// It owns the engine exactly like a live session does, so they exclude
    /// each other.
    pub(super) importing: Arc<AtomicBool>,
    /// Set by `cancel_import`; checked between decode packets and between
    /// transcription windows.
    pub(super) import_cancel: Arc<AtomicBool>,
    /// Last progress reported by the running import, so a window that opens
    /// mid-import can show where it is instead of nothing.
    pub(super) import_progress: Arc<Mutex<Option<MeetingImportProgress>>>,
}

/// Returns the slot to `Idle` when `stop()` ends — normally or by panic — so
/// a crash inside the finalize pass can never wedge the app in "finalizing".
struct FinalizeGuard<'a> {
    manager: &'a MeetingManager,
    generation: u64,
}

impl Drop for FinalizeGuard<'_> {
    fn drop(&mut self) {
        let changed = {
            let mut slot = self
                .manager
                .slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let same = slot
                .session
                .as_ref()
                .is_some_and(|s| s.generation == self.generation);
            if same && slot.state == MeetingState::Finalizing {
                slot.state = MeetingState::Idle;
                self.manager.active.store(false, Ordering::SeqCst);
                true
            } else {
                false
            }
        };
        if changed {
            self.manager.emit_state();
        }
    }
}

impl MeetingManager {
    pub fn new(app_handle: &AppHandle, transcription_manager: Arc<TranscriptionManager>) -> Self {
        // If the app data dir cannot be resolved (should not happen in
        // practice), every store call fails with the reason rather than
        // writing a stray database into the working directory.
        let store = MeetingStore::new(app_handle).unwrap_or_else(|e| {
            log::error!("Failed to initialize MeetingStore: {}", e);
            MeetingStore::unavailable(e.to_string())
        });
        Self {
            app_handle: app_handle.clone(),
            transcription_manager,
            store,
            slot: Arc::new(Mutex::new(Slot {
                state: MeetingState::Idle,
                session: None,
            })),
            active: Arc::new(AtomicBool::new(false)),
            next_generation: Arc::new(AtomicU64::new(0)),
            importing: Arc::new(AtomicBool::new(false)),
            import_cancel: Arc::new(AtomicBool::new(false)),
            import_progress: Arc::new(Mutex::new(None)),
        }
    }

    /// Whether a meeting session (running or finalizing) or an exclusive job
    /// currently owns the transcription engine. Consulted by the
    /// TranscriptionManager idle-watcher to keep the model loaded, and by
    /// dictation to leave the engine alone.
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::SeqCst) || self.importing.load(Ordering::SeqCst)
    }

    pub fn status(&self) -> MeetingState {
        self.slot.lock().unwrap().state
    }

    /// The session in the slot: live, finalizing, or the most recent one.
    pub(super) fn current_session(&self) -> Option<Arc<Session>> {
        self.slot.lock().unwrap().session.clone()
    }

    /// Snapshot for `get_meeting_session` / `meeting-session-changed`.
    pub fn session_info(&self) -> MeetingSessionInfo {
        let slot = self.slot.lock().unwrap();
        match (&slot.session, slot.state) {
            (Some(session), MeetingState::Running | MeetingState::Finalizing) => {
                MeetingSessionInfo {
                    state: slot.state.as_str().to_string(),
                    meeting_id: session.meeting_id(),
                    started_at_ms: Some(session.started_at_ms),
                }
            }
            _ => MeetingSessionInfo {
                state: slot.state.as_str().to_string(),
                meeting_id: None,
                started_at_ms: None,
            },
        }
    }

    /// Row id of the meeting that is running or still being saved.
    pub fn live_meeting_id(&self) -> Option<i64> {
        self.session_info().meeting_id
    }

    /// Refuse to touch the meeting that is live or finalizing: deleting,
    /// discarding, recovering or re-transcribing it would destroy the meeting
    /// in progress (or be overwritten by its finalize a moment later).
    pub fn ensure_not_live(&self, id: i64) -> Result<(), String> {
        if self.live_meeting_id() == Some(id) {
            Err(
                "This meeting is in progress; stop it and wait for it to be saved first."
                    .to_string(),
            )
        } else {
            Ok(())
        }
    }

    /// Absolute epoch-ms start time of the live session, or `None` when idle.
    /// Lets a UI that opens (or reloads) mid-meeting show the REAL elapsed time
    /// instead of counting up from the moment it attached.
    pub fn session_started_at_ms(&self) -> Option<i64> {
        self.session_info().started_at_ms
    }

    /// Reset the prolonged-silence timer used by the auto-end flow. Called
    /// when the user answers the "end meeting?" prompt with "keep going".
    pub fn reset_silence_timer(&self) {
        if let Some(session) = self.current_session() {
            session.reset_silence_timer();
        }
    }

    /// The transcript of the live (or most recent) session.
    pub fn full_transcript(&self) -> String {
        self.current_session().map(|s| s.text()).unwrap_or_default()
    }

    /// Access the persistence store (used by list/get commands).
    pub fn store(&self) -> &MeetingStore {
        &self.store
    }

    /// Start a meeting session. Ensures the transcription model is loading,
    /// creates a fresh session and spawns the capture loop.
    pub fn start(&self) -> Result<(), String> {
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (&self.transcription_manager, &self.next_generation);
            return Err("Meeting mode capture is only supported on macOS".to_string());
        }

        #[cfg(target_os = "macos")]
        {
            let session = {
                let mut slot = self.slot.lock().unwrap();
                match slot.state {
                    MeetingState::Running => return Err("Meeting already running".to_string()),
                    MeetingState::Finalizing => {
                        return Err(
                            "The previous meeting is still being saved. Try again in a moment."
                                .to_string(),
                        )
                    }
                    MeetingState::Idle => {}
                }
                if self.importing.load(Ordering::SeqCst) {
                    return Err(
                        "A recording is being imported. Wait for it to finish or cancel it first."
                            .to_string(),
                    );
                }
                let generation = self.next_generation.fetch_add(1, Ordering::SeqCst) + 1;
                let session = Arc::new(Session::new(generation, now_epoch_ms()));
                slot.session = Some(session.clone());
                slot.state = MeetingState::Running;
                self.active.store(true, Ordering::SeqCst);
                session
            };

            // Kick off the model load (background, same path as dictation). The
            // live transcription calls block-wait on the loading condvar if
            // needed.
            let settings = crate::settings::get_settings(&self.app_handle);
            self.transcription_manager
                .initiate_model_load_for(settings.meeting_model_id());

            let handle = {
                let manager = self.clone();
                let session = session.clone();
                std::thread::spawn(move || {
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        manager.run_capture_loop(&session)
                    }));
                    let error = match outcome {
                        Ok(Ok(())) => None,
                        Ok(Err(e)) => Some(e),
                        Err(_) => Some("The meeting capture thread crashed.".to_string()),
                    };
                    if let Some(error) = error {
                        log::error!("Meeting capture loop ended with error: {}", error);
                        manager.abort_session(&session, &error);
                    }
                })
            };
            *session.worker.lock().unwrap() = Some(handle);

            // TITLE (naming): try to name the session after the real meeting
            // (calendar event, else the meeting app's window title) in the
            // background — AX / EventKit calls must never delay the start.
            {
                let manager = self.clone();
                let session = session.clone();
                std::thread::spawn(move || manager.resolve_session_title(&session));
            }

            self.emit_state();
            log::info!("Meeting session started");
            Ok(())
        }
    }

    /// The capture thread failed (mic denied, no device, tap failure, VAD
    /// init, or a panic). Return the slot to idle and tell the UI why — the
    /// state used to stay "running" with nothing captured.
    ///
    /// A failure before the sources came up leaves no row; its buffers are
    /// removed. A failure mid-meeting keeps the row (still `recording`) so the
    /// audio captured so far can be recovered.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn abort_session(&self, session: &Arc<Session>, error: &str) {
        let aborted = {
            let mut slot = self.slot.lock().unwrap();
            let same = slot
                .session
                .as_ref()
                .is_some_and(|s| s.generation == session.generation);
            if same && slot.state == MeetingState::Running {
                slot.state = MeetingState::Idle;
                self.active.store(false, Ordering::SeqCst);
                true
            } else {
                // A stop() is already finalizing it and will persist or
                // discard whatever there is.
                false
            }
        };
        if !aborted {
            return;
        }
        session.stop_signal.store(true, Ordering::SeqCst);
        #[cfg(target_os = "macos")]
        self.clear_subtitles(session);
        match session.meeting_id() {
            None => {
                if let Some(buffers) = session.buffers() {
                    buffers.remove_files();
                }
                self.emit_error(&format!("Could not start the meeting: {}", error));
            }
            Some(id) => {
                self.persist_incremental_now(session);
                self.update_progress_clock(session);
                log::warn!(
                    "meeting: row {} left recoverable after a capture failure",
                    id
                );
                self.emit_error(&format!(
                    "The meeting stopped unexpectedly ({}). What was recorded can be recovered.",
                    error
                ));
            }
        }
        self.emit_state();
    }

    /// Stop the meeting: end capture, run the finalize pass, persist, save the
    /// audio, start the LLM title/summary. The slot reads `Finalizing` for the
    /// whole stop and only returns to `Idle` once everything is written.
    ///
    /// When idle, returns the most recent session's result (idempotent). A
    /// second stop while one is in progress returns `Err(ALREADY_STOPPING)`.
    pub fn stop(&self) -> Result<StopMeetingResult, String> {
        let session = {
            let mut slot = self.slot.lock().unwrap();
            match slot.state {
                MeetingState::Idle => {
                    return Ok(match &slot.session {
                        Some(s) => StopMeetingResult {
                            meeting_id: s.saved_id(),
                            transcript: s.text(),
                        },
                        None => StopMeetingResult {
                            meeting_id: None,
                            transcript: String::new(),
                        },
                    });
                }
                MeetingState::Finalizing => return Err(ALREADY_STOPPING.to_string()),
                MeetingState::Running => {}
            }
            let Some(session) = slot.session.clone() else {
                slot.state = MeetingState::Idle;
                self.active.store(false, Ordering::SeqCst);
                return Ok(StopMeetingResult {
                    meeting_id: None,
                    transcript: String::new(),
                });
            };
            slot.state = MeetingState::Finalizing;
            session
        };
        let guard = FinalizeGuard {
            manager: self,
            generation: session.generation,
        };
        // The tray and UI see "finalizing" right away, before the (possibly
        // minutes-long) finalize pass.
        self.emit_state();

        session.stop_signal.store(true, Ordering::SeqCst);
        // start() stores the capture handle right after the slot turns
        // Running; a stop racing that instant waits for it (bounded).
        let mut handle = session.worker.lock().unwrap().take();
        for _ in 0..100 {
            if handle.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            handle = session.worker.lock().unwrap().take();
        }
        if let Some(handle) = handle {
            if let Err(e) = handle.join() {
                log::warn!("Failed to join meeting capture thread: {:?}", e);
            }
        }

        // Hybrid transcription: re-transcribe the FULL per-source audio for a
        // higher-quality, labeled transcript that REPLACES the live preview.
        #[cfg(target_os = "macos")]
        self.finalize_session(&session);

        // Persist. Failures are logged, never propagated, so a stop always
        // succeeds and returns the transcript.
        self.persist_session(&session);

        #[cfg(target_os = "macos")]
        self.save_session_audio(&session);

        // Best-effort LLM title + summary, spawned; they carry the row id they
        // were started for.
        self.maybe_auto_title(&session);
        self.maybe_auto_summarize(&session);

        let result = StopMeetingResult {
            meeting_id: session.saved_id(),
            transcript: session.text(),
        };
        drop(guard);
        log::info!("Meeting session stopped");
        Ok(result)
    }

    /// The app is quitting. A running meeting cannot be finalized in the time
    /// a quit allows, so end capture (bounded), flush the capture buffers and
    /// the transcript so far, and leave the row `recording` — the recovery
    /// banner offers it on next launch. A meeting already finalizing is left
    /// as is: its row is still `recording` until finalize writes it, so an
    /// interrupted save is recoverable the same way.
    pub fn shutdown(&self) {
        let session = {
            let slot = self.slot.lock().unwrap();
            match slot.state {
                MeetingState::Running => slot.session.clone(),
                _ => None,
            }
        };
        let Some(session) = session else {
            return;
        };
        log::info!("meeting: app quitting mid-meeting; saving what was captured for recovery");
        session.shutting_down.store(true, Ordering::SeqCst);
        session.stop_signal.store(true, Ordering::SeqCst);
        let handle = session.worker.lock().unwrap().take();
        if let Some(handle) = handle {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(6);
            while !handle.is_finished() && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            if handle.is_finished() {
                let _ = handle.join();
            } else {
                log::warn!("meeting: capture did not stop in time during quit");
            }
        }
        self.persist_incremental_now(&session);
        self.update_progress_clock(&session);
    }

    /// Persist the session on stop(). The common path FINALIZES the in-progress
    /// row (status `recording` → `completed`, final transcript/segments,
    /// buffer paths cleared). If no in-progress row exists (the insert failed),
    /// or finalizing it fails, falls back to a plain INSERT so the transcript
    /// isn't lost. Empty sessions are discarded or kept for recovery.
    fn persist_session(&self, session: &Session) {
        let (segments, transcript) = {
            let t = session.transcript();
            (t.segments().to_vec(), t.text().to_string())
        };
        let ended_at = now_epoch_ms();
        let started_at = session.started_at_ms;
        let duration_ms = (ended_at - started_at).max(0);
        let current_id = session.meeting_id();

        // An empty transcript means one of two very different things:
        //   1. Nothing was said, or nothing was captured — nothing worth keeping.
        //   2. Audio WAS captured but every transcription call failed.
        // Deleting the row in case 2 destroys a recording the user cannot get
        // back. Keep it instead, as an in-progress row the recovery flow can
        // re-run once the cause is fixed.
        if transcript.trim().is_empty() {
            let failure = session.finalize_error.lock().unwrap().clone();
            let captured_samples = session
                .buffers()
                .map(|b| super::buffers::sample_count(&b.mixed))
                .unwrap_or(0);
            let voiced = session.voiced.load(Ordering::Relaxed);
            match empty_session_outcome(failure.as_deref(), captured_samples, voiced) {
                EmptySessionOutcome::PreserveForRecovery => {
                    log::warn!(
                        "meeting: no transcript from a session with {} samples of audio; \
                         keeping it for recovery (reason: {})",
                        captured_samples,
                        failure.as_deref().unwrap_or("no text was produced")
                    );
                    self.preserve_failed_session(session);
                    return;
                }
                EmptySessionOutcome::Discard => {
                    log::debug!("Meeting transcript empty; discarding the session");
                    if let Some(id) = current_id {
                        if let Err(e) = self.store.delete_meeting(id) {
                            log::warn!("meeting: failed to discard empty row {}: {}", id, e);
                        }
                        *session.meeting_id.lock().unwrap() = None;
                    }
                    if let Some(buffers) = session.buffers() {
                        buffers.remove_files();
                    }
                    return;
                }
            }
        }

        if let Some(id) = current_id {
            match self
                .store
                .finalize_meeting(id, &transcript, &segments, ended_at, duration_ms)
            {
                Ok(()) => {
                    log::info!("Finalized meeting row {} (completed)", id);
                    self.persist_usage(id, &session.usage);
                    self.export_markdown(id);
                    *session.saved_id.lock().unwrap() = Some(id);
                    return;
                }
                Err(e) => log::error!(
                    "Failed to finalize meeting row {}: {}; saving as a new row",
                    id,
                    e
                ),
            }
        }

        // Fallback: no usable in-progress row. Insert a completed row directly.
        let title = session
            .title()
            .unwrap_or_else(|| default_meeting_title(started_at));
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
                self.persist_usage(id, &session.usage);
                self.export_markdown(id);
                *session.saved_id.lock().unwrap() = Some(id);
            }
            Err(e) => log::error!("Failed to persist meeting session: {}", e),
        }
    }

    /// Keep a session whose transcription failed wholesale. The row stays in
    /// `recording` status on purpose: that is exactly what the recovery banner
    /// offers a "Recover" button for, so once the user fixes the cause one
    /// click re-runs the finalize pass over the SAME audio. The playback audio
    /// is written now; the capture buffers are kept for the recovery pass.
    fn preserve_failed_session(&self, session: &Session) {
        let Some(id) = session.meeting_id() else {
            return;
        };
        self.persist_incremental_now(session);
        self.update_progress_clock(session);
        #[cfg(target_os = "macos")]
        if let Some(buffers) = session.buffers() {
            if let Err(e) = self.save_audio_from_raw(id, &buffers.mixed) {
                log::error!("meeting: failed to save playback audio: {}", e);
            }
        }
        #[cfg(not(target_os = "macos"))]
        let _ = id;
    }

    /// Save the playback audio of the meeting `persist_session` saved, then
    /// drop the capture buffers now that the session is fully on disk. When
    /// the audio cannot be written the buffers are kept (and swept later).
    #[cfg(target_os = "macos")]
    fn save_session_audio(&self, session: &Session) {
        let (Some(id), Some(buffers)) = (session.saved_id(), session.buffers()) else {
            return;
        };
        match self.save_audio_from_raw(id, &buffers.mixed) {
            Ok(_) => buffers.remove_files(),
            Err(e) => log::error!("meeting: failed to save playback audio: {}", e),
        }
    }

    /// Insert the in-progress row of `session` (status `recording`) with its
    /// buffer paths, once capture is up. Announces the row id.
    #[cfg(target_os = "macos")]
    pub(super) fn insert_session_row(
        &self,
        session: &Session,
        buffers: &super::session::SessionBuffers,
    ) {
        let started_at = session.started_at_ms;
        // Prefer the explicit session title (calendar / window) when the
        // background resolution already landed; datetime otherwise.
        let title = session
            .title()
            .unwrap_or_else(|| default_meeting_title(started_at));
        let stored = crate::meeting::store::StoredBuffers {
            mic: Some(buffers.mic.to_string_lossy().to_string()),
            system: Some(buffers.system.to_string_lossy().to_string()),
            mixed: Some(buffers.mixed.to_string_lossy().to_string()),
        };
        match self.store.start_meeting(started_at, &title, &stored) {
            Ok(id) => {
                log::info!("meeting: inserted in-progress row {} (recording)", id);
                *session.meeting_id.lock().unwrap() = Some(id);
                // Close the race with the naming thread: if it stashed a title
                // after we read it but before the id was registered above,
                // rename the row now.
                if let Some(resolved) = session.title() {
                    if resolved != title {
                        self.apply_title(id, &resolved);
                    }
                }
                self.emit_session_changed();
            }
            Err(e) => log::error!("meeting: failed to insert in-progress row: {}", e),
        }
    }

    /// Refresh meeting `id`'s copy in the user's export folder (Obsidian vault
    /// etc.), when one is configured. Best-effort, never fails the caller.
    pub fn export_markdown(&self, id: i64) {
        super::export::sync_to_export_dir(&self.app_handle, &self.store, id);
    }

    /// TITLE (naming): resolve an explicit title for `session` from the
    /// calendar event in progress or the meeting app's window title. Tries
    /// twice (browser tab titles can take a moment to reflect the joined
    /// meeting). Scoped to its own session: a slow attempt that finishes after
    /// this meeting stopped (and another started) can no longer rename the
    /// wrong meeting.
    #[cfg(target_os = "macos")]
    fn resolve_session_title(&self, session: &Session) {
        for attempt in 0..2 {
            if attempt > 0 {
                std::thread::sleep(std::time::Duration::from_secs(12));
            }
            if session.is_stopping() {
                return;
            }
            let Some(title) = crate::meeting_naming::resolve_session_title(&self.app_handle) else {
                continue;
            };
            if session.is_stopping() {
                return;
            }
            *session.title.lock().unwrap() = Some(title.clone());
            // Rename the in-progress row when it is already inserted; otherwise
            // the capture loop reads the stash at insert time.
            if let Some(id) = session.meeting_id() {
                self.apply_title(id, &title);
            }
            return;
        }
    }

    /// Persist `title` on meeting row `id` and notify the UI via
    /// `"meeting-title-update"`. Shared by the naming resolution and the LLM
    /// auto-title.
    pub(super) fn apply_title(&self, id: i64, title: &str) {
        if let Err(e) = self.store.update_title(id, title) {
            log::error!("meeting title: failed to persist: {}", e);
            return;
        }
        self.export_markdown(id);
        use tauri::Emitter;
        let _ = self.app_handle.emit(
            "meeting-title-update",
            MeetingTitleUpdate {
                id,
                title: title.to_string(),
            },
        );
    }

    /// AUTO-TITLE. Generate a short title from the transcript via the active
    /// post-process LLM provider and store it on the saved row, replacing the
    /// datetime default. Spawned, so it never blocks stop(). Skipped when the
    /// session already carries an explicit name from the calendar / window.
    fn maybe_auto_title(&self, session: &Session) {
        if session.title().is_some() {
            return;
        }
        let Some(id) = session.saved_id() else {
            return;
        };
        let transcript = session.text();
        if transcript.trim().is_empty() {
            return;
        }
        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            match super::summarize::generate_title(&manager.app_handle, &transcript).await {
                Ok(title) => {
                    let title = title.trim().to_string();
                    if !title.is_empty() {
                        manager.apply_title(id, &title);
                    }
                }
                // Graceful fallback: keep the datetime title.
                Err(e) => log::info!("meeting auto-title skipped: {}", e),
            }
        });
    }

    /// AUTO-SUMMARIZE. If `meeting_auto_summarize` is enabled, summarize the
    /// meeting just saved and persist + emit the result for THAT row (the id
    /// is captured now; reading "the last saved meeting" when the LLM answered
    /// wrote summaries onto whichever meeting had finished since).
    fn maybe_auto_summarize(&self, session: &Session) {
        let settings = crate::settings::get_settings(&self.app_handle);
        if !settings.meeting_auto_summarize {
            return;
        }
        let Some(id) = session.saved_id() else {
            return;
        };
        let transcript = session.text();
        if transcript.trim().is_empty() {
            return;
        }
        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            // The user's notes (typed during the meeting) are useful context.
            let notes = manager.store.get_meeting(id).ok().and_then(|r| r.notes);
            match super::summarize::summarize_transcript(
                &manager.app_handle,
                &transcript,
                notes.as_deref(),
            )
            .await
            {
                Ok(summary) => manager.save_summary(id, &summary),
                Err(e) => log::error!("meeting auto-summarize failed: {}", e),
            }
        });
    }

    /// Summarize the most recent session's transcript (the `summarize_meeting`
    /// commands). The target row is read once, up front, and the summary is
    /// saved onto it — never onto whatever happens to be "last" when the LLM
    /// answers.
    pub async fn summarize_latest(&self, template: Option<&str>) -> Result<String, String> {
        let (id, transcript) = match self.current_session() {
            Some(session) => (session.saved_id(), session.text()),
            None => (None, String::new()),
        };
        if transcript.trim().is_empty() {
            return Err("No transcript to summarize. Start and run a meeting first.".to_string());
        }
        let notes = id
            .and_then(|id| self.store.get_meeting(id).ok())
            .and_then(|r| r.notes);
        let content = super::summarize::summarize_transcript_ext(
            &self.app_handle,
            &transcript,
            template,
            notes.as_deref(),
        )
        .await?;
        if let Some(id) = id {
            self.save_summary(id, &content);
        }
        Ok(content)
    }

    /// Store `summary` on row `id`, refresh its export, and emit
    /// `"meeting-summary-update"` with `{ id, summary }`.
    pub fn save_summary(&self, id: i64, summary: &str) {
        if let Err(e) = self.store.update_summary(id, summary) {
            log::error!("meeting {}: failed to save summary: {}", id, e);
            return;
        }
        self.export_markdown(id);
        use tauri::Emitter;
        let _ = self.app_handle.emit(
            "meeting-summary-update",
            MeetingSummaryUpdate {
                id,
                summary: summary.to_string(),
            },
        );
    }

    /// Append a segment (optionally with a translation / speaker) to
    /// `session` and emit `"meeting-transcript-update"`. The incremental
    /// persist happens on the session worker, never here: this runs on the
    /// Gemini Live callback (async runtime) and the session worker.
    pub(super) fn push_segment_with(
        &self,
        session: &Session,
        text: String,
        translation: Option<String>,
        speaker: Option<String>,
        timestamp_ms: u64,
        source: TranscriptSource,
    ) {
        if text.trim().is_empty() {
            return;
        }
        let segment = TranscriptSegment {
            text,
            timestamp_ms,
            source,
            translation,
            speaker,
        };
        let full = {
            let mut transcript = session.transcript();
            transcript.push(segment.clone());
            transcript.text().to_string()
        };
        session.persist_dirty.store(true, Ordering::SeqCst);
        use tauri::Emitter;
        let _ = self.app_handle.emit(
            "meeting-transcript-update",
            MeetingTranscriptUpdate {
                segment,
                full_transcript: full,
            },
        );
    }

    /// Replace the session's transcript with `segments` (the FINAL transcript)
    /// and emit a synthetic update so the UI swaps the live preview for it.
    /// The emitted `segment` is the last one, for payload-shape compatibility;
    /// the authoritative content is `full_transcript` plus the saved record.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(super) fn replace_transcript(&self, session: &Session, segments: Vec<TranscriptSegment>) {
        let (full, last) = {
            let mut transcript = session.transcript();
            transcript.replace(segments);
            (
                transcript.text().to_string(),
                transcript.segments().last().cloned(),
            )
        };
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

    /// CRASH-RECOVERY: write the session's current transcript + segments to
    /// its in-progress row. Best-effort.
    pub(super) fn persist_incremental_now(&self, session: &Session) {
        let Some(id) = session.meeting_id() else {
            return;
        };
        let (segments, transcript) = {
            let t = session.transcript();
            (t.segments().to_vec(), t.text().to_string())
        };
        let ended_at = now_epoch_ms();
        let duration_ms = (ended_at - session.started_at_ms).max(0);
        if let Err(e) =
            self.store
                .update_in_progress(id, &transcript, &segments, ended_at, duration_ms)
        {
            log::warn!("meeting: incremental persist of row {} skipped: {}", id, e);
        }
    }

    /// Bump the in-progress row's `ended_at`/`duration_ms` to "now" so the
    /// saved length of a running meeting stays truthful without rewriting the
    /// transcript. No-op before the row exists. Best-effort.
    pub(super) fn update_progress_clock(&self, session: &Session) {
        let Some(id) = session.meeting_id() else {
            return;
        };
        let now = now_epoch_ms();
        let duration_ms = (now - session.started_at_ms).max(0);
        if let Err(e) = self.store.update_progress_timestamp(id, now, duration_ms) {
            log::warn!(
                "meeting: failed to update progress clock of row {}: {}",
                id,
                e
            );
        }
    }

    /// Write the token usage tallied in `usage` onto row `id`.
    ///
    /// Skipped when nothing was spent, so a purely local meeting keeps a NULL
    /// column and the UI can tell "no cloud model was used" apart from "a cloud
    /// model was used and cost nothing". Merged into what the row already
    /// records: a recovered meeting was charged once by the session that
    /// crashed.
    pub(super) fn persist_usage(&self, id: i64, usage: &Mutex<crate::ai_usage::MeetingUsage>) {
        let usage = usage.lock().unwrap().clone();
        if usage.is_empty() {
            return;
        }
        let estimate = usage.estimate_usd();
        log::info!(
            "usage: meeting {} used {} model(s), estimated ${:.4}{}",
            id,
            usage.entries.len(),
            estimate.usd,
            if estimate.complete {
                ""
            } else {
                " (partial: unpriced model)"
            }
        );
        let mut merged = self
            .store
            .get_meeting(id)
            .ok()
            .and_then(|record| record.usage)
            .unwrap_or_default();
        for entry in &usage.entries {
            merged.add_with_audio(
                &entry.model,
                entry.input_tokens,
                entry.output_tokens,
                entry.audio_seconds,
            );
        }
        match serde_json::to_string(&merged) {
            Ok(json) => {
                if let Err(e) = self.store.update_usage(id, &json) {
                    log::warn!("usage: failed to record for meeting {}: {}", id, e);
                }
            }
            Err(e) => log::warn!("usage: could not serialize: {}", e),
        }
    }

    /// Emit `"meeting-state-changed"` (the state string) and
    /// `"meeting-session-changed"` (`MeetingSessionInfo`).
    pub(super) fn emit_state(&self) {
        use tauri::Emitter;
        let info = self.session_info();
        let _ = self.app_handle.emit(
            crate::commands::meeting::MEETING_STATE_CHANGED_EVENT,
            info.state.clone(),
        );
        let _ = self.app_handle.emit(
            crate::commands::meeting::MEETING_SESSION_CHANGED_EVENT,
            info,
        );
    }

    /// Emit only `"meeting-session-changed"` (the row id became known).
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(super) fn emit_session_changed(&self) {
        use tauri::Emitter;
        let _ = self.app_handle.emit(
            crate::commands::meeting::MEETING_SESSION_CHANGED_EVENT,
            self.session_info(),
        );
    }

    /// Emit a `"meeting-error"` signal so the UI can surface a failure instead
    /// of just showing an empty transcript. Best-effort.
    pub(super) fn emit_error(&self, message: &str) {
        use tauri::Emitter;
        let _ = self.app_handle.emit("meeting-error", message.to_string());
    }

    /// Emit a `"meeting-finalizing"` signal so the UI can show progress while
    /// the on-stop full-audio re-transcription runs.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(super) fn emit_finalizing(&self, finalizing: bool) {
        use tauri::Emitter;
        let _ = self
            .app_handle
            .emit("meeting-finalizing", MeetingFinalizing { finalizing });
    }

    /// Emit a throttled `"meeting-audio-level"` event for the live visualizer.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(super) fn emit_audio_level(&self, bars: Vec<f32>, wave: Vec<f32>, peak: f32) {
        use tauri::Emitter;
        let _ = self.app_handle.emit(
            "meeting-audio-level",
            MeetingAudioLevel { bars, wave, peak },
        );
    }
}

/// Join transcript segments into a single transcript string, ordered by their
/// relative timestamp and separated by spaces (the same text `TranscriptBuf`
/// maintains). Used by recovery, imports and re-transcription.
pub(super) fn join_segments(segments: &[TranscriptSegment]) -> String {
    let mut ordered: Vec<&TranscriptSegment> = segments.iter().collect();
    ordered.sort_by_key(|s| s.timestamp_ms);
    ordered
        .iter()
        .map(|s| s.text.as_str())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Current time as epoch milliseconds.
pub(super) fn now_epoch_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// What to do with a session that ended with an empty transcript.
#[derive(Debug, PartialEq, Eq)]
enum EmptySessionOutcome {
    /// Nothing worth keeping — drop the row and its buffers.
    Discard,
    /// Audio was captured but transcription failed. Keep the row (in
    /// `recording` status) plus its buffers so the recovery flow can retry.
    PreserveForRecovery,
}

/// Captured audio (16 kHz mixed track) past which an empty-transcript session
/// is always kept: 20 seconds. Shorter ones are accidental starts.
const KEEP_AUDIO_MIN_SAMPLES: u64 = 16_000 * 20;

/// Decide the fate of an empty-transcript session.
///
/// An empty transcript does not prove nothing was said: a quiet microphone,
/// a VAD that missed speech or a cloud model that returned nothing all look
/// the same here, and deleting the buffers makes that unrecoverable. So any
/// session with real audio — a reported failure, detected speech, or simply
/// more than [`KEEP_AUDIO_MIN_SAMPLES`] — is kept for the recovery flow, where
/// the user can re-transcribe it or discard it themselves. Only a short,
/// silent, error-free session (an accidental start) is dropped.
fn empty_session_outcome(
    failure: Option<&str>,
    captured_samples: u64,
    voiced: bool,
) -> EmptySessionOutcome {
    if captured_samples == 0 {
        return EmptySessionOutcome::Discard;
    }
    if failure.is_some() || voiced || captured_samples >= KEEP_AUDIO_MIN_SAMPLES {
        return EmptySessionOutcome::PreserveForRecovery;
    }
    EmptySessionOutcome::Discard
}

/// True when the time since the last observed speech frame has exceeded the
/// configured prolonged-silence timeout. Pure, so the threshold logic is
/// unit-testable without a running capture loop.
#[cfg(target_os = "macos")]
pub(super) fn silence_exceeded(anchor_elapsed: std::time::Duration, timeout_secs: u32) -> bool {
    anchor_elapsed > std::time::Duration::from_secs(timeout_secs as u64)
}

/// Default human-readable title for a meeting, derived from its absolute start
/// time.
pub(super) fn default_meeting_title(started_at_ms: i64) -> String {
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
    use super::{empty_session_outcome, EmptySessionOutcome, TranscriptSegment, TranscriptSource};

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
    fn silence_exceeded_fires_only_past_the_timeout() {
        use super::silence_exceeded;
        use std::time::Duration;
        assert!(!silence_exceeded(Duration::from_secs(179), 180));
        assert!(!silence_exceeded(Duration::from_secs(180), 180));
        assert!(silence_exceeded(Duration::from_millis(180_001), 180));
        assert!(silence_exceeded(Duration::from_secs(300), 180));
    }

    #[test]
    fn empty_session_with_failed_transcription_is_kept_for_recovery() {
        // A meeting whose every transcription call failed (e.g. no API
        // balance) used to be deleted outright, throwing away its audio.
        assert_eq!(
            empty_session_outcome(Some("402 Payment Required"), 5_968_320, false),
            EmptySessionOutcome::PreserveForRecovery
        );
    }

    #[test]
    fn a_long_session_with_no_transcript_keeps_its_audio() {
        // A 3-minute meeting on a cloud model came back with no text and no
        // detected speech, and was deleted with its audio. Length alone is
        // reason enough to keep it.
        assert_eq!(
            empty_session_outcome(None, 16_000 * 180, false),
            EmptySessionOutcome::PreserveForRecovery
        );
        assert_eq!(
            empty_session_outcome(None, 16_000 * 5, true),
            EmptySessionOutcome::PreserveForRecovery
        );
    }

    #[test]
    fn only_a_short_silent_session_is_discarded() {
        assert_eq!(
            empty_session_outcome(None, 16_000 * 5, false),
            EmptySessionOutcome::Discard
        );
        // Nothing captured: a retry would have nothing to read.
        assert_eq!(
            empty_session_outcome(Some("model unavailable"), 0, true),
            EmptySessionOutcome::Discard
        );
    }
}
