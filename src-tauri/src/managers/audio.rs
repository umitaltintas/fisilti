use crate::audio_toolkit::audio::MAX_RECORDING_SAMPLES;
use crate::audio_toolkit::{
    is_microphone_access_denied, list_input_devices, vad::SmoothedVad, AudioRecorder, SileroVad,
};
use crate::helpers::clamshell;
use crate::settings::{get_settings, AppSettings};
use crate::utils;
use log::{debug, error, info};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager};

const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

fn set_mute(mute: bool) {
    // Expected behavior:
    // - Windows: works on most systems using standard audio drivers.
    // - Linux: works on many systems (PipeWire, PulseAudio, ALSA),
    //   but some distros may lack the tools used.
    // - macOS: works on most standard setups via AppleScript.
    // If unsupported, fails silently.

    #[cfg(target_os = "windows")]
    {
        unsafe {
            use windows::Win32::{
                Media::Audio::{
                    eMultimedia, eRender, Endpoints::IAudioEndpointVolume, IMMDeviceEnumerator,
                    MMDeviceEnumerator,
                },
                System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED},
            };

            macro_rules! unwrap_or_return {
                ($expr:expr) => {
                    match $expr {
                        Ok(val) => val,
                        Err(_) => return,
                    }
                };
            }

            // Initialize the COM library for this thread.
            // If already initialized (e.g., by another library like Tauri), this does nothing.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

            let all_devices: IMMDeviceEnumerator =
                unwrap_or_return!(CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL));
            let default_device =
                unwrap_or_return!(all_devices.GetDefaultAudioEndpoint(eRender, eMultimedia));
            let volume_interface = unwrap_or_return!(
                default_device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)
            );

            let _ = volume_interface.SetMute(mute, std::ptr::null());
        }
    }

    #[cfg(target_os = "linux")]
    {
        use std::process::Command;

        let mute_val = if mute { "1" } else { "0" };
        let amixer_state = if mute { "mute" } else { "unmute" };

        // Try multiple backends to increase compatibility
        // 1. PipeWire (wpctl)
        if Command::new("wpctl")
            .args(["set-mute", "@DEFAULT_AUDIO_SINK@", mute_val])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return;
        }

        // 2. PulseAudio (pactl)
        if Command::new("pactl")
            .args(["set-sink-mute", "@DEFAULT_SINK@", mute_val])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return;
        }

        // 3. ALSA (amixer)
        let _ = Command::new("amixer")
            .args(["set", "Master", amixer_state])
            .output();
    }

    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        let script = format!(
            "set volume output muted {}",
            if mute { "true" } else { "false" }
        );
        let _ = Command::new("osascript").args(["-e", &script]).output();
    }
}

/// Whether the default output device is currently muted, where that can be
/// read. `None` means unknown, in which case muting proceeds as before.
fn is_output_muted() -> Option<bool> {
    #[cfg(target_os = "windows")]
    {
        unsafe {
            use windows::Win32::{
                Media::Audio::{
                    eMultimedia, eRender, Endpoints::IAudioEndpointVolume, IMMDeviceEnumerator,
                    MMDeviceEnumerator,
                },
                System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED},
            };

            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let all_devices: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
            let default_device = all_devices
                .GetDefaultAudioEndpoint(eRender, eMultimedia)
                .ok()?;
            let volume_interface = default_device
                .Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)
                .ok()?;
            return volume_interface.GetMute().ok().map(|m| m.as_bool());
        }
    }

    #[cfg(target_os = "linux")]
    {
        use std::process::Command;

        let run = |cmd: &str, args: &[&str]| -> Option<String> {
            let out = Command::new(cmd).args(args).output().ok()?;
            out.status
                .success()
                .then(|| String::from_utf8_lossy(&out.stdout).to_string())
        };
        if let Some(out) = run("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"]) {
            return Some(out.contains("[MUTED]"));
        }
        if let Some(out) = run("pactl", &["get-sink-mute", "@DEFAULT_SINK@"]) {
            return Some(out.to_lowercase().contains("yes"));
        }
        if let Some(out) = run("amixer", &["get", "Master"]) {
            return Some(out.contains("[off]"));
        }
        return None;
    }

    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        let out = Command::new("osascript")
            .args(["-e", "output muted of (get volume settings)"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        return match String::from_utf8_lossy(&out.stdout).trim() {
            "true" => Some(true),
            "false" => Some(false),
            // "missing value": the output device has no mute control.
            _ => None,
        };
    }

    #[allow(unreachable_code)]
    None
}

