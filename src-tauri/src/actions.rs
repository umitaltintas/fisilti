#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use crate::apple_intelligence;
use crate::audio_feedback::{play_feedback_sound, play_feedback_sound_blocking, SoundType};
use crate::audio_toolkit::is_microphone_access_denied;
use crate::managers::audio::AudioRecordingManager;
use crate::managers::history::HistoryManager;
use crate::managers::transcription::TranscriptionManager;
use crate::settings::{get_settings, AppSettings, APPLE_INTELLIGENCE_PROVIDER_ID};
use crate::shortcut;
use crate::tray::{change_tray_icon, TrayIconState};
use crate::utils::{
    self, emit_dictation_error, show_processing_overlay, show_recording_overlay,
    show_transcribing_overlay, DictationStage,
};
use crate::TranscriptionCoordinator;
use ferrous_opencc::{config::BuiltinConfig, OpenCC};
use log::{debug, error, warn};
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tauri::Manager;
use tauri::{AppHandle, Emitter};

#[derive(Clone, serde::Serialize)]
struct RecordingErrorEvent {
    error_type: String,
    detail: Option<String>,
}

/// Drop guard that notifies the [`TranscriptionCoordinator`] when the
/// transcription pipeline finishes — whether it completes normally or panics.
struct FinishGuard(AppHandle);
impl Drop for FinishGuard {
    fn drop(&mut self) {
        if let Some(c) = self.0.try_state::<TranscriptionCoordinator>() {
            c.notify_processing_finished();
        }
    }
}

// Shortcut Action Trait
pub trait ShortcutAction: Send + Sync {
    fn start(&self, app: &AppHandle, binding_id: &str, shortcut_str: &str);
    fn stop(&self, app: &AppHandle, binding_id: &str, shortcut_str: &str);
}

// Transcribe Action
struct TranscribeAction {
    post_process: bool,
}

/// Field name for structured output JSON schema
const TRANSCRIPTION_FIELD: &str = "transcription";

/// Strip invisible Unicode characters that some LLMs may insert
fn strip_invisible_chars(s: &str) -> String {
    s.replace(['\u{200B}', '\u{200C}', '\u{200D}', '\u{FEFF}'], "")
}

/// Build a system prompt from the user's prompt template.
/// Removes `${output}` placeholder since the transcription is sent as the user message.
fn build_system_prompt(prompt_template: &str) -> String {
    prompt_template.replace("${output}", "").trim().to_string()
}

