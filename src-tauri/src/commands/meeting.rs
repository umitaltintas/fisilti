// Meeting mode commands.
//
// Thin Tauri command wrappers around `MeetingManager`. The manager is stored in
// Tauri state as `Arc<MeetingManager>` (see `initialize_core_logic` in lib.rs).
// The LLM summary/title logic lives in `meeting::summarize`.
//
// These commands are ADDITIVE and ISOLATED from the dictation flow.

use std::sync::Arc;

use tauri::{AppHandle, Manager, State};

use crate::meeting::{
    InterruptedMeeting, MeetingImportProgress, MeetingListItem, MeetingManager, MeetingRecord,
    MeetingSessionInfo, MeetingState, StopMeetingResult,
};

/// Event emitted with the slot state — `"idle"`, `"running"` or
/// `"finalizing"` — whenever it changes, whatever triggered it (UI command,
/// tray, global shortcut, auto-end, a capture failure). Observers (e.g. the
/// tray) listen for this to keep their indicator in sync.
pub const MEETING_STATE_CHANGED_EVENT: &str = "meeting-state-changed";

/// Event emitted with a `MeetingSessionInfo` payload whenever the state
/// changes AND as soon as the live meeting's row id becomes known.
pub const MEETING_SESSION_CHANGED_EVENT: &str = "meeting-session-changed";

/// SHARED start path used by the `start_meeting` command, the tray menu item,
/// the global shortcut and the auto-detect prompt. The manager emits the
/// state events itself.
pub fn start_meeting_session(
    _app: &AppHandle,
    meeting_manager: &Arc<MeetingManager>,
) -> Result<(), String> {
    meeting_manager.start()
}

/// SHARED stop path used by the `stop_meeting` command, the tray menu item,
/// the global shortcut and auto-end. Blocking (runs the finalize pass): call
/// it off the main thread. A second stop while one is in progress returns
/// `Err(ALREADY_STOPPING)`.
pub fn stop_meeting_session(
    _app: &AppHandle,
    meeting_manager: &Arc<MeetingManager>,
) -> Result<StopMeetingResult, String> {
    meeting_manager.stop()
}

/// SHARED toggle path: stop if running, start if idle. While the previous
/// meeting is still being saved there is nothing to toggle: starting would be
/// refused and stopping is already happening.
pub fn toggle_meeting_session(
    app: &AppHandle,
    meeting_manager: &Arc<MeetingManager>,
) -> Result<Option<StopMeetingResult>, String> {
    match meeting_manager.status() {
        MeetingState::Running => stop_meeting_session(app, meeting_manager).map(Some),
        MeetingState::Idle => start_meeting_session(app, meeting_manager).map(|()| None),
        MeetingState::Finalizing => Err("The previous meeting is still being saved.".to_string()),
    }
}

/// Convenience for the tray / shortcut handlers that only have an `AppHandle`:
/// resolves the managed `Arc<MeetingManager>` and toggles the session. Logs and
/// swallows errors (e.g. capture unsupported off-macOS) so callers stay simple.
pub fn toggle_meeting_from_app(app: &AppHandle) {
    let manager = app.state::<Arc<MeetingManager>>();
    let manager = (*manager).clone();
    let app = app.clone();
    // The tray menu item and global shortcut both fire on the MAIN event-loop
    // thread, and stop() runs the long, blocking finalize pass. Dispatch to a
    // blocking thread instead (the result is only logged).
    tauri::async_runtime::spawn_blocking(move || match toggle_meeting_session(&app, &manager) {
        Ok(Some(_)) => log::info!("Meeting stopped via tray/shortcut"),
        Ok(None) => log::info!("Meeting started via tray/shortcut"),
        Err(e) => log::warn!("Toggle meeting via tray/shortcut failed: {}", e),
    });
}

/// Start a continuous meeting session: ensure the transcription model is
/// loaded, then begin capturing mixed mic + system audio, segmenting it with
/// VAD, and transcribing each segment.
///
/// macOS-only (CoreAudio tap). Returns an "unsupported" error on other
/// platforms.
#[tauri::command]
#[specta::specta]
pub fn start_meeting(
    app: AppHandle,
    meeting_manager: State<Arc<MeetingManager>>,
) -> Result<(), String> {
    start_meeting_session(&app, &meeting_manager)
}

