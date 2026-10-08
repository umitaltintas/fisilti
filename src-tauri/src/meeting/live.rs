// Gemini Live streaming for a running meeting: one Live session per capture
// source, the fragment accumulator that turns streamed partials into whole
// segments, and the subtitle strip fed by the remote side.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::manager::{MeetingManager, TranscriptSource};
use super::session::Session;

impl MeetingManager {
    /// Build the Gemini Live config from settings, or `None` when the feature
    /// is off or unconfigured. A missing API key is worth a log line: the user
    /// turned the feature on and would otherwise see silence.
    fn live_config(&self) -> Option<crate::gemini_live::LiveConfig> {
        use crate::gemini_live::LiveMode;
        let settings = crate::settings::get_settings(&self.app_handle);

        let (mode, configured_model) = match settings.meeting_live_mode.as_str() {
            "translate" => {
                let target = settings.meeting_live_translate_target.trim();
                (
                    LiveMode::Translate {
                        target_language: if target.is_empty() {
                            "en".to_string()
                        } else {
                            target.to_string()
                        },
                    },
                    settings.meeting_live_translate_model.clone(),
                )
            }
            "transcribe" => (
                LiveMode::Transcribe {
                    // Hints come from the meeting language when the user pinned
                    // one; otherwise let the model detect, which is what a
                    // mixed-language call needs.
                    language_codes: super::finalize::meeting_language_hints(&settings),
                    smart: settings.meeting_gemini_smart,
                },
                settings.meeting_live_transcribe_model.clone(),
            ),
            _ => return None,
        };

        let api_key = settings.gemini_api_key.trim().to_string();
        if api_key.is_empty() {
            log::warn!(
                "meeting: live mode '{}' is enabled but no Gemini API key is set",
                settings.meeting_live_mode
            );
            return None;
        }

        let model = {
            let m = configured_model.trim();
            if m.is_empty() {
                mode.default_model().to_string()
            } else {
                m.to_string()
            }
        };
        Some(crate::gemini_live::LiveConfig {
            api_key,
            model,
            mode,
        })
    }

    /// Open one Gemini Live session per capture source, so the resulting
    /// segments keep their "you" / "others" label. Returns `None` when live
    /// mode is off, leaving the normal transcription paths in charge.
    pub(super) fn start_live_sessions(&self, session: &Arc<Session>) -> Option<LiveSessions> {
        let config = self.live_config()?;
        log::info!(
            "meeting: gemini live on (model={}, mode={:?})",
            config.model,
            config.mode
        );
        session.live_gemini_active.store(true, Ordering::Relaxed);
        session.live_translate_active.store(
            matches!(config.mode, crate::gemini_live::LiveMode::Translate { .. }),
            Ordering::Relaxed,
        );
        let model = config.model.clone();
        let (mic, mic_pending) =
            self.open_live_source(session, config.clone(), TranscriptSource::Mic, "mic");
        let (system, system_pending) =
            self.open_live_source(session, config, TranscriptSource::System, "system");
        Some(LiveSessions {
            mic,
            system,
            mic_pending,
            system_pending,
            model,
        })
    }

    /// Update the subtitle strip: `live` is the utterance still being spoken,
    /// `finished` (when present) is the line that just settled.
    ///
    /// Hides the strip once nothing is left to show, so a long silence does not
    /// leave a stale sentence floating over the screen.
    fn update_subtitles(
        &self,
        session: &Session,
        live: Option<&str>,
        finished: Option<&LiveSegment>,
    ) {
        let snapshot = {
            let mut feed = session.subtitles.lock().unwrap();
            match finished {
                // Prefer the translation when there is one: the strip exists so
                // the user can follow a language they do not speak.
                Some(segment) => feed.settle(
                    segment
                        .translation
                        .as_deref()
                        .unwrap_or(segment.original.as_str()),
                ),
                None => {
                    if let Some(text) = live {
                        feed.set_pending(text);
                    }
                }
            }
            if feed.is_empty() {
                None
            } else {
                Some(feed.snapshot())
            }
        };
        match snapshot {
            Some(update) => {
                log::debug!(
                    "subtitle: settled={} chars, pending={} chars",
                    update.settled.len(),
                    update.pending.len()
                );
                crate::subtitle_overlay::show_subtitle(&self.app_handle, update)
            }
            None => crate::subtitle_overlay::hide_subtitle(&self.app_handle),
        }
    }

