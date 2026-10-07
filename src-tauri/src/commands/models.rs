use crate::managers::model::{ModelInfo, ModelManager};
use crate::managers::transcription::{ModelStateEvent, TranscriptionManager};
use crate::settings::{get_settings, update_settings, ModelUnloadTimeout};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};

#[tauri::command]
#[specta::specta]
pub async fn get_available_models(
    model_manager: State<'_, Arc<ModelManager>>,
) -> Result<Vec<ModelInfo>, String> {
    Ok(model_manager.get_available_models())
}

#[tauri::command]
#[specta::specta]
pub async fn get_model_info(
    model_manager: State<'_, Arc<ModelManager>>,
    model_id: String,
) -> Result<Option<ModelInfo>, String> {
    Ok(model_manager.get_model_info(&model_id))
}

#[tauri::command]
#[specta::specta]
pub async fn download_model(
    app_handle: AppHandle,
    model_manager: State<'_, Arc<ModelManager>>,
    model_id: String,
) -> Result<(), String> {
    let result = model_manager
        .download_model(&model_id)
        .await
        .map_err(|e| e.to_string());

    if let Err(ref error) = result {
        let _ = app_handle.emit(
            "model-download-failed",
            serde_json::json!({ "model_id": &model_id, "error": error }),
        );
    }

    result
}

#[tauri::command]
#[specta::specta]
pub async fn delete_model(
    app_handle: AppHandle,
    model_manager: State<'_, Arc<ModelManager>>,
    transcription_manager: State<'_, Arc<TranscriptionManager>>,
    model_id: String,
) -> Result<(), String> {
    let settings = get_settings(&app_handle);
    let is_resident = transcription_manager.get_current_model().as_deref() == Some(&model_id);

    // A running meeting owns the engine; deleting the model it is using (or
    // will finalize with) would fail the meeting mid-session.
    if transcription_manager.meeting_is_running()
        && (is_resident || settings.meeting_model_id() == model_id)
    {
        return Err("This model is in use by the running meeting. Stop the meeting first.".into());
    }

    // An in-flight download would recreate the .partial right after it is
    // deleted.
    if model_manager.is_downloading(&model_id) {
        model_manager
            .cancel_download(&model_id)
            .map_err(|e| e.to_string())?;
    }

    // Release the engine before its files go away (Windows cannot delete a
    // file that is mapped). If the deletion then fails, the model simply
    // reloads on next use.
    if is_resident {
        transcription_manager
            .unload_model()
            .map_err(|e| format!("Failed to unload model: {}", e))?;
    }

    model_manager
        .delete_model(&model_id)
        .map_err(|e| e.to_string())?;

    // Only now that the files are gone, stop pointing at them.
    update_settings(&app_handle, |settings| {
        if settings.selected_model == model_id {
            settings.selected_model = String::new();
        }
        if settings.meeting_selected_model.trim() == model_id {
            // Empty means "follow the dictation model".
            settings.meeting_selected_model = String::new();
        }
    });

    Ok(())
}

