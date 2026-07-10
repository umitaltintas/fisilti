// Meeting auto-detection (macOS): a background poll thread that watches for
// meeting apps (Zoom, Teams, Webex, …) or browsers (Meet runs in a tab)
// actively USING the microphone, prompts the user to start a meeting
// transcription session, and — while a session is running — drives the
// "end meeting?" flow (prolonged silence via `MeetingManager`, or the meeting
// app releasing the microphone), auto-ending the session if the prompt is
// ignored for `meeting_auto_end_grace_secs`.
//
// Detection strategy: CoreAudio process objects (macOS 14+, same HAL API
// family the meeting capture tap already uses via `cidre`):
//   kAudioHardwarePropertyProcessObjectList → per-process
//   kAudioProcessPropertyBundleID + kAudioProcessPropertyIsRunningInput.
// A process from the known meeting-app/browser bundle allowlist that is
// running audio INPUT is treated as "in a meeting". Our own process is excluded
// by PID (its capture tap makes it show as running input during a session).
//
// All public functions below are called from `commands/meeting.rs` wrappers,
// from `MeetingManager`'s capture loop (silence), and from `lib.rs` (spawn).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::{AppHandle, Manager};

use crate::meeting::MeetingState;
use crate::meeting_prompt::{EndReason, MeetingPromptPayload};

/// Event emitted whenever the detection snapshot changes (a meeting app started
/// / stopped using the mic, or a different app took over), carrying a
/// `MeetingDetectionStatus` payload for the settings UI / debugging.
pub const MEETING_DETECTION_CHANGED_EVENT: &str = "meeting-detection-changed";

/// Poll cadence of the background detector thread.
#[cfg(target_os = "macos")]
const POLL_INTERVAL: Duration = Duration::from_secs(3);

/// Consecutive idle polls a meeting signal must persist before the "start"
/// prompt appears. Debounces mic-test blips (~2 × 3s ≈ 6s).
#[cfg(any(target_os = "macos", test))]
const START_DEBOUNCE_POLLS: u32 = 2;

/// Consecutive running polls the meeting signal must stay ABSENT before the
/// app-closed auto-end fires (~5 × 3s ≈ 15s).
#[cfg(any(target_os = "macos", test))]
const AUTO_END_ABSENT_POLLS: u32 = 5;

/// Detection status snapshot for the frontend (settings UI / debugging).
#[derive(serde::Serialize, Clone, Debug, specta::Type)]
pub struct MeetingDetectionStatus {
    /// True when a meeting app is currently using the microphone.
    pub detected: bool,
    /// Human-readable name of the detected app, when `detected`.
    pub app_name: Option<String>,
}

/// Shared state for the detector thread + prompt responses. Managed in Tauri
/// state as `Arc<MeetingDetector>` by `spawn_meeting_detector`.
pub struct MeetingDetector {
    pub(crate) state: Mutex<DetectorState>,
}

/// Mutable detector state guarded by `MeetingDetector::state`.
#[derive(Default)]
pub(crate) struct DetectorState {
    /// Latest detection snapshot: read by `detection_status`, diffed each tick
    /// to decide whether to emit `MEETING_DETECTION_CHANGED_EVENT`.
    detected: bool,
    app_name: Option<String>,
    /// Debounce / snooze / auto-end state machine driven by the poll thread and
    /// the start-prompt responses.
    sm: DetectionSm,
    /// An "end meeting?" prompt is currently shown with its grace timer armed.
    end_pending: bool,
    /// Bumped on every end-prompt request/response so a superseded grace-timer
    /// thread can detect it lost the race and abort without ending the session.
    end_generation: u64,
}

impl MeetingDetector {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(DetectorState::default()),
        }
    }
}

impl Default for MeetingDetector {
    fn default() -> Self {
        Self::new()
    }
}

