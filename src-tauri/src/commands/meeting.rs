// Meeting mode (Step 3) commands.
//
// Thin Tauri command wrappers around `MeetingManager`. The manager is stored in
// Tauri state as `Arc<MeetingManager>` (see `initialize_core_logic` in lib.rs).
//
// These commands are ADDITIVE and ISOLATED from the dictation flow.

use std::sync::Arc;

use tauri::State;

use crate::meeting::{MeetingManager, MeetingState};

/// Start a continuous meeting session: ensure the transcription model is
/// loaded, then begin capturing mixed mic + system audio, segmenting it with
/// VAD, and transcribing each segment.
///
/// macOS-only (CoreAudio tap). Returns an "unsupported" error on other
/// platforms.
#[tauri::command]
#[specta::specta]
pub fn start_meeting(meeting_manager: State<Arc<MeetingManager>>) -> Result<(), String> {
    meeting_manager.start()
}

/// Stop the meeting session and return the final accumulated transcript text.
#[tauri::command]
#[specta::specta]
pub fn stop_meeting(meeting_manager: State<Arc<MeetingManager>>) -> Result<String, String> {
    meeting_manager.stop()
}

/// Return the transcript accumulated so far (for polling during a session).
#[tauri::command]
#[specta::specta]
pub fn get_meeting_transcript(
    meeting_manager: State<Arc<MeetingManager>>,
) -> Result<String, String> {
    Ok(meeting_manager.full_transcript())
}

/// Return the meeting session status: "idle" or "running".
#[tauri::command]
#[specta::specta]
pub fn get_meeting_status(meeting_manager: State<Arc<MeetingManager>>) -> Result<String, String> {
    let status = match meeting_manager.status() {
        MeetingState::Idle => "idle",
        MeetingState::Running => "running",
    };
    Ok(status.to_string())
}
