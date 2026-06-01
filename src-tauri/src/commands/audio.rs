use crate::audio_feedback;
use crate::audio_toolkit::audio::{list_input_devices, list_output_devices};
use crate::managers::audio::{AudioRecordingManager, MicrophoneMode};
use crate::settings::{get_settings, write_settings};
use log::warn;
use serde::{Deserialize, Serialize};
use specta::Type;
use std::sync::Arc;
use tauri::{AppHandle, Manager};

#[cfg(target_os = "windows")]
use winreg::{
    enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE},
    RegKey, HKEY,
};

#[derive(Serialize, Type)]
pub struct CustomSounds {
    start: bool,
    stop: bool,
}

fn custom_sound_exists(app: &AppHandle, sound_type: &str) -> bool {
    crate::portable::resolve_app_data(app, &format!("custom_{}.wav", sound_type))
        .map_or(false, |path| path.exists())
}

#[tauri::command]
#[specta::specta]
pub fn check_custom_sounds(app: AppHandle) -> CustomSounds {
    CustomSounds {
        start: custom_sound_exists(&app, "start"),
        stop: custom_sound_exists(&app, "stop"),
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Type)]
pub struct AudioDevice {
    pub index: String,
    pub name: String,
    pub is_default: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum PermissionAccess {
    Allowed,
    Denied,
    Unknown,
}

#[derive(Serialize, Deserialize, Debug, Clone, Type)]
pub struct WindowsMicrophonePermissionStatus {
    pub supported: bool,
    pub overall_access: PermissionAccess,
    pub device_access: PermissionAccess,
    pub app_access: PermissionAccess,
    pub desktop_app_access: PermissionAccess,
}

#[cfg(target_os = "windows")]
fn read_registry_permission_access(root_hkey: HKEY, path: &str) -> PermissionAccess {
    let root = RegKey::predef(root_hkey);
    let Ok(key) = root.open_subkey(path) else {
        return PermissionAccess::Unknown;
    };

    let Ok(value) = key.get_value::<String, _>("Value") else {
        return PermissionAccess::Unknown;
    };

    match value.to_ascii_lowercase().as_str() {
        "allow" => PermissionAccess::Allowed,
        "deny" => PermissionAccess::Denied,
        _ => PermissionAccess::Unknown,
    }
}

#[cfg(target_os = "windows")]
fn get_windows_microphone_permission_status_impl() -> WindowsMicrophonePermissionStatus {
    const MICROPHONE_PATH: &str =
        "Software\\Microsoft\\Windows\\CurrentVersion\\CapabilityAccessManager\\ConsentStore\\microphone";
    const DESKTOP_APPS_PATH: &str =
        "Software\\Microsoft\\Windows\\CurrentVersion\\CapabilityAccessManager\\ConsentStore\\microphone\\NonPackaged";

    let device_access = read_registry_permission_access(HKEY_LOCAL_MACHINE, MICROPHONE_PATH);
    let app_access = read_registry_permission_access(HKEY_CURRENT_USER, MICROPHONE_PATH);
    let desktop_app_access = read_registry_permission_access(HKEY_CURRENT_USER, DESKTOP_APPS_PATH);

    let overall_access = if [device_access, app_access, desktop_app_access]
        .into_iter()
        .any(|access| access == PermissionAccess::Denied)
    {
        PermissionAccess::Denied
    } else if [device_access, app_access, desktop_app_access]
        .into_iter()
        .all(|access| access == PermissionAccess::Allowed)
    {
        PermissionAccess::Allowed
    } else {
        PermissionAccess::Unknown
    };

    WindowsMicrophonePermissionStatus {
        supported: true,
        overall_access,
        device_access,
        app_access,
        desktop_app_access,
    }
}

#[tauri::command]
#[specta::specta]
pub fn get_windows_microphone_permission_status() -> WindowsMicrophonePermissionStatus {
    #[cfg(target_os = "windows")]
    {
        get_windows_microphone_permission_status_impl()
    }

    #[cfg(not(target_os = "windows"))]
    {
        WindowsMicrophonePermissionStatus {
            supported: false,
            overall_access: PermissionAccess::Unknown,
            device_access: PermissionAccess::Unknown,
            app_access: PermissionAccess::Unknown,
            desktop_app_access: PermissionAccess::Unknown,
        }
    }
}

#[tauri::command]
#[specta::specta]
pub fn open_microphone_privacy_settings() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use std::process::Command;
        Command::new("cmd")
            .args(["/C", "start", "", "ms-settings:privacy-microphone"])
            .spawn()
            .map_err(|e| format!("Failed to open Windows microphone privacy settings: {}", e))?;
        return Ok(());
    }

    #[cfg(not(target_os = "windows"))]
    {
        Err("Opening microphone privacy settings is only supported on Windows".to_string())
    }
}