/// What a poll tick decided the thread should do. Executed OUTSIDE the state
/// lock so the prompt/stop calls (which re-enter the lock) can't deadlock.
#[cfg(any(target_os = "macos", test))]
#[derive(Debug, PartialEq, Eq)]
enum PollAction {
    /// Nothing to do this tick.
    None,
    /// Show the "start transcription?" prompt for the named app.
    ShowStartPrompt(String),
    /// Hide the "start transcription?" prompt.
    HideStartPrompt,
    /// Arm the auto-end flow because the meeting app released the microphone.
    RequestAutoEnd,
}

/// Pure debounce / snooze / auto-end state machine for the detector poll loop.
///
/// Deliberately free of `AppHandle` / CoreAudio so it can be unit-tested by
/// feeding a sequence of "signal present?" ticks and asserting the emitted
/// [`PollAction`]s. The poll thread interprets each action against the real
/// prompt/session APIs.
#[derive(Default)]
pub(crate) struct DetectionSm {
    /// Consecutive idle polls with a meeting signal present (start debounce).
    present_streak: u32,
    /// Consecutive running polls with NO signal (app-closed auto-end debounce).
    absent_streak: u32,
    /// The "start transcription?" prompt is currently on screen.
    start_prompt_visible: bool,
    /// Detection snoozed (user dismissed) until the current signal clears.
    snoozed: bool,
    /// A meeting-app signal has been observed at least once this session.
    session_saw_signal: bool,
    /// The previous tick ran with a live session. Used to detect the
    /// running→idle transition so a stopped session doesn't immediately
    /// re-prompt for the same (still ongoing) meeting.
    was_running: bool,
}

#[cfg(any(target_os = "macos", test))]
impl DetectionSm {
    /// One poll tick while NO meeting session is running.
    ///
    /// Debounces the signal ([`START_DEBOUNCE_POLLS`]) before asking to start,
    /// honors the snooze set by a dismissal, and lifts the snooze the moment the
    /// signal clears so the next meeting re-prompts.
    fn step_idle(&mut self, signal: Option<&str>) -> PollAction {
        // Running→idle transition: the session just ended (stopped by the user
        // or auto-ended) while the meeting app may still be on the call. Snooze
        // so the same meeting doesn't immediately re-prompt; the snooze lifts
        // as soon as the signal clears (same rule as a dismissal).
        if self.was_running {
            self.was_running = false;
            if signal.is_some() {
                self.snoozed = true;
            }
        }

        // No session is running → forget any prior session's auto-end tracking.
        self.session_saw_signal = false;
        self.absent_streak = 0;

        match signal {
            Some(name) => {
                self.present_streak = self.present_streak.saturating_add(1);
                if self.present_streak >= START_DEBOUNCE_POLLS
                    && !self.snoozed
                    && !self.start_prompt_visible
                {
                    self.start_prompt_visible = true;
                    PollAction::ShowStartPrompt(name.to_string())
                } else {
                    PollAction::None
                }
            }
            None => {
                self.present_streak = 0;
                // Signal gone → lift the snooze so a new meeting re-prompts, and
                // hide a prompt the user never answered (meeting ended first).
                self.snoozed = false;
                if self.start_prompt_visible {
                    self.start_prompt_visible = false;
                    PollAction::HideStartPrompt
                } else {
                    PollAction::None
                }
            }
        }
    }

