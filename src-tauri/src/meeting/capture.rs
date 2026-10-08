// The capture side of a live meeting (macOS): mic + system audio capture,
// per-source VAD segmentation, the raw capture buffers, and the session
// worker that runs the rough live transcription and incremental persistence
// OFF the capture thread.
//
// Threads of a running session:
//   * capture thread  — `run_capture_loop`: mixes, conditions, VADs and writes
//     buffers. Never transcribes and never touches the database, so a slow
//     transcription can no longer stall capture.
//   * mic worker      — owns the cpal input stream, resamples to 16 kHz.
//   * system task     — drains the CoreAudio tap on the async runtime.
//   * session worker  — `run_session_worker`: live transcription jobs from a
//     bounded queue, plus the periodic incremental persist + progress clock.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::audio_toolkit::constants::WHISPER_SAMPLE_RATE;

use super::buffers::RawF32Writer;
use super::manager::{MeetingManager, TranscriptSource};
use super::session::{Session, SessionBuffers};

/// Segmentation tuning shared by both per-source processors.
pub(super) struct SegConfig {
    pub frame_samples: usize,
    pub min_samples: usize,
    pub max_samples: usize,
    pub merge_gap_samples: u64,
    /// Longest a merged (pending) segment may grow before it is sent for
    /// transcription. Without it a continuous talker — each segment ending
    /// at the 22 s force-flush and the next starting within the merge gap —
    /// grew one unbounded buffer that was never transcribed until they paused.
    pub max_pending_samples: usize,
}

/// Live segments waiting for transcription. Small on purpose; see
/// [`LiveQueue::submit`] for what happens when it is full.
const LIVE_QUEUE_CAPACITY: usize = 4;

/// One finished live segment to transcribe.
pub(super) struct LiveJob {
    audio: Vec<f32>,
    start_samples: u64,
    source: TranscriptSource,
}

/// Producer side of the live-transcription queue.
struct LiveQueue {
    tx: mpsc::SyncSender<LiveJob>,
    /// The meeting model runs locally (cloud models skip the live pass: one
    /// request per VAD segment would be absurd; the finalize pass covers it).
    live_pass: bool,
    dropped: u64,
}

impl LiveQueue {
    /// Queue a segment for the rough live transcription.
    ///
    /// DROP POLICY: when the worker is behind and the queue is full, the
    /// segment is dropped from the live preview (logged). Blocking instead
    /// would stall the capture thread and overflow the audio channels, and
    /// nothing is lost for good: the audio is in the capture buffers and the
    /// finalize pass re-transcribes all of it on stop.
    fn submit(
        &mut self,
        session: &Session,
        audio: Vec<f32>,
        start_samples: u64,
        source: TranscriptSource,
    ) {
        if audio.is_empty()
            || !self.live_pass
            // Gemini Live produces its own segments from the same audio;
            // transcribing here too would duplicate every utterance.
            || session.live_gemini_active.load(Ordering::Relaxed)
        {
            return;
        }
        match self.tx.try_send(LiveJob {
            audio,
            start_samples,
            source,
        }) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(_)) => {
                self.dropped += 1;
                if self.dropped == 1 || self.dropped.is_multiple_of(10) {
                    log::warn!(
                        "meeting: live transcription is behind; dropped {} segment(s) from the \
                         preview (the final pass still covers them)",
                        self.dropped
                    );
                }
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {}
        }
    }
}

/// Holds back a finished segment so a follow-up starting within the merge gap
/// joins it into one transcript block — up to `max_pending_samples`.
#[derive(Default)]
pub(super) struct SegmentMerger {
    pending: Option<(Vec<f32>, u64)>,
    /// Sample index where the pending segment's audio ended.
    pending_end: u64,
}

impl SegmentMerger {
    /// Offer the finished segment `[start, end)`. Returns a segment that is
    /// now complete and should be transcribed, if any.
    pub fn push(
        &mut self,
        audio: Vec<f32>,
        start: u64,
        end: u64,
        cfg: &SegConfig,
    ) -> Option<(Vec<f32>, u64)> {
        match self.pending.take() {
            Some((mut prev, prev_start)) => {
                let gap = start.saturating_sub(self.pending_end);
                let merged_len = prev.len() + gap as usize + audio.len();
                if gap <= cfg.merge_gap_samples && merged_len <= cfg.max_pending_samples {
                    prev.extend(std::iter::repeat_n(0.0, gap as usize));
                    prev.extend_from_slice(&audio);
                    self.pending = Some((prev, prev_start));
                    self.pending_end = end;
                    None
                } else {
                    self.pending = Some((audio, start));
                    self.pending_end = end;
                    Some((prev, prev_start))
                }
            }
            None => {
                self.pending = Some((audio, start));
                self.pending_end = end;
                None
            }
        }
    }

