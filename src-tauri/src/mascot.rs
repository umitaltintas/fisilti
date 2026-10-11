//! The desktop mascot: a small character that floats above every app, can be
//! dragged anywhere on screen, and opens a quick-action menu when clicked.
//!
//! It is a shortcut surface, not a new feature: every action goes through the
//! same path the tray menu takes (`signal_handle::send_transcription_input`,
//! `toggle_meeting_from_app`, `copy_last_transcript`, …), so starting dictation
//! from the mascot is indistinguishable from starting it with the hotkey.
//!
//! **It must never take focus.** Dictation pastes into the frontmost app; if a
//! click on the mascot activated Fısıltı, the text would land in the mascot
//! instead of the document the user was typing in. On macOS the window is
//! therefore a non-activating `NSPanel` that cannot become key (clicks still
//! arrive thanks to `accept_first_mouse`). Elsewhere it is a plain
//! always-on-top window, like the meeting prompt.
//!
//! The window is small (just the character) while idle, and grows while the
//! menu is open: a transparent window still swallows clicks, so a permanently
//! large one would leave a dead zone over whatever sits underneath.
//!
//! The collapsed window's position is persisted in `mascot_position` after
//! every drag (debounced), and validated against the connected monitors on the
//! next show, so unplugging a display never strands the mascot off-screen.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, WebviewUrl};

use crate::settings::{self, MascotPosition};
use crate::tray::TrayIconState;

#[cfg(target_os = "macos")]
use tauri_nspanel::{tauri_panel, CollectionBehavior, PanelBuilder, PanelLevel, StyleMask};

/// Window label, also listed in the capabilities.
pub const MASCOT_WINDOW: &str = "mascot";

/// Emitted (globally) whenever dictation changes state, with `"idle"`,
/// `"recording"` or `"transcribing"`. The mascot animates from it.
pub const DICTATION_STATE_CHANGED_EVENT: &str = "dictation-state-changed";

/// Size of the window while only the character is shown.
const COLLAPSED_SIZE: f64 = 88.0;
/// Size of the window while the menu is open. The character keeps its exact
/// screen position; the window grows around it (see [`set_mascot_menu_open`]).
const EXPANDED_WIDTH: f64 = 232.0;
const EXPANDED_HEIGHT: f64 = 330.0;

/// Default spot: bottom-right corner, clear of the Dock / taskbar.
const DEFAULT_RIGHT_MARGIN: f64 = 28.0;
const DEFAULT_BOTTOM_MARGIN: f64 = 120.0;

/// How long the window must sit still after a drag before the position is
/// written to settings.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(600);

#[cfg(target_os = "macos")]
tauri_panel! {
    panel!(MascotPanel {
        config: {
            // Never key: a click must not steal focus from the app dictation
            // will paste into.
            can_become_key_window: false,
            can_become_main_window: false,
            is_floating_panel: true
        }
    })
}

/// Last dictation state reported by the tray (see [`set_dictation_state`]),
/// so a freshly mounted page can render it without waiting for an event.
static DICTATION_STATE: AtomicU8 = AtomicU8::new(0);
/// True while the menu is open: the window is then expanded and moved around
/// the character, and those moves are not user drags to persist.
static MENU_OPEN: AtomicBool = AtomicBool::new(false);
/// Bumped on every move; a pending save only runs if it is still current.
static MOVE_GENERATION: AtomicU64 = AtomicU64::new(0);
/// `(menu_above, mascot_left)` of the current expansion, needed to find the
/// character again when collapsing.
static LAST_LAYOUT: Mutex<(bool, f64)> = Mutex::new((true, 0.0));

fn dictation_state_name(value: u8) -> &'static str {
    match value {
        1 => "recording",
        2 => "transcribing",
        _ => "idle",
    }
}

/// Record the dictation state and tell the mascot. Called from
/// `tray::change_tray_icon`, the single place every dictation state change
/// already goes through.
pub fn set_dictation_state(app: &AppHandle, state: &TrayIconState) {
    let value = match state {
        TrayIconState::Idle => 0,
        TrayIconState::Recording => 1,
        TrayIconState::Transcribing => 2,
    };
    DICTATION_STATE.store(value, Ordering::SeqCst);
    let _ = app.emit(DICTATION_STATE_CHANGED_EVENT, dictation_state_name(value));
}

/// Where the collapsed window goes on first show: bottom-right of the primary
/// monitor, in logical coordinates.
fn default_position(app: &AppHandle) -> Option<(f64, f64)> {
    let monitor = app.primary_monitor().ok().flatten()?;
    let scale = monitor.scale_factor();
    let x = monitor.position().x as f64 / scale;
    let y = monitor.position().y as f64 / scale;
    let width = monitor.size().width as f64 / scale;
    let height = monitor.size().height as f64 / scale;
    Some((
        x + width - COLLAPSED_SIZE - DEFAULT_RIGHT_MARGIN,
        y + height - COLLAPSED_SIZE - DEFAULT_BOTTOM_MARGIN,
    ))
}