    /// One poll tick while a meeting session IS running.
    ///
    /// Ensures a stale "start" prompt is dismissed, tracks whether the meeting
    /// app was ever seen this session, and — only once it has been — requests
    /// auto-end after the app stays absent for [`AUTO_END_ABSENT_POLLS`]. The
    /// `end_pending` guard mirrors the shared pending flag so a request is not
    /// re-issued while one is already outstanding.
    fn step_running(
        &mut self,
        signal: Option<&str>,
        auto_end: bool,
        end_pending: bool,
    ) -> PollAction {
        self.was_running = true;
        self.present_streak = 0;

        // A "start" prompt must never linger once a session is live. Dismiss it
        // first; auto-end (if ever wanted) fires on a later tick.
        if self.start_prompt_visible {
            self.start_prompt_visible = false;
            return PollAction::HideStartPrompt;
        }

        match signal {
            Some(_) => {
                self.session_saw_signal = true;
                self.absent_streak = 0;
            }
            None => {
                self.absent_streak = self.absent_streak.saturating_add(1);
            }
        }

        if self.session_saw_signal
            && auto_end
            && !end_pending
            && self.absent_streak >= AUTO_END_ABSENT_POLLS
        {
            PollAction::RequestAutoEnd
        } else {
            PollAction::None
        }
    }
}

/// Known meeting-app / browser bundle ids → human names. Matched by exact id OR
/// by dotted-prefix so helper subprocesses count: browser audio typically runs
/// in a helper whose bundle id is "<browser>.helper…". Google Meet, Whereby and
/// other web meetings run in a browser tab, so a browser using the mic counts as
/// a meeting.
#[cfg(any(target_os = "macos", test))]
const MEETING_APPS: &[(&str, &str)] = &[
    // Dedicated meeting / call apps.
    ("us.zoom.xos", "Zoom"),
    ("com.microsoft.teams2", "Microsoft Teams"),
    ("com.microsoft.teams", "Microsoft Teams"),
    ("Cisco-Systems.Spark", "Webex"),
    ("com.cisco.webexmeetingsapp", "Webex"),
    ("com.tinyspeck.slackmacgap", "Slack"),
    ("com.hnc.Discord", "Discord"),
    ("com.apple.FaceTime", "FaceTime"),
    ("com.skype.skype", "Skype"),
    ("net.whatsapp.WhatsApp", "WhatsApp"),
    ("com.tdesktop.Telegram", "Telegram"),
    ("ru.keepcoder.Telegram", "Telegram"),
    // Browsers (Meet / Whereby / etc. run in a tab).
    ("com.google.Chrome", "Chrome"),
    ("com.apple.Safari", "Safari"),
    ("org.mozilla.firefox", "Firefox"),
    ("com.microsoft.edgemac", "Edge"),
    ("company.thebrowser.Browser", "Arc"),
    ("com.brave.Browser", "Brave"),
    ("com.vivaldi.Vivaldi", "Vivaldi"),
    ("com.operasoftware.Opera", "Opera"),
    ("org.chromium.Chromium", "Chromium"),
    ("app.zen-browser.zen", "Zen"),
];

/// A meeting app currently pulling mic input, matched against the allowlist.
#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MeetingSignal {
    /// Human-readable display name (e.g. "Zoom", "Chrome").
    pub name: &'static str,
    /// The allowlist MAIN bundle id (e.g. "com.google.Chrome"), even when the
    /// matched process was a helper subprocess. Used by `meeting_naming` to
    /// find the window-owning application.
    pub bundle_id: &'static str,
}

/// Map a process bundle id to its allowlist entry, matching by exact id or
/// dotted-prefix (so "com.google.Chrome.helper.Renderer" → "Chrome", but
/// "com.google.ChromeBeta" does NOT match "com.google.Chrome").
#[cfg(any(target_os = "macos", test))]
fn meeting_app_match(bundle_id: &str) -> Option<MeetingSignal> {
    MEETING_APPS.iter().find_map(|&(id, name)| {
        let is_match = bundle_id == id
            || bundle_id
                .strip_prefix(id)
                .is_some_and(|rest| rest.starts_with('.'));
        is_match.then_some(MeetingSignal {
            name,
            bundle_id: id,
        })
    })
}

