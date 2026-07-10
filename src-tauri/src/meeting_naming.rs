// Meeting session naming (macOS): resolve a human title for a starting session
// from, in priority order:
//   1. the calendar event in progress right now (EventKit; opt-in via the
//      `meeting_calendar_names` setting, needs the Calendars TCC permission)
//   2. the window/tab title of the meeting app currently pulling mic input
//      (AX API; reuses the accessibility permission the app already requires
//      for pasting)
// `MeetingManager` falls back to the LLM auto-title and finally the datetime
// default when neither source yields a meaningful name.
//
// Everything here is best-effort and non-fatal: any permission gap, API error
// or unavailable OS feature simply yields `None`.

#[cfg(target_os = "macos")]
use tauri::AppHandle;

/// Resolve an explicit title for the session that is starting, or `None` when
/// no source produced a meaningful name. Only exists on macOS — the meeting
/// capture itself is macOS-only, so there is never a session to name elsewhere.
#[cfg(target_os = "macos")]
pub fn resolve_session_title(app: &AppHandle) -> Option<String> {
    let settings = crate::settings::get_settings(app);
    if settings.meeting_calendar_names {
        if let Some(title) = current_calendar_event_title() {
            log::info!("meeting naming: using calendar event title");
            return Some(title);
        }
    }
    if let Some(title) = meeting_window_title() {
        log::info!("meeting naming: using meeting-app window title");
        return Some(title);
    }
    None
}

/// Maximum length of a resolved title; longer window/event titles are cut at a
/// word boundary with an ellipsis.
#[cfg(any(target_os = "macos", test))]
const MAX_TITLE_LEN: usize = 80;

// ---------------------------------------------------------------------------
// Source 1: calendar (EventKit)
// ---------------------------------------------------------------------------

/// Current Calendars authorization for meeting naming, as a stable string the
/// settings UI can match on: `"authorized"` (full access), `"denied"`
/// (denied/restricted/write-only), `"notDetermined"`, or `"unavailable"`
/// (non-macOS or macOS < 14).
#[cfg(target_os = "macos")]
pub fn calendar_access_status() -> &'static str {
    use objc2_event_kit::{EKAuthorizationStatus, EKEntityType, EKEventStore};

    if !full_access_request_available() {
        return "unavailable";
    }
    let status = unsafe { EKEventStore::authorizationStatusForEntityType(EKEntityType::Event) };
    match status {
        EKAuthorizationStatus::FullAccess => "authorized",
        EKAuthorizationStatus::NotDetermined => "notDetermined",
        _ => "denied",
    }
}

#[cfg(not(target_os = "macos"))]
pub fn calendar_access_status() -> &'static str {
    "unavailable"
}

/// Request full calendar access, showing the system prompt on first call.
/// BLOCKS until the user answers (call from a blocking thread). Returns true
/// when full access is granted.
#[cfg(target_os = "macos")]
pub fn request_calendar_access() -> bool {
    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_event_kit::{EKAuthorizationStatus, EKEntityType, EKEventStore};
    use objc2_foundation::NSError;

    if !full_access_request_available() {
        return false;
    }
    // Already decided? Don't re-prompt (the system wouldn't anyway).
    let status = unsafe { EKEventStore::authorizationStatusForEntityType(EKEntityType::Event) };
    match status {
        EKAuthorizationStatus::FullAccess => return true,
        EKAuthorizationStatus::NotDetermined => {}
        _ => return false,
    }

    let (tx, rx) = std::sync::mpsc::channel::<bool>();
    let store = unsafe { EKEventStore::new() };
    // The block keeps its own retain of the store so the store outlives the
    // in-flight request even after this function's `store` binding is dropped.
    let store_for_block = store.clone();
    let block = RcBlock::new(move |granted: Bool, _error: *mut NSError| {
        let _keep_alive = &store_for_block;
        let _ = tx.send(granted.as_bool());
    });
    unsafe { store.requestFullAccessToEventsWithCompletion(RcBlock::as_ptr(&block)) };

    // The completion fires once the user answers the system prompt; cap the
    // wait so a dismissed/stuck prompt can't hang the caller forever.
    rx.recv_timeout(std::time::Duration::from_secs(180))
        .unwrap_or(false)
}

#[cfg(not(target_os = "macos"))]
pub fn request_calendar_access() -> bool {
    false
}