/// Stop the meeting session. Resolves once the meeting is fully saved, with
/// the id of the row that was persisted (`None` when the empty session was
/// discarded) and the final transcript. Errors when a stop is already in
/// progress.
#[tauri::command]
#[specta::specta]
pub async fn stop_meeting(
    app: AppHandle,
    meeting_manager: State<'_, Arc<MeetingManager>>,
) -> Result<StopMeetingResult, String> {
    // Run the stop (finalize/persist) on a blocking thread so the main event
    // loop — and the tray's `meeting-state-changed` listener — keep running.
    let manager = (*meeting_manager).clone();
    tauri::async_runtime::spawn_blocking(move || stop_meeting_session(&app, &manager))
        .await
        .map_err(|e| format!("Stop task failed: {}", e))?
}

/// Return the transcript accumulated so far (for polling during a session).
#[tauri::command]
#[specta::specta]
pub fn get_meeting_transcript(
    meeting_manager: State<Arc<MeetingManager>>,
) -> Result<String, String> {
    Ok(meeting_manager.full_transcript())
}

/// Return the meeting slot status: `"idle"`, `"running"` or `"finalizing"`.
#[tauri::command]
#[specta::specta]
pub fn get_meeting_status(meeting_manager: State<Arc<MeetingManager>>) -> Result<String, String> {
    Ok(meeting_manager.status().as_str().to_string())
}

/// State, live row id and start time of the meeting slot — the same payload
/// as the `meeting-session-changed` event.
#[tauri::command]
#[specta::specta]
pub fn get_meeting_session(
    meeting_manager: State<Arc<MeetingManager>>,
) -> Result<MeetingSessionInfo, String> {
    Ok(meeting_manager.session_info())
}

/// Return the live session's start time as epoch milliseconds, or `null`
/// when no session is running. The UI uses it to render a truthful elapsed
/// timer when it attaches to a session that was started elsewhere (tray, global
/// shortcut, or the meeting auto-detect prompt).
#[tauri::command]
#[specta::specta]
pub fn get_meeting_started_at(
    meeting_manager: State<Arc<MeetingManager>>,
) -> Result<Option<i64>, String> {
    Ok(meeting_manager.session_started_at_ms())
}

/// Summarize the most recent meeting's transcript into meeting notes using the
/// SAME LLM provider/model/api-key the user configured for dictation
/// post-processing. The summary is saved onto that meeting's row (and
/// announced on `meeting-summary-update`) when it has been saved.
#[tauri::command]
#[specta::specta]
pub async fn summarize_meeting(
    meeting_manager: State<'_, Arc<MeetingManager>>,
) -> Result<String, String> {
    let manager = (*meeting_manager).clone();
    manager.summarize_latest(None).await
}

/// Like `summarize_meeting`, but with an optional template selector + custom
/// prompt override. `template` is matched first against a configured
/// `meeting_summary_templates` id; if no template matches it is treated as a
/// raw custom prompt. `None`/empty → the default prompt.
#[tauri::command]
#[specta::specta]
pub async fn summarize_meeting_with(
    meeting_manager: State<'_, Arc<MeetingManager>>,
    template: Option<String>,
) -> Result<String, String> {
    let manager = (*meeting_manager).clone();
    manager.summarize_latest(template.as_deref()).await
}

/// Regenerate the summary for an ALREADY-PERSISTED meeting `id` (e.g. the user
/// picked a different template). Reads the stored transcript + user notes,
/// runs the summary, persists it onto that row, and returns it.
#[tauri::command]
#[specta::specta]
pub async fn regenerate_meeting_summary(
    app: AppHandle,
    meeting_manager: State<'_, Arc<MeetingManager>>,
    id: i64,
    template: Option<String>,
) -> Result<String, String> {
    let record = meeting_manager
        .store()
        .get_meeting(id)
        .map_err(|e| format!("Failed to load meeting: {}", e))?;
    if record.transcript.trim().is_empty() {
        return Err("This meeting has no transcript to summarize.".to_string());
    }
    let content = crate::meeting::summarize::summarize_transcript_ext(
        &app,
        &record.transcript,
        template.as_deref(),
        record.notes.as_deref(),
    )
    .await?;
    meeting_manager.save_summary(id, &content);
    Ok(content)
}