#[tauri::command]
#[specta::specta]
pub fn update_microphone_mode(app: AppHandle, always_on: bool) -> Result<(), String> {
    // Update settings
    let mut settings = get_settings(&app);
    settings.always_on_microphone = always_on;
    write_settings(&app, settings);

    // Update the audio manager mode
    let rm = app.state::<Arc<AudioRecordingManager>>();
    let new_mode = if always_on {
        MicrophoneMode::AlwaysOn
    } else {
        MicrophoneMode::OnDemand
    };

    rm.update_mode(new_mode)
        .map_err(|e| format!("Failed to update microphone mode: {}", e))
}

#[tauri::command]
#[specta::specta]
pub fn get_microphone_mode(app: AppHandle) -> Result<bool, String> {
    let settings = get_settings(&app);
    Ok(settings.always_on_microphone)
}

#[tauri::command]
#[specta::specta]
pub fn get_available_microphones() -> Result<Vec<AudioDevice>, String> {
    let devices =
        list_input_devices().map_err(|e| format!("Failed to list audio devices: {}", e))?;

    let mut result = vec![AudioDevice {
        index: "default".to_string(),
        name: "Default".to_string(),
        is_default: true,
    }];

    result.extend(devices.into_iter().map(|d| AudioDevice {
        index: d.index,
        name: d.name,
        is_default: false, // The explicit default is handled separately
    }));

    Ok(result)
}