/// Logical bounds `(x, y, width, height)` of every connected monitor.
fn monitor_bounds(app: &AppHandle) -> Vec<(f64, f64, f64, f64)> {
    app.available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            let scale = m.scale_factor();
            (
                m.position().x as f64 / scale,
                m.position().y as f64 / scale,
                m.size().width as f64 / scale,
                m.size().height as f64 / scale,
            )
        })
        .collect()
}

/// Whether the centre of a collapsed window at `(x, y)` is on some monitor.
fn is_on_screen(bounds: &[(f64, f64, f64, f64)], x: f64, y: f64) -> bool {
    let (cx, cy) = (x + COLLAPSED_SIZE / 2.0, y + COLLAPSED_SIZE / 2.0);
    bounds
        .iter()
        .any(|&(mx, my, mw, mh)| cx >= mx && cx < mx + mw && cy >= my && cy < my + mh)
}

/// The saved position when it is still on a connected display, otherwise the
/// default one.
fn initial_position(app: &AppHandle) -> (f64, f64) {
    let saved = settings::get_settings(app).mascot_position;
    if let Some(MascotPosition { x, y }) = saved {
        if is_on_screen(&monitor_bounds(app), x, y) {
            return (x, y);
        }
        log::info!("mascot: saved position ({x:.0}, {y:.0}) is off-screen; using the default");
    }
    default_position(app).unwrap_or((100.0, 100.0))
}

/// Current logical position of the window's top-left corner.
fn logical_position(window: &tauri::WebviewWindow) -> Option<(f64, f64)> {
    let pos = window.outer_position().ok()?;
    let scale = window.scale_factor().ok()?;
    Some((pos.x as f64 / scale, pos.y as f64 / scale))
}

/// Persist the collapsed window's position once it has stopped moving.
fn schedule_position_save(app: &AppHandle) {
    if MENU_OPEN.load(Ordering::SeqCst) {
        return;
    }
    let generation = MOVE_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(SAVE_DEBOUNCE);
        if MOVE_GENERATION.load(Ordering::SeqCst) != generation || MENU_OPEN.load(Ordering::SeqCst)
        {
            return;
        }
        let Some(window) = app.get_webview_window(MASCOT_WINDOW) else {
            return;
        };
        let Some((x, y)) = logical_position(&window) else {
            return;
        };
        settings::update_settings(&app, |s| {
            s.mascot_position = Some(MascotPosition { x, y });
        });
        log::debug!("mascot: saved position ({x:.0}, {y:.0})");
    });
}

fn attach_move_listener(app: &AppHandle, window: &tauri::WebviewWindow) {
    let app = app.clone();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::Moved(_) = event {
            schedule_position_save(&app);
        }
    });
}