/// List persisted meetings, newest-first (lightweight rows). When `query` is a
/// non-empty string, filters by a case-insensitive substring match against the
/// title, transcript, or summary. `None`/empty → all meetings (legacy behavior).
#[tauri::command]
#[specta::specta]
pub fn list_meetings(
    meeting_manager: State<Arc<MeetingManager>>,
    query: Option<String>,
) -> Result<Vec<MeetingListItem>, String> {
    meeting_manager
        .store()
        .list_meetings(query.as_deref())
        .map_err(|e| format!("Failed to list meetings: {}", e))
}

/// Fetch a single full meeting record (transcript + segments + summary).
#[tauri::command]
#[specta::specta]
pub fn get_meeting(
    meeting_manager: State<Arc<MeetingManager>>,
    id: i64,
) -> Result<MeetingRecord, String> {
    meeting_manager
        .store()
        .get_meeting(id)
        .map_err(|e| format!("Failed to get meeting: {}", e))
}

/// Return the absolute filesystem path to a meeting's saved mixed-audio WAV,
/// for the frontend to play back. The frontend should pass this path to
/// Tauri's `convertFileSrc()` and use the result as an `<audio>` `src`; the
/// app's asset protocol is enabled with a broad scope so the converted URL is
/// directly loadable. Errors if the meeting has no saved audio.
#[tauri::command]
#[specta::specta]
pub fn get_meeting_audio_path(
    meeting_manager: State<Arc<MeetingManager>>,
    id: i64,
) -> Result<String, String> {
    match meeting_manager
        .store()
        .get_audio_path(id)
        .map_err(|e| format!("Failed to get meeting audio path: {}", e))?
    {
        Some(path) => Ok(path),
        None => Err(format!("Meeting {} has no saved audio", id)),
    }
}

/// Delete a persisted meeting by id, with everything it owns on disk (capture
/// buffers, playback audio, exported note). Errors for the meeting that is
/// in progress (running or still being saved).
#[tauri::command]
#[specta::specta]
pub fn delete_meeting(meeting_manager: State<Arc<MeetingManager>>, id: i64) -> Result<(), String> {
    meeting_manager.delete_meeting(id)
}

/// Where a meeting's transcription would actually run, for the trust
/// indicator.
///
/// The UI showed an unconditional "100% on-device" badge, which stopped being
/// true the moment a cloud transcription model or any Gemini path could be
/// selected. A privacy claim that is right most of the time is worse than no
/// claim: it is exactly the situation where the user stops checking.
#[derive(serde::Serialize, specta::Type)]
pub struct TranscriptionLocation {
    /// Names of the cloud services audio would be sent to. Empty means the
    /// meeting really is fully on-device.
    pub cloud_providers: Vec<String>,
}

#[tauri::command]
#[specta::specta]
pub fn get_transcription_location(app: AppHandle) -> Result<TranscriptionLocation, String> {
    let settings = crate::settings::get_settings(&app);
    let mut cloud_providers: Vec<String> = Vec::new();

    // Gemini Live streaming. The batch finalize pass is no longer a separate
    // flag — it follows the meeting model, which the check below covers.
    if settings.meeting_live_mode != "off" {
        cloud_providers.push("Google Gemini".to_string());
    }

    // The model transcribing THIS meeting may itself be a cloud model. Name the
    // provider it actually talks to — a Gemini model does not go through
    // OpenRouter, and telling the user it does would be a false privacy claim.
    if let Some(engine) = app
        .try_state::<Arc<crate::managers::model::ModelManager>>()
        .and_then(|mm| mm.get_model_info(settings.meeting_model_id()))
        .map(|m| m.engine_type)
    {
        let provider = match engine {
            crate::managers::model::EngineType::OpenRouter
            | crate::managers::model::EngineType::OpenRouterAsr => Some("OpenRouter"),
            crate::managers::model::EngineType::Gemini => Some("Google Gemini"),
            _ => None,
        };
        if let Some(provider) = provider {
            if !cloud_providers.iter().any(|p| p == provider) {
                cloud_providers.push(provider.to_string());
            }
        }
    }

    Ok(TranscriptionLocation { cloud_providers })
}