const WHISPER_SAMPLE_RATE: usize = 16000;

/* ──────────────────────────────────────────────────────────────── */

#[derive(Clone, Debug)]
pub enum RecordingState {
    Idle,
    Recording { binding_id: String },
}

#[derive(Clone, Debug)]
pub enum MicrophoneMode {
    AlwaysOn,
    OnDemand,
}

/// Microphone stream + mute bookkeeping. One mutex for all of it: these used
/// to be three separate mutexes taken in different orders by different paths
/// (`apply_mute`: mute→open; stream start/stop: open→mute), which could
/// deadlock.
///
/// Lock order, everywhere: `state` → `mic` → `recorder`. Never take `mic`
/// while holding `recorder`.
#[derive(Debug, Default)]
struct MicState {
    is_open: bool,
    is_recording: bool,
    /// We muted the output and owe an unmute.
    did_mute: bool,
    /// Bumped on every recording start. A delayed `apply_mute` carries the
    /// session it was scheduled for and does nothing if that recording has
    /// already ended — otherwise it could mute the system after the stop
    /// (and nothing would ever unmute it).
    session: u64,
}

/// Whether a mute requested for `requested_session` should be applied now.
fn mute_applies(mic: &MicState, requested_session: u64, enabled: bool) -> bool {
    enabled && mic.is_open && mic.is_recording && mic.session == requested_session && !mic.did_mute
}

/* ──────────────────────────────────────────────────────────────── */

/// A sink that receives recorded audio as it arrives. Installed for the
/// duration of one recording by a streaming transcriber; `None` the rest of the
/// time, which is the normal buffer-then-transcribe path.
pub type FrameSink = Arc<dyn Fn(&[f32]) + Send + Sync + 'static>;

/// Payload of the `recording-error` event (same shape actions.rs emits).
#[derive(Clone, serde::Serialize)]
struct RecordingErrorEvent {
    error_type: String,
    detail: Option<String>,
}

fn create_audio_recorder(
    vad_path: &str,
    app_handle: &tauri::AppHandle,
    frame_sink: Arc<Mutex<Option<FrameSink>>>,
) -> Result<AudioRecorder, anyhow::Error> {
    let silero = SileroVad::new(vad_path, 0.3)
        .map_err(|e| anyhow::anyhow!("Failed to create SileroVad: {}", e))?;
    let smoothed_vad = SmoothedVad::new(Box::new(silero), 15, 15, 2);

    // Recorder with VAD plus a spectrum-level callback that forwards updates to
    // the frontend.
    let recorder = AudioRecorder::new()
        .map_err(|e| anyhow::anyhow!("Failed to create AudioRecorder: {}", e))?
        .with_vad(Box::new(smoothed_vad))
        .with_level_callback({
            let app_handle = app_handle.clone();
            move |levels| {
                utils::emit_levels(&app_handle, &levels);
            }
        })
        // Cloned out of the lock before calling, so a slow sink cannot hold the
        // capture thread's mutex while it works.
        .with_frame_callback(move |frames| {
            let sink = lock(&frame_sink).clone();
            if let Some(sink) = sink {
                sink(frames);
            }
        })
        // Runs on the capture thread: hand the stop to another thread so the
        // capture loop is free to answer it.
        .with_limit_callback({
            let app_handle = app_handle.clone();
            move || {
                let app = app_handle.clone();
                std::thread::spawn(move || stop_at_length_limit(&app));
            }
        });

    Ok(recorder)
}

