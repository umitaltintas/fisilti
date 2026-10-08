use crate::managers::history::{HistoryEntry, HistoryManager};
use crate::managers::model::ModelManager;
use crate::managers::transcription::TranscriptionManager;
use crate::meeting::{MeetingManager, MeetingState};
use crate::settings;
use crate::tray_i18n::get_tray_translations;
use log::{error, info, warn};
use std::sync::Arc;
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::TrayIcon;
use tauri::{AppHandle, Manager, Theme};
use tauri_plugin_clipboard_manager::ClipboardExt;

#[derive(Clone, Debug, PartialEq)]
pub enum TrayIconState {
    Idle,
    Recording,
    Transcribing,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AppTheme {
    Dark,
    Light,
    Colored, // Pink/colored theme for Linux
}

/// Gets the current app theme, with Linux defaulting to Colored theme
pub fn get_current_theme(app: &AppHandle) -> AppTheme {
    if cfg!(target_os = "linux") {
        // On Linux, always use the colored theme
        AppTheme::Colored
    } else {
        // On other platforms, map system theme to our app theme
        if let Some(main_window) = app.get_webview_window("main") {
            match main_window.theme().unwrap_or(Theme::Dark) {
                Theme::Light => AppTheme::Light,
                Theme::Dark => AppTheme::Dark,
                _ => AppTheme::Dark, // Default fallback
            }
        } else {
            AppTheme::Dark
        }
    }
}

/// Gets the appropriate icon path for the given theme and state
pub fn get_icon_path(theme: AppTheme, state: TrayIconState) -> &'static str {
    match (theme, state) {
        // Dark theme uses light icons
        (AppTheme::Dark, TrayIconState::Idle) => "resources/tray_idle.png",
        (AppTheme::Dark, TrayIconState::Recording) => "resources/tray_recording.png",
        (AppTheme::Dark, TrayIconState::Transcribing) => "resources/tray_transcribing.png",
        // Light theme uses dark icons
        (AppTheme::Light, TrayIconState::Idle) => "resources/tray_idle_dark.png",
        (AppTheme::Light, TrayIconState::Recording) => "resources/tray_recording_dark.png",
        (AppTheme::Light, TrayIconState::Transcribing) => "resources/tray_transcribing_dark.png",
        // Colored theme uses pink icons (for Linux)
        (AppTheme::Colored, TrayIconState::Idle) => "resources/fisilti.png",
        (AppTheme::Colored, TrayIconState::Recording) => "resources/recording.png",
        (AppTheme::Colored, TrayIconState::Transcribing) => "resources/transcribing.png",
    }
}

pub fn change_tray_icon(app: &AppHandle, icon: TrayIconState) {
    let tray = app.state::<TrayIcon>();
    let theme = get_current_theme(app);

    // A running meeting keeps the recording icon even when dictation reports
    // Idle, so the meeting indicator survives a dictation-driven Idle reset.
    let effective = if icon == TrayIconState::Idle && meeting_is_active(app) {
        TrayIconState::Recording
    } else {
        icon.clone()
    };

    let icon_path = get_icon_path(theme, effective.clone());

    let _ = tray.set_icon(Some(
        Image::from_path(
            app.path()
                .resolve(icon_path, tauri::path::BaseDirectory::Resource)
                .expect("failed to resolve"),
        )
        .expect("failed to set icon"),
    ));

    // Update menu based on the EFFECTIVE state (not the requested `icon`): when a
    // meeting overrides a dictation-driven Idle back to Recording, the menu must
    // build the Recording layout too, or the icon and menu disagree.
    update_tray_menu(app, &effective, None);
}

