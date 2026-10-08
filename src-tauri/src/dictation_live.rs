//! Streaming dictation through the Gemini Live API.
//!
//! The batch path is strictly sequential: record, upload, transcribe, paste.
//! For a three-second dictation that is roughly two seconds of upload plus two
//! of model time *after* the user has stopped talking. Streaming removes the
//! wait rather than shrinking it — the audio is transcribed while it is being
//! spoken, so releasing the key only has to drain the tail.
//!
//! Deliberately narrower than [`crate::gemini_live`]'s meeting usage: one
//! audio source, no subtitles, no translation, no session resumption. A
//! dictation is seconds long, so the reconnect machinery a multi-hour meeting
//! needs would only add failure modes here.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::gemini_live::{LiveConfig, LiveMode, LiveSession, LiveTranscript};

/// How long `finish` waits for the model to close the final turn after the last
/// audio frame. The tail is the only latency streaming cannot remove: the model
/// has to hear the end of the sentence before it can commit it.
const TAIL_TIMEOUT: Duration = Duration::from_millis(2500);

/// How often the tail wait re-checks. Short enough to feel immediate on the
/// common case where the turn closes almost at once.
const TAIL_POLL: Duration = Duration::from_millis(25);

/// A dictation being transcribed as it is spoken.
pub struct DictationLive {
    session: LiveSession,
    state: Arc<Mutex<TranscriptState>>,
}

#[derive(Default)]
struct TranscriptState {
    /// Finalized fragments, in arrival order.
    committed: Vec<String>,
    /// Whether the model has closed a turn since the last audio was pushed.
    /// `finish` waits for this rather than a fixed sleep.
    turn_complete: bool,
}

impl DictationLive {
    /// Open a streaming session. Returns immediately; the socket connects in
    /// the background, so a slow connect never delays the recording from
    /// starting.
    ///
    /// `language` is the app's selected language, or `"auto"` to let the model
    /// detect — the same contract every other transcription path uses.
    pub fn start(api_key: String, model: String, language: &str) -> Self {
        let language_codes = match language {
            "auto" => Vec::new(),
            // Whisper's Simplified/Traditional split has no meaning here, the
            // same collapse the other cloud paths do.
            "zh-Hans" | "zh-Hant" => vec!["zh".to_string()],
            other => vec![other.to_string()],
        };

        let state = Arc::new(Mutex::new(TranscriptState::default()));
        let cb_state = state.clone();

        let session = LiveSession::start(
            LiveConfig {
                api_key,
                model,
                mode: LiveMode::Transcribe {
                    language_codes,
                    // Verbatim, like the batch dictation path: cleanup would
                    // silently rewrite the words being typed into an editor.
                    smart: false,
                },
            },
            "dictation",
            move |transcript: LiveTranscript| {
                let mut guard = cb_state.lock().unwrap_or_else(|e| e.into_inner());
                // `interim` is a rolling guess at the current utterance and is
                // deliberately ignored: pasting it would type text that the
                // next message revises.
                if let Some(text) = transcript.original.as_deref() {
                    if !text.trim().is_empty() {
                        guard.committed.push(text.to_string());
                    }
                }
                if transcript.turn_complete {
                    guard.turn_complete = true;
                }
            },
        );

        Self { session, state }
    }

    /// Feed captured audio. Cheap and non-blocking — safe to call from the
    /// capture thread.
    pub fn push_audio(&self, frames: &[f32]) {
        if !frames.is_empty() {
            self.state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .turn_complete = false;
        }
        self.session.push_audio(frames);
    }

    /// Stop streaming and return what was transcribed.
    ///
    /// Waits briefly for the model to close the turn it is mid-way through,
    /// because the last word of a dictation is usually still in flight when the
    /// user releases the key. Gives up after [`TAIL_TIMEOUT`] and returns
    /// whatever arrived: a slightly clipped transcript beats an indefinite
    /// hang, and the caller falls back to the batch path when this is empty.
    pub fn finish(&self) -> String {
        let deadline = Instant::now() + TAIL_TIMEOUT;
        loop {
            if self
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .turn_complete
            {
                break;
            }
            if Instant::now() >= deadline {
                log::warn!(
                    "dictation-live: turn did not close within {:?}; using what arrived",
                    TAIL_TIMEOUT
                );
                break;
            }
            std::thread::sleep(TAIL_POLL);
        }

        self.session.stop();

        let guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let text = guard
            .committed
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        text.trim().to_string()
    }

