use crate::actions::ACTION_MAP;
use crate::managers::audio::AudioRecordingManager;
use log::{debug, error, warn};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

const DEBOUNCE: Duration = Duration::from_millis(30);

/// Commands processed sequentially by the coordinator thread.
enum Command {
    Input {
        binding_id: String,
        hotkey_string: String,
        is_pressed: bool,
        push_to_talk: bool,
    },
    Cancel {
        recording_was_active: bool,
    },
    ProcessingFinished,
}

/// Pipeline lifecycle, owned exclusively by the coordinator thread.
enum Stage {
    Idle,
    Recording(String), // binding_id
    Processing,
}

/// Serialises all transcription lifecycle events through a single thread
/// to eliminate race conditions between keyboard shortcuts, signals, and
/// the async transcribe-paste pipeline.
pub struct TranscriptionCoordinator {
    tx: Sender<Command>,
}

pub fn is_transcribe_binding(id: &str) -> bool {
    id == "transcribe" || id == "transcribe_with_post_process"
}

impl TranscriptionCoordinator {
    pub fn new(app: AppHandle) -> Self {
        let (tx, rx) = mpsc::channel();

        thread::spawn(move || {
            let mut stage = Stage::Idle;
            let mut last_press: Option<Instant> = None;
            let mut processing_since: Option<Instant> = None;

            loop {
                let cmd = match rx.recv_timeout(WATCHDOG_TICK) {
                    Ok(cmd) => cmd,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if processing_is_stuck(&stage, processing_since, Instant::now()) {
                            error!(
                                "Dictation pipeline did not finish within {:?}; forcing idle",
                                PROCESSING_WATCHDOG
                            );
                            stage = Stage::Idle;
                            processing_since = None;
                            crate::utils::emit_dictation_error(
                                &app,
                                crate::utils::DictationStage::Recording,
                                "Dictation stopped responding and was reset.",
                            );
                            crate::utils::hide_recording_overlay(&app);
                            crate::tray::change_tray_icon(&app, crate::tray::TrayIconState::Idle);
                        }
                        continue;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                };

                // Each command runs under its own catch_unwind: a panic in one
                // action must cost that one dictation, not the coordinator
                // thread (and with it every hotkey until the app restarts).
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    handle_command(&app, &mut stage, &mut last_press, cmd)
                }));
                if let Err(e) = outcome {
                    error!("Transcription coordinator command panicked: {e:?}; resetting to idle");
                    stage = Stage::Idle;
                    crate::utils::hide_recording_overlay(&app);
                    crate::tray::change_tray_icon(&app, crate::tray::TrayIconState::Idle);
                }

                processing_since = match (&stage, processing_since) {
                    (Stage::Processing, Some(since)) => Some(since),
                    (Stage::Processing, None) => Some(Instant::now()),
                    _ => None,
                };
            }
            debug!("Transcription coordinator exited");
        });

        Self { tx }
    }
    /// Send a keyboard/signal input event for a transcribe binding.
    /// For signal-based toggles, use `is_pressed: true` and `push_to_talk: false`.
    pub fn send_input(
        &self,
        binding_id: &str,
        hotkey_string: &str,
        is_pressed: bool,
        push_to_talk: bool,
    ) {
        if self
            .tx
            .send(Command::Input {
                binding_id: binding_id.to_string(),
                hotkey_string: hotkey_string.to_string(),
                is_pressed,
                push_to_talk,
            })
            .is_err()
        {
            warn!("Transcription coordinator channel closed");
        }
    }

    pub fn notify_cancel(&self, recording_was_active: bool) {
        if self
            .tx
            .send(Command::Cancel {
                recording_was_active,
            })
            .is_err()
        {
            warn!("Transcription coordinator channel closed");
        }
    }

    pub fn notify_processing_finished(&self) {
        if self.tx.send(Command::ProcessingFinished).is_err() {
            warn!("Transcription coordinator channel closed");
        }
    }
}