/// A recording hit the length cap: tell the user and stop it through the
/// coordinator, exactly as a key press would, so the audio captured so far is
/// still transcribed.
fn stop_at_length_limit(app: &tauri::AppHandle) {
    let Some(rm) = app.try_state::<Arc<AudioRecordingManager>>() else {
        return;
    };
    let binding_id = match &*lock(&rm.state) {
        RecordingState::Recording { binding_id } => binding_id.clone(),
        RecordingState::Idle => return,
    };
    utils::emit_dictation_error(
        app,
        utils::DictationStage::Recording,
        format!(
            "Recording reached the {}-minute limit and was stopped.",
            MAX_RECORDING_SAMPLES / WHISPER_SAMPLE_RATE / 60
        ),
    );
    crate::signal_handle::send_transcription_input(app, &binding_id, "length-limit");
}

/// Lock a mutex, recovering the data if a previous holder panicked: every
/// value guarded here is a flag or handle that stays meaningful after a panic,
/// and refusing to continue would leave dictation dead until restart.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/* ──────────────────────────────────────────────────────────────── */

#[derive(Clone)]
pub struct AudioRecordingManager {
    state: Arc<Mutex<RecordingState>>,
    mode: Arc<Mutex<MicrophoneMode>>,
    app_handle: tauri::AppHandle,

    recorder: Arc<Mutex<Option<AudioRecorder>>>,
    frame_sink: Arc<Mutex<Option<FrameSink>>>,
    mic: Arc<Mutex<MicState>>,
    close_generation: Arc<AtomicU64>,
}

/// How a stopped recording ended, beyond its samples.
#[derive(Debug, Default, Clone, Copy)]
pub struct RecordingDiagnostics {
    /// The microphone stopped delivering audio (unplugged, Bluetooth switch,
    /// sleep) or did not answer the stop; the audio is probably incomplete.
    pub device_failed: bool,
    /// The recording hit the length cap and was cut.
    pub truncated: bool,
}

impl AudioRecordingManager {
    /* ---------- construction ------------------------------------------------ */

    /// Never fails: an always-on microphone that cannot be opened at launch
    /// (permission revoked, device gone) is reported and left closed, and the
    /// next recording tries again. Failing here used to abort startup on every
    /// launch.
    pub fn new(app: &tauri::AppHandle) -> Self {
        let settings = get_settings(app);
        let mode = if settings.always_on_microphone {
            MicrophoneMode::AlwaysOn
        } else {
            MicrophoneMode::OnDemand
        };

        let manager = Self {
            state: Arc::new(Mutex::new(RecordingState::Idle)),
            mode: Arc::new(Mutex::new(mode.clone())),
            app_handle: app.clone(),

            recorder: Arc::new(Mutex::new(None)),
            frame_sink: Arc::new(Mutex::new(None)),
            mic: Arc::new(Mutex::new(MicState::default())),
            close_generation: Arc::new(AtomicU64::new(0)),
        };

        // Always-on?  Open immediately.
        if matches!(mode, MicrophoneMode::AlwaysOn) {
            if let Err(e) = manager.start_microphone_stream() {
                let detail = e.to_string();
                error!("Could not open the always-on microphone at launch: {detail}");
                let error_type = if is_microphone_access_denied(&detail) {
                    "microphone_permission_denied"
                } else {
                    "unknown"
                };
                let _ = app.emit(
                    "recording-error",
                    RecordingErrorEvent {
                        error_type: error_type.to_string(),
                        detail: Some(detail),
                    },
                );
            }
        }

        manager
    }

    /* ---------- helper methods --------------------------------------------- */

    fn get_effective_microphone_device(&self, settings: &AppSettings) -> Option<cpal::Device> {
        // Check if we're in clamshell mode and have a clamshell microphone configured
        let use_clamshell_mic = if let Ok(is_clamshell) = clamshell::is_clamshell() {
            is_clamshell && settings.clamshell_microphone.is_some()
        } else {
            false
        };

        let device_name = if use_clamshell_mic {
            settings.clamshell_microphone.as_ref()?
        } else {
            settings.selected_microphone.as_ref()?
        };

        // Find the device by name
        match list_input_devices() {
            Ok(devices) => devices
                .into_iter()
                .find(|d| d.name == *device_name)
                .map(|d| d.device),
            Err(e) => {
                debug!("Failed to list devices, using default: {}", e);
                None
            }
        }
    }