async fn post_process_transcription(settings: &AppSettings, transcription: &str) -> Option<String> {
    let provider = match settings.active_post_process_provider().cloned() {
        Some(provider) => provider,
        None => {
            debug!("Post-processing enabled but no provider is selected");
            return None;
        }
    };

    let model = settings
        .post_process_models
        .get(&provider.id)
        .cloned()
        .unwrap_or_default();

    if model.trim().is_empty() {
        debug!(
            "Post-processing skipped because provider '{}' has no model configured",
            provider.id
        );
        return None;
    }

    let selected_prompt_id = match &settings.post_process_selected_prompt_id {
        Some(id) => id.clone(),
        None => {
            debug!("Post-processing skipped because no prompt is selected");
            return None;
        }
    };

    let prompt = match settings
        .post_process_prompts
        .iter()
        .find(|prompt| prompt.id == selected_prompt_id)
    {
        Some(prompt) => prompt.prompt.clone(),
        None => {
            debug!(
                "Post-processing skipped because prompt '{}' was not found",
                selected_prompt_id
            );
            return None;
        }
    };

    if prompt.trim().is_empty() {
        debug!("Post-processing skipped because the selected prompt is empty");
        return None;
    }

    debug!(
        "Starting LLM post-processing with provider '{}' (model: {})",
        provider.id, model
    );

    let api_key = settings.post_process_key_for(&provider.id);

    if provider.supports_structured_output {
        debug!("Using structured outputs for provider '{}'", provider.id);

        let system_prompt = build_system_prompt(&prompt);
        let user_content = transcription.to_string();

        // Handle Apple Intelligence separately since it uses native Swift APIs
        if provider.id == APPLE_INTELLIGENCE_PROVIDER_ID {
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            {
                if !apple_intelligence::check_apple_intelligence_availability() {
                    debug!(
                        "Apple Intelligence selected but not currently available on this device"
                    );
                    return None;
                }

                let token_limit = model.trim().parse::<i32>().unwrap_or(0);
                return match apple_intelligence::process_text_with_system_prompt(
                    &system_prompt,
                    &user_content,
                    token_limit,
                ) {
                    Ok(result) => {
                        if result.trim().is_empty() {
                            debug!("Apple Intelligence returned an empty response");
                            None
                        } else {
                            let result = strip_invisible_chars(&result);
                            debug!(
                                "Apple Intelligence post-processing succeeded. Output length: {} chars",
                                result.len()
                            );
                            Some(result)
                        }
                    }
                    Err(err) => {
                        error!("Apple Intelligence post-processing failed: {}", err);
                        None
                    }
                };
            }

            #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
            {
                debug!("Apple Intelligence provider selected on unsupported platform");
                return None;
            }
        }

        // Define JSON schema for transcription output
        let json_schema = serde_json::json!({
            "type": "object",
            "properties": {
                (TRANSCRIPTION_FIELD): {
                    "type": "string",
                    "description": "The cleaned and processed transcription text"
                }
            },
            "required": [TRANSCRIPTION_FIELD],
            "additionalProperties": false
        });

        match crate::llm_client::send_chat_completion_with_schema(
            &provider,
            api_key.clone(),
            &model,
            user_content,
            Some(system_prompt),
            Some(json_schema),
        )
        .await
        {
            Ok(Some(content)) => {
                // Parse the JSON response to extract the transcription field
                match serde_json::from_str::<serde_json::Value>(&content) {
                    Ok(json) => {
                        if let Some(transcription_value) =
                            json.get(TRANSCRIPTION_FIELD).and_then(|t| t.as_str())
                        {
                            let result = strip_invisible_chars(transcription_value);
                            debug!(
                                "Structured output post-processing succeeded for provider '{}'. Output length: {} chars",
                                provider.id,
                                result.len()
                            );
                            return Some(result);
                        } else {
                            error!("Structured output response missing 'transcription' field");
                            return Some(strip_invisible_chars(&content));
                        }
                    }
                    Err(e) => {
                        error!(
                            "Failed to parse structured output JSON: {}. Returning raw content.",
                            e
                        );
                        return Some(strip_invisible_chars(&content));
                    }
                }
            }
            Ok(None) => {
                error!("LLM API response has no content");
                return None;
            }
            Err(e) => {
                warn!(
                    "Structured output failed for provider '{}': {}. Falling back to legacy mode.",
                    provider.id, e
                );
                // Fall through to legacy mode below
            }
        }
    }

    // Legacy mode: Replace ${output} variable in the prompt with the actual text
    let processed_prompt = prompt.replace("${output}", transcription);
    debug!("Processed prompt length: {} chars", processed_prompt.len());

    match crate::llm_client::send_chat_completion(&provider, api_key, &model, processed_prompt)
        .await
    {
        Ok(Some(content)) => {
            let content = strip_invisible_chars(&content);
            debug!(
                "LLM post-processing succeeded for provider '{}'. Output length: {} chars",
                provider.id,
                content.len()
            );
            Some(content)
        }
        Ok(None) => {
            error!("LLM API response has no content");
            None
        }
        Err(e) => {
            error!(
                "LLM post-processing failed for provider '{}': {}. Falling back to original transcription.",
                provider.id,
                e
            );
            None
        }
    }
}