    /// Take the subtitle strip down (session ending).
    pub(super) fn clear_subtitles(&self, session: &Session) {
        *session.subtitles.lock().unwrap() = SubtitleFeed::default();
        crate::subtitle_overlay::hide_subtitle(&self.app_handle);
    }

    /// Start a live session for one source, accumulating streamed fragments into
    /// whole segments.
    fn open_live_source(
        &self,
        session: &Arc<Session>,
        config: crate::gemini_live::LiveConfig,
        source: TranscriptSource,
        label: &str,
    ) -> (crate::gemini_live::LiveSession, LiveBuilder) {
        let manager = self.clone();
        let session = session.clone();
        // Transcripts arrive as partial fragments; accumulate until the API
        // marks the turn complete, then emit one segment.
        let pending: LiveBuilder = Arc::new(Mutex::new(LiveSegmentBuilder::default()));
        let drain_handle = pending.clone();
        // Log the detected source language once per source. Worth having when a
        // user reports the wrong language being translated.
        let language_logged = Arc::new(AtomicBool::new(false));
        let source_label = label.to_string();
        // Only the remote side is subtitled, and only when the user asked for
        // it. Both are settled once here rather than per fragment: the callback
        // runs on every partial and `get_settings` deserializes the store.
        let subtitled = source == TranscriptSource::System
            && crate::settings::get_settings(&self.app_handle).meeting_subtitles;
        let translating = matches!(config.mode, crate::gemini_live::LiveMode::Translate { .. });
        log::info!(
            "gemini-live[{}]: subtitles {}",
            label,
            if subtitled { "on" } else { "off" }
        );
        let live = crate::gemini_live::LiveSession::start(config, label, move |fragment| {
            if let Some(language) = &fragment.source_language {
                if !language_logged.swap(true, Ordering::Relaxed) {
                    log::info!(
                        "gemini-live[{}]: detected source language {}",
                        source_label,
                        language
                    );
                }
            }
            let (finished, running_translation) = {
                let mut builder = pending.lock().unwrap();
                builder.absorb(&fragment, session.elapsed_ms());
                // Snapshot the translation as it grows, for the strip. Taken
                // under the same lock so it can never lag the fragment that
                // produced it.
                let running = builder.translation.trim().to_string();
                // Translate sessions do not appear to send `turnComplete` at
                // all — waiting for it produced hours of subtitles and an empty
                // stored transcript. A finalized original with a translation
                // already attached is a real, observed boundary, so it closes
                // the segment too. The translation can trail slightly into the
                // next one; a near-aligned transcript beats no transcript.
                let ready = fragment.turn_complete
                    || (translating && fragment.original.is_some() && !running.is_empty());
                if ready {
                    (builder.take(), running)
                } else {
                    (None, running)
                }
            };

            if subtitled && !session.is_stopping() {
                // The two modes have different "text so far". Translation
                // arrives as appended fragments, so the running accumulation IS
                // the in-progress line. Transcription instead resends a whole
                // revised hypothesis on `interim`, which replaces it.
                let live_text = if translating {
                    Some(running_translation)
                } else {
                    fragment.interim.clone()
                };
                manager.update_subtitles(&session, live_text.as_deref(), finished.as_ref());
            }

            if let Some(segment) = finished {
                manager.push_segment_with(
                    &session,
                    segment.original,
                    segment.translation,
                    None,
                    segment.timestamp_ms,
                    source,
                );
            }
        });
        (live, drain_handle)
    }

    /// Emit whatever each source's builder still holds.
    ///
    /// The last utterance of a meeting has no following one to close it, so
    /// without this it would sit in the accumulator and be lost — which for a
    /// short meeting means the entire transcript.
    pub(super) fn drain_live_builders(&self, session: &Session, sessions: &LiveSessions) {
        for (builder, source) in [
            (&sessions.mic_pending, TranscriptSource::Mic),
            (&sessions.system_pending, TranscriptSource::System),
        ] {
            let leftover = builder.lock().unwrap().take_final();
            if let Some(segment) = leftover {
                log::info!("gemini-live: flushed a trailing segment on stop");
                self.push_segment_with(
                    session,
                    segment.original,
                    segment.translation,
                    None,
                    segment.timestamp_ms,
                    source,
                );
            }
        }
    }

