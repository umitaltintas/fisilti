// Meeting prompt window: a small, focusable, always-on-top panel used by the
// meeting auto-detection feature. It asks the user to start transcription when
// a meeting is detected ("start" prompt) and whether to end the session after
// prolonged silence or when the meeting app releases the microphone ("end"
// prompt).
//
// Unlike the recording overlay (which is deliberately non-focusable), this
// window must accept clicks: its buttons invoke `accept_meeting_prompt`,
// `dismiss_meeting_prompt` and `respond_meeting_auto_end` (see
// `commands/meeting.rs`). The React page lives in `src/meeting-prompt/`.
//
// The window is a plain `WebviewWindowBuilder` on every platform (no
// `tauri_nspanel`) so it can become key and receive clicks. On macOS the app
// may run under the Accessory activation policy; we deliberately do NOT change
// it — `always_on_top(true)` plus `set_focus()` is enough to bring the panel
// forward and let a single click land on a button (helped by
// `accept_first_mouse`).

use std::sync::{Mutex, Once};

use serde::Serialize;
use tauri::{
    AppHandle, Emitter, Listener, LogicalPosition, Manager, Position, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

/// Label of the prompt webview window (must match the capabilities entry).
pub const MEETING_PROMPT_WINDOW_LABEL: &str = "meeting_prompt";

/// Event carrying a `MeetingPromptPayload`, emitted to the prompt window every
/// time it is (re)shown so the page renders the right prompt variant.
pub const MEETING_PROMPT_UPDATE_EVENT: &str = "meeting-prompt-update";

/// Event the React page emits once, on mount, to signal that it is listening.
/// Used to close the first-show race (the page may not have registered its
/// `MEETING_PROMPT_UPDATE_EVENT` listener yet when we emit right after
/// creating the window). See [`ensure_ready_listener`].
const MEETING_PROMPT_READY_EVENT: &str = "meeting-prompt-ready";

/// Logical size of the prompt window (points on macOS).
const PROMPT_WIDTH: f64 = 380.0;
const PROMPT_HEIGHT: f64 = 150.0;

/// Margin between the window and the right edge of the monitor.
const PROMPT_RIGHT_MARGIN: f64 = 16.0;

/// Margin between the window and the top of the monitor. Slightly larger on
/// macOS so the panel clears the menu bar (monitor coordinates start above it).
#[cfg(target_os = "macos")]
const PROMPT_TOP_MARGIN: f64 = 44.0;
#[cfg(not(target_os = "macos"))]
const PROMPT_TOP_MARGIN: f64 = 16.0;

/// State backing the prompt window, registered once in Tauri managed state.
struct MeetingPromptState {
    /// The payload of the currently displayed prompt. Re-emitted to the page
    /// when it signals readiness, and cleared by [`hide_meeting_prompt`].
    current: Mutex<Option<MeetingPromptPayload>>,
    /// Guards one-time registration of the `MEETING_PROMPT_READY_EVENT`
    /// listener (see [`ensure_ready_listener`]).
    ready_listener: Once,
}

impl Default for MeetingPromptState {
    fn default() -> Self {
        Self {
            current: Mutex::new(None),
            ready_listener: Once::new(),
        }
    }
}

/// Why the "end meeting?" prompt is being shown.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    /// No speech detected for `meeting_silence_timeout_secs`.
    Silence,
    /// The detected meeting app stopped using the microphone.
    AppClosed,
}

/// Payload of `MEETING_PROMPT_UPDATE_EVENT`. Serialized with a `kind` tag so
/// the frontend can switch on `payload.kind === "start" | "end"`.
#[derive(Serialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MeetingPromptPayload {
    #[serde(rename = "start", rename_all = "camelCase")]
    Start {
        /// Human-readable name of the detected meeting app ("Zoom", "Chrome"…).
        app_name: String,
    },
    #[serde(rename = "end", rename_all = "camelCase")]
    End {
        reason: EndReason,
        /// Seconds until the session auto-ends if the prompt is ignored.
        grace_secs: u32,
    },
}

/// Register [`MeetingPromptState`] in Tauri state on first use. Idempotent:
/// `manage` is a no-op (returns `false`) when the type is already managed, so
/// concurrent callers are safe.
fn ensure_state(app: &AppHandle) {
    if app.try_state::<MeetingPromptState>().is_none() {
        app.manage(MeetingPromptState::default());
    }
}

/// Register the `MEETING_PROMPT_READY_EVENT` listener exactly once.
///
/// When the page (re)mounts it emits `MEETING_PROMPT_READY_EVENT`; we respond
/// by re-emitting the stored current payload to the window. Combined with the
/// immediate `emit_to` in [`show_meeting_prompt`], this guarantees the page
/// receives the payload whether or not its listener was ready at show time.
fn ensure_ready_listener(app: &AppHandle) {
    let state = app.state::<MeetingPromptState>();
    state.ready_listener.call_once(|| {
        let app_handle = app.clone();
        app.listen(MEETING_PROMPT_READY_EVENT, move |_event| {
            let payload = {
                let state = app_handle.state::<MeetingPromptState>();
                let guard = match state.current.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
                guard.clone()
            };
            if let Some(payload) = payload {
                if let Err(e) = app_handle.emit_to(
                    MEETING_PROMPT_WINDOW_LABEL,
                    MEETING_PROMPT_UPDATE_EVENT,
                    payload,
                ) {
                    log::warn!("Failed to re-emit meeting prompt payload on ready: {}", e);
                }
            }
        });
    });
}

