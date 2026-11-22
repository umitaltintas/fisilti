// Re-export all audio components
mod device;
mod recorder;
mod resampler;
mod utils;
mod visualizer;

// Meeting mode (Step 1): macOS-only CoreAudio system-audio tap. Isolated from
// the dictation flow and only compiled on macOS.
#[cfg(target_os = "macos")]
mod core_audio;
// Meeting mode (Step 2): mic+system audio mixer.
#[cfg(meeting_capture)]
mod mixer;
// Meeting mode on Linux: PipeWire/PulseAudio system capture + device queries.
#[cfg(target_os = "linux")]
mod pulse;
#[cfg(target_os = "macos")]
mod system_audio;

pub use device::{list_input_devices, list_output_devices, CpalDeviceInfo};
pub use recorder::{
    is_microphone_access_denied, AudioRecorder, StopOutcome, MAX_RECORDING_SAMPLES,
};
pub use resampler::FrameResampler;
pub use utils::{read_wav_samples, save_wav_file, verify_wav_file};
pub use visualizer::AudioVisualiser;

#[cfg(target_os = "macos")]
pub use core_audio::{detect_output_route, CoreAudioCapture, CoreAudioStream, OutputRoute};
#[cfg(meeting_capture)]
pub use mixer::{AudioMixerRingBuffer, MeetingMixer, MixSource, ProfessionalAudioMixer};
#[cfg(target_os = "linux")]
pub use pulse::{
    detect_output_route, recording_apps, sound_server_available, OutputRoute, RecordingApp,
    SystemAudioCapture, SystemAudioStream,
};
#[cfg(target_os = "macos")]
pub use system_audio::{SystemAudioCapture, SystemAudioStream};
