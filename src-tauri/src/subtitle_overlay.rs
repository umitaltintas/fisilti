//! A click-through subtitle strip pinned near the bottom of the screen,
//! showing what the Gemini Live stream is hearing (or translating) right now.
//!
//! This exists because the live text was, until now, only visible in the
//! Fisilti window — and nobody looks at the Fisilti window during a call. In
//! translate mode especially, the whole point is following a conversation in a
//! language you do not speak, which only works if the text is where your eyes
//! already are.
//!
//! It reuses the recording overlay's window recipe (`PanelLevel::Status`,
//! `full_screen_auxiliary`, so it survives a fullscreen Zoom) with two
//! deliberate differences:
//!
//! * **Never captured.** The panel's `sharingType` is set to
//!   `NSWindowSharingNone`, so screen sharing and recordings do not pick it up.
//!   Subtitles are a reading aid for the person running the app; broadcasting
//!   a speculative, half-corrected transcript of colleagues to the whole call
//!   is a different feature with different consent questions.
//! * **Wide and short.** A subtitle strip, not a badge.
//!
//! macOS only, like the meeting capture it renders.

#[cfg(target_os = "macos")]
use tauri::{AppHandle, Emitter, Listener, Manager, WebviewUrl};

#[cfg(target_os = "macos")]
use tauri_nspanel::{tauri_panel, CollectionBehavior, PanelBuilder, PanelLevel};

/// Window label, also used by the frontend page.
pub const SUBTITLE_WINDOW: &str = "subtitle_overlay";

/// Fraction of the screen width the strip occupies. Wide enough for a long
/// sentence at a readable size, narrow enough not to span an ultrawide edge to
/// edge, where the eye cannot take a line in at a glance.
#[cfg(target_os = "macos")]
const WIDTH_FRACTION: f64 = 0.6;
#[cfg(target_os = "macos")]
const MAX_WIDTH: f64 = 1100.0;
/// Tall enough that the capped text (see `SUBTITLE_MAX_CHARS`) always fits with
/// room to spare. The window is transparent and click-through, so unused height
/// costs nothing — whereas being one line too short clips the newest words,
/// which are the only ones that matter.
#[cfg(target_os = "macos")]
const HEIGHT: f64 = 200.0;
/// Distance from the bottom of the visible screen area. Clears the Dock and
/// sits roughly where video-call captions and player subtitles already live, so
/// it lands where the eye expects them.
#[cfg(target_os = "macos")]
const BOTTOM_MARGIN: f64 = 90.0;

#[cfg(target_os = "macos")]
tauri_panel! {
    panel!(SubtitleOverlayPanel {
        config: {
            // Click-through: the strip floats over the meeting UI and must not
            // swallow a click meant for the mute button underneath it.
            can_become_key_window: false,
            // Required for the panel to appear while another app is frontmost —
            // which is always, since the user is in a call, not in Fisilti.
            is_floating_panel: true
        }
    })
}

/// Text currently shown on the strip. `settled` is finalized transcript the
/// model will not revise; `pending` is the speculative hypothesis for the
/// utterance still being spoken, which the UI dims to signal exactly that.
#[derive(Clone, serde::Serialize)]
pub struct SubtitleUpdate {
    pub settled: String,
    pub pending: String,
}

/// Where the strip belongs right now, in logical coordinates: centred near the
/// bottom of the display the user is actually looking at.
///
/// Uses the cursor's monitor rather than the primary one. On a two-display
/// setup those are routinely different, and a subtitle strip on the screen you
/// are not watching is indistinguishable from a broken feature.
#[cfg(target_os = "macos")]
fn strip_geometry(app_handle: &AppHandle) -> Option<(f64, f64, f64)> {
    let monitor = crate::overlay::get_monitor_with_cursor(app_handle)?;
    // Monitor position/size are physical; dividing by the scale factor gives
    // the logical coordinates the window API expects. Setting a physical
    // position would be converted using the scale of whichever monitor the
    // window is currently on, which is wrong exactly when it matters — moving
    // the strip across displays.
    let scale = monitor.scale_factor();
    let monitor_x = monitor.position().x as f64 / scale;
    let monitor_y = monitor.position().y as f64 / scale;
    let monitor_width = monitor.size().width as f64 / scale;
    let monitor_height = monitor.size().height as f64 / scale;

    let width = (monitor_width * WIDTH_FRACTION).min(MAX_WIDTH);
    let x = monitor_x + (monitor_width - width) / 2.0;
    let y = monitor_y + monitor_height - HEIGHT - BOTTOM_MARGIN;
    Some((x, y, width))
}

/// Move and size the strip for the display the user is on. Called before every
/// show, not just at creation: the panel is built once at startup, and by the
/// time a meeting starts the user may well be on another screen.
#[cfg(target_os = "macos")]
fn position_strip(app_handle: &AppHandle, window: &tauri::WebviewWindow) {
    let Some((x, y, width)) = strip_geometry(app_handle) else {
        log::warn!("subtitle overlay: no monitor available; leaving the strip where it is");
        return;
    };
    log::debug!(
        "subtitle overlay: positioning at ({:.0}, {:.0}) width {:.0}",
        x,
        y,
        width
    );
    let _ = window.set_size(tauri::Size::Logical(tauri::LogicalSize {
        width,
        height: HEIGHT,
    }));
    let _ = window.set_position(tauri::Position::Logical(tauri::LogicalPosition { x, y }));
}