#[cfg(target_os = "macos")]
fn create_window(app: &AppHandle) -> Option<tauri::WebviewWindow> {
    let (x, y) = initial_position(app);
    let panel = PanelBuilder::<_, MascotPanel>::new(app, MASCOT_WINDOW)
        .url(WebviewUrl::App("src/mascot/index.html".into()))
        .title("Fısıltı")
        .position(tauri::Position::Logical(LogicalPosition { x, y }))
        .size(tauri::Size::Logical(LogicalSize {
            width: COLLAPSED_SIZE,
            height: COLLAPSED_SIZE,
        }))
        // Above normal windows, below the menu bar and the recording overlay.
        .level(PanelLevel::Floating)
        .style_mask(StyleMask::empty().borderless().nonactivating_panel())
        .has_shadow(false)
        .transparent(true)
        .no_activate(true)
        .accepts_mouse_moved_events(true)
        .corner_radius(0.0)
        .with_window(|w| {
            w.decorations(false)
                .transparent(true)
                .accept_first_mouse(true)
                .skip_taskbar(true)
        })
        .collection_behavior(
            CollectionBehavior::new()
                .can_join_all_spaces()
                .full_screen_auxiliary(),
        )
        .build();
    match panel {
        Ok(panel) => {
            panel.hide();
            app.get_webview_window(MASCOT_WINDOW)
        }
        Err(e) => {
            log::error!("mascot: failed to create panel: {e}");
            None
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn create_window(app: &AppHandle) -> Option<tauri::WebviewWindow> {
    let (x, y) = initial_position(app);
    let mut builder = tauri::WebviewWindowBuilder::new(
        app,
        MASCOT_WINDOW,
        WebviewUrl::App("src/mascot/index.html".into()),
    )
    .title("Fısıltı")
    .inner_size(COLLAPSED_SIZE, COLLAPSED_SIZE)
    .position(x, y)
    .resizable(false)
    .maximizable(false)
    .minimizable(false)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .transparent(true)
    .shadow(false)
    .focused(false)
    .accept_first_mouse(true)
    .visible(false);
    if let Some(data_dir) = crate::portable::data_dir() {
        builder = builder.data_directory(data_dir.join("webview"));
    }
    match builder.build() {
        Ok(window) => Some(window),
        Err(e) => {
            log::error!("mascot: failed to create window: {e}");
            None
        }
    }
}

/// Show or hide the mascot, creating the window on first show.
pub fn set_visible(app: &AppHandle, visible: bool) {
    let existing = app.get_webview_window(MASCOT_WINDOW);
    if !visible {
        if let Some(window) = existing {
            let _ = window.hide();
        }
        return;
    }
    let window = match existing {
        Some(window) => window,
        None => {
            let Some(window) = create_window(app) else {
                return;
            };
            attach_move_listener(app, &window);
            window
        }
    };
    if let Err(e) = window.show() {
        log::warn!("mascot: failed to show: {e}");
    }
}

/// Startup hook: show the mascot when the user has it enabled.
pub fn init(app: &AppHandle) {
    if settings::get_settings(app).show_mascot {
        set_visible(app, true);
    }
}

/// What the page needs to render on mount.
#[derive(Serialize, Clone, Debug, Type)]
pub struct MascotStatus {
    /// `"idle"`, `"recording"` or `"transcribing"`.
    pub dictation: String,
    /// `"idle"`, `"running"` or `"finalizing"`.
    pub meeting: String,
    /// Meeting capture exists on macOS only; the menu hides the item elsewhere.
    pub meeting_supported: bool,
}

#[tauri::command]
#[specta::specta]
pub fn get_mascot_status(app: AppHandle) -> MascotStatus {
    let meeting = app
        .try_state::<std::sync::Arc<crate::meeting::MeetingManager>>()
        .map(|m| m.status().as_str())
        .unwrap_or("idle");
    MascotStatus {
        dictation: dictation_state_name(DICTATION_STATE.load(Ordering::SeqCst)).to_string(),
        meeting: meeting.to_string(),
        meeting_supported: cfg!(target_os = "macos"),
    }
}

/// How the page should lay out the expanded window.
#[derive(Serialize, Clone, Debug, Type)]
pub struct MascotLayout {
    /// The menu sits above the character (false: below, near the top edge).
    pub menu_above: bool,
    /// Offset of the character's left edge inside the window, in logical px.
    pub mascot_left: f64,
}

/// Grow the window for the menu (or shrink it back) without moving the
/// character on screen. The menu goes above the character unless that would
/// leave the monitor, and the window is clamped horizontally so the menu stays
/// fully visible at a screen edge.
#[tauri::command]
#[specta::specta]
pub fn set_mascot_menu_open(app: AppHandle, open: bool) -> Result<MascotLayout, String> {
    let window = app
        .get_webview_window(MASCOT_WINDOW)
        .ok_or("mascot window not found")?;
    let (wx, wy) = logical_position(&window).ok_or("mascot window has no position")?;

    // The character's top-left on screen, wherever the window currently is.
    let was_open = MENU_OPEN.swap(open, Ordering::SeqCst);
    let (char_x, char_y) = if was_open {
        let (menu_above, mascot_left) = LAST_LAYOUT.lock().map(|g| *g).unwrap_or((true, 0.0));
        let y = if menu_above {
            wy + EXPANDED_HEIGHT - COLLAPSED_SIZE
        } else {
            wy
        };
        (wx + mascot_left, y)
    } else {
        (wx, wy)
    };

    if !open {
        window
            .set_size(LogicalSize::new(COLLAPSED_SIZE, COLLAPSED_SIZE))
            .map_err(|e| e.to_string())?;
        window
            .set_position(LogicalPosition::new(char_x, char_y))
            .map_err(|e| e.to_string())?;
        return Ok(MascotLayout {
            menu_above: true,
            mascot_left: 0.0,
        });
    }

    // Monitor the character is on (fall back to the primary one).
    let bounds = monitor_bounds(&app);
    let (cx, cy) = (char_x + COLLAPSED_SIZE / 2.0, char_y + COLLAPSED_SIZE / 2.0);
    let monitor = bounds
        .iter()
        .copied()
        .find(|&(mx, my, mw, mh)| cx >= mx && cx < mx + mw && cy >= my && cy < my + mh)
        .or_else(|| bounds.first().copied());

    let (layout, x, y) = expanded_geometry(char_x, char_y, monitor);
    if let Ok(mut guard) = LAST_LAYOUT.lock() {
        *guard = (layout.menu_above, layout.mascot_left);
    }
    window
        .set_size(LogicalSize::new(EXPANDED_WIDTH, EXPANDED_HEIGHT))
        .map_err(|e| e.to_string())?;
    window
        .set_position(LogicalPosition::new(x, y))
        .map_err(|e| e.to_string())?;
    Ok(layout)
}

/// Pure geometry of the expanded window: where it goes so that the character
/// stays at `(char_x, char_y)` and the menu fits on `monitor`.
fn expanded_geometry(
    char_x: f64,
    char_y: f64,
    monitor: Option<(f64, f64, f64, f64)>,
) -> (MascotLayout, f64, f64) {
    let centred_x = char_x - (EXPANDED_WIDTH - COLLAPSED_SIZE) / 2.0;
    let (x, menu_above) = match monitor {
        Some((mx, my, mw, _)) => {
            let max_x = (mx + mw - EXPANDED_WIDTH).max(mx);
            let x = centred_x.clamp(mx, max_x);
            // The window must not extend past the menu bar / top edge.
            let above_top = char_y + COLLAPSED_SIZE - EXPANDED_HEIGHT;
            (x, above_top >= my)
        }
        None => (centred_x, true),
    };
    // Keep the character fully inside the window even at a screen edge.
    let mascot_left = (char_x - x).clamp(0.0, EXPANDED_WIDTH - COLLAPSED_SIZE);
    let x = char_x - mascot_left;
    let y = if menu_above {
        char_y + COLLAPSED_SIZE - EXPANDED_HEIGHT
    } else {
        char_y
    };
    (
        MascotLayout {
            menu_above,
            mascot_left,
        },
        x,
        y,
    )
}

/// The menu's actions. Each one reuses the tray's path.
#[derive(Deserialize, Clone, Copy, Debug, Type)]
#[serde(rename_all = "snake_case")]
pub enum MascotAction {
    ToggleDictation,
    ToggleMeeting,
    CopyLastTranscript,
    OpenApp,
    OpenHistory,
    OpenMeetings,
    Cancel,
    Hide,
}

#[tauri::command]
#[specta::specta]
pub fn mascot_action(app: AppHandle, action: MascotAction) -> Result<(), String> {
    log::info!("mascot: {action:?}");
    match action {
        MascotAction::ToggleDictation => {
            crate::signal_handle::send_transcription_input(&app, "transcribe", "mascot");
        }
        MascotAction::ToggleMeeting => {
            // stop() blocks on the finalize pass; keep it off the IPC thread.
            let app = app.clone();
            std::thread::spawn(move || crate::commands::meeting::toggle_meeting_from_app(&app));
        }
        MascotAction::CopyLastTranscript => crate::tray::copy_last_transcript(&app),
        MascotAction::OpenApp => crate::show_main_window(&app),
        MascotAction::OpenHistory => {
            crate::show_main_window(&app);
            let _ = app.emit("navigate-section", "history");
        }
        MascotAction::OpenMeetings => {
            crate::show_main_window(&app);
            let _ = app.emit("navigate-section", "meeting");
        }
        MascotAction::Cancel => crate::utils::cancel_current_operation(&app),
        MascotAction::Hide => {
            change_show_mascot_setting(app, false)?;
        }
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_show_mascot_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    settings::update_settings(&app, |s| s.show_mascot = enabled);
    set_visible(&app, enabled);
    // Let the settings page reflect a hide triggered from the mascot itself.
    let _ = app.emit("mascot-visibility-changed", enabled);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONITOR: (f64, f64, f64, f64) = (0.0, 0.0, 1440.0, 900.0);

    #[test]
    fn menu_opens_above_and_keeps_the_character_in_place() {
        let (layout, x, y) = expanded_geometry(700.0, 700.0, Some(MONITOR));
        assert!(layout.menu_above);
        assert_eq!(x + layout.mascot_left, 700.0);
        assert_eq!(y + EXPANDED_HEIGHT - COLLAPSED_SIZE, 700.0);
    }

    #[test]
    fn menu_flips_below_near_the_top_edge() {
        let (layout, _, y) = expanded_geometry(700.0, 40.0, Some(MONITOR));
        assert!(!layout.menu_above);
        assert_eq!(y, 40.0);
    }

    #[test]
    fn window_is_clamped_at_the_right_edge() {
        let char_x = 1440.0 - COLLAPSED_SIZE;
        let (layout, x, _) = expanded_geometry(char_x, 700.0, Some(MONITOR));
        assert_eq!(x + EXPANDED_WIDTH, 1440.0);
        assert_eq!(x + layout.mascot_left, char_x);
    }

    #[test]
    fn off_screen_positions_are_rejected() {
        assert!(is_on_screen(&[MONITOR], 100.0, 100.0));
        assert!(!is_on_screen(&[MONITOR], 3000.0, 100.0));
    }
}