/// Reflect the current meeting recording state in the tray: swap the icon to a
/// recording variant + set a "Recording…" tooltip/title while a meeting runs,
/// and restore the idle icon/tooltip when it stops. Also refreshes the menu so
/// the Start/Stop Meeting label flips. Called from the `meeting-state-changed`
/// listener so BOTH tray- and UI-initiated meetings update the indicator.
pub fn update_meeting_indicator(app: &AppHandle) {
    let active = meeting_is_active(app);
    let tray = app.state::<TrayIcon>();

    let theme = get_current_theme(app);
    let icon_state = if active {
        TrayIconState::Recording
    } else {
        TrayIconState::Idle
    };
    let icon_path = get_icon_path(theme, icon_state.clone());
    if let Ok(resolved) = app
        .path()
        .resolve(icon_path, tauri::path::BaseDirectory::Resource)
    {
        if let Ok(image) = Image::from_path(resolved) {
            let _ = tray.set_icon(Some(image));
        }
    }

    // Tooltip/title indicator. The title is hidden on most platforms but the
    // tooltip is widely shown on hover; set both for good measure.
    //
    // IMPORTANT: clear with an EMPTY string, not `None`. On macOS `set_title(None)`
    // / `set_tooltip(None)` does NOT remove an existing value (the previous
    // "Recording…" text persists in the menu bar even after the icon reverts to
    // idle); passing `Some("")` actually clears the displayed text.
    let strings = get_tray_translations(Some(settings::get_settings(app).app_language));
    let tooltip = if active {
        strings.recording_indicator.clone()
    } else {
        String::new()
    };
    let _ = tray.set_tooltip(Some(tooltip.as_str()));
    #[cfg(target_os = "macos")]
    {
        let title = if active {
            strings.recording_indicator.as_str()
        } else {
            ""
        };
        let _ = tray.set_title(Some(title));
    }

    // Refresh the menu so the Start/Stop Meeting label reflects the new state.
    update_tray_menu(app, &icon_state, None);
}

/// Whether a meeting session is currently running. Resolved from the managed
/// `Arc<MeetingManager>` if present (it always is after core init); defaults to
/// `false` when the manager isn't available yet (early startup).
fn meeting_is_active(app: &AppHandle) -> bool {
    app.try_state::<Arc<MeetingManager>>()
        .map(|m| m.status() == MeetingState::Running)
        .unwrap_or(false)
}