    /// Hand back whatever is still pending (end of session).
    pub fn take(&mut self) -> Option<(Vec<f32>, u64)> {
        self.pending.take()
    }
}

/// Live VAD segmentation state for ONE capture source (mic or system): its
/// own `SmoothedVad`, the in-progress segment and the merge state, producing
/// source-labeled segments.
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
    merger: SegmentMerger,
}

impl SourceProcessor {
    fn new(vad: crate::audio_toolkit::vad::SmoothedVad, source: TranscriptSource) -> Self {
        Self {
            vad,
            source,
            frame_accum: Vec::new(),
            segment: Vec::new(),
            total_samples: 0,
            segment_start: 0,
            merger: SegmentMerger::default(),
        }
    }

    /// Feed newly-captured 16 kHz mono samples for this source, segmenting
    /// them and queueing completed segments for live transcription.
    fn feed(&mut self, samples: &[f32], cfg: &SegConfig, session: &Session, queue: &mut LiveQueue) {
        use crate::audio_toolkit::vad::{VadFrame, VoiceActivityDetector};

        self.frame_accum.extend_from_slice(samples);
        let mut offset = 0;
        while self.frame_accum.len() - offset >= cfg.frame_samples {
            let frame = &self.frame_accum[offset..offset + cfg.frame_samples];
            offset += cfg.frame_samples;
            self.total_samples += cfg.frame_samples as u64;

            match self.vad.push_frame(frame) {
                Ok(VadFrame::Speech(speech)) => {
                    // Either source speaking resets the prolonged-silence timer.
                    session.note_speech();
                    if self.segment.is_empty() {
                        let prelen = speech.len() as u64;
                        self.segment_start = self.total_samples.saturating_sub(prelen);
                    }
                    self.segment.extend_from_slice(speech);
                    if self.segment.len() >= cfg.max_samples {
                        self.finish_current(cfg, session, queue);
                    }
                }
                Ok(VadFrame::Noise) => {
                    if !self.segment.is_empty() {
                        if self.segment.len() >= cfg.min_samples {
                            self.finish_current(cfg, session, queue);
                        } else {
                            self.segment.clear();
                        }
                    }
                }
                Err(e) => log::warn!("meeting VAD frame error [{:?}]: {}", self.source, e),
            }
        }
        self.frame_accum.drain(..offset);
    }

    fn finish_current(&mut self, cfg: &SegConfig, session: &Session, queue: &mut LiveQueue) {
        if self.segment.is_empty() {
            return;
        }
        let audio = std::mem::take(&mut self.segment);
        if let Some((ready, start)) =
            self.merger
                .push(audio, self.segment_start, self.total_samples, cfg)
        {
            queue.submit(session, ready, start, self.source);
        }
    }

    /// Flush any trailing in-progress + pending segment on teardown.
    fn finish(&mut self, cfg: &SegConfig, session: &Session, queue: &mut LiveQueue) {
        self.finish_current(cfg, session, queue);
        if let Some((audio, start)) = self.merger.take() {
            queue.submit(session, audio, start, self.source);
        }
    }
}