    fn schedule_lazy_close(&self) {
        let gen = self.close_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let app = self.app_handle.clone();
        std::thread::spawn(move || {
            std::thread::sleep(STREAM_IDLE_TIMEOUT);
            let rm = app.state::<Arc<AudioRecordingManager>>();
            // Hold state lock across the check AND close to serialize against
            // try_start_recording, preventing a race where the stream is closed
            // under an active recording.
            let state = lock(&rm.state);
            if rm.close_generation.load(Ordering::SeqCst) == gen
                && matches!(*state, RecordingState::Idle)
            {
                // stop_microphone_stream takes mic → recorder, which is the
                // documented order after state: no deadlock.
                info!(
                    "Closing idle microphone stream after {:?}",
                    STREAM_IDLE_TIMEOUT
                );
                rm.stop_microphone_stream();
            }
        });
    }

    /// Close the stream after a recording ended (lazily if configured), in
    /// on-demand mode only.
    fn release_stream_after_recording(&self) {
        if matches!(*lock(&self.mode), MicrophoneMode::OnDemand) {
            if get_settings(&self.app_handle).lazy_stream_close {
                self.schedule_lazy_close();
            } else {
                self.stop_microphone_stream();
            }
        }
    }

    /* ---------- microphone life-cycle -------------------------------------- */

    /// The current recording session, to hand to a delayed [`Self::apply_mute`].
    pub fn current_session(&self) -> u64 {
        lock(&self.mic).session
    }

    /// Mute the output for recording `session`, if `mute_while_recording` is
    /// on, that recording is still running, and the output was not already
    /// muted by the user (in which case it must also stay muted afterwards).
    pub fn apply_mute(&self, session: u64) {
        let enabled = get_settings(&self.app_handle).mute_while_recording;
        let mut mic = lock(&self.mic);
        if !mute_applies(&mic, session, enabled) {
            return;
        }
        if is_output_muted() == Some(true) {
            debug!("Output already muted by the user; leaving it alone");
            return;
        }
        set_mute(true);
        mic.did_mute = true;
        debug!("Mute applied");
    }

    /// Removes mute if it was applied
    pub fn remove_mute(&self) {
        let mut mic = lock(&self.mic);
        if mic.did_mute {
            set_mute(false);
            mic.did_mute = false;
            debug!("Mute removed");
        }
    }

    pub fn start_microphone_stream(&self) -> Result<(), anyhow::Error> {
        let mut mic = lock(&self.mic);
        if mic.is_open {
            debug!("Microphone stream already active");
            return Ok(());
        }

        let start_time = Instant::now();

        let vad_path = self
            .app_handle
            .path()
            .resolve(
                "resources/models/silero_vad_v4.onnx",
                tauri::path::BaseDirectory::Resource,
            )
            .map_err(|e| anyhow::anyhow!("Failed to resolve VAD path: {}", e))?;
        let mut recorder_opt = lock(&self.recorder);

        if recorder_opt.is_none() {
            *recorder_opt = Some(create_audio_recorder(
                &vad_path.to_string_lossy(),
                &self.app_handle,
                self.frame_sink.clone(),
            )?);
        }

        // Get the selected device from settings, considering clamshell mode
        let settings = get_settings(&self.app_handle);
        let selected_device = self.get_effective_microphone_device(&settings);

        if let Some(rec) = recorder_opt.as_mut() {
            rec.open(selected_device)
                .map_err(|e| anyhow::anyhow!("Failed to open recorder: {}", e))?;
        }

        mic.is_open = true;
        info!(
            "Microphone stream initialized in {:?}",
            start_time.elapsed()
        );
        Ok(())
    }