pub fn update_tray_menu(app: &AppHandle, state: &TrayIconState, locale: Option<&str>) {
    let settings = settings::get_settings(app);

    let locale = locale.unwrap_or(&settings.app_language);
    let strings = get_tray_translations(Some(locale.to_string()));

    // Dictation quick-toggle. Dictation is the app's primary feature but used
    // to be reachable only through the global shortcut — the tray offered
    // "Cancel" and nothing to start with. Mirrors the meeting item below.
    //
    // No accelerator: the binding is user-configurable and lives in the global
    // shortcut plugin, so hard-coding one here would either lie or fight it.
    let dictation_label = if *state == TrayIconState::Recording {
        &strings.stop_dictation
    } else {
        &strings.start_dictation
    };
    // While a transcription is already running there is nothing to start or
    // stop — only "Cancel" applies, and it is in the same menu.
    let toggle_dictation_i = MenuItem::with_id(
        app,
        "toggle_dictation",
        dictation_label,
        *state != TrayIconState::Transcribing,
        None::<&str>,
    )
    .expect("failed to create toggle dictation item");

    // Meeting quick-start item: "Start Meeting" when idle, "Stop Meeting" while
    // a meeting is running. Capture is macOS-only; on other platforms keep the
    // item present but disabled so the menu layout stays cross-platform and the
    // user gets a visible hint rather than a silent no-op.
    let meeting_active = meeting_is_active(app);
    // While the last meeting is being saved there is nothing to start or stop.
    #[cfg(target_os = "macos")]
    let meeting_enabled = app
        .try_state::<Arc<MeetingManager>>()
        .is_none_or(|m| m.status() != MeetingState::Finalizing);
    #[cfg(not(target_os = "macos"))]
    let meeting_enabled = false;
    let meeting_label = if meeting_active {
        &strings.stop_meeting
    } else {
        &strings.start_meeting
    };
    let toggle_meeting_i = MenuItem::with_id(
        app,
        "toggle_meeting",
        meeting_label,
        meeting_enabled,
        None::<&str>,
    )
    .expect("failed to create toggle meeting item");

    // Opens the main window on the Meeting section (past transcripts list).
    let meetings_i = MenuItem::with_id(app, "meetings", &strings.meetings, true, None::<&str>)
        .expect("failed to create meetings item");

    // The dictation counterpart to "Meetings…". Both archives are one click
    // from the tray, and both say which archive they are.
    let history_i = MenuItem::with_id(app, "history", &strings.history, true, None::<&str>)
        .expect("failed to create history item");

    // Platform-specific accelerators
    #[cfg(target_os = "macos")]
    let (settings_accelerator, quit_accelerator) = (Some("Cmd+,"), Some("Cmd+Q"));
    #[cfg(not(target_os = "macos"))]
    let (settings_accelerator, quit_accelerator) = (Some("Ctrl+,"), Some("Ctrl+Q"));

    // Create common menu items
    let version_label = if cfg!(debug_assertions) {
        format!("Fısıltı v{} (Dev)", env!("CARGO_PKG_VERSION"))
    } else {
        format!("Fısıltı v{}", env!("CARGO_PKG_VERSION"))
    };
    let version_i = MenuItem::with_id(app, "version", &version_label, false, None::<&str>)
        .expect("failed to create version item");
    let settings_i = MenuItem::with_id(
        app,
        "settings",
        &strings.settings,
        true,
        settings_accelerator,
    )
    .expect("failed to create settings item");
    let check_updates_i = MenuItem::with_id(
        app,
        "check_updates",
        &strings.check_updates,
        settings.update_checks_enabled,
        None::<&str>,
    )
    .expect("failed to create check updates item");
    let copy_last_transcript_i = MenuItem::with_id(
        app,
        "copy_last_transcript",
        &strings.copy_last_transcript,
        true,
        None::<&str>,
    )
    .expect("failed to create copy last transcript item");
    let model_loaded = app.state::<Arc<TranscriptionManager>>().is_model_loaded();
    let quit_i = MenuItem::with_id(app, "quit", &strings.quit, true, quit_accelerator)
        .expect("failed to create quit item");
    let separator = || PredefinedMenuItem::separator(app).expect("failed to create separator");

    // Build the model submenu. Cloud entries report `is_downloaded == true` so
    // they land here too; since cloud models arrived, a single alphabetical
    // list interleaved "Gemini 2.5 Flash (Cloud)" with "Whisper Small" and gave
    // no hint which ones need the network. Split them the same way the Models
    // page does, under disabled header rows.
    let model_manager = app.state::<Arc<ModelManager>>();
    let models = model_manager.get_available_models();
    let current_model_id = &settings.selected_model;

    // The two "Custom OpenRouter model" entries do nothing until a slug is
    // configured in Settings → Models, so hide them until then — unless one is
    // already active, which must stay visible and checked.
    let custom_slug_set = !settings.openrouter_custom_model.trim().is_empty();
    let mut selectable: Vec<_> = models
        .into_iter()
        .filter(|m| m.is_downloaded)
        .filter(|m| {
            let is_unconfigured_custom =
                matches!(m.id.as_str(), "openrouter-custom" | "openrouter-asr-custom")
                    && !custom_slug_set;
            !is_unconfigured_custom || m.id == *current_model_id
        })
        .collect();
    selectable.sort_by(|a, b| a.name.cmp(&b.name));

    let (cloud_models, on_device_models): (Vec<_>, Vec<_>) = selectable
        .into_iter()
        .partition(|m| m.engine_type.is_cloud());

    // "Model: Whisper Small" rather than a bare "Whisper Small", which reads
    // like a command rather than the current value.
    let active_model_name = on_device_models
        .iter()
        .chain(cloud_models.iter())
        .find(|m| m.id == *current_model_id)
        .map(|m| m.name.clone());
    let submenu_label = match &active_model_name {
        Some(name) => format!("{}: {}", strings.model, name),
        None => strings.model.clone(),
    };

    let model_submenu = {
        let submenu = Submenu::with_id(app, "model_submenu", &submenu_label, true)
            .expect("failed to create model submenu");

        let section = |heading: &str, group: &[crate::managers::model::ModelInfo]| {
            if group.is_empty() {
                return;
            }
            let header = MenuItem::with_id(
                app,
                format!("model_heading:{heading}"),
                heading,
                false,
                None::<&str>,
            )
            .expect("failed to create model heading");
            let _ = submenu.append(&header);
            for model in group {
                let is_active = model.id == *current_model_id;
                let item_id = format!("model_select:{}", model.id);
                let item = CheckMenuItem::with_id(
                    app,
                    &item_id,
                    &model.name,
                    true,
                    is_active,
                    None::<&str>,
                )
                .expect("failed to create model item");
                let _ = submenu.append(&item);
            }
        };

        section(&strings.models_on_device, &on_device_models);
        section(&strings.models_cloud, &cloud_models);

        submenu
    };

    // Unloading a model by hand is a memory-management chore the app already
    // does on a timer (`model_unload_timeout`), so it only earns a menu slot in
    // debug mode — same rule as the Developer group under Advanced.
    let unload_model_i = MenuItem::with_id(
        app,
        "unload_model",
        &strings.unload_model,
        model_loaded,
        None::<&str>,
    )
    .expect("failed to create unload model item");

    // One layout for every state, in a fixed order: what you can do now, then
    // what you can open, then what you can configure. Items change label or go
    // disabled between states instead of appearing and disappearing, so muscle
    // memory keeps working while a recording runs.
    let cancel_i = MenuItem::with_id(app, "cancel", &strings.cancel, true, None::<&str>)
        .expect("failed to create cancel item");
    // A separator is a real platform menu item and cannot sit in two places, so
    // each one is its own instance rather than a reused reference.
    let (sep1, sep2, sep3, sep4, sep5) = (
        separator(),
        separator(),
        separator(),
        separator(),
        separator(),
    );

    let mut items: Vec<&dyn IsMenuItem<tauri::Wry>> = vec![&version_i, &sep1];
    items.push(&toggle_dictation_i);
    items.push(&toggle_meeting_i);
    if *state != TrayIconState::Idle {
        items.push(&cancel_i);
    }
    items.push(&sep2);
    items.push(&copy_last_transcript_i);
    items.push(&history_i);
    items.push(&meetings_i);
    items.push(&sep3);
    items.push(&model_submenu);
    if settings.debug_mode {
        items.push(&unload_model_i);
    }
    items.push(&sep4);
    items.push(&settings_i);
    items.push(&check_updates_i);
    items.push(&sep5);
    items.push(&quit_i);

    let menu = Menu::with_items(app, &items).expect("failed to create menu");

    let tray = app.state::<TrayIcon>();
    let _ = tray.set_menu(Some(menu));
    let _ = tray.set_icon_as_template(true);
}

