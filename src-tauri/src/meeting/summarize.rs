// LLM helpers for meetings: the summary ("meeting notes") and the auto-title.
//
// Both reuse the post-processing provider/model/API key the user configured
// for dictation, read-only — nothing here changes dictation behaviour.

use tauri::AppHandle;

/// Default system prompt for summarizing a meeting transcript into notes.
///
/// Instructs the model to answer in the SAME language as the transcript (so a
/// Turkish transcript yields Turkish notes) and to produce a short summary,
/// key points, decisions, and action items.
pub(crate) const DEFAULT_MEETING_SUMMARY_PROMPT: &str = "You are an assistant that writes clear, concise meeting notes from a raw meeting transcript. \
Respond in the SAME LANGUAGE as the transcript (do not translate). \
Produce well-structured notes with the following sections, using the section names in the transcript's language:\n\
1. Summary - a short paragraph summarizing the meeting.\n\
2. Key discussion points - a bullet list of the main topics discussed.\n\
3. Decisions - a bullet list of decisions made (or note that none were made).\n\
4. Action items - a bullet list of follow-up tasks, with the responsible person if mentioned.\n\
Only use information present in the transcript. Do not invent details.";

/// Summarize a meeting `transcript` into notes using the default prompt.
pub(crate) async fn summarize_transcript(
    app: &AppHandle,
    transcript: &str,
    notes: Option<&str>,
) -> Result<String, String> {
    summarize_transcript_ext(app, transcript, None, notes).await
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

    let api_key = settings.post_process_key_for(&provider.id);
    let system_prompt = resolve_system_prompt(&settings.meeting_summary_templates, template);
    let prompt = build_summary_prompt(&system_prompt, notes, transcript);

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

/// System prompt for a summary: a configured template id → that template's
/// prompt; any other non-empty text → used as a custom prompt; else default.
fn resolve_system_prompt(
    templates: &[crate::settings::MeetingSummaryTemplate],
    template: Option<&str>,
) -> String {
    match template.map(str::trim).filter(|t| !t.is_empty()) {
        Some(sel) => templates
            .iter()
            .find(|t| t.id == sel)
            .map(|t| t.prompt.clone())
            .unwrap_or_else(|| sel.to_string()),
        None => DEFAULT_MEETING_SUMMARY_PROMPT.to_string(),
    }
}

/// Plain (non-structured) chat prompt: instructions, the user's own notes as
/// optional context, then the transcript.
fn build_summary_prompt(system_prompt: &str, notes: Option<&str>, transcript: &str) -> String {
    let notes_block = match notes.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => format!(
            "\n\nThe user also provided their own notes. Treat them as additional context and \
incorporate them where relevant:\n{}",
            n
        ),
        None => String::new(),
    };
    format!(
        "{}{}\n\nTranscript:\n{}",
        system_prompt, notes_block, transcript
    )
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
    let api_key = settings.post_process_key_for(&provider.id);

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
            let title = clean_title(&content);
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

/// First non-empty line, stripped of surrounding quotes/markdown.
fn clean_title(content: &str) -> String {
    content
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .trim_matches(|c| c == '"' || c == '\'' || c == '#' || c == '*')
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_are_included_only_when_present() {
        let with = build_summary_prompt("SYS", Some("  my notes "), "words");
        assert!(with.starts_with("SYS"));
        assert!(with.contains("their own notes"));
        assert!(with.contains("my notes"));
        assert!(with.ends_with("Transcript:\nwords"));
        let without = build_summary_prompt("SYS", Some("   "), "words");
        assert_eq!(without, "SYS\n\nTranscript:\nwords");
    }

    #[test]
    fn template_selection_falls_back_to_custom_then_default() {
        let templates = vec![crate::settings::MeetingSummaryTemplate {
            id: "brief".into(),
            name: "Brief".into(),
            prompt: "BRIEF PROMPT".into(),
        }];
        assert_eq!(
            resolve_system_prompt(&templates, Some("brief")),
            "BRIEF PROMPT"
        );
        assert_eq!(
            resolve_system_prompt(&templates, Some("Write haiku")),
            "Write haiku"
        );
        assert_eq!(
            resolve_system_prompt(&templates, None),
            DEFAULT_MEETING_SUMMARY_PROMPT
        );
    }

    #[test]
    fn titles_lose_quotes_markdown_and_extra_lines() {
        assert_eq!(clean_title("\n\"Q3 Planning\"\nextra"), "Q3 Planning");
        assert_eq!(clean_title("**Roadmap**"), "Roadmap");
        assert_eq!(clean_title("# Roadmap"), "Roadmap");
        assert_eq!(clean_title("  "), "");
    }
}