    pub fn stop_microphone_stream(&self) {
        let mut mic = lock(&self.mic);
        if !mic.is_open {
            return;
        }

        if mic.did_mute {
            set_mute(false);
            mic.did_mute = false;
        }

        if let Some(rec) = lock(&self.recorder).as_mut() {
            // If still recording, stop first.
            if mic.is_recording {
                let _ = rec.stop();
                mic.is_recording = false;
            }
            let _ = rec.close();
        }

        mic.is_open = false;
        debug!("Microphone stream stopped");
    }

    /* ---------- mode switching --------------------------------------------- */

    pub fn update_mode(&self, new_mode: MicrophoneMode) -> Result<(), anyhow::Error> {
        let cur_mode = lock(&self.mode).clone();

        match (cur_mode, &new_mode) {
            (MicrophoneMode::AlwaysOn, MicrophoneMode::OnDemand) => {
                if matches!(*lock(&self.state), RecordingState::Idle) {
                    self.close_generation.fetch_add(1, Ordering::SeqCst);
                    self.stop_microphone_stream();
                }
            }
            (MicrophoneMode::OnDemand, MicrophoneMode::AlwaysOn) => {
                self.close_generation.fetch_add(1, Ordering::SeqCst);
                self.start_microphone_stream()?;
            }
            _ => {}
        }

        *lock(&self.mode) = new_mode;
        Ok(())
    }

    /* ---------- recording --------------------------------------------------- */

    pub fn try_start_recording(&self, binding_id: &str) -> Result<(), String> {
        let mut state = lock(&self.state);

        if !matches!(*state, RecordingState::Idle) {
            return Err("Already recording".to_string());
        }

        // Make sure the microphone is open. In always-on mode it normally
        // already is, but it may have failed to open at launch or been closed
        // by a device change — retry rather than fail every dictation.
        self.close_generation.fetch_add(1, Ordering::SeqCst); // cancel a pending lazy close
        if let Err(e) = self.start_microphone_stream() {
            let msg = format!("{e}");
            error!("Failed to open microphone stream: {msg}");
            return Err(msg);
        }

        let started = match lock(&self.recorder).as_ref() {
            Some(rec) => rec.start().map_err(|e| e.to_string()),
            None => Err("Recorder not available".to_string()),
        };

        match started {
            Ok(()) => {
                {
                    let mut mic = lock(&self.mic);
                    mic.is_recording = true;
                    mic.session = mic.session.wrapping_add(1);
                }
                *state = RecordingState::Recording {
                    binding_id: binding_id.to_string(),
                };
                debug!("Recording started for binding {binding_id}");
                Ok(())
            }
            Err(e) => {
                // Don't leave an on-demand microphone open (and its privacy
                // indicator lit) after a start that never began recording.
                drop(state);
                if matches!(*lock(&self.mode), MicrophoneMode::OnDemand) {
                    self.stop_microphone_stream();
                }
                Err(e)
            }
        }
    }

    pub fn update_selected_device(&self) -> Result<(), anyhow::Error> {
        // If currently open, restart the microphone stream to use the new device
        let is_open = lock(&self.mic).is_open;
        if is_open {
            self.close_generation.fetch_add(1, Ordering::SeqCst);
            self.stop_microphone_stream();
            self.start_microphone_stream()?;
        }
        Ok(())
    }