fn last_transcript_text(entry: &HistoryEntry) -> &str {
    entry
        .post_processed_text
        .as_deref()
        .unwrap_or(&entry.transcription_text)
}

pub fn set_tray_visibility(app: &AppHandle, visible: bool) {
    let tray = app.state::<TrayIcon>();
    if let Err(e) = tray.set_visible(visible) {
        error!("Failed to set tray visibility: {}", e);
    } else {
        info!("Tray visibility set to: {}", visible);
    }
}

pub fn copy_last_transcript(app: &AppHandle) {
    let history_manager = app.state::<Arc<HistoryManager>>();
    let entry = match history_manager.get_latest_completed_entry() {
        Ok(Some(entry)) => entry,
        Ok(None) => {
            warn!("No completed transcription history entries available for tray copy.");
            return;
        }
        Err(err) => {
            error!(
                "Failed to fetch last completed transcription entry: {}",
                err
            );
            return;
        }
    };

    let text = last_transcript_text(&entry);
    if text.trim().is_empty() {
        warn!("Last completed transcription is empty; skipping tray copy.");
        return;
    }

    if let Err(err) = app.clipboard().write_text(text) {
        error!("Failed to copy last transcript to clipboard: {}", err);
        return;
    }

    info!("Copied last transcript to clipboard via tray.");
}

#[cfg(test)]
mod tests {
    use super::last_transcript_text;
    use crate::managers::history::HistoryEntry;

    fn build_entry(transcription: &str, post_processed: Option<&str>) -> HistoryEntry {
        HistoryEntry {
            id: 1,
            file_name: "fisilti-1.wav".to_string(),
            timestamp: 0,
            saved: false,
            title: "Recording".to_string(),
            transcription_text: transcription.to_string(),
            post_processed_text: post_processed.map(|text| text.to_string()),
            post_process_prompt: None,
            post_process_requested: false,
        }
    }

    #[test]
    fn uses_post_processed_text_when_available() {
        let entry = build_entry("raw", Some("processed"));
        assert_eq!(last_transcript_text(&entry), "processed");
    }

    #[test]
    fn falls_back_to_raw_transcription() {
        let entry = build_entry("raw", None);
        assert_eq!(last_transcript_text(&entry), "raw");
    }
}