async fn maybe_convert_chinese_variant(
    settings: &AppSettings,
    transcription: &str,
) -> Option<String> {
    // Check if language is set to Simplified or Traditional Chinese
    let is_simplified = settings.selected_language == "zh-Hans";
    let is_traditional = settings.selected_language == "zh-Hant";

    if !is_simplified && !is_traditional {
        debug!("selected_language is not Simplified or Traditional Chinese; skipping translation");
        return None;
    }

    debug!(
        "Starting Chinese translation using OpenCC for language: {}",
        settings.selected_language
    );

    // Use OpenCC to convert based on selected language
    let config = if is_simplified {
        // Convert Traditional Chinese to Simplified Chinese
        BuiltinConfig::Tw2sp
    } else {
        // Convert Simplified Chinese to Traditional Chinese
        BuiltinConfig::S2twp
    };

    match OpenCC::from_config(config) {
        Ok(converter) => {
            let converted = converter.convert(transcription);
            debug!(
                "OpenCC translation completed. Input length: {}, Output length: {}",
                transcription.len(),
                converted.len()
            );
            Some(converted)
        }
        Err(e) => {
            error!("Failed to initialize OpenCC converter: {}. Falling back to original transcription.", e);
            None
        }
    }
}

pub(crate) struct ProcessedTranscription {
    pub final_text: String,
    pub post_processed_text: Option<String>,
    pub post_process_prompt: Option<String>,
}

pub(crate) async fn process_transcription_output(
    app: &AppHandle,
    transcription: &str,
    post_process: bool,
) -> ProcessedTranscription {
    let settings = get_settings(app);
    let mut final_text = transcription.to_string();
    let mut post_processed_text: Option<String> = None;
    let mut post_process_prompt: Option<String> = None;

    if let Some(converted_text) = maybe_convert_chinese_variant(&settings, transcription).await {
        final_text = converted_text;
    }

    if post_process {
        if let Some(processed_text) = post_process_transcription(&settings, &final_text).await {
            post_processed_text = Some(processed_text.clone());
            final_text = processed_text;

            if let Some(prompt_id) = &settings.post_process_selected_prompt_id {
                if let Some(prompt) = settings
                    .post_process_prompts
                    .iter()
                    .find(|prompt| &prompt.id == prompt_id)
                {
                    post_process_prompt = Some(prompt.prompt.clone());
                }
            }
        }
    } else if final_text != transcription {
        post_processed_text = Some(final_text.clone());
    }

    ProcessedTranscription {
        final_text,
        post_processed_text,
        post_process_prompt,
    }
}

/// File name for a dictation's saved audio. Millisecond time plus a random
/// suffix: the old whole-second name let two dictations in the same second
/// overwrite each other's audio (and history rows point at the wrong file).
fn recording_file_name(unix_millis: i64, salt: u16) -> String {
    format!("fisilti-{unix_millis}-{salt:04x}.wav")
}

/// A few random bits without a dependency: std's per-process random hasher
/// keys, mixed with the time.
fn random_salt() -> u16 {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default(),
    );
    hasher.finish() as u16
}

/// Classify a transcription failure for the `dictation-error` event.
fn transcription_error_stage(err: &anyhow::Error) -> DictationStage {
    if err
        .downcast_ref::<crate::managers::transcription::ModelNotLoadedError>()
        .is_some()
    {
        DictationStage::ModelLoad
    } else {
        DictationStage::Transcription
    }
}

/// Return the UI to idle after a dictation ends, however it ended.
fn reset_dictation_ui(app: &AppHandle) {
    utils::hide_recording_overlay(app);
    change_tray_icon(app, TrayIconState::Idle);
}

