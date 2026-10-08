use crate::managers::audio::AudioRecordingManager;
use crate::managers::transcription::TranscriptionManager;
use crate::shortcut;
use crate::TranscriptionCoordinator;
use log::info;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};

// Re-export all utility modules for easy access
// pub use crate::audio_feedback::*;
pub use crate::clipboard::*;
pub use crate::overlay::*;
pub use crate::tray::*;

/// Event name for user-facing dictation failures.
pub const DICTATION_ERROR_EVENT: &str = "dictation-error";

/// Where in the dictation pipeline a failure happened. Serialized as the
/// `stage` string of [`DictationError`]; the frontend picks a localized title
/// from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DictationStage {
    /// The model ran (or the cloud call was made) and failed.
    Transcription,
    /// Transcribed fine, but the text could not be inserted.
    Paste,
    /// No usable model could be loaded for this dictation.
    ModelLoad,
    /// Nothing was heard: no audio, or the VAD dropped everything.
    NoSpeech,
    /// The microphone hung, vanished or hit the length limit.
    Recording,
}

impl DictationStage {
    pub fn as_str(self) -> &'static str {
        match self {
            DictationStage::Transcription => "transcription",
            DictationStage::Paste => "paste",
            DictationStage::ModelLoad => "model_load",
            DictationStage::NoSpeech => "no_speech",
            DictationStage::Recording => "recording",
        }
    }
}

/// Payload of the `dictation-error` event.
#[derive(Clone, Debug, serde::Serialize, specta::Type)]
pub struct DictationError {
    /// One of "transcription" | "paste" | "model_load" | "no_speech" | "recording".
    pub stage: String,
    /// Human-readable English detail; the frontend wraps it in a localized title.
    pub message: String,
}

/// Tell the user a dictation failed. Every user-facing failure in the
/// dictation pipeline goes through here so none is only logged.
pub fn emit_dictation_error(app: &AppHandle, stage: DictationStage, message: impl Into<String>) {
    let payload = DictationError {
        stage: stage.as_str().to_string(),
        message: message.into(),
    };
    log::warn!("Dictation error ({}): {}", payload.stage, payload.message);
    if let Err(e) = app.emit(DICTATION_ERROR_EVENT, payload) {
        log::error!("Failed to emit {DICTATION_ERROR_EVENT}: {e}");
    }
}

/// Centralized cancellation function that can be called from anywhere in the app.
/// Handles cancelling both recording and transcription operations and updates UI state.
pub fn cancel_current_operation(app: &AppHandle) {
    info!("Initiating operation cancellation...");

    // Unregister the cancel shortcut asynchronously
    shortcut::unregister_cancel_shortcut(app);

    // Cancel any ongoing recording
    let audio_manager = app.state::<Arc<AudioRecordingManager>>();
    let recording_was_active = audio_manager.is_recording();
    crate::dictation_live::abort_active(app);
    // Also restores the output mute if the recording applied one.
    audio_manager.cancel_recording();

    // Update tray icon and hide overlay
    change_tray_icon(app, crate::tray::TrayIconState::Idle);
    hide_recording_overlay(app);

    // Unload model if immediate unload is enabled (a no-op while a meeting
    // owns the engine).
    let tm = app.state::<Arc<TranscriptionManager>>();
    tm.maybe_unload_immediately("cancellation");

    // Notify coordinator so it can keep lifecycle state coherent.
    if let Some(coordinator) = app.try_state::<TranscriptionCoordinator>() {
        coordinator.notify_cancel(recording_was_active);
    }

    info!("Operation cancellation completed - returned to idle state");
}

/// Check if using the Wayland display server protocol
#[cfg(target_os = "linux")]
pub fn is_wayland() -> bool {
    std::env::var("WAYLAND_DISPLAY").is_ok()
        || std::env::var("XDG_SESSION_TYPE")
            .map(|v| v.to_lowercase() == "wayland")
            .unwrap_or(false)
}

/// Check if running on KDE Plasma desktop environment
#[cfg(target_os = "linux")]
pub fn is_kde_plasma() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP")
        .map(|v| v.to_uppercase().contains("KDE"))
        .unwrap_or(false)
        || std::env::var("KDE_SESSION_VERSION").is_ok()
}

/// Check if running on KDE Plasma with Wayland
#[cfg(target_os = "linux")]
pub fn is_kde_wayland() -> bool {
    is_wayland() && is_kde_plasma()
}