/// Discard an interrupted meeting: delete the row AND the files only it
/// referenced (its raw capture buffers, which exist purely so the row can be
/// re-finalized, plus any playback audio). Errors for the meeting that is in
/// progress — it is in `recording` status too, but it is not interrupted.
#[tauri::command]
#[specta::specta]
pub fn discard_interrupted_meeting(
    meeting_manager: State<Arc<MeetingManager>>,
    id: i64,
) -> Result<(), String> {
    meeting_manager.delete_meeting(id)
}

/// Manually rename a meeting (Phase 2 item 2). Overwrites the `title` column.
#[tauri::command]
#[specta::specta]
pub fn update_meeting_title(
    meeting_manager: State<Arc<MeetingManager>>,
    id: i64,
    title: String,
) -> Result<(), String> {
    meeting_manager
        .store()
        .update_title(id, &title)
        .map_err(|e| format!("Failed to update meeting title: {}", e))?;
    meeting_manager.export_markdown(id);
    Ok(())
}

/// Save the user's own editable notes for a meeting. Distinct from the AI
/// `summary`; stored in the `notes` column. Works on the in-progress
/// (`recording`) row too, so notes can be autosaved live — finalize never
/// writes the notes column, so nothing typed here is overwritten.
#[tauri::command]
#[specta::specta]
pub fn update_meeting_notes(
    meeting_manager: State<Arc<MeetingManager>>,
    id: i64,
    notes: String,
) -> Result<(), String> {
    meeting_manager
        .store()
        .update_notes(id, &notes)
        .map_err(|e| format!("Failed to update meeting notes: {}", e))?;
    meeting_manager.export_markdown(id);
    Ok(())
}

/// Export a meeting as a clean Markdown document (Phase 2 item 6): title,
/// date/time, duration, labeled transcript, user notes, and AI summary. The
/// frontend handles the file-save dialog (Phase 4); this returns the string.
#[tauri::command]
#[specta::specta]
pub fn export_meeting_markdown(
    meeting_manager: State<Arc<MeetingManager>>,
    id: i64,
) -> Result<String, String> {
    let record = meeting_manager
        .store()
        .get_meeting(id)
        .map_err(|e| format!("Failed to load meeting: {}", e))?;
    Ok(crate::meeting::export::render_meeting_markdown(&record))
}

/// List meetings interrupted by a crash/OS-kill (still in `recording` status),
/// newest-first, excluding the meeting that is running or being saved right
/// now. Each item reports whether its capture buffers still exist so the UI
/// can offer a high-quality re-finalize vs. salvaging the partial text.
#[tauri::command]
#[specta::specta]
pub fn list_interrupted_meetings(
    meeting_manager: State<Arc<MeetingManager>>,
) -> Result<Vec<InterruptedMeeting>, String> {
    meeting_manager
        .store()
        .list_interrupted(meeting_manager.live_meeting_id())
        .map_err(|e| format!("Failed to list interrupted meetings: {}", e))
}

/// Recover an interrupted meeting (Phase 2 item 1). If the per-source temp audio
/// buffers still exist, re-runs the finalize pass for a high-quality labeled
/// transcript and saves the mixed playback WAV; otherwise keeps the partial
/// transcript that was incrementally saved. Either way the row is flipped to
/// `completed` (so it isn't offered for recovery again) and the temp files are
/// cleaned up. Returns the recovered full transcript.
#[tauri::command]
#[specta::specta]
pub async fn recover_meeting(
    meeting_manager: State<'_, Arc<MeetingManager>>,
    id: i64,
) -> Result<String, String> {
    let manager = (*meeting_manager).clone();
    tauri::async_runtime::spawn_blocking(move || manager.recover_meeting(id))
        .await
        .map_err(|e| format!("Recovery task failed: {}", e))?
}