impl ShortcutAction for TranscribeAction {
    fn start(&self, app: &AppHandle, binding_id: &str, _shortcut_str: &str) {
        let start_time = Instant::now();
        debug!("TranscribeAction::start called for binding: {}", binding_id);

        // Load model in the background
        let tm = app.state::<Arc<TranscriptionManager>>();
        tm.initiate_model_load();

        // Streaming models transcribe while the user speaks, so the session has
        // to exist before the first frame is captured. Returns false for every
        // other model, leaving the buffered path untouched.
        let streaming = crate::dictation_live::begin_if_selected(app);

        let binding_id = binding_id.to_string();
        change_tray_icon(app, TrayIconState::Recording);
        show_recording_overlay(app);

        let rm = app.state::<Arc<AudioRecordingManager>>();

        // Get the microphone mode to determine audio feedback timing
        let settings = get_settings(app);
        let is_always_on = settings.always_on_microphone;
        debug!("Microphone mode - always_on: {}", is_always_on);

        let recording_start_time = Instant::now();
        let recording_result = rm.try_start_recording(&binding_id);

        match recording_result {
            Ok(()) => {
                debug!("Recording started in {:?}", recording_start_time.elapsed());
                // Play the start sound, then mute. The mute is tied to this
                // recording's session so a late one (the sound can take a
                // while) is dropped if the recording already ended.
                let session = rm.current_session();
                let app_clone = app.clone();
                let rm_clone = Arc::clone(&rm);
                std::thread::spawn(move || {
                    if !is_always_on {
                        // Small delay to ensure the on-demand microphone stream is active
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                    debug!("Handling delayed audio feedback/mute sequence");
                    // Helper handles disabled audio feedback by returning early, so we reuse it
                    // to keep mute sequencing consistent in every mode.
                    play_feedback_sound_blocking(&app_clone, SoundType::Start);
                    rm_clone.apply_mute(session);
                });

                // Dynamically register the cancel shortcut in a separate task to avoid deadlock
                shortcut::register_cancel_shortcut(app);
            }
            Err(err) => {
                debug!("Failed to start recording: {}", err);
                // The streaming session would otherwise stay attached and
                // pick up the NEXT dictation's audio.
                if streaming {
                    crate::dictation_live::abort_active(app);
                }
                // Starting failed (for example due to blocked microphone permissions).
                // Revert UI state so we don't stay stuck in the recording overlay.
                reset_dictation_ui(app);
                let error_type = if is_microphone_access_denied(&err) {
                    "microphone_permission_denied"
                } else {
                    "unknown"
                };
                let _ = app.emit(
                    "recording-error",
                    RecordingErrorEvent {
                        error_type: error_type.to_string(),
                        detail: Some(err),
                    },
                );
            }
        }

        debug!(
            "TranscribeAction::start completed in {:?}",
            start_time.elapsed()
        );
    }

    fn stop(&self, app: &AppHandle, binding_id: &str, _shortcut_str: &str) {
        // Unregister the cancel shortcut when transcription stops
        shortcut::unregister_cancel_shortcut(app);

        let stop_time = Instant::now();
        debug!("TranscribeAction::stop called for binding: {}", binding_id);

        let ah = app.clone();
        let rm = Arc::clone(&app.state::<Arc<AudioRecordingManager>>());
        let tm = Arc::clone(&app.state::<Arc<TranscriptionManager>>());
        let hm = Arc::clone(&app.state::<Arc<HistoryManager>>());

        change_tray_icon(app, TrayIconState::Transcribing);
        show_transcribing_overlay(app);

        // Unmute before playing audio feedback so the stop sound is audible
        rm.remove_mute();

        // Play audio feedback for recording stop
        play_feedback_sound(app, SoundType::Stop);

        let binding_id = binding_id.to_string(); // Clone binding_id for the async task
        let post_process = self.post_process;

        tauri::async_runtime::spawn(async move {
            let _guard = FinishGuard(ah.clone());
            debug!(
                "Starting async transcription task for binding: {}",
                binding_id
            );

            // Everything below that blocks (stopping the recorder, draining
            // the streaming tail, running the model, pasting) runs on the
            // blocking pool, not on an async worker thread.
            let stop_recording_time = Instant::now();
            let stopped = {
                let rm = Arc::clone(&rm);
                let binding_id = binding_id.clone();
                tauri::async_runtime::spawn_blocking(move || {
                    rm.stop_recording_with_diagnostics(&binding_id)
                })
                .await
                .unwrap_or_else(|e| {
                    error!("Stopping the recording panicked: {}", e);
                    None
                })
            };

            let Some((samples, diagnostics)) = stopped else {
                debug!("No samples retrieved from recording stop");
                crate::dictation_live::abort_active(&ah);
                reset_dictation_ui(&ah);
                return;
            };
            debug!(
                "Recording stopped and samples retrieved in {:?}, sample count: {}",
                stop_recording_time.elapsed(),
                samples.len()
            );

            if diagnostics.device_failed {
                emit_dictation_error(
                    &ah,
                    DictationStage::Recording,
                    "The microphone stopped delivering audio (was it disconnected or switched?). The recording may be incomplete.",
                );
            }

            if samples.is_empty() {
                debug!("Recording produced no audio samples; skipping persistence");
                // The streaming session has nothing to wait for either.
                crate::dictation_live::abort_active(&ah);
                if !diagnostics.device_failed {
                    emit_dictation_error(
                        &ah,
                        DictationStage::NoSpeech,
                        "No speech was detected in the recording.",
                    );
                }
                reset_dictation_ui(&ah);
                return;
            }

            // Save WAV concurrently with transcription
            let sample_count = samples.len();
            let file_name =
                recording_file_name(chrono::Utc::now().timestamp_millis(), random_salt());
            let wav_path = hm.recordings_dir().join(&file_name);
            let wav_path_for_verify = wav_path.clone();
            let samples_for_wav = samples.clone();
            let wav_handle = tauri::async_runtime::spawn_blocking(move || {
                crate::audio_toolkit::save_wav_file(&wav_path, &samples_for_wav)
            });

            // Transcribe concurrently with WAV save. A streaming
            // session has already done the work while the user spoke,
            // so prefer its text; an empty result means the socket
            // never delivered anything, and the buffered path still has
            // the audio to fall back on.
            let transcription_time = Instant::now();
            let transcription_result = {
                let ah = ah.clone();
                let tm = Arc::clone(&tm);
                tauri::async_runtime::spawn_blocking(move || {
                    let streamed = crate::dictation_live::finish_active(&ah)
                        .filter(|text| !text.trim().is_empty());
                    match streamed {
                        Some(text) => Ok(text),
                        None => tm.transcribe(samples),
                    }
                })
                .await
                .unwrap_or_else(|e| Err(anyhow::anyhow!("Transcription task panicked: {}", e)))
            };

            // Await WAV save and verify
            let wav_saved = match wav_handle.await {
                Ok(Ok(())) => {
                    match crate::audio_toolkit::verify_wav_file(&wav_path_for_verify, sample_count)
                    {
                        Ok(()) => true,
                        Err(e) => {
                            error!("WAV verification failed: {}", e);
                            false
                        }
                    }
                }
                Ok(Err(e)) => {
                    error!("Failed to save WAV file: {}", e);
                    false
                }
                Err(e) => {
                    error!("WAV save task panicked: {}", e);
                    false
                }
            };

            match transcription_result {
                Ok(transcription) => {
                    debug!(
                        "Transcription completed in {:?}: '{}'",
                        transcription_time.elapsed(),
                        transcription
                    );

                    if post_process {
                        show_processing_overlay(&ah);
                    }
                    let processed =
                        process_transcription_output(&ah, &transcription, post_process).await;

                    // Save to history if WAV was saved
                    if wav_saved {
                        if let Err(err) = hm.save_entry(
                            file_name,
                            transcription,
                            post_process,
                            processed.post_processed_text.clone(),
                            processed.post_process_prompt.clone(),
                        ) {
                            error!("Failed to save history entry: {}", err);
                        }
                    }

                    if processed.final_text.trim().is_empty() {
                        emit_dictation_error(
                            &ah,
                            DictationStage::NoSpeech,
                            "No speech was recognized in the recording.",
                        );
                        reset_dictation_ui(&ah);
                    } else {
                        let paste_time = Instant::now();
                        let final_text = processed.final_text;
                        let ah_paste = ah.clone();
                        let paste_result = tauri::async_runtime::spawn_blocking(move || {
                            utils::paste(final_text, ah_paste)
                        })
                        .await
                        .unwrap_or_else(|e| Err(format!("Paste task panicked: {}", e)));
                        match paste_result {
                            Ok(()) => {
                                debug!("Text pasted successfully in {:?}", paste_time.elapsed())
                            }
                            Err(e) => {
                                error!("Failed to paste transcription: {}", e);
                                emit_dictation_error(
                                    &ah,
                                    DictationStage::Paste,
                                    format!("The text could not be inserted: {e}"),
                                );
                            }
                        }
                        reset_dictation_ui(&ah);
                    }
                }
                Err(err) => {
                    error!("Dictation transcription failed: {:#}", err);
                    emit_dictation_error(&ah, transcription_error_stage(&err), err.to_string());
                    // Save entry with empty text so user can retry
                    if wav_saved {
                        if let Err(save_err) =
                            hm.save_entry(file_name, String::new(), post_process, None, None)
                        {
                            error!("Failed to save failed history entry: {}", save_err);
                        }
                    }
                    reset_dictation_ui(&ah);
                }
            }
        });

        debug!(
            "TranscribeAction::stop completed in {:?}",
            stop_time.elapsed()
        );
    }
}
// Cancel Action
struct CancelAction;

impl ShortcutAction for CancelAction {
    fn start(&self, app: &AppHandle, _binding_id: &str, _shortcut_str: &str) {
        utils::cancel_current_operation(app);
    }