/// Shared logic for switching the active model, used by both the Tauri command
/// and the tray menu handler.
///
/// Validates the model, updates the persisted setting, and loads the model
/// unless the unload timeout is set to "Immediately" (in which case the model
/// will be loaded on-demand during the next transcription).
///
/// Refused while a meeting is running: only one engine is resident and the
/// meeting owns it, and with "meetings follow the dictation model" changing
/// the selection would also silently change the meeting's model mid-session.
pub fn switch_active_model(app: &AppHandle, model_id: &str) -> Result<(), String> {
    let model_manager = app.state::<Arc<ModelManager>>();
    let transcription_manager = app.state::<Arc<TranscriptionManager>>();

    if transcription_manager.meeting_is_running() {
        return Err("Can't switch models while a meeting is running.".to_string());
    }

    // Atomically claim the loading slot — prevents concurrent model loads
    // from tray double-clicks or overlapping commands. The guard resets the
    // flag on drop (including early returns, errors, and panics).
    let _loading_guard = transcription_manager
        .try_start_loading()
        .ok_or_else(|| "Model load already in progress".to_string())?;

    // Check if model exists and is available
    let model_info = model_manager
        .get_model_info(model_id)
        .ok_or_else(|| format!("Model not found: {}", model_id))?;

    if !model_info.is_downloaded {
        return Err(format!("Model not downloaded: {}", model_id));
    }

    // Persist the new selection early so the frontend sees the correct model
    // when it reacts to events emitted by load_model.
    let (old_model, old_language, unload_timeout) = update_settings(app, |settings| {
        let previous = (
            settings.selected_model.clone(),
            settings.selected_language.clone(),
            settings.model_unload_timeout,
        );
        settings.selected_model = model_id.to_string();

        // Reset language to auto if the new model doesn't support the currently selected language.
        // This prevents stale language settings from causing errors (e.g. Canary receiving zh-Hans)
        // and stops downstream processing (e.g. OpenCC) from running on an irrelevant language.
        if settings.selected_language != "auto"
            && !model_info.supported_languages.is_empty()
            && !model_info
                .supported_languages
                .contains(&settings.selected_language)
        {
            log::info!(
                "Resetting language from '{}' to 'auto' (not supported by {})",
                settings.selected_language,
                model_id
            );
            settings.selected_language = "auto".to_string();
        }
        previous
    });

    // Skip eager loading if unload is set to "Immediately" — the model
    // will be loaded on-demand during the next transcription.
    if unload_timeout == ModelUnloadTimeout::Immediately {
        // Notify frontend — load_model won't be called so no events
        // would otherwise be emitted.
        let _ = app.emit(
            "model-state-changed",
            ModelStateEvent {
                event_type: "selection_changed".to_string(),
                model_id: Some(model_id.to_string()),
                model_name: Some(model_info.name.clone()),
                error: None,
            },
        );
        log::info!(
            "Model selection changed to {} (not loading — unload set to Immediately).",
            model_id
        );
        return Ok(());
    }

    // Load the model. On failure, revert the persisted selection — the
    // language too, which may have been reset to "auto" for the new model.
    if let Err(e) = transcription_manager.load_model(model_id) {
        update_settings(app, |settings| {
            if settings.selected_model == model_id {
                settings.selected_model = old_model;
                settings.selected_language = old_language;
            }
        });
        return Err(e.to_string());
    }

    Ok(())
}
#[tauri::command]
#[specta::specta]
pub async fn set_active_model(
    app_handle: AppHandle,
    _model_manager: State<'_, Arc<ModelManager>>,
    _transcription_manager: State<'_, Arc<TranscriptionManager>>,
    model_id: String,
) -> Result<(), String> {
    switch_active_model(&app_handle, &model_id)
}

#[tauri::command]
#[specta::specta]
pub async fn get_current_model(app_handle: AppHandle) -> Result<String, String> {
    let settings = get_settings(&app_handle);
    Ok(settings.selected_model)
}

#[tauri::command]
#[specta::specta]
pub async fn get_transcription_model_status(
    transcription_manager: State<'_, Arc<TranscriptionManager>>,
) -> Result<Option<String>, String> {
    Ok(transcription_manager.get_current_model())
}

#[tauri::command]
#[specta::specta]
pub async fn is_model_loading(
    transcription_manager: State<'_, Arc<TranscriptionManager>>,
) -> Result<bool, String> {
    // Check if transcription manager has a loaded model
    let current_model = transcription_manager.get_current_model();
    Ok(current_model.is_none())
}

#[tauri::command]
#[specta::specta]
pub async fn has_any_models_available(
    model_manager: State<'_, Arc<ModelManager>>,
) -> Result<bool, String> {
    let models = model_manager.get_available_models();
    Ok(models.iter().any(|m| m.is_downloaded))
}

#[tauri::command]
#[specta::specta]
pub async fn has_any_models_or_downloads(
    model_manager: State<'_, Arc<ModelManager>>,
) -> Result<bool, String> {
    let models = model_manager.get_available_models();
    // Return true if any models are downloaded OR if any downloads are in progress
    Ok(models.iter().any(|m| m.is_downloaded))
}

#[tauri::command]
#[specta::specta]
pub async fn cancel_download(
    model_manager: State<'_, Arc<ModelManager>>,
    model_id: String,
) -> Result<(), String> {
    model_manager
        .cancel_download(&model_id)
        .map_err(|e| e.to_string())
}