/// Import a recording made elsewhere (a phone voice memo, a conference
/// recording) as a meeting: decode, transcribe with the meeting model, title
/// and summarize, save to History. Returns the new meeting id. Progress arrives
/// as `"meeting-import-progress"`, the outcome also as
/// `"meeting-import-finished"`.
#[tauri::command]
#[specta::specta]
pub async fn import_meeting_recording(
    meeting_manager: State<'_, Arc<MeetingManager>>,
    path: String,
) -> Result<i64, String> {
    let manager = (*meeting_manager).clone();
    tauri::async_runtime::spawn_blocking(move || {
        manager.import_recording(std::path::Path::new(&path))
    })
    .await
    .map_err(|e| format!("Import task failed: {}", e))?
}

/// Transcribe a saved meeting again from its stored audio, replacing its
/// transcript (and summary, when it had one). For meetings that came back
/// empty or garbled. Shares the import's progress/finished events and cancel.
#[tauri::command]
#[specta::specta]
pub async fn retranscribe_meeting(
    meeting_manager: State<'_, Arc<MeetingManager>>,
    id: i64,
) -> Result<i64, String> {
    let manager = (*meeting_manager).clone();
    tauri::async_runtime::spawn_blocking(move || manager.retranscribe_meeting(id))
        .await
        .map_err(|e| format!("Re-transcription task failed: {}", e))?
}

/// Cancel the running import at its next checkpoint.
#[tauri::command]
#[specta::specta]
pub fn cancel_meeting_import(meeting_manager: State<Arc<MeetingManager>>) {
    meeting_manager.cancel_import();
}

/// Where the running import is, or `None` when nothing is importing.
#[tauri::command]
#[specta::specta]
pub fn get_meeting_import_progress(
    meeting_manager: State<Arc<MeetingManager>>,
) -> Option<MeetingImportProgress> {
    meeting_manager.import_progress()
}

/// File extensions the import accepts, for the file picker and drag-and-drop.
#[tauri::command]
#[specta::specta]
pub fn get_supported_import_extensions() -> Vec<String> {
    crate::meeting::import::SUPPORTED_EXTENSIONS
        .iter()
        .map(|e| e.to_string())
        .collect()
}

/// User accepted the auto-detection "start transcription?" prompt: hides the
/// prompt window and starts a meeting session (same shared path as the UI
/// button / tray / shortcut).
#[tauri::command]
#[specta::specta]
pub fn accept_meeting_prompt(app: AppHandle) -> Result<(), String> {
    crate::meeting_detector::accept_start_prompt(&app)
}

/// User dismissed the auto-detection "start transcription?" prompt: hides the
/// prompt window and snoozes detection until the current meeting-app signal
/// clears (so the same meeting doesn't re-prompt).
#[tauri::command]
#[specta::specta]
pub fn dismiss_meeting_prompt(app: AppHandle) -> Result<(), String> {
    crate::meeting_detector::dismiss_start_prompt(&app)
}

/// User answered the "end meeting?" prompt. `continue_meeting` true keeps the
/// session running (and resets the silence timer); false stops it (finalize
/// runs on a blocking thread inside the helper).
#[tauri::command]
#[specta::specta]
pub fn respond_meeting_auto_end(app: AppHandle, continue_meeting: bool) -> Result<(), String> {
    crate::meeting_detector::respond_auto_end(&app, continue_meeting)
}

/// Snapshot of the meeting auto-detection state (whether a meeting app is
/// currently using the microphone, and which one) for the settings UI.
#[tauri::command]
#[specta::specta]
pub fn get_meeting_detection_status(
    app: AppHandle,
) -> Result<crate::meeting_detector::MeetingDetectionStatus, String> {
    Ok(crate::meeting_detector::detection_status(&app))
}

/// Authorization status of macOS calendar access for meeting naming:
/// `"authorized"` | `"denied"` | `"notDetermined"` | `"unavailable"`.
#[tauri::command]
#[specta::specta]
pub fn get_calendar_access_status() -> Result<String, String> {
    Ok(crate::meeting_naming::calendar_access_status().to_string())
}

/// Request macOS calendar access for meeting naming, showing the system prompt
/// on first call. Resolves `true` when full access is granted. Runs on a
/// blocking thread — the system prompt can stay open for a while.
#[tauri::command]
#[specta::specta]
pub async fn request_calendar_access() -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(crate::meeting_naming::request_calendar_access)
        .await
        .map_err(|e| format!("Calendar access request failed: {}", e))
}