    /// Stop the live sessions, flush their trailing utterances and charge
    /// their usage to the session.
    pub(super) fn finish_live_sessions(&self, session: &Session, live: &LiveSessions) {
        live.stop();
        // Read the meters AFTER stopping, so the final usage report the server
        // sends on close is included.
        self.drain_live_builders(session, live);
        let (mic_in, mic_out) = live.mic.usage_totals();
        let (sys_in, sys_out) = live.system.usage_totals();
        // Both sources are separate sessions streaming in parallel, so the
        // billable audio is their SUM, not the meeting's wall-clock length.
        let audio_seconds = live.mic.audio_seconds() + live.system.audio_seconds();
        log::info!(
            "usage: live sessions streamed {}s of audio, reported {} tokens",
            audio_seconds,
            mic_in + sys_in + mic_out + sys_out
        );
        session.record_usage(
            &live.model,
            mic_in + sys_in,
            mic_out + sys_out,
            audio_seconds,
        );
    }
}

/// The per-source Gemini Live sessions owned by a running capture loop.
#[cfg(target_os = "macos")]
pub(super) struct LiveSessions {
    pub(super) mic: crate::gemini_live::LiveSession,
    pub(super) system: crate::gemini_live::LiveSession,
    /// Each source's in-progress accumulator, so the trailing utterance can be
    /// flushed when the meeting stops rather than stranded.
    mic_pending: LiveBuilder,
    system_pending: LiveBuilder,
    /// Model both sessions run on, needed to price their tokens.
    model: String,
}

/// Shared handle on a source's segment accumulator.
#[cfg(target_os = "macos")]
pub(super) type LiveBuilder = Arc<Mutex<LiveSegmentBuilder>>;

#[cfg(target_os = "macos")]
impl LiveSessions {
    /// Ask both sessions to end their audio streams, then wait (bounded) for
    /// the server to flush the last turn and its usage report.
    fn stop(&self) {
        self.mic.stop();
        self.system.stop();
        let deadline = Instant::now() + std::time::Duration::from_millis(2_500);
        for session in [&self.mic, &self.system] {
            let left = deadline.saturating_duration_since(Instant::now());
            if !session.wait_finished(left) {
                log::warn!("gemini-live: session did not finish flushing in time");
            }
        }
    }
}

/// Accumulates streamed Live API fragments into one finished segment.
///
/// The API emits the original and the translation as separate partial strings,
/// so both are concatenated until the turn closes. The timestamp is taken from
/// the FIRST fragment of a turn, so a segment is ordered by when its speech
/// started rather than when the model finished translating it.
#[cfg(target_os = "macos")]
#[derive(Default)]
pub(super) struct LiveSegmentBuilder {
    original: String,
    translation: String,
    timestamp_ms: Option<u64>,
    /// Latest speculative hypothesis for the utterance in progress.
    ///
    /// Kept as a LAST RESORT for the stored transcript. The API sometimes
    /// streams an utterance as interim updates and never finalizes it before
    /// the meeting stops — which produced subtitles the user could read while
    /// the saved transcript had nothing from that source at all. An
    /// unfinalized sentence is worth far more than a missing one.
    last_interim: String,
}

#[cfg(target_os = "macos")]
pub(super) struct LiveSegment {
    original: String,
    translation: Option<String>,
    timestamp_ms: u64,
}

#[cfg(target_os = "macos")]
impl LiveSegmentBuilder {
    fn absorb(&mut self, fragment: &crate::gemini_live::LiveTranscript, elapsed_ms: u64) {
        if fragment.original.is_some() || fragment.translation.is_some() {
            self.timestamp_ms.get_or_insert(elapsed_ms);
        }
        if let Some(text) = &fragment.interim {
            // Replaces, never appends: each interim restates the whole
            // in-progress utterance.
            self.last_interim = text.clone();
            self.timestamp_ms.get_or_insert(elapsed_ms);
        }
        if let Some(text) = &fragment.original {
            self.original.push_str(text);
        }
        if let Some(text) = &fragment.translation {
            self.translation.push_str(text);
        }
    }

