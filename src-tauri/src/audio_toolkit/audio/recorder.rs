use std::{
    io::Error,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    time::{Duration, Instant},
};

use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    Device, Sample, SizedSample,
};

use crate::audio_toolkit::{
    audio::{AudioVisualiser, FrameResampler},
    constants,
    vad::{self, VadFrame},
    VoiceActivityDetector,
};

enum Cmd {
    Start,
    Stop(mpsc::Sender<StopOutcome>),
    Shutdown,
}

enum AudioChunk {
    Samples(Vec<f32>),
    EndOfStream,
}

/// What a recording produced, plus how it ended.
#[derive(Debug, Default)]
pub struct StopOutcome {
    /// 16 kHz mono samples, VAD-filtered.
    pub samples: Vec<f32>,
    /// The device stopped delivering audio before the stop (unplugged,
    /// Bluetooth profile switch, sleep): the recording is probably cut short.
    pub device_stalled: bool,
    /// The recording hit [`MAX_RECORDING_SAMPLES`] and the rest was dropped.
    pub truncated: bool,
}

/// How long the capture loop waits for audio before checking for commands.
/// Bounds how long a Stop/Shutdown can go unseen when the device has gone
/// silent; the old blocking `recv()` waited forever.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// No audio for this long while recording counts as a stalled device.
const DEVICE_STALL: Duration = Duration::from_secs(2);

/// Longest the caller of `stop()` waits for the capture thread to answer.
/// Normally it answers within a few hundred milliseconds; this only guards
/// against the thread itself being wedged.
const STOP_REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// Longest `close()` waits for the capture thread to exit before detaching
/// it. Dropping a stream on a vanished device can block inside the driver.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(3);

/// Upper bound on one recording: 30 minutes at 16 kHz (~115 MB of f32).
/// Past this the buffer would only grow until the app runs out of memory.
pub const MAX_RECORDING_SAMPLES: usize = constants::WHISPER_SAMPLE_RATE as usize * 60 * 30;

pub struct AudioRecorder {
    device: Option<Device>,
    cmd_tx: Option<mpsc::Sender<Cmd>>,
    worker_handle: Option<std::thread::JoinHandle<()>>,
    /// Fires when the worker thread has fully exited (stream dropped).
    worker_done: Option<mpsc::Receiver<()>>,
    vad: Option<Arc<Mutex<Box<dyn vad::VoiceActivityDetector>>>>,
    level_cb: Option<Arc<dyn Fn(Vec<f32>) + Send + Sync + 'static>>,
    frame_cb: Option<FrameCallback>,
    limit_cb: Option<LimitCallback>,
}

/// Observer for the 16 kHz mono frames as they are recorded.
type FrameCallback = Arc<dyn Fn(&[f32]) + Send + Sync + 'static>;

/// Called once per recording when it reaches [`MAX_RECORDING_SAMPLES`].
type LimitCallback = Arc<dyn Fn() + Send + Sync + 'static>;