fn start(app: &AppHandle, stage: &mut Stage, binding_id: &str, hotkey_string: &str) {
    let Some(action) = ACTION_MAP.get(binding_id) else {
        warn!("No action in ACTION_MAP for '{binding_id}'");
        return;
    };
    action.start(app, binding_id, hotkey_string);
    if app
        .try_state::<Arc<AudioRecordingManager>>()
        .map_or(false, |a| a.is_recording())
    {
        *stage = Stage::Recording(binding_id.to_string());
    } else {
        debug!("Start for '{binding_id}' did not begin recording; staying idle");
    }
}

fn stop(app: &AppHandle, stage: &mut Stage, binding_id: &str, hotkey_string: &str) {
    let Some(action) = ACTION_MAP.get(binding_id) else {
        warn!("No action in ACTION_MAP for '{binding_id}'");
        return;
    };
    action.stop(app, binding_id, hotkey_string);
    *stage = Stage::Processing;
}

/// How often the coordinator wakes up to check the watchdog.
const WATCHDOG_TICK: Duration = Duration::from_secs(1);

/// Longest the pipeline may stay in Processing before the coordinator gives up
/// on it and returns to Idle. Generous on purpose — a 30-minute recording on a
/// slow local model plus LLM post-processing legitimately takes minutes — it
/// only exists so that one wedged pipeline cannot disable dictation until the
/// app restarts.
const PROCESSING_WATCHDOG: Duration = Duration::from_secs(10 * 60);

fn processing_is_stuck(stage: &Stage, since: Option<Instant>, now: Instant) -> bool {
    matches!(stage, Stage::Processing)
        && since.is_some_and(|s| now.duration_since(s) >= PROCESSING_WATCHDOG)
}

fn handle_command(
    app: &AppHandle,
    stage: &mut Stage,
    last_press: &mut Option<Instant>,
    cmd: Command,
) {
    match cmd {
        Command::Input {
            binding_id,
            hotkey_string,
            is_pressed,
            push_to_talk,
        } => {
            // Debounce rapid-fire press events (key repeat / double-tap).
            // Releases always pass through for push-to-talk.
            if is_pressed {
                let now = Instant::now();
                if last_press.map_or(false, |t| now.duration_since(t) < DEBOUNCE) {
                    debug!("Debounced press for '{binding_id}'");
                    return;
                }
                *last_press = Some(now);
            }

            if push_to_talk {
                if is_pressed && matches!(stage, Stage::Idle) {
                    start(app, stage, &binding_id, &hotkey_string);
                } else if !is_pressed
                    && matches!(&*stage, Stage::Recording(id) if id == &binding_id)
                {
                    stop(app, stage, &binding_id, &hotkey_string);
                }
            } else if is_pressed {
                match &*stage {
                    Stage::Idle => {
                        start(app, stage, &binding_id, &hotkey_string);
                    }
                    Stage::Recording(id) if id == &binding_id => {
                        stop(app, stage, &binding_id, &hotkey_string);
                    }
                    _ => {
                        debug!("Ignoring press for '{binding_id}': pipeline busy")
                    }
                }
            }
        }
        Command::Cancel {
            recording_was_active,
        } => {
            // Don't reset during processing — wait for the pipeline to finish.
            if !matches!(stage, Stage::Processing)
                && (recording_was_active || matches!(stage, Stage::Recording(_)))
            {
                *stage = Stage::Idle;
            }
        }
        Command::ProcessingFinished => {
            // Only a pipeline we are waiting for may end Processing. A late
            // finish from one the watchdog already gave up on must not reset a
            // dictation that has started since.
            if matches!(stage, Stage::Processing) {
                *stage = Stage::Idle;
            } else {
                debug!("Ignoring ProcessingFinished outside Processing");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_watchdog_fires_only_for_a_long_processing_stage() {
        let start = Instant::now();
        let later = start + PROCESSING_WATCHDOG;
        assert!(processing_is_stuck(&Stage::Processing, Some(start), later));
        assert!(!processing_is_stuck(
            &Stage::Processing,
            Some(start),
            start + Duration::from_secs(5)
        ));
        assert!(!processing_is_stuck(&Stage::Idle, Some(start), later));
        assert!(!processing_is_stuck(
            &Stage::Recording("transcribe".into()),
            Some(start),
            later
        ));
        assert!(!processing_is_stuck(&Stage::Processing, None, later));
    }
}