    fn stop(&self, _app: &AppHandle, _binding_id: &str, _shortcut_str: &str) {
        // Nothing to do on stop for cancel
    }
}

// Test Action
struct TestAction;

impl ShortcutAction for TestAction {
    fn start(&self, app: &AppHandle, binding_id: &str, shortcut_str: &str) {
        log::info!(
            "Shortcut ID '{}': Started - {} (App: {})", // Changed "Pressed" to "Started" for consistency
            binding_id,
            shortcut_str,
            app.package_info().name
        );
    }

    fn stop(&self, app: &AppHandle, binding_id: &str, shortcut_str: &str) {
        log::info!(
            "Shortcut ID '{}': Stopped - {} (App: {})", // Changed "Released" to "Stopped" for consistency
            binding_id,
            shortcut_str,
            app.package_info().name
        );
    }
}

// Toggle Meeting Action
//
// Opt-in global shortcut to start/stop a meeting recording without opening the
// window. Toggles on key press only (start when idle, stop when running) via
// the SAME shared helper the `start_meeting`/`stop_meeting` commands and the
// tray menu item use, so there is no duplicated start/stop logic. stop() can
// block (finalize pass), so the work is done on a background thread.
struct ToggleMeetingAction;

impl ShortcutAction for ToggleMeetingAction {
    fn start(&self, app: &AppHandle, _binding_id: &str, _shortcut_str: &str) {
        let app_clone = app.clone();
        std::thread::spawn(move || {
            crate::commands::meeting::toggle_meeting_from_app(&app_clone);
        });
    }