impl AudioRecorder {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Ok(AudioRecorder {
            device: None,
            cmd_tx: None,
            worker_handle: None,
            worker_done: None,
            vad: None,
            level_cb: None,
            frame_cb: None,
            limit_cb: None,
        })
    }

    /// Called (on the capture thread, once per recording) when a recording
    /// reaches [`MAX_RECORDING_SAMPLES`]. Audio past the limit is dropped, so
    /// the owner should stop the recording. Must not block.
    pub fn with_limit_callback<F>(mut self, cb: F) -> Self
    where
        F: Fn() + Send + Sync + 'static,
    {
        self.limit_cb = Some(Arc::new(cb));
        self
    }

    pub fn with_vad(mut self, vad: Box<dyn VoiceActivityDetector>) -> Self {
        self.vad = Some(Arc::new(Mutex::new(vad)));
        self
    }

    pub fn with_level_callback<F>(mut self, cb: F) -> Self
    where
        F: Fn(Vec<f32>) + Send + Sync + 'static,
    {
        self.level_cb = Some(Arc::new(cb));
        self
    }

    /// Observe the recorded audio as it arrives: 16 kHz mono frames, already
    /// resampled and VAD-filtered — the exact samples that end up in the
    /// buffer `stop()` returns.
    ///
    /// This is what lets a streaming transcriber work on the audio while the
    /// user is still speaking, instead of waiting for the whole clip. The
    /// callback runs on the capture consumer thread, so it must not block.
    pub fn with_frame_callback<F>(mut self, cb: F) -> Self
    where
        F: Fn(&[f32]) + Send + Sync + 'static,
    {
        self.frame_cb = Some(Arc::new(cb));
        self
    }

    pub fn open(&mut self, device: Option<Device>) -> Result<(), Box<dyn std::error::Error>> {
        if self.worker_handle.is_some() {
            return Ok(()); // already open
        }

        let (sample_tx, sample_rx) = mpsc::channel::<AudioChunk>();
        let (cmd_tx, cmd_rx) = mpsc::channel::<Cmd>();
        let (init_tx, init_rx) = mpsc::sync_channel::<Result<(), String>>(1);

        let host = crate::audio_toolkit::get_cpal_host();
        let device = match device {
            Some(dev) => dev,
            None => host
                .default_input_device()
                .ok_or_else(|| Error::new(std::io::ErrorKind::NotFound, "No input device found"))?,
        };

        let thread_device = device.clone();
        let vad = self.vad.clone();
        // Move the optional level callback into the worker thread
        let level_cb = self.level_cb.clone();
        let frame_cb = self.frame_cb.clone();
        let limit_cb = self.limit_cb.clone();
        let (done_tx, done_rx) = mpsc::channel::<()>();

        let worker = std::thread::spawn(move || {
            // Dropped last (declared first), after the stream: signals close().
            let _done = DoneSignal(done_tx);
            let stop_flag = Arc::new(AtomicBool::new(false));
            let stop_flag_for_stream = stop_flag.clone();
            let init_result = (|| -> Result<(cpal::Stream, u32), String> {
                let config = AudioRecorder::get_preferred_config(&thread_device)
                    .map_err(|e| format!("Failed to fetch preferred config: {e}"))?;

                let sample_rate = config.sample_rate().0;
                let channels = config.channels() as usize;

                log::info!(
                    "Using device: {:?}\nSample rate: {}\nChannels: {}\nFormat: {:?}",
                    thread_device.name(),
                    sample_rate,
                    channels,
                    config.sample_format()
                );

                let stream = match config.sample_format() {
                    cpal::SampleFormat::U8 => AudioRecorder::build_stream::<u8>(
                        &thread_device,
                        &config,
                        sample_tx,
                        channels,
                        stop_flag_for_stream,
                    )
                    .map_err(|e| format!("Failed to build input stream: {e}"))?,
                    cpal::SampleFormat::I8 => AudioRecorder::build_stream::<i8>(
                        &thread_device,
                        &config,
                        sample_tx,
                        channels,
                        stop_flag_for_stream,
                    )
                    .map_err(|e| format!("Failed to build input stream: {e}"))?,
                    cpal::SampleFormat::I16 => AudioRecorder::build_stream::<i16>(
                        &thread_device,
                        &config,
                        sample_tx,
                        channels,
                        stop_flag_for_stream,
                    )
                    .map_err(|e| format!("Failed to build input stream: {e}"))?,
                    cpal::SampleFormat::I32 => AudioRecorder::build_stream::<i32>(
                        &thread_device,
                        &config,
                        sample_tx,
                        channels,
                        stop_flag_for_stream,
                    )
                    .map_err(|e| format!("Failed to build input stream: {e}"))?,
                    cpal::SampleFormat::F32 => AudioRecorder::build_stream::<f32>(
                        &thread_device,
                        &config,
                        sample_tx,
                        channels,
                        stop_flag_for_stream,
                    )
                    .map_err(|e| format!("Failed to build input stream: {e}"))?,
                    sample_format => {
                        return Err(format!("Unsupported sample format: {sample_format:?}"));
                    }
                };

                stream
                    .play()
                    .map_err(|e| format!("Failed to start microphone stream: {e}"))?;

                Ok((stream, sample_rate))
            })();

            match init_result {
                Ok((stream, sample_rate)) => {
                    let _ = init_tx.send(Ok(()));
                    // Keep the stream alive while we process samples.
                    run_consumer(
                        sample_rate,
                        vad,
                        sample_rx,
                        cmd_rx,
                        Callbacks {
                            level: level_cb,
                            frame: frame_cb,
                            limit: limit_cb,
                        },
                        stop_flag,
                    );
                    drop(stream);
                }
                Err(error_message) => {
                    log::error!("{error_message}");
                    let _ = init_tx.send(Err(error_message));
                }
            }
        });

        match init_rx.recv() {
            Ok(Ok(())) => {
                self.device = Some(device);
                self.cmd_tx = Some(cmd_tx);
                self.worker_handle = Some(worker);
                self.worker_done = Some(done_rx);
                Ok(())
            }
            Ok(Err(error_message)) => {
                let _ = worker.join();
                let kind = if is_microphone_access_denied(&error_message) {
                    std::io::ErrorKind::PermissionDenied
                } else {
                    std::io::ErrorKind::Other
                };
                Err(Box::new(Error::new(kind, error_message)))
            }
            Err(recv_error) => {
                let _ = worker.join();
                Err(Box::new(Error::new(
                    std::io::ErrorKind::Other,
                    format!("Failed to initialize microphone worker: {recv_error}"),
                )))
            }
        }
    }

    pub fn start(&self) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(tx) = &self.cmd_tx {
            tx.send(Cmd::Start)?;
        }
        Ok(())
    }

    pub fn stop(&self) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
        Ok(self.stop_with_outcome()?.samples)
    }

    /// Stop recording and return the samples along with how the recording
    /// ended. Never blocks for more than [`STOP_REPLY_TIMEOUT`], even when the
    /// device has stopped delivering audio.
    pub fn stop_with_outcome(&self) -> Result<StopOutcome, Box<dyn std::error::Error>> {
        let (resp_tx, resp_rx) = mpsc::channel();
        let Some(tx) = &self.cmd_tx else {
            return Err("Recorder is not open".into());
        };
        tx.send(Cmd::Stop(resp_tx))?;
        match resp_rx.recv_timeout(STOP_REPLY_TIMEOUT) {
            Ok(outcome) => Ok(outcome),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(Box::new(Error::new(
                std::io::ErrorKind::TimedOut,
                "The microphone did not respond to stop",
            ))),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(Box::new(Error::new(
                std::io::ErrorKind::BrokenPipe,
                "The microphone capture thread is gone",
            ))),
        }
    }

    pub fn close(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(tx) = self.cmd_tx.take() {
            let _ = tx.send(Cmd::Shutdown);
        }
        let done = self.worker_done.take();
        if let Some(h) = self.worker_handle.take() {
            // Join only once the worker has signalled it is finished; if the
            // driver wedges while the stream is dropped, detach the thread
            // rather than hang the caller (and every lock it holds) forever.
            let finished = match done {
                Some(rx) => !matches!(
                    rx.recv_timeout(CLOSE_TIMEOUT),
                    Err(mpsc::RecvTimeoutError::Timeout)
                ),
                None => true,
            };
            if finished {
                let _ = h.join();
            } else {
                log::warn!(
                    "Microphone thread did not exit within {:?}; detaching it",
                    CLOSE_TIMEOUT
                );
            }
        }
        self.device = None;
        Ok(())
    }

    fn build_stream<T>(
        device: &cpal::Device,
        config: &cpal::SupportedStreamConfig,
        sample_tx: mpsc::Sender<AudioChunk>,
        channels: usize,
        stop_flag: Arc<AtomicBool>,
    ) -> Result<cpal::Stream, cpal::BuildStreamError>
    where
        T: Sample + SizedSample + Send + 'static,
        f32: cpal::FromSample<T>,
    {
        let mut output_buffer = Vec::new();
        let mut eos_sent = false;

        let stream_cb = move |data: &[T], _: &cpal::InputCallbackInfo| {
            if stop_flag.load(Ordering::Relaxed) {
                if !eos_sent {
                    let _ = sample_tx.send(AudioChunk::EndOfStream);
                    eos_sent = true;
                }
                return;
            }
            eos_sent = false;

            output_buffer.clear();

            if channels == 1 {
                output_buffer.extend(data.iter().map(|&sample| sample.to_sample::<f32>()));
            } else {
                let frame_count = data.len() / channels;
                output_buffer.reserve(frame_count);

                for frame in data.chunks_exact(channels) {
                    let mono_sample = frame
                        .iter()
                        .map(|&sample| sample.to_sample::<f32>())
                        .sum::<f32>()
                        / channels as f32;
                    output_buffer.push(mono_sample);
                }
            }

            if sample_tx
                .send(AudioChunk::Samples(output_buffer.clone()))
                .is_err()
            {
                log::error!("Failed to send samples");
            }
        };

        device.build_input_stream(
            &config.clone().into(),
            stream_cb,
            |err| log::error!("Stream error: {}", err),
            None,
        )
    }

    fn get_preferred_config(
        device: &cpal::Device,
    ) -> Result<cpal::SupportedStreamConfig, Box<dyn std::error::Error>> {
        // Use the device's native/default sample rate and let the FrameResampler
        // in run_consumer() downsample to 16kHz. This avoids forcing hardware into
        // a non-native rate which can cause issues on some devices (Bluetooth
        // codecs, certain ALSA drivers, etc.).
        let default_config = device.default_input_config()?;
        let target_rate = default_config.sample_rate();

        // Try to find the best sample format at the device's default rate
        let supported_configs = match device.supported_input_configs() {
            Ok(configs) => configs,
            Err(e) => {
                log::warn!("Could not enumerate input configs ({e}), using device default");
                return Ok(default_config);
            }
        };
        let mut best_config: Option<cpal::SupportedStreamConfigRange> = None;

        for config_range in supported_configs {
            if config_range.min_sample_rate() <= target_rate
                && config_range.max_sample_rate() >= target_rate
            {
                match best_config {
                    None => best_config = Some(config_range),
                    Some(ref current) => {
                        // Prioritize F32 > I16 > I32 > others
                        let score = |fmt: cpal::SampleFormat| match fmt {
                            cpal::SampleFormat::F32 => 4,
                            cpal::SampleFormat::I16 => 3,
                            cpal::SampleFormat::I32 => 2,
                            _ => 1,
                        };

                        if score(config_range.sample_format()) > score(current.sample_format()) {
                            best_config = Some(config_range);
                        }
                    }
                }
            }
        }

        if let Some(config) = best_config {
            return Ok(config.with_sample_rate(target_rate));
        }

        // Fall back to device default if no config matched (exotic/virtual devices)
        log::warn!(
            "No supported config matched device default rate {:?}, using default config",
            target_rate
        );
        Ok(default_config)
    }
}

