// Meeting mode (Step 3) commands.
//
// Thin Tauri command wrappers around `MeetingManager`. The manager is stored in
// Tauri state as `Arc<MeetingManager>` (see `initialize_core_logic` in lib.rs).
//
// These commands are ADDITIVE and ISOLATED from the dictation flow.

use std::sync::Arc;

use tauri::{AppHandle, State};

use crate::meeting::{MeetingListItem, MeetingManager, MeetingRecord, MeetingState};

/// Default system prompt for summarizing a meeting transcript into notes.
///
/// Reused by `summarize_meeting`. Instructs the model to answer in the SAME
/// language as the transcript (so a Turkish transcript yields Turkish notes)
/// and to produce a short summary, key points, decisions, and action items.
pub(crate) const DEFAULT_MEETING_SUMMARY_PROMPT: &str = "You are an assistant that writes clear, concise meeting notes from a raw meeting transcript. \
Respond in the SAME LANGUAGE as the transcript (do not translate). \
Produce well-structured notes with the following sections, using the section names in the transcript's language:\n\
1. Summary - a short paragraph summarizing the meeting.\n\
2. Key discussion points - a bullet list of the main topics discussed.\n\
3. Decisions - a bullet list of decisions made (or note that none were made).\n\
4. Action items - a bullet list of follow-up tasks, with the responsible person if mentioned.\n\
Only use information present in the transcript. Do not invent details.";

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

/// Summarize the accumulated meeting transcript into meeting notes using the
/// SAME LLM provider/model/api-key the user already configured for dictation
/// post-processing (reads `settings::get_settings`). Does NOT modify or depend
/// on dictation's post-processing behavior; it only reuses `llm_client`
/// read-only.
///
/// Returns the LLM-generated meeting notes, or a clear, actionable error if
/// there is no transcript or no LLM provider/model configured.
#[tauri::command]
#[specta::specta]
pub async fn summarize_meeting(
    app: AppHandle,
    meeting_manager: State<'_, Arc<MeetingManager>>,
) -> Result<String, String> {
    // 1. Fetch the accumulated transcript text.
    let transcript = meeting_manager.full_transcript();
    if transcript.trim().is_empty() {
        return Err("No transcript to summarize. Start and run a meeting first.".to_string());
    }

    // 2-3. Run the shared summarization path (resolve active provider, send
    // transcript to the LLM with the meeting-notes prompt).
    let content = summarize_transcript(&app, &transcript).await?;

    // Persist the summary onto the meeting row saved on the most recent stop().
    // If there's no saved id (summarize called for an unsaved session), this is
    // a no-op and we still return the summary.
    if let Err(e) = meeting_manager.update_saved_summary(&content) {
        log::error!("{}", e);
    }
    Ok(content)
}

/// Summarize a meeting `transcript` into notes using the SAME active
/// post-processing provider/model/api-key the user configured for dictation.
/// Shared by the `summarize_meeting` command and the on-stop auto-summarize
/// path in `MeetingManager`. Returns the generated notes, or a clear error.
pub(crate) async fn summarize_transcript(
    app: &AppHandle,
    transcript: &str,
) -> Result<String, String> {
    let settings = crate::settings::get_settings(app);

    let provider = settings.active_post_process_provider().cloned().ok_or_else(|| {
        "No LLM provider is configured. Set up a post-processing provider in Settings (e.g. a local Ollama instance or an API key) and try again.".to_string()
    })?;

    let model = settings
        .post_process_models
        .get(&provider.id)
        .cloned()
        .unwrap_or_default();
    if model.trim().is_empty() {
        return Err(format!(
            "No model is configured for provider '{}'. Choose a model in Settings and try again.",
            provider.id
        ));
    }

    let api_key = settings
        .post_process_api_keys
        .get(&provider.id)
        .cloned()
        .unwrap_or_default();

    // Use the plain (non-structured) chat completion path: meeting notes are
    // free-form text, so we send the summary instructions as the system prompt
    // and the transcript as the user message.
    let prompt = format!(
        "{}\n\nTranscript:\n{}",
        DEFAULT_MEETING_SUMMARY_PROMPT, transcript
    );

    match crate::llm_client::send_chat_completion(&provider, api_key, &model, prompt).await {
        Ok(Some(content)) => {
            let content = content.trim().to_string();
            if content.is_empty() {
                Err("The LLM returned an empty summary.".to_string())
            } else {
                Ok(content)
            }
        }
        Ok(None) => Err("The LLM response contained no content.".to_string()),
        Err(e) => Err(format!("Failed to summarize meeting: {}", e)),
    }
}

/// List all persisted meetings, newest-first (lightweight rows).
#[tauri::command]
#[specta::specta]
pub fn list_meetings(
    meeting_manager: State<Arc<MeetingManager>>,
) -> Result<Vec<MeetingListItem>, String> {
    meeting_manager
        .store()
        .list_meetings()
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

/// Delete a persisted meeting by id.
#[tauri::command]
#[specta::specta]
pub fn delete_meeting(meeting_manager: State<Arc<MeetingManager>>, id: i64) -> Result<(), String> {
    meeting_manager
        .store()
        .delete_meeting(id)
        .map_err(|e| format!("Failed to delete meeting: {}", e))
}