/// Scan CoreAudio process objects for a known meeting app actively using the
/// microphone (audio INPUT). Returns the matched app, or `None` when nothing
/// matches or the API errors (older macOS, permission, …). Never panics:
/// any CoreAudio error is logged at debug and treated as "no signal".
#[cfg(target_os = "macos")]
fn detect_meeting_signal() -> Option<MeetingSignal> {
    use cidre::core_audio as ca;

    let processes = match ca::System::processes() {
        Ok(p) => p,
        Err(e) => {
            log::debug!("meeting-detector: process object list failed: {:?}", e);
            return None;
        }
    };
    let own_pid = std::process::id();

    for process in &processes {
        // Exclude ourselves: our capture tap / aggregate device makes this
        // process report running input during a meeting session.
        if let Ok(pid) = process.pid() {
            if pid as u32 == own_pid {
                continue;
            }
        }
        // Only processes actively pulling mic input count as "in a meeting".
        if !matches!(process.is_running_input(), Ok(true)) {
            continue;
        }
        let bundle = match process.bundle_id() {
            Ok(b) => b.to_string(),
            Err(_) => continue,
        };
        if let Some(signal) = meeting_app_match(&bundle) {
            return Some(signal);
        }
    }
    None
}

/// One-shot detection of the meeting app currently using the microphone, for
/// `meeting_naming`'s window-title lookup. Independent of the poll thread's
/// cached state so a manual session start (auto-detect off) still resolves.
#[cfg(target_os = "macos")]
pub(crate) fn current_meeting_signal() -> Option<MeetingSignal> {
    detect_meeting_signal()
}

/// Register the `Arc<MeetingDetector>` in Tauri state and start the poll
/// thread. Called once from `initialize_core_logic`. The state is registered on
/// EVERY platform so the meeting commands never panic; the poll thread is
/// macOS-only.
pub fn spawn_meeting_detector(app: AppHandle) {
    // Register the detector state on every platform (commands resolve it).
    app.manage(Arc::new(MeetingDetector::new()));

    #[cfg(target_os = "macos")]
    {
        std::thread::spawn(move || poll_loop(app));
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
    }
}