pub fn is_microphone_access_denied(error_message: &str) -> bool {
    let normalized = error_message.to_lowercase();
    normalized.contains("access is denied")
        || normalized.contains("permission denied")
        || normalized.contains("0x80070005")
}

#[cfg(test)]
mod tests {
    use super::is_microphone_access_denied;

    #[test]
    fn detects_access_is_denied() {
        assert!(is_microphone_access_denied("Access is denied"));
    }

    #[test]
    fn detects_permission_denied() {
        assert!(is_microphone_access_denied("permission denied"));
    }

    #[test]
    fn detects_windows_error_code() {
        assert!(is_microphone_access_denied("WASAPI error: 0x80070005"));
    }

    #[test]
    fn does_not_match_unrelated_errors() {
        assert!(!is_microphone_access_denied("device not found"));
    }
}

/// Sends on drop, so `close()` learns the worker thread has finished.
struct DoneSignal(mpsc::Sender<()>);

impl Drop for DoneSignal {
    fn drop(&mut self) {
        let _ = self.0.send(());
    }
}

struct Callbacks {
    level: Option<Arc<dyn Fn(Vec<f32>) + Send + Sync + 'static>>,
    frame: Option<FrameCallback>,
    limit: Option<LimitCallback>,
}

/// The recording buffer, capped at [`MAX_RECORDING_SAMPLES`].
struct RecordingBuffer {
    samples: Vec<f32>,
    cap: usize,
    truncated: bool,
}