    /// Stop the recording for `binding_id` and return its samples plus how it
    /// ended. `None` when that binding was not recording.
    pub fn stop_recording_with_diagnostics(
        &self,
        binding_id: &str,
    ) -> Option<(Vec<f32>, RecordingDiagnostics)> {
        let mut state = lock(&self.state);

        match *state {
            RecordingState::Recording {
                binding_id: ref active,
            } if active == binding_id => {
                *state = RecordingState::Idle;
                drop(state);

                // Optionally keep recording for a bit longer to capture trailing audio
                let settings = get_settings(&self.app_handle);
                if settings.extra_recording_buffer_ms > 0 {
                    debug!(
                        "Extra recording buffer: sleeping {}ms before stopping",
                        settings.extra_recording_buffer_ms
                    );
                    std::thread::sleep(Duration::from_millis(settings.extra_recording_buffer_ms));
                }

                let mut diagnostics = RecordingDiagnostics::default();
                // Bounded: the recorder answers within a few seconds even when
                // the device has gone silent.
                let outcome = lock(&self.recorder)
                    .as_ref()
                    .map(|rec| rec.stop_with_outcome());
                let samples = match outcome {
                    Some(Ok(outcome)) => {
                        diagnostics.device_failed = outcome.device_stalled;
                        diagnostics.truncated = outcome.truncated;
                        outcome.samples
                    }
                    Some(Err(e)) => {
                        error!("stop() failed: {e}");
                        diagnostics.device_failed = true;
                        Vec::new()
                    }
                    None => {
                        error!("Recorder not available");
                        diagnostics.device_failed = true;
                        Vec::new()
                    }
                };

                {
                    let mut mic = lock(&self.mic);
                    mic.is_recording = false;
                }
                // Any mute still applied belongs to this recording.
                self.remove_mute();

                if diagnostics.device_failed {
                    // A stalled device stream rarely recovers on its own:
                    // reopen it next time instead of reusing it.
                    self.close_generation.fetch_add(1, Ordering::SeqCst);
                    self.stop_microphone_stream();
                } else {
                    // In on-demand mode, close the mic (lazily if the setting is enabled)
                    self.release_stream_after_recording();
                }

                // Pad if very short
                let s_len = samples.len();
                let samples = if s_len < WHISPER_SAMPLE_RATE && s_len > 0 {
                    let mut padded = samples;
                    padded.resize(WHISPER_SAMPLE_RATE * 5 / 4, 0.0);
                    padded
                } else {
                    samples
                };
                Some((samples, diagnostics))
            }
            _ => None,
        }
    }
    /// Stream recorded audio to `sink` until [`Self::clear_frame_sink`].
    ///
    /// Installed before recording starts so a streaming transcriber sees the
    /// whole utterance. Replacing an existing sink is intentional: only one
    /// recording runs at a time, so a leftover sink would be a bug, not a
    /// second listener to preserve.
    pub fn set_frame_sink(&self, sink: FrameSink) {
        *lock(&self.frame_sink) = Some(sink);
    }

    /// Stop streaming. Safe to call when no sink is installed, so the caller
    /// can clear unconditionally on every stop path including cancellation.
    pub fn clear_frame_sink(&self) {
        *lock(&self.frame_sink) = None;
    }

    pub fn is_recording(&self) -> bool {
        matches!(*lock(&self.state), RecordingState::Recording { .. })
    }

    /// Cancel any ongoing recording without returning audio samples. Also
    /// restores the output if this recording muted it.
    pub fn cancel_recording(&self) {
        let mut state = lock(&self.state);

        if let RecordingState::Recording { .. } = *state {
            *state = RecordingState::Idle;
            drop(state);

            if let Some(rec) = lock(&self.recorder).as_ref() {
                let _ = rec.stop(); // Discard the result
            }

            lock(&self.mic).is_recording = false;
            self.remove_mute();

            // In on-demand mode, close the mic (lazily if the setting is enabled)
            self.release_stream_after_recording();
        } else {
            drop(state);
            // A mute can outlive the recording state (e.g. a cancel racing
            // the stop); never leave the system muted.
            self.remove_mute();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recording(session: u64) -> MicState {
        MicState {
            is_open: true,
            is_recording: true,
            did_mute: false,
            session,
        }
    }

    #[test]
    fn mute_applies_to_the_running_recording() {
        assert!(mute_applies(&recording(3), 3, true));
    }

    #[test]
    fn a_late_mute_for_a_finished_recording_is_dropped() {
        // The recording it was scheduled for has stopped...
        let mut mic = recording(3);
        mic.is_recording = false;
        assert!(!mute_applies(&mic, 3, true));
        // ...or a newer one has started since.
        assert!(!mute_applies(&recording(4), 3, true));
    }

    #[test]
    fn mute_respects_the_setting_and_does_not_double_apply() {
        assert!(!mute_applies(&recording(1), 1, false));
        let mut mic = recording(1);
        mic.did_mute = true;
        assert!(!mute_applies(&mic, 1, true));
    }
}