/// The background detection poll loop (macOS). Runs for the app's lifetime,
/// surviving every CoreAudio error (the loop must never crash — e.g. on macOS
/// < 14 the process-object API is unavailable and every tick simply sees "no
/// signal").
#[cfg(target_os = "macos")]
fn poll_loop(app: AppHandle) {
    use tauri::Emitter;

    // One-time availability probe so an older macOS logs a single clear line
    // instead of nothing.
    match cidre::core_audio::System::processes() {
        Ok(_) => log::info!("meeting-detector: started (process-object detection available)"),
        Err(e) => log::info!(
            "meeting-detector: process-object API unavailable ({:?}); auto-detection will stay idle",
            e
        ),
    }

    loop {
        std::thread::sleep(POLL_INTERVAL);

        let settings = crate::settings::get_settings(&app);
        let auto_detect = settings.meeting_auto_detect;
        let auto_end = settings.meeting_auto_end;

        let manager = match app.try_state::<Arc<crate::meeting::MeetingManager>>() {
            Some(m) => (*m).clone(),
            None => continue,
        };
        let detector = match app.try_state::<Arc<MeetingDetector>>() {
            Some(d) => (*d).clone(),
            None => continue,
        };
        let running = manager.status() == MeetingState::Running;

        // Poll CoreAudio only when it can matter: auto-detect enabled, or a
        // session is running (so app-closed auto-end can track the signal).
        let active = auto_detect || running;
        let signal = if active {
            detect_meeting_signal()
        } else {
            None
        };
        let signal_name: Option<&str> = signal.map(|s| s.name);

        let mut actions: Vec<PollAction> = Vec::new();
        let mut hide_end_prompt = false;
        let mut changed: Option<MeetingDetectionStatus> = None;
        {
            let mut st = detector.state.lock().unwrap();

            // Snapshot + change detection.
            let detected = signal.is_some();
            if st.detected != detected || st.app_name.as_deref() != signal_name {
                st.detected = detected;
                st.app_name = signal_name.map(str::to_string);
                changed = Some(MeetingDetectionStatus {
                    detected,
                    app_name: signal_name.map(str::to_string),
                });
            }

            if !active {
                // Auto-detect off and idle: drop transient prompt/debounce state.
                if st.sm.start_prompt_visible {
                    actions.push(PollAction::HideStartPrompt);
                }
                st.sm = DetectionSm::default();
                // Same safety net as the idle branch below: a manually-started
                // session (auto-detect off) stopped by another path while its
                // end prompt was still up must not leave the window lingering.
                if st.end_pending {
                    st.end_pending = false;
                    st.end_generation = st.end_generation.wrapping_add(1);
                    hide_end_prompt = true;
                }
            } else if running {
                let end_pending = st.end_pending;
                actions.push(st.sm.step_running(signal_name, auto_end, end_pending));
            } else {
                actions.push(st.sm.step_idle(signal_name));
                // Safety net: the session was stopped by another path while an
                // end prompt was still up → clear it and hide the window.
                if st.end_pending {
                    st.end_pending = false;
                    st.end_generation = st.end_generation.wrapping_add(1);
                    hide_end_prompt = true;
                }
            }
        }

        // Execute the decided actions OUTSIDE the lock.
        let mut request_end = false;
        for action in actions {
            match action {
                PollAction::ShowStartPrompt(app_name) => {
                    crate::meeting_prompt::show_meeting_prompt(
                        &app,
                        MeetingPromptPayload::Start { app_name },
                    )
                }
                PollAction::HideStartPrompt => crate::meeting_prompt::hide_meeting_prompt(&app),
                PollAction::RequestAutoEnd => request_end = true,
                PollAction::None => {}
            }
        }
        if hide_end_prompt {
            crate::meeting_prompt::hide_meeting_prompt(&app);
        }
        if request_end {
            request_auto_end(&app, EndReason::AppClosed);
        }
        if let Some(status) = changed {
            let _ = app.emit(MEETING_DETECTION_CHANGED_EVENT, status);
        }
    }
}

/// Ask the user whether to end the running meeting session (shows the "end"
/// prompt and arms the auto-end grace timer). Idempotent while a prompt is
/// already pending; no-op when no session is running or auto-end is disabled.
/// Called from the capture loop (silence) and the detector thread (app closed).
pub fn request_auto_end(app: &AppHandle, reason: EndReason) {
    let manager = match app.try_state::<Arc<crate::meeting::MeetingManager>>() {
        Some(m) => (*m).clone(),
        None => return,
    };
    if manager.status() != MeetingState::Running {
        return;
    }

    let settings = crate::settings::get_settings(app);
    if !settings.meeting_auto_end {
        return;
    }
    let grace_secs = settings.meeting_auto_end_grace_secs;

    let detector = match app.try_state::<Arc<MeetingDetector>>() {
        Some(d) => (*d).clone(),
        None => return,
    };

    // Arm exactly one pending prompt; idempotent while one is already pending
    // (the silence path calls this ~1×/s while silence exceeds the threshold).
    let generation = {
        let mut st = detector.state.lock().unwrap();
        if st.end_pending {
            return;
        }
        st.end_pending = true;
        st.end_generation = st.end_generation.wrapping_add(1);
        st.end_generation
    };

    crate::meeting_prompt::show_meeting_prompt(
        app,
        MeetingPromptPayload::End { reason, grace_secs },
    );

    // Grace timer: auto-end if the user ignores the prompt. On its own thread so
    // the (blocking) finalize inside stop is fine.
    let app = app.clone();
    let detector = detector.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(grace_secs as u64));

        // Only proceed if THIS request is still the outstanding one (a user
        // response bumps the generation to abort us).
        {
            let st = detector.state.lock().unwrap();
            if !st.end_pending || st.end_generation != generation {
                return;
            }
        }

        let manager = match app.try_state::<Arc<crate::meeting::MeetingManager>>() {
            Some(m) => (*m).clone(),
            None => return,
        };
        if manager.status() != MeetingState::Running {
            // Session already stopped elsewhere; clear the pending flag and make
            // sure the (now moot) prompt window isn't left on screen.
            detector.state.lock().unwrap().end_pending = false;
            crate::meeting_prompt::hide_meeting_prompt(&app);
            return;
        }

        log::info!(
            "meeting auto-end: grace elapsed ({}s, {:?}); ending session",
            grace_secs,
            reason
        );
        crate::meeting_prompt::hide_meeting_prompt(&app);
        if let Err(e) = crate::commands::meeting::stop_meeting_session(&app, &manager) {
            log::warn!("meeting auto-end: stop failed: {}", e);
        }
        detector.state.lock().unwrap().end_pending = false;
    });
}