    /// Seconds of audio streamed, for usage accounting.
    pub fn audio_seconds(&self) -> u64 {
        self.session.audio_seconds()
    }
}

// --- Process-wide handle to the in-flight dictation ------------------------
//
// One dictation runs at a time (the coordinator serializes recordings), so a
// single slot is the whole state machine. Kept here rather than in Tauri state
// so the start/stop pair reads as two calls into one module.

static ACTIVE: Mutex<Option<Arc<DictationLive>>> = Mutex::new(None);

fn active_slot() -> std::sync::MutexGuard<'static, Option<Arc<DictationLive>>> {
    ACTIVE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Start streaming if the selected dictation model is the Live one.
///
/// Returns whether a session was started. A missing API key is not an error
/// here: it returns `false` and the recording proceeds down the normal buffered
/// path, whose Gemini client fails with "No Gemini API key set", which the
/// dictation pipeline reports to the user as a `dictation-error`.
///
/// Any session or frame sink left over from an earlier dictation is torn down
/// first. Without that, a stale sink kept streaming a later *local* dictation
/// to Google, and the stale session's text replaced the local transcript.
pub fn begin_if_selected(app: &tauri::AppHandle) -> bool {
    use tauri::Manager;

    abort_active(app);

    let settings = crate::settings::get_settings(app);
    let Some(model) = app
        .try_state::<Arc<crate::managers::model::ModelManager>>()
        .and_then(|mm| mm.get_model_info(&settings.selected_model))
    else {
        return false;
    };
    if !matches!(
        model.engine_type,
        crate::managers::model::EngineType::GeminiLive
    ) {
        return false;
    }

    let api_key = settings.gemini_api_key.trim().to_string();
    if api_key.is_empty() {
        log::warn!("dictation-live: no Gemini API key set; falling back to the buffered path");
        return false;
    }

    let live_model = if model.filename.trim().is_empty() {
        crate::gemini_live::DEFAULT_LIVE_TRANSCRIBE_MODEL.to_string()
    } else {
        model.filename.trim().to_string()
    };

    let session = Arc::new(DictationLive::start(
        api_key,
        live_model,
        &settings.selected_language,
    ));
    let sink: crate::managers::audio::FrameSink = {
        let session = session.clone();
        Arc::new(move |frames: &[f32]| session.push_audio(frames))
    };

    if let Some(rm) = app.try_state::<Arc<crate::managers::audio::AudioRecordingManager>>() {
        rm.set_frame_sink(sink);
    } else {
        return false;
    }

    *active_slot() = Some(session);
    log::info!("dictation-live: streaming session started");
    true
}

/// Stop the in-flight session and return its transcript, or `None` when no
/// session was running. An empty string means the session produced nothing and
/// the caller should fall back to the buffered path.
pub fn finish_active(app: &tauri::AppHandle) -> Option<String> {
    use tauri::Manager;

    if let Some(rm) = app.try_state::<Arc<crate::managers::audio::AudioRecordingManager>>() {
        rm.clear_frame_sink();
    }
    let session = active_slot().take()?;
    let seconds = session.audio_seconds();
    let text = session.finish();
    log::info!(
        "dictation-live: {} s streamed, {} chars transcribed",
        seconds,
        text.len()
    );
    Some(text)
}

/// Drop the in-flight session without waiting for its tail. Used when the user
/// cancels, where there is no transcript to wait for.
pub fn abort_active(app: &tauri::AppHandle) {
    use tauri::Manager;

    if let Some(rm) = app.try_state::<Arc<crate::managers::audio::AudioRecordingManager>>() {
        rm.clear_frame_sink();
    }
    if let Some(session) = active_slot().take() {
        session.session.stop();
        log::info!("dictation-live: session aborted");
    }
}