    /// Take the accumulated segment, resetting for the next turn. Returns
    /// `None` for a turn that produced no text (the model emitted only audio).
    fn take(&mut self) -> Option<LiveSegment> {
        self.take_inner(false)
    }

    /// As `take`, but allowed to fall back to the unfinalized hypothesis.
    ///
    /// Only used when the session is ending. Mid-session the finalized text is
    /// usually moments away, and emitting the guess first would store the same
    /// sentence twice; at stop there is no "moments away" left.
    fn take_final(&mut self) -> Option<LiveSegment> {
        self.take_inner(true)
    }

    fn take_inner(&mut self, allow_interim: bool) -> Option<LiveSegment> {
        let mut original = std::mem::take(&mut self.original).trim().to_string();
        let translation = std::mem::take(&mut self.translation).trim().to_string();
        let interim = std::mem::take(&mut self.last_interim).trim().to_string();
        let timestamp_ms = self.timestamp_ms.take().unwrap_or(0);
        // Nothing was ever finalized, but the model told us what it heard.
        // Storing that beats storing silence.
        if allow_interim && original.is_empty() && translation.is_empty() && !interim.is_empty() {
            original = interim;
        }
        if original.is_empty() && translation.is_empty() {
            return None;
        }
        // When only the translation came through, it IS the transcript — better
        // than dropping the turn entirely.
        let (original, translation) = if original.is_empty() {
            (translation, String::new())
        } else {
            (original, translation)
        };
        Some(LiveSegment {
            original,
            translation: if translation.is_empty() {
                None
            } else {
                Some(translation)
            },
            timestamp_ms,
        })
    }
}

/// Rolling text for the subtitle strip.
///
/// Keeps the last few finalized lines plus the utterance still being spoken.
/// Bounded on purpose: a strip that grows without limit either overflows its
/// window or shrinks its own text, and the useful reading window during a
/// conversation is the last sentence or two, not the meeting so far.
#[cfg(target_os = "macos")]
#[derive(Default)]
pub struct SubtitleFeed {
    settled: std::collections::VecDeque<String>,
    pending: String,
}

/// How many finalized lines stay on screen under the in-progress one.
#[cfg(target_os = "macos")]
const SUBTITLE_HISTORY_LINES: usize = 1;

/// Hard cap on what the strip may show at once.
///
/// Sized like a real subtitle: roughly three lines at the strip's width. The
/// limit is enforced HERE rather than left to CSS, because a clamp that
/// silently stops applying (build tools are known to drop
/// `-webkit-box-orient`) turns into text running off the bottom of the window
/// with nothing to catch it.
#[cfg(target_os = "macos")]
pub(super) const SUBTITLE_MAX_CHARS: usize = 160;

/// Below this many characters of leftover budget, history is dropped entirely
/// rather than shown as a stump. A three-letter tail of a sentence that has
/// scrolled away is not context, it is litter.
#[cfg(target_os = "macos")]
const SUBTITLE_MIN_HISTORY_CHARS: usize = 24;

#[cfg(target_os = "macos")]
impl SubtitleFeed {
    /// A speculative hypothesis REPLACES the pending line — the API resends the
    /// whole in-progress utterance each time, so appending would stutter the
    /// text back on itself.
    fn set_pending(&mut self, text: &str) {
        self.pending = text.trim().to_string();
    }

    /// Finalized text retires the pending line and joins the history.
    fn settle(&mut self, text: &str) {
        let text = text.trim();
        self.pending.clear();
        if text.is_empty() {
            return;
        }
        self.settled.push_back(text.to_string());
        while self.settled.len() > SUBTITLE_HISTORY_LINES {
            self.settled.pop_front();
        }
    }