/// Sets a stop flag when dropped — including during a panic unwind — so a
/// crashed capture thread can never leave the mic stream or the system tap
/// running behind it.
struct StopOnDrop(Arc<AtomicBool>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// Report a capture-buffer write error to the UI once per session.
fn report_buffer_error(manager: &MeetingManager, session: &Session, writer: &mut RawF32Writer) {
    if let Some(error) = writer.take_new_error() {
        log::error!("meeting: capture buffer write failed: {}", error);
        if !session.buffer_error_reported.swap(true, Ordering::SeqCst) {
            manager.emit_error(&format!(
                "Recording to disk failed ({}). The rest of this meeting may not be saved.",
                error
            ));
        }
    }
}

impl MeetingManager {
    /// The capture + mix + VAD loop of one session.
    ///
    /// An independent cpal mic worker resamples to 16 kHz and forwards frames
    /// over a channel; the CoreAudio system-audio tap is resampled to 16 kHz;
    /// a `MeetingMixer` produces the mixed stream for the level meter and the
    /// playback buffer. Each source runs its own VAD to cut source-labeled
    /// segments for the live transcription.
    ///
    /// Returns `Err` when capture could not start (no mic, permission denied,
    /// tap failure, VAD init); the caller then aborts the session.
    pub(super) fn run_capture_loop(&self, session: &Arc<Session>) -> Result<(), String> {
        use crate::audio_toolkit::audio::{
            AudioVisualiser, FrameResampler, MeetingMixer, MixSource, SystemAudioCapture,
        };
        use crate::audio_toolkit::vad::SmoothedVad;
        use crate::audio_toolkit::SileroVad;
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        use futures_util::StreamExt;
        use tauri::Manager;

        // --- VAD setup (dedicated instances, NOT shared with dictation) ---
        const VAD_FRAME_SAMPLES: usize = (WHISPER_SAMPLE_RATE as usize * 30) / 1000; // 480 / 30 ms
                                                                                     // Pre-roll captured before speech onset (~450 ms). Same as dictation.
        const VAD_PREFILL_FRAMES: usize = 15;
        // Silence tail tolerated before a segment ends: 40 × 30 ms = 1200 ms
        // (dictation uses ~450 ms), so brief pauses stay in one segment.
        const VAD_HANGOVER_FRAMES: usize = 40;
        // Consecutive voice frames required to (re)enter speech.
        const VAD_ONSET_FRAMES: usize = 2;
        // Max samples per segment before a forced flush (~22 s).
        const MAX_SEGMENT_SAMPLES: usize = WHISPER_SAMPLE_RATE as usize * 22;
        // Minimum segment worth transcribing (~400 ms).
        const MIN_SEGMENT_SAMPLES: usize = (WHISPER_SAMPLE_RATE as usize * 2) / 5;
        // A segment starting within this gap of the previous one's END merges
        // into it (~800 ms).
        const SEGMENT_MERGE_GAP_SAMPLES: u64 = (WHISPER_SAMPLE_RATE as u64 * 4) / 5;
        // Cap on a merged segment (~30 s, whisper's native window).
        const MAX_PENDING_SAMPLES: usize = WHISPER_SAMPLE_RATE as usize * 30;

        // --- Live audio-level visualizer (SEPARATE from dictation mic-level) ---
        const VIS_BUCKETS: usize = 16;
        const VIS_WINDOW_SIZE: usize = 512;
        const WAVE_POINTS: usize = 96;
        const LEVEL_EMIT_INTERVAL: Duration = Duration::from_millis(50);
        // Bounded audio channels: generous (seconds of audio), and they only
        // fill if the capture thread stalls — which it no longer does, since
        // transcription moved to the session worker.
        const MIC_CHANNEL_FRAMES: usize = 512;
        const SYS_CHANNEL_BATCHES: usize = 512;
        // Push buffered capture bytes to the OS this often, so a crash or a
        // forced quit loses at most a few seconds of audio.
        const BUFFER_FLUSH_INTERVAL: Duration = Duration::from_secs(5);

        let mut visualizer = AudioVisualiser::new(
            WHISPER_SAMPLE_RATE,
            VIS_WINDOW_SIZE,
            VIS_BUCKETS,
            80.0,
            6000.0,
        );
        let mut level_accum: Vec<f32> = Vec::with_capacity(WHISPER_SAMPLE_RATE as usize / 10);
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
        let mut mic_proc = SourceProcessor::new(make_vad()?, TranscriptSource::Mic);
        let mut system_proc = SourceProcessor::new(make_vad()?, TranscriptSource::System);

        // --- Per-source full-audio buffer files ---
        // Recorded on the session BEFORE the files are created, so an abort
        // removes whatever was created.
        let buf_dir = super::buffers::buffer_dir(&self.app_handle);
        let stamp = format!("{}_{}", std::process::id(), session.started_at_ms);
        let buffers = SessionBuffers {
            mic: buf_dir.join(format!("fisilti_meeting_{}_mic.f32", stamp)),
            system: buf_dir.join(format!("fisilti_meeting_{}_sys.f32", stamp)),
            mixed: buf_dir.join(format!("fisilti_meeting_{}_mix.f32", stamp)),
        };
        *session.buffers.lock().unwrap() = Some(buffers.clone());
        let mut mic_buf_writer = RawF32Writer::create(&buffers.mic)
            .map_err(|e| format!("Failed to create mic buffer: {}", e))?;
        let mut system_buf_writer = RawF32Writer::create(&buffers.system)
            .map_err(|e| format!("Failed to create system buffer: {}", e))?;
        let mut mixed_buf_writer = RawF32Writer::create(&buffers.mixed)
            .map_err(|e| format!("Failed to create mixed buffer: {}", e))?;

        // --- Mic capture on a dedicated thread (independent cpal stream) ---
        let (mic_tx, mic_rx) = mpsc::sync_channel::<Vec<f32>>(MIC_CHANNEL_FRAMES);
        let mic_stop = Arc::new(AtomicBool::new(false));
        let _mic_stop_guard = StopOnDrop(mic_stop.clone());
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

            // The cpal callback must never block: a full channel drops the
            // chunk (the capture thread only falls this far behind if it is
            // wedged, and then audio is lost either way).
            let (raw_tx, raw_rx) = mpsc::sync_channel::<Vec<f32>>(256);

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
                            let _ = raw_tx.try_send(mono);
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
            let mut dropped: u64 = 0;
            let mut forward = |frame: &[f32]| {
                if mic_tx.try_send(frame.to_vec()).is_err() {
                    dropped += 1;
                }
            };

            loop {
                if mic_stop_worker.load(Ordering::Relaxed) {
                    break;
                }
                match raw_rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(raw) => resampler.push(&raw, &mut forward),
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
            resampler.finish(&mut forward);
            drop(stream);
            if dropped > 0 {
                log::warn!(
                    "meeting: dropped {} mic frame(s) while capture was behind",
                    dropped
                );
            }
        });

        // Wait for mic init (or fail).
        let mic_init = mic_init_rx
            .recv()
            .unwrap_or_else(|e| Err(format!("Mic worker died: {e}")));
        if let Err(e) = mic_init {
            mic_stop.store(true, Ordering::Relaxed);
            let _ = mic_handle.join();
            return Err(format!("Microphone unavailable: {e}"));
        }

        // --- System audio capture on Tauri's async runtime ---
        let sys_stream = match SystemAudioCapture::start() {
            Ok(s) => s,
            Err(e) => {
                mic_stop.store(true, Ordering::Relaxed);
                let _ = mic_handle.join();
                return Err(format!("System audio capture unavailable: {e}"));
            }
        };
        let sys_rate = sys_stream.sample_rate();
        // Live system sample-rate handle: the CoreAudio IO-proc updates this
        // when the device rate changes (e.g. AirPods switching profiles); the
        // resampler is rebuilt on change so the ratio stays correct.
        let sys_rate_handle = sys_stream.sample_rate_handle();
        let mut sys_resampler = FrameResampler::new(
            sys_rate as usize,
            WHISPER_SAMPLE_RATE as usize,
            Duration::from_millis(30),
        );
        let mut sys_resampler_rate = sys_rate;

        let (sys_tx, sys_rx) = mpsc::sync_channel::<Vec<f32>>(SYS_CHANNEL_BATCHES);
        let sys_stop = Arc::new(AtomicBool::new(false));
        let _sys_stop_guard = StopOnDrop(sys_stop.clone());
        let sys_stop_task = sys_stop.clone();
        let sys_task = tauri::async_runtime::spawn(async move {
            // A tick that fires even when the tap delivers nothing, so the
            // stop flag is always noticed: `stream.next().await` alone could
            // wait forever, and teardown blocks on this task.
            const TICK: Duration = Duration::from_millis(250);
            let mut stream = sys_stream;
            let mut batch: Vec<f32> = Vec::with_capacity(1024);
            let tick = tokio::time::sleep(TICK);
            tokio::pin!(tick);
            let mut dropped: u64 = 0;
            let mut send = |batch: &mut Vec<f32>| -> bool {
                match sys_tx.try_send(std::mem::take(batch)) {
                    Ok(()) => true,
                    Err(mpsc::TrySendError::Full(_)) => {
                        dropped += 1;
                        true
                    }
                    Err(mpsc::TrySendError::Disconnected(_)) => false,
                }
            };
            loop {
                if sys_stop_task.load(Ordering::Relaxed) {
                    break;
                }
                tokio::select! {
                    biased;
                    sample = stream.next() => match sample {
                        Some(s) => {
                            batch.push(s);
                            if batch.len() >= 1024 && !send(&mut batch) {
                                break;
                            }
                        }
                        None => break,
                    },
                    _ = &mut tick => {
                        tick.as_mut().reset(tokio::time::Instant::now() + TICK);
                        if !batch.is_empty() && !send(&mut batch) {
                            break;
                        }
                    }
                }
            }
            if !batch.is_empty() {
                let _ = sys_tx.try_send(batch);
            }
            drop(stream);
            if dropped > 0 {
                log::warn!(
                    "meeting: dropped {} system-audio batch(es) while capture was behind",
                    dropped
                );
            }
        });

        log::info!(
            "meeting: capture loop running at {} Hz (system in {} Hz)",
            WHISPER_SAMPLE_RATE,
            sys_rate
        );

        // CRASH-RECOVERY: both sources are up, so INSERT the in-progress row
        // now (status `recording`) with the buffer paths. Inserting earlier
        // left a phantom "interrupted" meeting whenever capture failed to start.
        self.insert_session_row(session, &buffers);

        // --- Session worker: live transcription + incremental persistence ---
        let (job_tx, job_rx) = mpsc::sync_channel::<LiveJob>(LIVE_QUEUE_CAPACITY);
        let mut queue = LiveQueue {
            tx: job_tx,
            live_pass: !self.meeting_model_is_cloud(),
            dropped: 0,
        };
        let session_worker = {
            let manager = self.clone();
            let session = session.clone();
            std::thread::spawn(move || manager.run_session_worker(&session, job_rx))
        };

        // --- Mixer (level meter + saved playback audio) ---
        let mut mixer = MeetingMixer::new();
        let mut mixed: Vec<f32> = Vec::new();
        let mut mic_frames: Vec<f32> = Vec::new();
        let mut sys_frames: Vec<f32> = Vec::new();

        let seg_cfg = SegConfig {
            frame_samples: VAD_FRAME_SAMPLES,
            min_samples: MIN_SEGMENT_SAMPLES,
            max_samples: MAX_SEGMENT_SAMPLES,
            merge_gap_samples: SEGMENT_MERGE_GAP_SAMPLES,
            max_pending_samples: MAX_PENDING_SAMPLES,
        };

        // --- Mic conditioning + echo mitigation, mic frames only ---
        // The output route is detected once at start; a mid-meeting headphone
        // plug/unplug isn't re-detected.
        let mut echo_duck =
            super::dsp::EchoDuck::new(crate::audio_toolkit::audio::detect_output_route());
        let mut mic_highpass =
            super::dsp::HighPass::new(super::dsp::MIC_HIGHPASS_HZ, WHISPER_SAMPLE_RATE as f32);
        let mut mic_norm = super::dsp::MicLoudnessNorm::new(WHISPER_SAMPLE_RATE);

        // --- Gemini Live streaming (opt-in) ---
        let live_sessions = self.start_live_sessions(session);

        // --- Prolonged-silence auto-end tracking ---
        const SILENCE_CHECK_INTERVAL: Duration = Duration::from_secs(1);
        const SILENCE_SETTINGS_REFRESH: Duration = Duration::from_secs(5);
        let mut silence_settings = crate::settings::get_settings(&self.app_handle);
        let mut last_silence_check = Instant::now();
        let mut last_settings_refresh = Instant::now();
        let mut last_buffer_flush = Instant::now();

        let mut sys_alive = true;
        let mut mic_alive = true;
        let mut capture_lost_reported = false;

        loop {
            if session.is_stopping() {
                break;
            }

            // Prolonged-silence auto-end: about once a second, ask to end the
            // meeting if no speech has been seen for the configured timeout.
            // `request_auto_end` is idempotent while a prompt is pending.
            if last_silence_check.elapsed() >= SILENCE_CHECK_INTERVAL {
                last_silence_check = Instant::now();
                if last_settings_refresh.elapsed() >= SILENCE_SETTINGS_REFRESH {
                    silence_settings = crate::settings::get_settings(&self.app_handle);
                    last_settings_refresh = Instant::now();
                }
                if silence_settings.meeting_auto_end {
                    let elapsed = session.silence_anchor.lock().unwrap().elapsed();
                    if super::manager::silence_exceeded(
                        elapsed,
                        silence_settings.meeting_silence_timeout_secs,
                    ) {
                        crate::meeting_detector::request_auto_end(
                            &self.app_handle,
                            crate::meeting_prompt::EndReason::Silence,
                        );
                    }
                }
            }

            if last_buffer_flush.elapsed() >= BUFFER_FLUSH_INTERVAL {
                last_buffer_flush = Instant::now();
                mic_buf_writer.flush();
                system_buf_writer.flush();
                mixed_buf_writer.flush();
            }

            mic_frames.clear();
            sys_frames.clear();

            // Rebuild the system resampler if the device rate changed.
            let live_sys_rate = sys_rate_handle.load(Ordering::Acquire);
            if live_sys_rate != 0 && live_sys_rate != sys_resampler_rate {
                log::info!(
                    "meeting: system sample rate changed {} -> {} Hz; rebuilding resampler",
                    sys_resampler_rate,
                    live_sys_rate
                );
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

            // Pace the loop on the system stream (blocks up to 100 ms); once
            // that is gone, on the mic.
            if sys_alive {
                match sys_rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(batch) => {
                        sys_resampler.push(&batch, &mut |frame: &[f32]| {
                            mixer.push(MixSource::System, frame);
                            sys_frames.extend_from_slice(frame);
                        });
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        // The tap ended (device change, buffer pressure). Used
                        // to end the whole session; the microphone is still
                        // worth recording.
                        sys_alive = false;
                        log::warn!("meeting: system audio stream ended; continuing mic-only");
                        self.emit_error(
                            "System audio capture stopped. Recording continues with the microphone only.",
                        );
                    }
                }
            } else if mic_alive {
                match mic_rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(frame) => {
                        mixer.push(MixSource::Microphone, &frame);
                        mic_frames.extend_from_slice(&frame);
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => mic_alive = false,
                }
            } else {
                if !capture_lost_reported {
                    capture_lost_reported = true;
                    log::error!("meeting: both audio sources ended");
                    self.emit_error(
                        "Audio capture stopped. Stop the meeting to save what was recorded.",
                    );
                }
                std::thread::sleep(Duration::from_millis(100));
            }

            // Drain any available mic frames (already 16 kHz mono).
            loop {
                match mic_rx.try_recv() {
                    Ok(frame) => {
                        mixer.push(MixSource::Microphone, &frame);
                        mic_frames.extend_from_slice(&frame);
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        if mic_alive {
                            mic_alive = false;
                            log::warn!("meeting: microphone stream ended");
                            if sys_alive {
                                self.emit_error(
                                    "Microphone capture stopped. Recording continues with system audio only.",
                                );
                            }
                        }
                        break;
                    }
                }
            }

            // --- Mic conditioning + echo mitigation ---
            // Order: echo duck -> high-pass -> loudness normalize, BEFORE the
            // buffer write + VAD feed, so the live pass and the finalize pass
            // both benefit. System audio and the saved mix are untouched.
            echo_duck.observe_system(&sys_frames);
            if !mic_frames.is_empty() {
                let ducked = echo_duck.apply(&mut mic_frames);
                if ducked {
                    // Remote audio is loud on speakers → the mic frames are
                    // (attenuated) echo of what the system tap already captured
                    // cleanly. ZERO them in the buffer: the finalize pass
                    // re-VADs the whole mic buffer with no knowledge of live
                    // ducking, so residual leakage would be transcribed again
                    // under "you". Silence keeps the buffer time-aligned.
                    // Double-talk tradeoff: a quiet local interjection over loud
                    // playback is sacrificed.
                    mic_frames.iter_mut().for_each(|s| *s = 0.0);
                    mic_buf_writer.write(&mic_frames);
                    // Feed the silence to the VAD too: it advances the mic's
                    // sample clock (skipping it made every later live
                    // timestamp drift early by the ducked time) and closes any
                    // segment the duck cut off.
                    mic_proc.feed(&mic_frames, &seg_cfg, session, &mut queue);
                } else {
                    mic_highpass.process(&mut mic_frames);
                    if let Some(norm) = mic_norm.as_mut() {
                        norm.process(&mut mic_frames);
                    }
                    mic_buf_writer.write(&mic_frames);
                    mic_proc.feed(&mic_frames, &seg_cfg, session, &mut queue);
                    if let Some(live) = &live_sessions {
                        live.mic.push_audio(&mic_frames);
                    }
                }
            }
            if !sys_frames.is_empty() {
                system_buf_writer.write(&sys_frames);
                system_proc.feed(&sys_frames, &seg_cfg, session, &mut queue);
                if let Some(live) = &live_sessions {
                    live.system.push_audio(&sys_frames);
                }
            }

            // --- Mixed stream: level meter + playback buffer ONLY ---
            mixer.drain_into(&mut mixed);
            if !mixed.is_empty() {
                mixed_buf_writer.write(&mixed);
                level_accum.extend_from_slice(&mixed);
                if let Some(bars) = visualizer.feed(&mixed) {
                    last_bars = bars;
                }
                if last_level_emit.elapsed() >= LEVEL_EMIT_INTERVAL {
                    let (wave, peak) = super::dsp::downsample_wave(&level_accum, WAVE_POINTS);
                    self.emit_audio_level(last_bars.clone(), wave, peak);
                    level_accum.clear();
                    last_level_emit = Instant::now();
                }
                mixed.clear();
            }

            for writer in [
                &mut mic_buf_writer,
                &mut system_buf_writer,
                &mut mixed_buf_writer,
            ] {
                report_buffer_error(self, session, writer);
            }
        }

        // --- Teardown: stop mic + system, flush resamplers + mixer + final ---
        // Close the live sessions first so the API flushes any in-flight turn
        // while the rest of the teardown runs.
        if let Some(live) = &live_sessions {
            self.finish_live_sessions(session, live);
        }
        // Take the strip down with the stream that fed it, before the (possibly
        // minutes-long) finalize pass, so it never outlives the meeting.
        self.clear_subtitles(session);
        mic_stop.store(true, Ordering::Relaxed);
        let _ = mic_handle.join();
        sys_stop.store(true, Ordering::Relaxed);
        // The async task tears down the CoreAudio tap on drop; wait for it,
        // bounded (its tick guarantees it notices the stop within ~250 ms).
        let joined = tauri::async_runtime::block_on(async {
            tokio::time::timeout(Duration::from_secs(3), sys_task).await
        });
        if joined.is_err() {
            log::warn!("meeting: system audio task did not stop in time");
        }

        // Drain trailing mic frames.
        let mut tail_mic: Vec<f32> = Vec::new();
        while let Ok(frame) = mic_rx.try_recv() {
            mixer.push(MixSource::Microphone, &frame);
            tail_mic.extend_from_slice(&frame);
        }
        if !tail_mic.is_empty() {
            // Same conditioning for the tail (no ducking: no fresh system RMS).
            mic_highpass.process(&mut tail_mic);
            if let Some(norm) = mic_norm.as_mut() {
                norm.process(&mut tail_mic);
            }
            mic_buf_writer.write(&tail_mic);
            mic_proc.feed(&tail_mic, &seg_cfg, session, &mut queue);
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
            system_buf_writer.write(&tail_sys);
            system_proc.feed(&tail_sys, &seg_cfg, session, &mut queue);
        }

        mixer.flush_into(&mut mixed);
        if !mixed.is_empty() {
            mixed_buf_writer.write(&mixed);
        }

        // Flush each source's final in-progress + pending segments.
        mic_proc.finish(&seg_cfg, session, &mut queue);
        system_proc.finish(&seg_cfg, session, &mut queue);

        // Ensure buffers are fully written to disk before stop() reads them.
        for writer in [
            &mut mic_buf_writer,
            &mut system_buf_writer,
            &mut mixed_buf_writer,
        ] {
            writer.flush();
            report_buffer_error(self, session, writer);
        }

        // Let the session worker finish the queued live segments (at most
        // LIVE_QUEUE_CAPACITY) and write the last incremental persist.
        drop(queue);
        if session_worker.join().is_err() {
            log::warn!("meeting: session worker panicked");
        }

        // One final flat level so the visualiser settles.
        self.emit_audio_level(vec![0.0; VIS_BUCKETS], vec![0.0; WAVE_POINTS], 0.0);

        Ok(())
    }

    /// The session worker: transcribes queued live segments and keeps the
    /// in-progress row current (incremental transcript persist + clock),
    /// until the capture loop drops the queue.
    fn run_session_worker(&self, session: &Arc<Session>, jobs: mpsc::Receiver<LiveJob>) {
        // Batch incremental writes: a busy meeting does one UPDATE every few
        // seconds rather than per segment. A crash loses at most this window.
        const PERSIST_INTERVAL: Duration = Duration::from_secs(3);
        // Keep the row's clock ticking even when no segments are produced
        // (cloud models skip the live pass entirely), so a crash never leaves
        // a long meeting looking like a zero-second one.
        const PROGRESS_CLOCK_INTERVAL: Duration = Duration::from_secs(5);

        let mut last_persist = Instant::now();
        let mut last_clock = Instant::now();
        loop {
            match jobs.recv_timeout(Duration::from_secs(1)) {
                Ok(job) => {
                    if !session.shutting_down.load(Ordering::Relaxed) {
                        self.transcribe_live_segment(session, job);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            if last_persist.elapsed() >= PERSIST_INTERVAL {
                last_persist = Instant::now();
                if session.persist_dirty.swap(false, Ordering::SeqCst) {
                    self.persist_incremental_now(session);
                }
            }
            if last_clock.elapsed() >= PROGRESS_CLOCK_INTERVAL {
                last_clock = Instant::now();
                self.update_progress_clock(session);
            }
        }
        if session.persist_dirty.swap(false, Ordering::SeqCst) {
            self.persist_incremental_now(session);
        }
    }

    /// Transcribe one live segment (the rough preview pass; the on-stop
    /// finalize replaces these with full-audio re-transcription).
    fn transcribe_live_segment(&self, session: &Session, job: LiveJob) {
        let timestamp_ms = job.start_samples.saturating_mul(1000) / WHISPER_SAMPLE_RATE as u64;
        match self.transcription_manager.transcribe_meeting(job.audio) {
            Ok(text) => {
                if !text.trim().is_empty() {
                    log::info!(
                        "meeting segment [{:?}] @ {}ms: {}",
                        job.source,
                        timestamp_ms,
                        text
                    );
                    self.push_segment_with(session, text, None, None, timestamp_ms, job.source);
                }
            }
            Err(e) => log::warn!("meeting segment transcription failed: {}", e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SegConfig, SegmentMerger};

    fn cfg() -> SegConfig {
        SegConfig {
            frame_samples: 480,
            min_samples: 100,
            max_samples: 1_000,
            merge_gap_samples: 50,
            max_pending_samples: 2_000,
        }
    }

    #[test]
    fn close_segments_merge_with_the_gap_filled_by_silence() {
        let mut merger = SegmentMerger::default();
        assert!(merger.push(vec![1.0; 100], 0, 100, &cfg()).is_none());
        // Starts 20 samples after the first ended: merged.
        assert!(merger.push(vec![1.0; 100], 120, 220, &cfg()).is_none());
        let (audio, start) = merger.take().expect("pending");
        assert_eq!(start, 0);
        assert_eq!(audio.len(), 220);
        assert!(audio[100..120].iter().all(|&s| s == 0.0));
    }

    #[test]
    fn a_distant_segment_releases_the_pending_one() {
        let mut merger = SegmentMerger::default();
        merger.push(vec![1.0; 100], 0, 100, &cfg());
        let (released, start) = merger
            .push(vec![2.0; 100], 500, 600, &cfg())
            .expect("first segment released");
        assert_eq!((released.len(), start), (100, 0));
        let (pending, start) = merger.take().expect("second pending");
        assert_eq!((pending[0], start), (2.0, 500));
    }

    #[test]
    fn a_continuous_talker_cannot_grow_the_pending_segment_without_bound() {
        // Every segment hits the force-flush length and the next starts
        // immediately — the shape of someone who never pauses.
        let cfg = cfg();
        let mut merger = SegmentMerger::default();
        let mut released = Vec::new();
        let mut start = 0u64;
        for _ in 0..20 {
            let end = start + cfg.max_samples as u64;
            if let Some((audio, _)) = merger.push(vec![1.0; cfg.max_samples], start, end, &cfg) {
                released.push(audio.len());
            }
            start = end;
        }
        assert!(
            !released.is_empty(),
            "segments must be released periodically"
        );
        assert!(released.iter().all(|&len| len <= cfg.max_pending_samples));
        let (pending, _) = merger.take().unwrap();
        assert!(pending.len() <= cfg.max_pending_samples);
    }
}