/// Store `payload` as the current prompt so the ready handshake can replay it.
fn store_payload(app: &AppHandle, payload: &MeetingPromptPayload) {
    let state = app.state::<MeetingPromptState>();
    let mut guard = match state.current.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    *guard = Some(payload.clone());
}

/// Compute the top-right position of the prompt in logical coordinates.
///
/// Uses the primary monitor's position/size directly (like the recording
/// overlay) and normalizes to logical points by the scale factor, which is
/// safe across monitors. Returns `None` when no monitor is available, in which
/// case the window keeps its builder default position.
fn calculate_prompt_position(app: &AppHandle) -> Option<(f64, f64)> {
    let monitor = app.primary_monitor().ok().flatten()?;
    let scale = monitor.scale_factor();
    let monitor_x = monitor.position().x as f64 / scale;
    let monitor_y = monitor.position().y as f64 / scale;
    let monitor_width = monitor.size().width as f64 / scale;

    let x = monitor_x + monitor_width - PROMPT_WIDTH - PROMPT_RIGHT_MARGIN;
    let y = monitor_y + PROMPT_TOP_MARGIN;
    Some((x, y))
}

/// Move the prompt window to its computed top-right position. Logs and ignores
/// errors (position is cosmetic).
fn position_prompt_window(app: &AppHandle, window: &WebviewWindow) {
    if let Some((x, y)) = calculate_prompt_position(app) {
        if let Err(e) = window.set_position(Position::Logical(LogicalPosition { x, y })) {
            log::warn!("Failed to position meeting prompt window: {}", e);
        }
    }
}

/// Create the prompt window (hidden). A plain, clickable, always-on-top,
/// transparent, borderless window sized `PROMPT_WIDTH`×`PROMPT_HEIGHT`.
fn create_prompt_window(app: &AppHandle) -> Result<WebviewWindow, tauri::Error> {
    let mut builder = WebviewWindowBuilder::new(
        app,
        MEETING_PROMPT_WINDOW_LABEL,
        WebviewUrl::App("src/meeting-prompt/index.html".into()),
    )
    .title("Meeting")
    .inner_size(PROMPT_WIDTH, PROMPT_HEIGHT)
    .resizable(false)
    .maximizable(false)
    .minimizable(false)
    .closable(false)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .transparent(true)
    .shadow(false)
    // Let the first click land on a button instead of only activating the
    // window (matters on macOS when the app is in Accessory activation).
    .accept_first_mouse(true)
    .visible(false);

    if let Some((x, y)) = calculate_prompt_position(app) {
        builder = builder.position(x, y);
    }

    if let Some(data_dir) = crate::portable::data_dir() {
        builder = builder.data_directory(data_dir.join("webview"));
    }

    builder.build()
}

/// Show the prompt window (creating it on first use) and emit
/// `MEETING_PROMPT_UPDATE_EVENT` with `payload`.
///
/// The payload is stored first so the ready handshake can replay it if the
/// page is not listening yet; then the window is shown, focused, and the
/// payload emitted directly to it. Errors are logged, never fatal.
pub fn show_meeting_prompt(app: &AppHandle, payload: MeetingPromptPayload) {
    log::info!("meeting-prompt: showing {:?}", payload);
    ensure_state(app);
    store_payload(app, &payload);
    ensure_ready_listener(app);

    let window = match app.get_webview_window(MEETING_PROMPT_WINDOW_LABEL) {
        Some(window) => window,
        None => match create_prompt_window(app) {
            Ok(window) => window,
            Err(e) => {
                log::error!("Failed to create meeting prompt window: {}", e);
                return;
            }
        },
    };

    position_prompt_window(app, &window);

    if let Err(e) = window.show() {
        log::warn!("Failed to show meeting prompt window: {}", e);
    }
    // Bring the panel forward so a click lands immediately. On macOS we do NOT
    // touch the activation policy; always_on_top + set_focus is sufficient.
    if let Err(e) = window.set_focus() {
        log::warn!("Failed to focus meeting prompt window: {}", e);
    }

    // Emit immediately for the common case where the page is already mounted.
    // If it is not, the ready handshake (see `ensure_ready_listener`) replays
    // the stored payload once the page mounts.
    if let Err(e) = app.emit_to(
        MEETING_PROMPT_WINDOW_LABEL,
        MEETING_PROMPT_UPDATE_EVENT,
        payload,
    ) {
        log::warn!("Failed to emit meeting prompt update: {}", e);
    }
}

/// Hide the prompt window if it exists and clear the stored payload so a stale
/// prompt is not replayed by a later ready handshake.
pub fn hide_meeting_prompt(app: &AppHandle) {
    if let Some(state) = app.try_state::<MeetingPromptState>() {
        let mut guard = match state.current.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *guard = None;
    }

    if let Some(window) = app.get_webview_window(MEETING_PROMPT_WINDOW_LABEL) {
        if window.is_visible().unwrap_or(false) {
            log::info!("meeting-prompt: hiding");
        }
        if let Err(e) = window.hide() {
            log::warn!("Failed to hide meeting prompt window: {}", e);
        }
    }
}