/// Handle the user's response to the "end meeting?" prompt. `continue_meeting`
/// true → keep the session, reset the silence timer; false → stop the session.
/// Always hides the prompt window (even when nothing was pending).
#[cfg(target_os = "macos")]
pub fn respond_auto_end(app: &AppHandle, continue_meeting: bool) -> Result<(), String> {
    let detector = app.try_state::<Arc<MeetingDetector>>();

    // Clear the pending flag and invalidate any armed grace timer.
    let was_pending = match &detector {
        Some(d) => {
            let mut st = d.state.lock().unwrap();
            let was = st.end_pending;
            st.end_pending = false;
            st.end_generation = st.end_generation.wrapping_add(1);
            was
        }
        None => false,
    };

    // Always dismiss the prompt window.
    crate::meeting_prompt::hide_meeting_prompt(app);

    if !was_pending {
        return Ok(());
    }

    if continue_meeting {
        if let Some(manager) = app.try_state::<Arc<crate::meeting::MeetingManager>>() {
            manager.reset_silence_timer();
        }
        return Ok(());
    }

    // End the session on a blocking thread (the finalize pass is long) and
    // return immediately.
    if let Some(manager) = app.try_state::<Arc<crate::meeting::MeetingManager>>() {
        let manager = (*manager).clone();
        let app = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            if let Err(e) = crate::commands::meeting::stop_meeting_session(&app, &manager) {
                log::warn!("meeting auto-end: manual end failed: {}", e);
            }
        });
    }
    Ok(())
}

/// Off-macOS: meetings never run, so there is nothing to end.
#[cfg(not(target_os = "macos"))]
pub fn respond_auto_end(app: &AppHandle, continue_meeting: bool) -> Result<(), String> {
    let _ = (app, continue_meeting);
    Err("meeting auto-end is not available on this platform".to_string())
}

/// User accepted the "start transcription?" prompt: hide it and start a
/// meeting session.
#[cfg(target_os = "macos")]
pub fn accept_start_prompt(app: &AppHandle) -> Result<(), String> {
    if let Some(detector) = app.try_state::<Arc<MeetingDetector>>() {
        let mut st = detector.state.lock().unwrap();
        st.sm.start_prompt_visible = false;
        // The meeting app is live → let app-closed auto-end arm later even if the
        // poll thread doesn't re-observe the signal before it releases the mic.
        st.sm.session_saw_signal = true;
    }
    crate::meeting_prompt::hide_meeting_prompt(app);

    let manager = app
        .try_state::<Arc<crate::meeting::MeetingManager>>()
        .ok_or_else(|| "Meeting manager is not initialized".to_string())?;
    let manager = (*manager).clone();
    crate::commands::meeting::start_meeting_session(app, &manager)
}

/// Off-macOS: auto-detection never runs, so there is no prompt to accept.
#[cfg(not(target_os = "macos"))]
pub fn accept_start_prompt(app: &AppHandle) -> Result<(), String> {
    let _ = app;
    Err("meeting auto-detection is not available on this platform".to_string())
}