    /// What the strip should show, trimmed to fit.
    ///
    /// Trims from the FRONT: the newest words are the ones being spoken right
    /// now, so they are what must survive. History gives way before the
    /// in-progress line does, and an unusually long in-progress line is cut
    /// from its own start rather than truncated at the end.
    fn snapshot(&self) -> crate::subtitle_overlay::SubtitleUpdate {
        let pending = tail_chars(&self.pending, SUBTITLE_MAX_CHARS);
        // Whatever the in-progress line leaves over is spent on history — but
        // only if it is enough to be worth reading.
        let budget = SUBTITLE_MAX_CHARS.saturating_sub(pending.chars().count());
        let settled = if budget < SUBTITLE_MIN_HISTORY_CHARS {
            String::new()
        } else {
            tail_chars(
                &self.settled.iter().cloned().collect::<Vec<_>>().join(" "),
                budget,
            )
        };
        crate::subtitle_overlay::SubtitleUpdate { settled, pending }
    }

    fn is_empty(&self) -> bool {
        self.settled.is_empty() && self.pending.is_empty()
    }
}

/// Last `max` characters of `text`, starting at a word boundary so the strip
/// never opens mid-word.
#[cfg(target_os = "macos")]
pub(super) fn tail_chars(text: &str, max: usize) -> String {
    let text = text.trim();
    if max == 0 {
        return String::new();
    }
    let len = text.chars().count();
    if len <= max {
        return text.to_string();
    }
    let tail: String = text.chars().skip(len - max).collect();
    // Start at the next whole word; fall back to the hard cut when the tail is
    // a single very long token.
    match tail.find(' ') {
        Some(space) => tail[space + 1..].trim_start().to_string(),
        None => tail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn an_utterance_that_never_finalized_is_still_saved_at_stop() {
        // The regression this guards: the system side streamed a whole meeting
        // as interim updates, never finalized before stop, and the saved
        // transcript had nothing from it — while the user had been reading it
        // in the subtitles the entire time.
        let mut builder = LiveSegmentBuilder::default();
        builder.absorb(&fragment_interim("duyduğum cümle"), 1_000);
        assert!(
            builder.take().is_none(),
            "mid-session must not emit a guess"
        );

        let mut builder = LiveSegmentBuilder::default();
        builder.absorb(&fragment_interim("duyduğum cümle"), 1_000);
        let segment = builder.take_final().expect("stop must rescue it");
        assert_eq!(segment.original, "duyduğum cümle");
        assert_eq!(segment.timestamp_ms, 1_000);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_finalized_utterance_is_never_stored_twice() {
        let mut builder = LiveSegmentBuilder::default();
        builder.absorb(&fragment_interim("duydu"), 1_000);
        builder.absorb(&fragment(Some("duyduğum cümle"), None, true), 1_000);
        let segment = builder.take().expect("finalized text");
        assert_eq!(segment.original, "duyduğum cümle");
        // The guess that preceded it must have been consumed, not left behind
        // to reappear at stop.
        assert!(builder.take_final().is_none());
    }

    #[cfg(target_os = "macos")]
    fn fragment_interim(text: &str) -> crate::gemini_live::LiveTranscript {
        crate::gemini_live::LiveTranscript {
            original: None,
            translation: None,
            source_language: None,
            interim: Some(text.to_string()),
            turn_complete: false,
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_revised_hypothesis_replaces_the_pending_line_rather_than_appending() {
        use super::SubtitleFeed;
        let mut feed = SubtitleFeed::default();
        // The API resends the whole in-progress utterance each time; appending
        // would stutter the text back on itself ("bu bu bir bu bir test").
        feed.set_pending("bu");
        feed.set_pending("bu bir");
        feed.set_pending("bu bir test");
        let snapshot = feed.snapshot();
        assert_eq!(snapshot.pending, "bu bir test");
        assert_eq!(snapshot.settled, "");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn settling_a_line_clears_the_guess_that_produced_it() {
        use super::SubtitleFeed;
        let mut feed = SubtitleFeed::default();
        feed.set_pending("bu bir tes");
        feed.settle("Bu bir test.");
        let snapshot = feed.snapshot();
        assert_eq!(snapshot.settled, "Bu bir test.");
        // Leaving the guess up would show the sentence twice, once misspelled.
        assert_eq!(snapshot.pending, "");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_strip_keeps_only_the_most_recent_line() {
        use super::SubtitleFeed;
        let mut feed = SubtitleFeed::default();
        for i in 1..=6 {
            feed.settle(&format!("line{}", i));
        }
        let settled = feed.snapshot().settled;
        assert_eq!(settled, "line6");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_long_utterance_is_capped_keeping_the_words_being_spoken_now() {
        use super::{SubtitleFeed, SUBTITLE_MAX_CHARS};
        let mut feed = SubtitleFeed::default();
        let long = (0..80)
            .map(|i| format!("word{}", i))
            .collect::<Vec<_>>()
            .join(" ");
        feed.set_pending(&long);
        let pending = feed.snapshot().pending;
        assert!(pending.chars().count() <= SUBTITLE_MAX_CHARS);
        // The tail is what the speaker is saying right now, so it must survive.
        assert!(pending.ends_with("word79"));
        // ...and the head must not, or the strip would show stale words.
        assert!(!pending.contains("word0 "));
        // Never opens mid-word.
        assert!(pending.starts_with("word"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn history_gives_way_before_the_line_being_spoken() {
        use super::{SubtitleFeed, SUBTITLE_MAX_CHARS};
        let mut feed = SubtitleFeed::default();
        feed.settle(&"old ".repeat(60));
        feed.set_pending(&"new ".repeat(60));
        let snapshot = feed.snapshot();
        // The in-progress line takes the whole budget, so history is dropped
        // entirely rather than both being half-shown.
        assert!(snapshot.pending.chars().count() <= SUBTITLE_MAX_CHARS);
        assert!(snapshot.settled.is_empty());
        assert!(
            snapshot.settled.chars().count() + snapshot.pending.chars().count()
                <= SUBTITLE_MAX_CHARS
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn short_text_is_left_exactly_as_it_is() {
        use super::tail_chars;
        assert_eq!(tail_chars("kısa bir cümle", 160), "kısa bir cümle");
        assert_eq!(tail_chars("", 160), "");
        assert_eq!(tail_chars("herhangi bir şey", 0), "");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn empty_finalized_text_does_not_add_a_blank_line() {
        use super::SubtitleFeed;
        let mut feed = SubtitleFeed::default();
        feed.set_pending("   ");
        // A turn that produced no text must leave the strip empty, so the
        // caller hides it rather than showing an empty plate.
        feed.settle("   ");
        assert!(feed.is_empty());
    }

    #[cfg(target_os = "macos")]
    fn fragment(
        original: Option<&str>,
        translation: Option<&str>,
        turn_complete: bool,
    ) -> crate::gemini_live::LiveTranscript {
        crate::gemini_live::LiveTranscript {
            original: original.map(str::to_string),
            translation: translation.map(str::to_string),
            source_language: None,
            interim: None,
            turn_complete,
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn live_fragments_accumulate_into_one_segment_per_turn() {
        // The Live API streams a sentence as several partial fragments. Emitting
        // each one would litter the meeting with word-sized segments.
        let mut builder = LiveSegmentBuilder::default();
        builder.absorb(&fragment(Some("Merhaba "), None, false), 1_000);
        builder.absorb(&fragment(Some("dünya"), Some("Hello "), false), 1_500);
        builder.absorb(&fragment(None, Some("world"), true), 2_000);

        let segment = builder.take().expect("a turn with text yields a segment");
        assert_eq!(segment.original, "Merhaba dünya");
        assert_eq!(segment.translation.as_deref(), Some("Hello world"));
        // The timestamp comes from the FIRST fragment, so segments order by when
        // the speech started rather than when translation finished.
        assert_eq!(segment.timestamp_ms, 1_000);

        // The builder resets for the next turn.
        assert!(builder.take().is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn live_turn_without_text_produces_nothing() {
        // Audio-only turns arrive routinely; they must not create empty segments.
        let mut builder = LiveSegmentBuilder::default();
        builder.absorb(&fragment(None, None, true), 500);
        assert!(builder.take().is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn live_translation_alone_becomes_the_transcript() {
        // If only the translated side came through, using it as the transcript
        // beats dropping the utterance.
        let mut builder = LiveSegmentBuilder::default();
        builder.absorb(&fragment(None, Some("Hello"), true), 250);
        let segment = builder.take().expect("segment");
        assert_eq!(segment.original, "Hello");
        assert!(segment.translation.is_none());
        assert_eq!(segment.timestamp_ms, 250);
    }
}