/// Create the panel, hidden. Safe to call when the feature is off — an unused
/// hidden panel costs nothing, and building it up front keeps the first
/// subtitle from waiting on window creation.
#[cfg(target_os = "macos")]
pub fn create_subtitle_overlay(app_handle: &AppHandle) {
    if app_handle.get_webview_window(SUBTITLE_WINDOW).is_some() {
        return;
    }
    let Some((x, y, width)) = strip_geometry(app_handle) else {
        log::warn!("subtitle overlay: no monitor available; not creating the panel");
        return;
    };

    match PanelBuilder::<_, SubtitleOverlayPanel>::new(app_handle, SUBTITLE_WINDOW)
        .url(WebviewUrl::App("src/subtitle/index.html".into()))
        .title("Subtitles")
        .position(tauri::Position::Logical(tauri::LogicalPosition { x, y }))
        .level(PanelLevel::Status)
        .size(tauri::Size::Logical(tauri::LogicalSize {
            width,
            height: HEIGHT,
        }))
        .has_shadow(false)
        .transparent(true)
        .no_activate(true)
        .corner_radius(0.0)
        .with_window(|w| w.decorations(false).transparent(true))
        .collection_behavior(
            CollectionBehavior::new()
                .can_join_all_spaces()
                .full_screen_auxiliary(),
        )
        .build()
    {
        Ok(panel) => {
            let _ = panel.hide();
            exclude_from_screen_capture(app_handle);
            // The page announces itself once mounted. Without this a page that
            // fails to load is indistinguishable from one with nothing to say:
            // the window is transparent, so both render as empty screen.
            app_handle.listen("subtitle-ready", |_| {
                log::debug!("subtitle overlay: page mounted and listening");
            });
            log::debug!("subtitle overlay: panel created (hidden)");
        }
        Err(e) => log::error!("subtitle overlay: failed to create panel: {}", e),
    }
}

/// Set `NSWindowSharingNone` so the strip never appears in a screen share or
/// recording. Best-effort: failing to hide it is not worth refusing to show
/// subtitles at all, but it IS worth a warning, because the user chose this
/// expecting privacy.
#[cfg(target_os = "macos")]
fn exclude_from_screen_capture(app_handle: &AppHandle) {
    use objc2::runtime::AnyObject;

    let Some(window) = app_handle.get_webview_window(SUBTITLE_WINDOW) else {
        return;
    };
    let ns_window = match window.ns_window() {
        Ok(ptr) if !ptr.is_null() => ptr as *mut AnyObject,
        _ => {
            log::warn!("subtitle overlay: no NSWindow; it may appear in screen shares");
            return;
        }
    };
    // NSWindowSharingNone == 0.
    const NS_WINDOW_SHARING_NONE: usize = 0;
    unsafe {
        let _: () = objc2::msg_send![ns_window, setSharingType: NS_WINDOW_SHARING_NONE];
    }
}

#[cfg(not(target_os = "macos"))]
pub fn create_subtitle_overlay(_app_handle: &tauri::AppHandle) {}

/// Push new text to the strip, showing it if it was hidden.
#[cfg(target_os = "macos")]
pub fn show_subtitle(app_handle: &AppHandle, update: SubtitleUpdate) {
    let Some(window) = app_handle.get_webview_window(SUBTITLE_WINDOW) else {
        log::warn!("subtitle overlay: panel missing; cannot show subtitles");
        return;
    };
    // Show BEFORE emitting. A panel that has never been shown may not have
    // mounted its page yet, and an event fired at a page with no listener is
    // simply dropped — which would silently eat the first subtitle of every
    // meeting, the one most likely to be noticed as missing.
    if !window.is_visible().unwrap_or(false) {
        position_strip(app_handle, &window);
        match window.show() {
            Ok(()) => log::debug!("subtitle overlay: shown"),
            Err(e) => log::warn!("subtitle overlay: failed to show panel: {}", e),
        }
    }
    if let Err(e) = window.emit("subtitle-update", update) {
        log::warn!("subtitle overlay: failed to emit update: {}", e);
    }
}

#[cfg(not(target_os = "macos"))]
pub fn show_subtitle(_app_handle: &tauri::AppHandle, _update: SubtitleUpdate) {}

/// Hide the strip and clear it, so the next meeting does not flash the tail of
/// the previous one before its first subtitle arrives.
#[cfg(target_os = "macos")]
pub fn hide_subtitle(app_handle: &AppHandle) {
    let Some(window) = app_handle.get_webview_window(SUBTITLE_WINDOW) else {
        return;
    };
    let _ = window.emit(
        "subtitle-update",
        SubtitleUpdate {
            settled: String::new(),
            pending: String::new(),
        },
    );
    let _ = window.hide();
}

#[cfg(not(target_os = "macos"))]
pub fn hide_subtitle(_app_handle: &tauri::AppHandle) {}
