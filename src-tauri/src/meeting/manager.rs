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

/// A single transcribed speech segment with its (relative) start timestamp.
#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct TranscriptSegment {
    /// Cleaned transcript text for this segment.
    pub text: String,
    /// Milliseconds since the meeting session started.
    pub timestamp_ms: u64,
}

/// Event payload emitted on `"meeting-transcript-update"` after each segment.
#[derive(Clone, Debug, Serialize, Type)]
pub struct MeetingTranscriptUpdate {
    pub segment: TranscriptSegment,
    /// The full transcript so far (all segments joined).
    pub full_transcript: String,
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

    /// Return the full accumulated transcript text (segments joined by spaces).
    pub fn full_transcript(&self) -> String {
        let segs = self.transcript.lock().unwrap();
        segs.iter()
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

        // Persist the session. Failures are logged, never propagated, so a
        // stop always succeeds and returns the transcript.
        self.persist_session();

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

    /// Append a transcribed segment and emit an update event.
    fn push_segment(&self, text: String, timestamp_ms: u64) {
        if text.trim().is_empty() {
            return;
        }
        let segment = TranscriptSegment { text, timestamp_ms };
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
        use crate::audio_toolkit::vad::{SmoothedVad, VadFrame, VoiceActivityDetector};
        use crate::audio_toolkit::SileroVad;
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        use futures_util::StreamExt;
        use std::sync::mpsc;
        use std::time::{Duration, Instant};
        use tauri::Manager;

        // --- VAD setup (dedicated instance, NOT shared with dictation) ---
        const VAD_FRAME_SAMPLES: usize = (WHISPER_SAMPLE_RATE as usize * 30) / 1000; // 480 samples / 30 ms

        // Max samples per segment before forced flush (~22 s) so a continuous
        // talker still gets periodic transcription.
        const MAX_SEGMENT_SAMPLES: usize = WHISPER_SAMPLE_RATE as usize * 22;
        // Minimum segment length worth transcribing (~200 ms) to skip blips.
        const MIN_SEGMENT_SAMPLES: usize = WHISPER_SAMPLE_RATE as usize / 5;

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

        let silero = SileroVad::new(&vad_path, 0.3)
            .map_err(|e| format!("Failed to create SileroVad for meeting: {}", e))?;
        // prefill=15, hangover=15 (~450 ms silence tail), onset=2 — same shape as
        // dictation's SmoothedVad, but a separate instance.
        let mut vad: SmoothedVad = SmoothedVad::new(Box::new(silero), 15, 15, 2);

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
        let mut sys_resampler = FrameResampler::new(
            sys_rate as usize,
            WHISPER_SAMPLE_RATE as usize,
            Duration::from_millis(30),
        );

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

        // --- Mix + VAD + segment state ---
        let mut mixer = MeetingMixer::new();
        let mut mixed: Vec<f32> = Vec::new();
        // Carry-over of mixed samples not yet aligned to a 480-sample frame.
        let mut frame_accum: Vec<f32> = Vec::with_capacity(VAD_FRAME_SAMPLES);
        // Current in-progress speech segment.
        let mut segment: Vec<f32> = Vec::new();
        let mut total_samples: u64 = 0;
        // Sample index (since session start) where the current segment began.
        let mut segment_start_samples: u64 = 0;

        loop {
            if self.stop_signal.load(Ordering::Relaxed) {
                break;
            }

            // Drain any available mic frames (non-blocking).
            while let Ok(frame) = mic_rx.try_recv() {
                mixer.push(MixSource::Microphone, &frame);
            }

            // Pull a system-audio batch (blocks up to 100 ms to pace the loop).
            match sys_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(batch) => {
                    sys_resampler.push(&batch, &mut |frame: &[f32]| {
                        mixer.push(MixSource::System, frame);
                    });
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // No system audio yet; keep looping so mic/stop are serviced.
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }

            mixer.drain_into(&mut mixed);

            // Feed mixed samples into the VAD frame-by-frame.
            if !mixed.is_empty() {
                // --- Live audio level: accumulate raw mixed samples, feed the
                // visualiser for bars, and emit a throttled event (~20 fps). ---
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

                frame_accum.append(&mut mixed);
                while frame_accum.len() >= VAD_FRAME_SAMPLES {
                    let frame: Vec<f32> = frame_accum.drain(0..VAD_FRAME_SAMPLES).collect();
                    total_samples += VAD_FRAME_SAMPLES as u64;

                    match vad.push_frame(&frame) {
                        Ok(VadFrame::Speech(speech)) => {
                            if segment.is_empty() {
                                // New segment begins. The SmoothedVad returns
                                // prefill+current on onset, so back-date the
                                // start by the speech length already captured.
                                let prelen = speech.len() as u64;
                                segment_start_samples = total_samples.saturating_sub(prelen);
                            }
                            segment.extend_from_slice(speech);

                            // Forced flush for a very long continuous talker.
                            if segment.len() >= MAX_SEGMENT_SAMPLES {
                                self.flush_segment(&mut segment, segment_start_samples);
                            }
                        }
                        Ok(VadFrame::Noise) => {
                            // Silence past hangover -> end of segment.
                            if !segment.is_empty() {
                                if segment.len() >= MIN_SEGMENT_SAMPLES {
                                    self.flush_segment(&mut segment, segment_start_samples);
                                } else {
                                    segment.clear();
                                }
                            }
                        }
                        Err(e) => {
                            log::warn!("meeting VAD frame error: {}", e);
                        }
                    }
                }
            }
        }

        // --- Teardown: stop mic + system, flush resamplers + mixer + final ---
        mic_stop.store(true, Ordering::Relaxed);
        let _ = mic_handle.join();
        sys_stop.store(true, Ordering::Relaxed);
        // The async task tears down the CoreAudio tap on drop; wait for it.
        let _ = tauri::async_runtime::block_on(sys_task);
        while let Ok(frame) = mic_rx.try_recv() {
            mixer.push(MixSource::Microphone, &frame);
        }
        // Drain any remaining system batches the task forwarded before exit.
        while let Ok(batch) = sys_rx.try_recv() {
            sys_resampler.push(&batch, &mut |frame: &[f32]| {
                mixer.push(MixSource::System, frame);
            });
        }
        sys_resampler.finish(&mut |frame: &[f32]| {
            mixer.push(MixSource::System, frame);
        });
        mixer.flush_into(&mut mixed);

        // Push any trailing mixed audio through the VAD.
        frame_accum.append(&mut mixed);
        while frame_accum.len() >= VAD_FRAME_SAMPLES {
            let frame: Vec<f32> = frame_accum.drain(0..VAD_FRAME_SAMPLES).collect();
            total_samples += VAD_FRAME_SAMPLES as u64;
            if let Ok(VadFrame::Speech(speech)) = vad.push_frame(&frame) {
                if segment.is_empty() {
                    segment_start_samples = total_samples.saturating_sub(speech.len() as u64);
                }
                segment.extend_from_slice(speech);
            }
        }

        // Flush the final in-progress segment regardless of min length.
        if !segment.is_empty() {
            self.flush_segment(&mut segment, segment_start_samples);
        }

        // Emit one final flat level so the visualiser settles to a flat line.
        self.emit_audio_level(vec![0.0; VIS_BUCKETS], vec![0.0; WAVE_POINTS], 0.0);

        Ok(())
    }

    /// Transcribe a completed segment and append the result. Clears `segment`.
    #[cfg(target_os = "macos")]
    fn flush_segment(&self, segment: &mut Vec<f32>, start_samples: u64) {
        use crate::audio_toolkit::constants::WHISPER_SAMPLE_RATE;

        if segment.is_empty() {
            return;
        }
        let audio = std::mem::take(segment);
        let timestamp_ms = start_samples.saturating_mul(1000) / WHISPER_SAMPLE_RATE as u64;

        match self.transcription_manager.transcribe(audio) {
            Ok(text) => {
                if !text.trim().is_empty() {
                    log::info!("meeting segment @ {}ms: {}", timestamp_ms, text);
                    self.push_segment(text, timestamp_ms);
                }
            }
            Err(e) => {
                log::warn!("meeting segment transcription failed: {}", e);
            }
        }
    }
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
    use super::downsample_wave;

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