    fn stop(&self, _app: &AppHandle, _binding_id: &str, _shortcut_str: &str) {
        // Toggle fires on press only; nothing to do on release.
    }
}

// Static Action Map
pub static ACTION_MAP: Lazy<HashMap<String, Arc<dyn ShortcutAction>>> = Lazy::new(|| {
    let mut map = HashMap::new();
    map.insert(
        "transcribe".to_string(),
        Arc::new(TranscribeAction {
            post_process: false,
        }) as Arc<dyn ShortcutAction>,
    );
    map.insert(
        "transcribe_with_post_process".to_string(),
        Arc::new(TranscribeAction { post_process: true }) as Arc<dyn ShortcutAction>,
    );
    map.insert(
        "cancel".to_string(),
        Arc::new(CancelAction) as Arc<dyn ShortcutAction>,
    );
    map.insert(
        "test".to_string(),
        Arc::new(TestAction) as Arc<dyn ShortcutAction>,
    );
    map.insert(
        "toggle_meeting".to_string(),
        Arc::new(ToggleMeetingAction) as Arc<dyn ShortcutAction>,
    );
    map
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_names_are_millisecond_precise_and_salted() {
        assert_eq!(
            recording_file_name(1_700_000_000_123, 0x0a1b),
            "fisilti-1700000000123-0a1b.wav"
        );
        // Same millisecond, different salt: distinct files.
        assert_ne!(
            recording_file_name(1_700_000_000_123, 1),
            recording_file_name(1_700_000_000_123, 2)
        );
    }

    #[test]
    fn a_missing_model_is_reported_as_a_load_failure() {
        let err = anyhow::Error::new(crate::managers::transcription::ModelNotLoadedError(
            "nope".to_string(),
        ));
        assert_eq!(transcription_error_stage(&err), DictationStage::ModelLoad);
        let err = anyhow::anyhow!("Whisper transcription failed");
        assert_eq!(
            transcription_error_stage(&err),
            DictationStage::Transcription
        );
    }
}