/// User dismissed the "start transcription?" prompt: hide it and snooze
/// detection until the current meeting signal clears.
#[cfg(target_os = "macos")]
pub fn dismiss_start_prompt(app: &AppHandle) -> Result<(), String> {
    if let Some(detector) = app.try_state::<Arc<MeetingDetector>>() {
        let mut st = detector.state.lock().unwrap();
        st.sm.start_prompt_visible = false;
        st.sm.snoozed = true;
    }
    crate::meeting_prompt::hide_meeting_prompt(app);
    Ok(())
}

/// Off-macOS: auto-detection never runs, so there is no prompt to dismiss.
#[cfg(not(target_os = "macos"))]
pub fn dismiss_start_prompt(app: &AppHandle) -> Result<(), String> {
    let _ = app;
    Err("meeting auto-detection is not available on this platform".to_string())
}

/// Current detection snapshot for the frontend.
pub fn detection_status(app: &AppHandle) -> MeetingDetectionStatus {
    if let Some(detector) = app.try_state::<Arc<MeetingDetector>>() {
        let st = detector.state.lock().unwrap();
        MeetingDetectionStatus {
            detected: st.detected,
            app_name: st.app_name.clone(),
        }
    } else {
        MeetingDetectionStatus {
            detected: false,
            app_name: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test shim: display name of the matched allowlist entry.
    fn meeting_app_name(bundle_id: &str) -> Option<&'static str> {
        meeting_app_match(bundle_id).map(|s| s.name)
    }

    #[test]
    fn bundle_matching_exact_and_prefix() {
        // Exact dedicated-app ids.
        assert_eq!(meeting_app_name("us.zoom.xos"), Some("Zoom"));
        assert_eq!(
            meeting_app_name("com.microsoft.teams2"),
            Some("Microsoft Teams")
        );
        assert_eq!(
            meeting_app_name("com.microsoft.teams"),
            Some("Microsoft Teams")
        );
        assert_eq!(meeting_app_name("com.apple.FaceTime"), Some("FaceTime"));
        assert_eq!(meeting_app_name("company.thebrowser.Browser"), Some("Arc"));

        // Browser helper subprocesses: dotted-prefix match. The signal always
        // reports the MAIN allowlist bundle id, not the helper's.
        assert_eq!(meeting_app_name("com.google.Chrome"), Some("Chrome"));
        assert_eq!(meeting_app_name("com.google.Chrome.helper"), Some("Chrome"));
        assert_eq!(
            meeting_app_match("com.google.Chrome.helper.Renderer"),
            Some(MeetingSignal {
                name: "Chrome",
                bundle_id: "com.google.Chrome"
            })
        );

        // A non-dotted extension is a DIFFERENT app and must not match.
        assert_eq!(meeting_app_name("com.google.ChromeBeta"), None);
        assert_eq!(meeting_app_name("com.unknown.app"), None);
        assert_eq!(meeting_app_name(""), None);
    }

    #[test]
    fn start_prompt_requires_debounce_then_hides_on_clear() {
        let mut sm = DetectionSm::default();
        // First sighting is below the debounce threshold → nothing yet.
        assert_eq!(sm.step_idle(Some("Zoom")), PollAction::None);
        // Second consecutive sighting → prompt.
        assert_eq!(
            sm.step_idle(Some("Zoom")),
            PollAction::ShowStartPrompt("Zoom".to_string())
        );
        assert!(sm.start_prompt_visible);
        // Still present → not re-shown.
        assert_eq!(sm.step_idle(Some("Zoom")), PollAction::None);
        // Signal clears → prompt hidden.
        assert_eq!(sm.step_idle(None), PollAction::HideStartPrompt);
        assert!(!sm.start_prompt_visible);
    }

    #[test]
    fn snooze_suppresses_until_signal_clears() {
        let mut sm = DetectionSm::default();
        sm.step_idle(Some("Zoom"));
        assert_eq!(
            sm.step_idle(Some("Zoom")),
            PollAction::ShowStartPrompt("Zoom".to_string())
        );
        // User dismisses (mirrors dismiss_start_prompt).
        sm.start_prompt_visible = false;
        sm.snoozed = true;
        // The same meeting keeps signaling → suppressed.
        assert_eq!(sm.step_idle(Some("Zoom")), PollAction::None);
        assert_eq!(sm.step_idle(Some("Zoom")), PollAction::None);
        // Meeting ends → snooze lifts.
        assert_eq!(sm.step_idle(None), PollAction::None);
        assert!(!sm.snoozed);
        // A new meeting re-prompts after the debounce.
        assert_eq!(sm.step_idle(Some("Zoom")), PollAction::None);
        assert_eq!(
            sm.step_idle(Some("Zoom")),
            PollAction::ShowStartPrompt("Zoom".to_string())
        );
    }

    #[test]
    fn running_hides_stale_start_prompt() {
        let mut sm = DetectionSm::default();
        sm.start_prompt_visible = true;
        assert_eq!(
            sm.step_running(Some("Zoom"), true, false),
            PollAction::HideStartPrompt
        );
        assert!(!sm.start_prompt_visible);
    }

    #[test]
    fn auto_end_fires_after_app_closes_once_seen() {
        let mut sm = DetectionSm::default();
        // See the meeting app while running.
        assert_eq!(sm.step_running(Some("Zoom"), true, false), PollAction::None);
        // It releases the mic: needs AUTO_END_ABSENT_POLLS consecutive absences.
        for _ in 0..(AUTO_END_ABSENT_POLLS - 1) {
            assert_eq!(sm.step_running(None, true, false), PollAction::None);
        }
        assert_eq!(
            sm.step_running(None, true, false),
            PollAction::RequestAutoEnd
        );
    }

    #[test]
    fn auto_end_needs_prior_signal() {
        let mut sm = DetectionSm::default();
        // Never saw the app → never auto-ends, however long it stays absent.
        for _ in 0..(AUTO_END_ABSENT_POLLS + 3) {
            assert_eq!(sm.step_running(None, true, false), PollAction::None);
        }
    }

    #[test]
    fn no_reprompt_for_same_meeting_after_session_stops() {
        let mut sm = DetectionSm::default();
        // A session runs while the meeting app holds the mic, then the user
        // stops it (running→idle) with the app STILL on the call.
        sm.step_running(Some("Zoom"), true, false);
        assert_eq!(sm.step_idle(Some("Zoom")), PollAction::None);
        assert!(sm.snoozed);
        // Even past the debounce threshold the prompt stays suppressed.
        assert_eq!(sm.step_idle(Some("Zoom")), PollAction::None);
        assert_eq!(sm.step_idle(Some("Zoom")), PollAction::None);
        // The meeting actually ends → snooze lifts; the NEXT meeting re-prompts.
        assert_eq!(sm.step_idle(None), PollAction::None);
        assert_eq!(sm.step_idle(Some("Zoom")), PollAction::None);
        assert_eq!(
            sm.step_idle(Some("Zoom")),
            PollAction::ShowStartPrompt("Zoom".to_string())
        );
    }

    #[test]
    fn auto_end_respects_flag_and_pending() {
        let mut sm = DetectionSm::default();
        sm.step_running(Some("Zoom"), true, false);
        for _ in 0..AUTO_END_ABSENT_POLLS {
            let _ = sm.step_running(None, true, false);
        }
        // auto_end disabled → suppressed.
        assert_eq!(sm.step_running(None, false, false), PollAction::None);
        // A prompt is already pending → suppressed (idempotent with the shared
        // pending flag).
        assert_eq!(sm.step_running(None, true, true), PollAction::None);
    }
}