/// `requestFullAccessToEventsWithCompletion:` exists on macOS 14+ only; on
/// older systems calling it would throw. The meeting feature itself already
/// requires macOS 14+, so simply report unavailable there.
#[cfg(target_os = "macos")]
fn full_access_request_available() -> bool {
    use objc2::sel;
    use objc2::ClassType;
    use objc2_event_kit::EKEventStore;

    EKEventStore::class().responds_to(sel!(requestFullAccessToEventsWithCompletion:))
}

/// Title of the calendar event happening right now, if calendar access is
/// granted and a suitable (non-all-day, titled) event overlaps the current
/// time. When several overlap, the most recently started one wins — that is
/// the meeting the user most plausibly just joined.
#[cfg(target_os = "macos")]
fn current_calendar_event_title() -> Option<String> {
    use objc2_event_kit::{EKAuthorizationStatus, EKEntityType, EKEventStore};
    use objc2_foundation::NSDate;

    let status = unsafe { EKEventStore::authorizationStatusForEntityType(EKEntityType::Event) };
    if status != EKAuthorizationStatus::FullAccess {
        return None;
    }

    let store = unsafe { EKEventStore::new() };
    // Events OVERLAPPING [now, now+60s] = the ongoing ones (plus anything
    // about to start within the minute, which is fine: joining a meeting a
    // minute early is common).
    let start = NSDate::now();
    let end = NSDate::dateWithTimeIntervalSinceNow(60.0);
    let predicate =
        unsafe { store.predicateForEventsWithStartDate_endDate_calendars(&start, &end, None) };
    let events = unsafe { store.eventsMatchingPredicate(&predicate) };

    let now_secs = start.timeIntervalSince1970();
    let mut best: Option<(f64, String)> = None;
    for event in events.iter() {
        if unsafe { event.isAllDay() } {
            continue;
        }
        let started = unsafe { event.startDate().timeIntervalSince1970() };
        // Skip events that haven't started yet beyond the small grace window
        // covered by the predicate range (defensive; range already caps it).
        if started > now_secs + 60.0 {
            continue;
        }
        let title = unsafe { event.title() }.to_string();
        let title = title.trim();
        if title.is_empty() {
            continue;
        }
        let candidate = (started, truncate_title(title));
        if best.as_ref().is_none_or(|(s, _)| candidate.0 > *s) {
            best = Some(candidate);
        }
    }
    best.map(|(_, title)| title)
}

// ---------------------------------------------------------------------------
// Source 2: meeting-app window title (AX)
// ---------------------------------------------------------------------------

/// Title of the detected meeting app's window, cleaned of app-name suffixes
/// and rejected when generic ("Zoom Meeting", a bare Meet code, …). Uses the
/// accessibility permission the app already holds for pasting; returns `None`
/// without it.
#[cfg(target_os = "macos")]
fn meeting_window_title() -> Option<String> {
    use cidre::{ax, ns};

    let signal = crate::meeting_detector::current_meeting_signal()?;
    if !ax::is_process_trusted() {
        log::debug!("meeting naming: accessibility not granted; skipping window title");
        return None;
    }

    // The CoreAudio match may be a helper subprocess (browser audio runs in a
    // helper); AX needs the MAIN app that owns the windows, so resolve the
    // allowlist bundle id to a running application.
    let apps = ns::RunningApp::with_bundle_id(&ns::String::with_str(signal.bundle_id));
    let app = apps.first()?;
    let element = ax::UiElement::with_app_pid(app.pid());
    // Never let a busy/hung app block the resolution thread for long.
    let _ = element.set_messaging_timeout_secs(1.0);

    // Focused window first (the meeting is most likely frontmost right after
    // the user joined), then the main window, then every window.
    let mut candidates: Vec<String> = Vec::new();
    for attr in [ax::attr::focused_window(), ax::attr::main_window()] {
        if let Ok(value) = element.attr_value(attr) {
            if let Some(window) = cast_ui_element(&value) {
                if let Some(title) = window_title(window) {
                    candidates.push(title);
                }
            }
        }
    }
    if let Ok(value) = element.attr_value(ax::attr::windows()) {
        if value.get_type_id() == cidre::cf::Array::type_id() {
            let windows: &cidre::cf::ArrayOf<ax::UiElement> =
                unsafe { std::mem::transmute::<&cidre::cf::Type, _>(value.as_ref()) };
            for window in windows.iter().take(20) {
                if let Some(title) = window_title(window) {
                    candidates.push(title);
                }
            }
        }
    }

    candidates
        .iter()
        .find_map(|raw| clean_window_title(raw, signal.name))
}

