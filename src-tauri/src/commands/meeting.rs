// Meeting mode (Step 3) commands.
//
// Thin Tauri command wrappers around `MeetingManager`. The manager is stored in
// Tauri state as `Arc<MeetingManager>` (see `initialize_core_logic` in lib.rs).
//
// These commands are ADDITIVE and ISOLATED from the dictation flow.

use std::sync::Arc;

use tauri::{AppHandle, State};

use crate::meeting::{
    InterruptedMeeting, MeetingListItem, MeetingManager, MeetingRecord, MeetingState,
};

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
    summarize_current(&app, &meeting_manager, None).await
}

/// Like `summarize_meeting`, but with an optional template selector + custom
/// prompt override (Phase 2 item 4). `template` is matched first against a
/// configured `meeting_summary_templates` id; if no template matches it is
/// treated as a raw custom prompt. `None`/empty → the default prompt. The
/// resulting summary is persisted onto the last-saved meeting row.
#[tauri::command]
#[specta::specta]
pub async fn summarize_meeting_with(
    app: AppHandle,
    meeting_manager: State<'_, Arc<MeetingManager>>,
    template: Option<String>,
) -> Result<String, String> {
    summarize_current(&app, &meeting_manager, template.as_deref()).await
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
    let content = summarize_transcript_ext(
        &app,
        &record.transcript,
        template.as_deref(),
        record.notes.as_deref(),
    )
    .await?;
    meeting_manager
        .store()
        .update_summary(id, &content)
        .map_err(|e| format!("Failed to persist summary: {}", e))?;
    Ok(content)
}

/// Shared body for `summarize_meeting`/`summarize_meeting_with`: summarize the
/// manager's current transcript and persist onto the last-saved row.
async fn summarize_current(
    app: &AppHandle,
    meeting_manager: &Arc<MeetingManager>,
    template: Option<&str>,
) -> Result<String, String> {
    let transcript = meeting_manager.full_transcript();
    if transcript.trim().is_empty() {
        return Err("No transcript to summarize. Start and run a meeting first.".to_string());
    }
    let content = summarize_transcript_ext(app, &transcript, template, None).await?;
    if let Err(e) = meeting_manager.update_saved_summary(&content) {
        log::error!("{}", e);
    }
    Ok(content)
}

/// Summarize a meeting `transcript` into notes using the default prompt and the
/// SAME active post-processing provider/model/api-key the user configured for
/// dictation. Used by the on-stop auto-summarize path. Returns notes or an error.
pub(crate) async fn summarize_transcript(
    app: &AppHandle,
    transcript: &str,
) -> Result<String, String> {
    summarize_transcript_ext(app, transcript, None, None).await
}

