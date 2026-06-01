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
#[cfg(target_os = "macos")]
mod system_audio;

pub use device::{list_input_devices, list_output_devices, CpalDeviceInfo};
pub use recorder::{is_microphone_access_denied, AudioRecorder};
pub use resampler::FrameResampler;
pub use utils::{read_wav_samples, save_wav_file, verify_wav_file};
pub use visualizer::AudioVisualiser;

#[cfg(target_os = "macos")]
pub use core_audio::{CoreAudioCapture, CoreAudioStream};
#[cfg(target_os = "macos")]
pub use system_audio::{SystemAudioCapture, SystemAudioStream};