/// Read a window element's AX title as a Rust string.
#[cfg(target_os = "macos")]
fn window_title(window: &cidre::ax::UiElement) -> Option<String> {
    use cidre::{ax, cf};

    let value = window.attr_value(ax::attr::title()).ok()?;
    if value.get_type_id() != cf::String::type_id() {
        return None;
    }
    let s: &cf::String = unsafe { std::mem::transmute::<&cf::Type, _>(value.as_ref()) };
    Some(s.to_string())
}

/// Downcast an AX attribute value to a UiElement when it is one.
#[cfg(target_os = "macos")]
fn cast_ui_element(value: &cidre::arc::R<cidre::cf::Type>) -> Option<&cidre::ax::UiElement> {
    use cidre::{ax, cf};

    if value.get_type_id() != ax::UiElement::type_id() {
        return None;
    }
    Some(unsafe { std::mem::transmute::<&cf::Type, &ax::UiElement>(value.as_ref()) })
}

// ---------------------------------------------------------------------------
// Title cleaning heuristics (pure, unit-tested)
// ---------------------------------------------------------------------------

/// Product-name suffixes browsers and meeting apps append to window titles,
/// stripped repeatedly from the end (e.g. "Weekly Sync - Google Chrome").
#[cfg(any(target_os = "macos", test))]
const TITLE_SUFFIXES: &[&str] = &[
    "Google Chrome",
    "Chromium",
    "Microsoft Edge",
    "Mozilla Firefox",
    "Firefox",
    "Safari",
    "Opera",
    "Vivaldi",
    "Brave",
    "Arc",
    "Zen Browser",
    "Microsoft Teams",
    "Zoom Workplace",
    "Zoom",
    "Webex",
    "Slack",
    "Discord",
    "Audio playing",
    "Camera or microphone recording",
    "Camera recording",
    "Microphone recording",
];

/// Titles that carry no meeting-specific information; a cleaned title equal to
/// one of these (case-insensitive) is rejected so a better source can win.
#[cfg(any(target_os = "macos", test))]
const GENERIC_TITLES: &[&str] = &[
    "zoom",
    "zoom meeting",
    "zoom workplace",
    "zoom webinar",
    "meeting",
    "microsoft teams",
    "teams",
    "meet",
    "google meet",
    "whereby",
    "facetime",
    "slack",
    "discord",
    "huddle",
    "webex",
    "calendar",
    "chat",
    "activity",
    "new tab",
    "untitled",
    "settings",
];

/// Clean a raw window title into a meeting name, or `None` when nothing
/// meaningful remains. `app_name` is the detected app's display name (also
/// treated as a strippable suffix / generic value).
#[cfg(any(target_os = "macos", test))]
fn clean_window_title(raw: &str, app_name: &str) -> Option<String> {
    let mut title = raw.trim();

    // Strip decorative prefixes some apps prepend (recording dot, mute state).
    for prefix in ["● ", "🔴 ", "* "] {
        if let Some(rest) = title.strip_prefix(prefix) {
            title = rest.trim_start();
        }
    }

    // Repeatedly strip "<sep> <product>" suffixes: "Standup - Google Chrome",
    // "Planning | Microsoft Teams", "Retro — Mozilla Firefox".
    let mut owned = title.to_string();
    loop {
        let before = owned.len();
        for sep in [" - ", " – ", " — ", " | "] {
            if let Some(idx) = owned.rfind(sep) {
                let tail = owned[idx + sep.len()..].trim();
                let is_product = TITLE_SUFFIXES.iter().any(|s| tail.eq_ignore_ascii_case(s))
                    || tail.eq_ignore_ascii_case(app_name);
                if is_product {
                    owned.truncate(idx);
                    let trimmed = owned.trim_end().len();
                    owned.truncate(trimmed);
                }
            }
        }
        if owned.len() == before {
            break;
        }
    }

    // "Meet – abc-defg-hij" / "Meet - Sprint Planning": drop the product
    // prefix and judge what remains.
    for prefix in ["Meet – ", "Meet - ", "Meet — "] {
        if let Some(rest) = owned.strip_prefix(prefix) {
            owned = rest.trim().to_string();
            break;
        }
    }

    let cleaned = owned.trim();
    if cleaned.len() < 3 {
        return None;
    }
    let lower = cleaned.to_lowercase();
    if lower == app_name.to_lowercase() || GENERIC_TITLES.contains(&lower.as_str()) {
        return None;
    }
    // A bare Google Meet room code ("abc-defg-hij") names nothing.
    if is_meet_code(cleaned) {
        return None;
    }

    Some(truncate_title(cleaned))
}