#[tauri::command]
#[specta::specta]
pub fn set_selected_microphone(app: AppHandle, device_name: String) -> Result<(), String> {
    let mut settings = get_settings(&app);
    settings.selected_microphone = if device_name == "default" {
        None
    } else {
        Some(device_name)
    };
    write_settings(&app, settings);

    // Update the audio manager to use the new device
    let rm = app.state::<Arc<AudioRecordingManager>>();
    rm.update_selected_device()
        .map_err(|e| format!("Failed to update selected device: {}", e))?;

    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn get_selected_microphone(app: AppHandle) -> Result<String, String> {
    let settings = get_settings(&app);
    Ok(settings
        .selected_microphone
        .unwrap_or_else(|| "default".to_string()))
}

#[tauri::command]
#[specta::specta]
pub fn get_available_output_devices() -> Result<Vec<AudioDevice>, String> {
    let devices =
        list_output_devices().map_err(|e| format!("Failed to list output devices: {}", e))?;

    let mut result = vec![AudioDevice {
        index: "default".to_string(),
        name: "Default".to_string(),
        is_default: true,
    }];

    result.extend(devices.into_iter().map(|d| AudioDevice {
        index: d.index,
        name: d.name,
        is_default: false, // The explicit default is handled separately
    }));

    Ok(result)
}

#[tauri::command]
#[specta::specta]
pub fn set_selected_output_device(app: AppHandle, device_name: String) -> Result<(), String> {
    let mut settings = get_settings(&app);
    settings.selected_output_device = if device_name == "default" {
        None
    } else {
        Some(device_name)
    };
    write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn get_selected_output_device(app: AppHandle) -> Result<String, String> {
    let settings = get_settings(&app);
    Ok(settings
        .selected_output_device
        .unwrap_or_else(|| "default".to_string()))
}

#[tauri::command]
#[specta::specta]
pub async fn play_test_sound(app: AppHandle, sound_type: String) {
    let sound = match sound_type.as_str() {
        "start" => audio_feedback::SoundType::Start,
        "stop" => audio_feedback::SoundType::Stop,
        _ => {
            warn!("Unknown sound type: {}", sound_type);
            return;
        }
    };
    audio_feedback::play_test_sound(&app, sound);
}

#[tauri::command]
#[specta::specta]
pub fn set_clamshell_microphone(app: AppHandle, device_name: String) -> Result<(), String> {
    let mut settings = get_settings(&app);
    settings.clamshell_microphone = if device_name == "default" {
        None
    } else {
        Some(device_name)
    };
    write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn get_clamshell_microphone(app: AppHandle) -> Result<String, String> {
    let settings = get_settings(&app);
    Ok(settings
        .clamshell_microphone
        .unwrap_or_else(|| "default".to_string()))
}

#[tauri::command]
#[specta::specta]
pub fn is_recording(app: AppHandle) -> bool {
    let audio_manager = app.state::<Arc<AudioRecordingManager>>();
    audio_manager.is_recording()
}

/// Meeting mode (Step 1) test: capture `seconds` of system audio via the
/// CoreAudio tap and write it to a 32-bit float WAV in the temp dir. Returns the
/// output path.
///
/// On macOS this triggers the Audio-Capture permission dialog on first run
/// (NSAudioCaptureUsageDescription, macOS 14.4+). Must run inside the bundled
/// app for the permission to be granted. This command is isolated from the
/// existing dictation flow.
#[tauri::command]
#[specta::specta]
pub async fn capture_system_audio_test(seconds: u32) -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        use crate::audio_toolkit::audio::SystemAudioCapture;
        use futures_util::StreamExt;
        use std::time::{Duration, Instant};

        let seconds = seconds.max(1);

        let mut stream =
            SystemAudioCapture::start().map_err(|e| format!("Failed to start capture: {}", e))?;
        let sample_rate = stream.sample_rate();
        log::info!(
            "capture_system_audio_test: capturing {}s at {} Hz",
            seconds,
            sample_rate
        );

        let target = (sample_rate as u64).saturating_mul(seconds as u64) as usize;
        let deadline = Instant::now() + Duration::from_secs(seconds as u64 + 5);
        let mut samples: Vec<f32> = Vec::with_capacity(target);

        while samples.len() < target {
            if Instant::now() >= deadline {
                log::warn!(
                    "capture_system_audio_test: hit deadline with {} samples",
                    samples.len()
                );
                break;
            }
            match stream.next().await {
                Some(s) => samples.push(s),
                None => break,
            }
        }

        // Drop the stream to tear down the CoreAudio tap before writing.
        drop(stream);

        let out_path = std::env::temp_dir().join(format!(
            "handy_system_audio_test_{}.wav",
            std::process::id()
        ));
        write_f32_wav(&out_path, &samples, sample_rate)
            .map_err(|e| format!("Failed to write WAV: {}", e))?;

        let peak = samples.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
        log::info!(
            "capture_system_audio_test: wrote {} samples to {:?} (peak amplitude {:.4})",
            samples.len(),
            out_path,
            peak
        );

        Ok(out_path.to_string_lossy().to_string())
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = seconds;
        Err("System audio capture is only supported on macOS".to_string())
    }
}

/// Meeting mode (Step 2) test: capture `seconds` of BOTH the microphone and
/// system audio, resample each to 16 kHz mono, mix them with clipping
/// prevention, and write the mixed result to a 16 kHz mono f32 WAV in the temp
/// dir. Returns the output path.
///
/// This is the transcription-ready target rate (`WHISPER_SAMPLE_RATE = 16000`),
/// so the output is directly usable by `TranscriptionManager::transcribe`.
///
/// Design: resample-then-mix. The mic is captured on a dedicated cpal input
/// stream (independent of the dictation `AudioRecordingManager` singleton),
/// resampled to 16 kHz via `FrameResampler`, and forwarded over a channel. The
/// system audio comes from Step 1's `SystemAudioStream` (~48 kHz), resampled to
/// 16 kHz. Both feed a `MeetingMixer` that mixes in 100 ms windows.
///
/// On macOS this triggers BOTH the Audio-Capture (system) and Microphone
/// permission dialogs. Must run inside the bundled app. Isolated from the
/// existing dictation flow.
#[tauri::command]
#[specta::specta]
pub async fn capture_mixed_audio_test(seconds: u32) -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        use crate::audio_toolkit::audio::{FrameResampler, MeetingMixer, MixSource, SystemAudioCapture};
        use crate::audio_toolkit::constants::WHISPER_SAMPLE_RATE;
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        use futures_util::StreamExt;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{mpsc, Arc};
        use std::time::{Duration, Instant};

        let seconds = seconds.max(1);

        // ---- Mic capture on a dedicated thread (independent cpal stream) ----
        // Resamples to 16 kHz inside the worker and forwards mono frames over a
        // channel. Never touches the dictation manager/state singletons.
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
                "capture_mixed_audio_test: mic '{:?}' {} Hz {} ch {:?}",
                device.name(),
                in_rate,
                channels,
                sample_format
            );

            // Raw mono f32 frames from the cpal callback -> resampler.
            let (raw_tx, raw_rx) = mpsc::channel::<Vec<f32>>();

            // Build an f32 input stream (handle the common formats).
            macro_rules! build {
                ($t:ty) => {{
                    let raw_tx = raw_tx.clone();
                    device.build_input_stream(
                        &config.clone().into(),
                        move |data: &[$t], _: &cpal::InputCallbackInfo| {
                            let mono: Vec<f32> = if channels <= 1 {
                                data.iter().map(|&s| cpal::Sample::to_sample::<f32>(s)).collect()
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
                        |err| log::error!("mic stream error: {err}"),
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

            // Drain raw frames, resample to 16 kHz, forward to the mixer loop.
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

        // Wait for the mic stream to initialize (or fail).
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

        // ---- System audio capture (Step 1), resampled to 16 kHz ----
        let mut sys_stream = match SystemAudioCapture::start() {
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

        log::info!(
            "capture_mixed_audio_test: mixing {}s at {} Hz (system in {} Hz)",
            seconds,
            WHISPER_SAMPLE_RATE,
            sys_rate
        );

        // ---- Mix loop ----
        let mut mixer = MeetingMixer::new();
        let mut mixed: Vec<f32> = Vec::new();
        let target = (WHISPER_SAMPLE_RATE as u64).saturating_mul(seconds as u64) as usize;
        let deadline = Instant::now() + Duration::from_secs(seconds as u64 + 5);

        while mixed.len() < target {
            if Instant::now() >= deadline {
                log::warn!(
                    "capture_mixed_audio_test: hit deadline with {} mixed samples",
                    mixed.len()
                );
                break;
            }

            // Drain any available mic frames (non-blocking).
            while let Ok(frame) = mic_rx.try_recv() {
                mixer.push(MixSource::Microphone, &frame);
            }

            // Pull one system sample (drives the loop cadence).
            match sys_stream.next().await {
                Some(s) => {
                    sys_resampler.push(&[s], &mut |frame: &[f32]| {
                        mixer.push(MixSource::System, frame);
                    });
                }
                None => break,
            }

            mixer.drain_into(&mut mixed);
        }

        // ---- Teardown: stop mic, flush both resamplers and the mixer ----
        mic_stop.store(true, Ordering::Relaxed);
        let _ = mic_handle.join();
        // Drain any mic frames produced during shutdown.
        while let Ok(frame) = mic_rx.try_recv() {
            mixer.push(MixSource::Microphone, &frame);
        }
        sys_resampler.finish(&mut |frame: &[f32]| {
            mixer.push(MixSource::System, frame);
        });
        drop(sys_stream);
        mixer.flush_into(&mut mixed);

        let out_path = std::env::temp_dir()
            .join(format!("handy_mixed_audio_test_{}.wav", std::process::id()));
        write_f32_wav(&out_path, &mixed, WHISPER_SAMPLE_RATE)
            .map_err(|e| format!("Failed to write WAV: {}", e))?;

        let peak = mixed.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
        log::info!(
            "capture_mixed_audio_test: wrote {} samples to {:?} (peak {:.4})",
            mixed.len(),
            out_path,
            peak
        );

        Ok(out_path.to_string_lossy().to_string())
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = seconds;
        Err("Mixed audio capture is only supported on macOS".to_string())
    }
}

/// Write mono f32 samples as a 32-bit float PCM WAV (format tag 3) with a
/// manually constructed 44-byte header. Avoids pulling extra crates.
#[cfg(target_os = "macos")]
fn write_f32_wav(
    path: &std::path::Path,
    samples: &[f32],
    sample_rate: u32,
) -> std::io::Result<()> {
    use std::io::Write;

    let channels: u16 = 1;
    let bits_per_sample: u16 = 32;
    let block_align: u16 = channels * (bits_per_sample / 8);
    let byte_rate: u32 = sample_rate * block_align as u32;
    let data_bytes: u32 = (samples.len() * std::mem::size_of::<f32>()) as u32;
    let riff_chunk_size: u32 = 36 + data_bytes;

    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);

    // RIFF header
    f.write_all(b"RIFF")?;
    f.write_all(&riff_chunk_size.to_le_bytes())?;
    f.write_all(b"WAVE")?;

    // fmt chunk (16 bytes, format tag 3 = IEEE float)
    f.write_all(b"fmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&3u16.to_le_bytes())?; // audio format: 3 = IEEE float
    f.write_all(&channels.to_le_bytes())?;
    f.write_all(&sample_rate.to_le_bytes())?;
    f.write_all(&byte_rate.to_le_bytes())?;
    f.write_all(&block_align.to_le_bytes())?;
    f.write_all(&bits_per_sample.to_le_bytes())?;

    // data chunk
    f.write_all(b"data")?;
    f.write_all(&data_bytes.to_le_bytes())?;
    for &s in samples {
        f.write_all(&s.to_le_bytes())?;
    }

    f.flush()?;
    Ok(())
}