impl RecordingBuffer {
    fn new(cap: usize) -> Self {
        Self {
            samples: Vec::new(),
            cap,
            truncated: false,
        }
    }

    fn reset(&mut self) {
        self.samples.clear();
        self.truncated = false;
    }

    /// Append as much of `buf` as fits. Returns the part that was kept, and
    /// whether this call is the one that hit the cap.
    fn push<'a>(&mut self, buf: &'a [f32]) -> (&'a [f32], bool) {
        let room = self.cap.saturating_sub(self.samples.len());
        let kept = &buf[..buf.len().min(room)];
        self.samples.extend_from_slice(kept);
        let just_hit = !self.truncated && kept.len() < buf.len();
        if kept.len() < buf.len() {
            self.truncated = true;
        }
        (kept, just_hit)
    }
}

fn run_consumer(
    in_sample_rate: u32,
    vad: Option<Arc<Mutex<Box<dyn vad::VoiceActivityDetector>>>>,
    sample_rx: mpsc::Receiver<AudioChunk>,
    cmd_rx: mpsc::Receiver<Cmd>,
    callbacks: Callbacks,
    stop_flag: Arc<AtomicBool>,
) {
    let Callbacks {
        level: level_cb,
        frame: frame_cb,
        limit: limit_cb,
    } = callbacks;

    let mut frame_resampler = FrameResampler::new(
        in_sample_rate as usize,
        constants::WHISPER_SAMPLE_RATE as usize,
        Duration::from_millis(30),
    );

    let mut buffer = RecordingBuffer::new(MAX_RECORDING_SAMPLES);
    let mut recording = false;
    let mut last_chunk_at = Instant::now();
    let mut stream_closed = false;

    // ---------- spectrum visualisation setup ---------------------------- //
    const BUCKETS: usize = 16;
    const WINDOW_SIZE: usize = 512;
    let mut visualizer = AudioVisualiser::new(
        in_sample_rate,
        WINDOW_SIZE,
        BUCKETS,
        400.0,  // vocal_min_hz
        4000.0, // vocal_max_hz
    );

    fn handle_frame(
        samples: &[f32],
        recording: bool,
        vad: &Option<Arc<Mutex<Box<dyn vad::VoiceActivityDetector>>>>,
        out_buf: &mut RecordingBuffer,
        frame_cb: &Option<FrameCallback>,
        limit_cb: &Option<LimitCallback>,
    ) {
        if !recording {
            return;
        }

        // Whatever is appended to the buffer is also handed to the observer, so
        // a streaming transcriber and the final buffer can never disagree about
        // what was recorded.
        let mut keep = |buf: &[f32]| {
            let (kept, just_hit_limit) = out_buf.push(buf);
            if !kept.is_empty() {
                if let Some(cb) = frame_cb {
                    cb(kept);
                }
            }
            if just_hit_limit {
                log::warn!(
                    "Recording reached the {}-sample limit; further audio is dropped",
                    MAX_RECORDING_SAMPLES
                );
                if let Some(cb) = limit_cb {
                    cb();
                }
            }
        };

        if let Some(vad_arc) = vad {
            let mut det = vad_arc.lock().unwrap_or_else(|e| e.into_inner());
            match det.push_frame(samples).unwrap_or(VadFrame::Speech(samples)) {
                VadFrame::Speech(buf) => keep(buf),
                VadFrame::Noise => {}
            }
        } else {
            keep(samples);
        }
    }

    loop {
        // Bounded wait: commands must be seen even when the device delivers
        // nothing at all (unplugged, Bluetooth profile switch, sleep).
        if !stream_closed {
            match sample_rx.recv_timeout(POLL_INTERVAL) {
                Ok(AudioChunk::Samples(raw)) => {
                    last_chunk_at = Instant::now();

                    // ---------- spectrum processing ---------------------- //
                    if let Some(buckets) = visualizer.feed(&raw) {
                        if let Some(cb) = &level_cb {
                            cb(buckets);
                        }
                    }

                    // ---------- existing pipeline ------------------------ //
                    frame_resampler.push(&raw, &mut |frame: &[f32]| {
                        handle_frame(frame, recording, &vad, &mut buffer, &frame_cb, &limit_cb)
                    });
                }
                Ok(AudioChunk::EndOfStream) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    // The stream is gone, but commands still need answering.
                    log::warn!("Microphone stream closed unexpectedly");
                    stream_closed = true;
                }
            }
        } else {
            std::thread::sleep(POLL_INTERVAL);
        }

        // non-blocking check for commands
        loop {
            let cmd = match cmd_rx.try_recv() {
                Ok(cmd) => cmd,
                Err(mpsc::TryRecvError::Empty) => break,
                // The owner went away without a Shutdown (recorder dropped).
                Err(mpsc::TryRecvError::Disconnected) => {
                    stop_flag.store(true, Ordering::Relaxed);
                    return;
                }
            };
            match cmd {
                Cmd::Start => {
                    stop_flag.store(false, Ordering::Relaxed);
                    buffer.reset();
                    recording = true;
                    // A fresh recording gets a fresh stall window.
                    last_chunk_at = Instant::now();
                    visualizer.reset();
                    if let Some(v) = &vad {
                        v.lock().unwrap_or_else(|e| e.into_inner()).reset();
                    }
                }
                Cmd::Stop(reply_tx) => {
                    let was_recording = recording;
                    recording = false;
                    stop_flag.store(true, Ordering::Relaxed);

                    let device_stalled =
                        was_recording && (stream_closed || last_chunk_at.elapsed() >= DEVICE_STALL);

                    // Drain all remaining audio until the producer confirms end-of-stream.
                    // The cpal callback sees the stop flag, sends EndOfStream, and goes
                    // silent — guaranteeing every captured sample is in the channel
                    // ahead of the sentinel. A stalled device will never send it,
                    // so don't wait long for one.
                    let drain_timeout = if device_stalled {
                        Duration::from_millis(100)
                    } else {
                        Duration::from_secs(2)
                    };
                    while !stream_closed {
                        match sample_rx.recv_timeout(drain_timeout) {
                            Ok(AudioChunk::Samples(remaining)) => {
                                frame_resampler.push(&remaining, &mut |frame: &[f32]| {
                                    handle_frame(
                                        frame,
                                        true,
                                        &vad,
                                        &mut buffer,
                                        &frame_cb,
                                        &limit_cb,
                                    )
                                });
                            }
                            Ok(AudioChunk::EndOfStream) => break,
                            Err(mpsc::RecvTimeoutError::Timeout) => {
                                if !device_stalled {
                                    log::warn!(
                                        "Timed out waiting for EndOfStream from audio callback"
                                    );
                                }
                                break;
                            }
                            Err(mpsc::RecvTimeoutError::Disconnected) => {
                                stream_closed = true;
                            }
                        }
                    }

                    frame_resampler.finish(&mut |frame: &[f32]| {
                        handle_frame(frame, true, &vad, &mut buffer, &frame_cb, &limit_cb)
                    });

                    if device_stalled {
                        log::warn!(
                            "Microphone delivered no audio for {:?} before stop",
                            last_chunk_at.elapsed()
                        );
                    }
                    let _ = reply_tx.send(StopOutcome {
                        samples: std::mem::take(&mut buffer.samples),
                        device_stalled,
                        truncated: buffer.truncated,
                    });
                    buffer.reset();

                    // Resume the audio callback so the consumer loop can continue
                    // receiving chunks (important for always-on microphone mode).
                    stop_flag.store(false, Ordering::Relaxed);
                }
                Cmd::Shutdown => {
                    stop_flag.store(true, Ordering::Relaxed);
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod buffer_tests {
    use super::RecordingBuffer;

    #[test]
    fn keeps_everything_under_the_cap() {
        let mut buf = RecordingBuffer::new(10);
        let (kept, hit) = buf.push(&[1.0; 4]);
        assert_eq!(kept.len(), 4);
        assert!(!hit);
        assert!(!buf.truncated);
    }

    #[test]
    fn reports_the_limit_exactly_once() {
        let mut buf = RecordingBuffer::new(10);
        buf.push(&[1.0; 8]);
        let (kept, hit) = buf.push(&[1.0; 5]);
        assert_eq!(kept.len(), 2);
        assert!(hit);
        let (kept, hit) = buf.push(&[1.0; 5]);
        assert!(kept.is_empty());
        assert!(!hit);
        assert_eq!(buf.samples.len(), 10);
        assert!(buf.truncated);
    }

    #[test]
    fn reset_rearms_the_limit() {
        let mut buf = RecordingBuffer::new(2);
        buf.push(&[1.0; 3]);
        buf.reset();
        assert!(!buf.truncated);
        let (_, hit) = buf.push(&[1.0; 3]);
        assert!(hit);
    }
}