/// True for Google Meet room codes like "abc-defg-hij" (3-4-3 lowercase
/// letters), which appear as the tab title in unnamed Meet calls.
#[cfg(any(target_os = "macos", test))]
fn is_meet_code(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 3
        && parts[0].len() == 3
        && parts[1].len() == 4
        && parts[2].len() == 3
        && parts
            .iter()
            .all(|p| p.chars().all(|c| c.is_ascii_lowercase()))
}

/// Cap a title at [`MAX_TITLE_LEN`], cutting at a word boundary with an
/// ellipsis when it overflows.
#[cfg(any(target_os = "macos", test))]
fn truncate_title(title: &str) -> String {
    if title.chars().count() <= MAX_TITLE_LEN {
        return title.to_string();
    }
    let cut: String = title.chars().take(MAX_TITLE_LEN).collect();
    let cut = match cut.rfind(' ') {
        Some(idx) if idx > MAX_TITLE_LEN / 2 => &cut[..idx],
        _ => cut.as_str(),
    };
    format!("{}…", cut.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_browser_suffix() {
        assert_eq!(
            clean_window_title("Weekly Sync - Google Chrome", "Chrome"),
            Some("Weekly Sync".to_string())
        );
        assert_eq!(
            clean_window_title("Retro — Mozilla Firefox", "Firefox"),
            Some("Retro".to_string())
        );
    }

    #[test]
    fn strips_teams_pipe_suffix() {
        assert_eq!(
            clean_window_title("Sprint Planning | Microsoft Teams", "Microsoft Teams"),
            Some("Sprint Planning".to_string())
        );
    }

    #[test]
    fn strips_stacked_suffixes() {
        // Meet tab in Chrome: page title + browser suffix.
        assert_eq!(
            clean_window_title("Meet - Design Review - Google Chrome", "Chrome"),
            Some("Design Review".to_string())
        );
    }

    #[test]
    fn rejects_generic_titles() {
        assert_eq!(clean_window_title("Zoom Meeting", "Zoom"), None);
        assert_eq!(clean_window_title("Zoom", "Zoom"), None);
        assert_eq!(
            clean_window_title("Microsoft Teams", "Microsoft Teams"),
            None
        );
        assert_eq!(
            clean_window_title("New Tab - Google Chrome", "Chrome"),
            None
        );
        assert_eq!(clean_window_title("", "Zoom"), None);
        assert_eq!(clean_window_title("  ", "Zoom"), None);
    }

    #[test]
    fn rejects_bare_meet_codes() {
        assert_eq!(
            clean_window_title("Meet – abc-defg-hij - Google Chrome", "Chrome"),
            None
        );
        // But a NAMED Meet call survives.
        assert_eq!(
            clean_window_title("Meet – Weekly Standup - Google Chrome", "Chrome"),
            Some("Weekly Standup".to_string())
        );
    }

    #[test]
    fn keeps_meaningful_zoom_topic() {
        assert_eq!(
            clean_window_title("Q3 Roadmap Review - Zoom Workplace", "Zoom"),
            Some("Q3 Roadmap Review".to_string())
        );
    }

    #[test]
    fn truncates_overlong_titles_at_word_boundary() {
        let long = "word ".repeat(40);
        let cleaned = clean_window_title(&long, "Zoom").unwrap();
        assert!(cleaned.chars().count() <= MAX_TITLE_LEN + 1);
        assert!(cleaned.ends_with('…'));
    }

    #[test]
    fn meet_code_shape() {
        assert!(is_meet_code("abc-defg-hij"));
        assert!(!is_meet_code("abc-defg"));
        assert!(!is_meet_code("Sprint-Planning-Q3"));
    }
}