/// Extended summarization: resolves the system prompt from an optional
/// `template` (a configured template id, else treated as a raw custom prompt,
/// else the default), optionally appends the user's own `notes` as extra
/// context, and sends to the active LLM provider. Returns the generated notes.
pub(crate) async fn summarize_transcript_ext(
    app: &AppHandle,
    transcript: &str,
    template: Option<&str>,
    notes: Option<&str>,
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

    // Resolve the system prompt: template id → custom prompt → default.
    let system_prompt = match template.map(str::trim).filter(|t| !t.is_empty()) {
        Some(sel) => settings
            .meeting_summary_templates
            .iter()
            .find(|t| t.id == sel)
            .map(|t| t.prompt.clone())
            .unwrap_or_else(|| sel.to_string()),
        None => DEFAULT_MEETING_SUMMARY_PROMPT.to_string(),
    };

    // Optionally include the user's own notes as additional context. Default
    // behavior (no notes) is unchanged.
    let notes_block = match notes.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => format!(
            "\n\nThe user also provided their own notes. Treat them as additional context and \
incorporate them where relevant:\n{}",
            n
        ),
        None => String::new(),
    };

    // Plain (non-structured) chat completion: instructions as system prompt,
    // transcript (+ optional notes) as the user message.
    let prompt = format!(
        "{}{}\n\nTranscript:\n{}",
        system_prompt, notes_block, transcript
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

/// Generate a short, human-readable title from a meeting `transcript` using the
/// active post-process LLM provider. Returns a single-line title (no quotes /
/// markdown). Errors if no provider is configured or the call fails — callers
/// (auto-title) treat that as a graceful fallback to the datetime title.
pub(crate) async fn generate_title(app: &AppHandle, transcript: &str) -> Result<String, String> {
    let settings = crate::settings::get_settings(app);
    let provider = settings
        .active_post_process_provider()
        .cloned()
        .ok_or_else(|| "No LLM provider configured for title generation.".to_string())?;
    let model = settings
        .post_process_models
        .get(&provider.id)
        .cloned()
        .unwrap_or_default();
    if model.trim().is_empty() {
        return Err(format!(
            "No model configured for provider '{}'.",
            provider.id
        ));
    }
    let api_key = settings
        .post_process_api_keys
        .get(&provider.id)
        .cloned()
        .unwrap_or_default();

    // Cap the transcript fed to the title prompt: the opening is plenty for a
    // title and keeps the request small.
    let snippet: String = transcript.chars().take(4000).collect();
    let prompt = format!(
        "Generate a short, descriptive title (at most 8 words) for the following meeting \
transcript. Respond in the SAME LANGUAGE as the transcript. Output ONLY the title text with no \
quotes, no markdown, and no trailing punctuation.\n\nTranscript:\n{}",
        snippet
    );

    match crate::llm_client::send_chat_completion(&provider, api_key, &model, prompt).await {
        Ok(Some(content)) => {
            // Sanitize: first non-empty line, strip surrounding quotes/markdown.
            let title = content
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .unwrap_or("")
                .trim_matches(|c| c == '"' || c == '\'' || c == '#' || c == '*')
                .trim()
                .to_string();
            if title.is_empty() {
                Err("The LLM returned an empty title.".to_string())
            } else {
                Ok(title)
            }
        }
        Ok(None) => Err("The LLM response contained no content.".to_string()),
        Err(e) => Err(format!("Failed to generate title: {}", e)),
    }
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

/// Delete a persisted meeting by id.
#[tauri::command]
#[specta::specta]
pub fn delete_meeting(meeting_manager: State<Arc<MeetingManager>>, id: i64) -> Result<(), String> {
    meeting_manager
        .store()
        .delete_meeting(id)
        .map_err(|e| format!("Failed to delete meeting: {}", e))
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
        .map_err(|e| format!("Failed to update meeting title: {}", e))
}

/// Save the user's own editable notes for a meeting (Phase 2 item 3). Distinct
/// from the AI `summary`; stored in the `notes` column.
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
        .map_err(|e| format!("Failed to update meeting notes: {}", e))
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
    Ok(render_meeting_markdown(&record))
}

/// Render a `MeetingRecord` as a Markdown document. Pure/testable.
fn render_meeting_markdown(record: &MeetingRecord) -> String {
    use crate::meeting::manager::TranscriptSource;
    use chrono::{DateTime, Local};
    use std::fmt::Write as _;

    let mut out = String::new();
    let _ = writeln!(out, "# {}", record.title.trim());
    out.push('\n');

    if let Some(dt) = DateTime::from_timestamp_millis(record.started_at) {
        let local = dt.with_timezone(&Local);
        let _ = writeln!(out, "- **Date:** {}", local.format("%B %e, %Y"));
        let _ = writeln!(out, "- **Time:** {}", local.format("%l:%M %p"));
    }
    let total_secs = (record.duration_ms / 1000).max(0);
    let _ = writeln!(
        out,
        "- **Duration:** {}h {}m {}s",
        total_secs / 3600,
        (total_secs % 3600) / 60,
        total_secs % 60
    );
    out.push('\n');

    if let Some(notes) = record.notes.as_deref() {
        if !notes.trim().is_empty() {
            let _ = writeln!(out, "## Notes\n\n{}\n", notes.trim());
        }
    }

    if let Some(summary) = record.summary.as_deref() {
        if !summary.trim().is_empty() {
            let _ = writeln!(out, "## Summary\n\n{}\n", summary.trim());
        }
    }

    let _ = writeln!(out, "## Transcript\n");
    if record.segments.is_empty() {
        // No per-segment labels available; emit the raw transcript.
        let _ = writeln!(out, "{}", record.transcript.trim());
    } else {
        let mut ordered: Vec<&_> = record.segments.iter().collect();
        ordered.sort_by_key(|s| s.timestamp_ms);
        for seg in ordered {
            let text = seg.text.trim();
            if text.is_empty() {
                continue;
            }
            let label = match seg.source {
                TranscriptSource::Mic => "You",
                TranscriptSource::System => "Others",
            };
            let ts = seg.timestamp_ms / 1000;
            let _ = writeln!(
                out,
                "- **[{:02}:{:02}] {}:** {}",
                ts / 60,
                ts % 60,
                label,
                text
            );
        }
    }

    out
}

/// List meetings interrupted by a crash/OS-kill (still in `recording` status),
/// newest-first. Each item reports whether its temp audio buffers still exist so
/// the UI can offer a high-quality re-finalize vs. salvaging the partial text.
#[tauri::command]
#[specta::specta]
pub fn list_interrupted_meetings(
    meeting_manager: State<Arc<MeetingManager>>,
) -> Result<Vec<InterruptedMeeting>, String> {
    meeting_manager
        .store()
        .list_interrupted()
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

#[cfg(test)]
mod tests {
    use super::render_meeting_markdown;
    use crate::meeting::manager::{TranscriptSegment, TranscriptSource};
    use crate::meeting::MeetingRecord;

    fn record_with(segments: Vec<TranscriptSegment>, transcript: &str) -> MeetingRecord {
        MeetingRecord {
            id: 1,
            started_at: 0,
            ended_at: 65_000,
            duration_ms: 65_000,
            title: "Weekly Sync".to_string(),
            transcript: transcript.to_string(),
            segments,
            summary: Some("AI summary text".to_string()),
            created_at: 0,
            audio_path: None,
            notes: Some("My own notes".to_string()),
            status: "completed".to_string(),
        }
    }

    #[test]
    fn markdown_includes_title_notes_summary_and_labeled_transcript() {
        let segments = vec![
            TranscriptSegment {
                text: "Hello team.".to_string(),
                timestamp_ms: 0,
                source: TranscriptSource::Mic,
            },
            TranscriptSegment {
                text: "Hi there.".to_string(),
                timestamp_ms: 5000,
                source: TranscriptSource::System,
            },
        ];
        let md = render_meeting_markdown(&record_with(segments, "Hello team. Hi there."));
        assert!(md.starts_with("# Weekly Sync"));
        assert!(md.contains("## Notes"));
        assert!(md.contains("My own notes"));
        assert!(md.contains("## Summary"));
        assert!(md.contains("AI summary text"));
        assert!(md.contains("## Transcript"));
        assert!(md.contains("You:** Hello team."));
        assert!(md.contains("Others:** Hi there."));
        // Duration line present (65s -> 0h 1m 5s).
        assert!(md.contains("0h 1m 5s"));
    }

    #[test]
    fn markdown_falls_back_to_raw_transcript_without_segments() {
        let md = render_meeting_markdown(&record_with(Vec::new(), "raw transcript body"));
        assert!(md.contains("## Transcript"));
        assert!(md.contains("raw transcript body"));
    }
}
