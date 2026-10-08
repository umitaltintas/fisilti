// Per-session state of a live meeting.
//
// Everything that belongs to ONE meeting — its transcript, row id, capture
// buffers, usage tally, resolved title — lives in a `Session` that `start()`
// creates and `stop()` takes a handle to. A later `start()` builds a fresh
// `Session` instead of resetting shared fields, so it can never reach into a
// session that is still finalizing (which is how a quick restart used to write
// the old transcript into the new row and leak the old capture buffers).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Instant;

use serde::Serialize;
use specta::Type;

use crate::meeting::manager::TranscriptSegment;

/// Lifecycle of the meeting slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeetingState {
    /// No meeting is capturing or being saved.
    Idle,
    /// Capturing audio.
    Running,
    /// `stop()` is in progress: capture has ended (or is ending) and the
    /// finalize pass, persistence and audio save are still running. A new
    /// meeting cannot start, and the row must not be deleted or recovered.
    Finalizing,
}

impl MeetingState {
    /// Wire name used by `get_meeting_status`, `meeting-state-changed` and
    /// `MeetingSessionInfo::state`.
    pub fn as_str(self) -> &'static str {
        match self {
            MeetingState::Idle => "idle",
            MeetingState::Running => "running",
            MeetingState::Finalizing => "finalizing",
        }
    }
}

/// Snapshot of the meeting slot for the UI (`get_meeting_session` and the
/// `meeting-session-changed` event).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Type)]
pub struct MeetingSessionInfo {
    /// `"idle"` | `"running"` | `"finalizing"`.
    pub state: String,
    /// Row id of the live meeting (status `recording` until it completes).
    /// `None` when idle, and briefly after start until the capture sources are
    /// up and the row is inserted — a second `meeting-session-changed` event
    /// follows the moment it is known.
    pub meeting_id: Option<i64>,
    /// Absolute epoch-ms start of the live meeting; `None` when idle.
    pub started_at_ms: Option<i64>,
}

/// What `stop_meeting` returns.
#[derive(Clone, Debug, Serialize, Type)]
pub struct StopMeetingResult {
    /// Row id of the meeting that was just saved; `None` when the session was
    /// empty and discarded (or kept only for recovery, still `recording`).
    pub meeting_id: Option<i64>,
    /// Final transcript text.
    pub transcript: String,
}

/// Paths of the raw per-source capture buffers (little-endian f32, 16 kHz
/// mono) the finalize pass and recovery read back.
#[derive(Clone, Debug)]
pub struct SessionBuffers {
    /// Full microphone ("you") audio.
    pub mic: std::path::PathBuf,
    /// Full system ("others") audio.
    pub system: std::path::PathBuf,
    /// Full mixed mono audio (used for the saved playback audio).
    pub mixed: std::path::PathBuf,
}

impl SessionBuffers {
    pub fn paths(&self) -> [&std::path::Path; 3] {
        [&self.mic, &self.system, &self.mixed]
    }

    /// Delete the buffer files. Missing files are fine.
    pub fn remove_files(&self) {
        for path in self.paths() {
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => log::warn!("meeting: could not remove buffer {:?}: {}", path, e),
            }
        }
    }
}

/// Segments kept in timestamp order plus the joined text, maintained
/// incrementally. Rebuilding (sort + join of the whole meeting) on every live
/// segment was quadratic over a long meeting.
#[derive(Default)]
pub struct TranscriptBuf {
    segments: Vec<TranscriptSegment>,
    text: String,
}

impl TranscriptBuf {
    /// Insert `segment` in timestamp order (after any equal timestamps, so
    /// arrival order breaks ties like a stable sort would).
    pub fn push(&mut self, segment: TranscriptSegment) {
        let pos = self
            .segments
            .partition_point(|s| s.timestamp_ms <= segment.timestamp_ms);
        if pos == self.segments.len() {
            append_text(&mut self.text, &segment.text);
            self.segments.push(segment);
        } else {
            // A late segment from the other source: rare, so a rebuild is fine.
            self.segments.insert(pos, segment);
            self.rebuild();
        }
    }

    /// Replace everything (the finalize pass's final transcript).
    pub fn replace(&mut self, mut segments: Vec<TranscriptSegment>) {
        segments.sort_by_key(|s| s.timestamp_ms);
        self.segments = segments;
        self.rebuild();
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn segments(&self) -> &[TranscriptSegment] {
        &self.segments
    }

    fn rebuild(&mut self) {
        self.text.clear();
        for segment in &self.segments {
            append_text(&mut self.text, &segment.text);
        }
    }
}

fn append_text(text: &mut String, piece: &str) {
    if piece.is_empty() {
        return;
    }
    if !text.is_empty() {
        text.push(' ');
    }
    text.push_str(piece);
}

/// Everything one meeting owns. Shared (`Arc`) between the manager slot, the
/// capture thread, the live-transcription worker, the naming thread and the
/// `stop()` that finalizes it.
pub struct Session {
    /// Distinguishes sessions; guards late callbacks from acting on a newer one.
    pub generation: u64,
    /// Absolute epoch-ms start; segment timestamps are relative to it.
    pub started_at_ms: i64,
    /// Tells the capture loop to wind down.
    pub stop_signal: AtomicBool,
    /// Set when the app is quitting: skip slow work (queued live transcription)
    /// and leave the row recoverable.
    pub shutting_down: AtomicBool,
    pub transcript: Mutex<TranscriptBuf>,
    /// Row id of the in-progress (`recording`) row, once inserted.
    pub meeting_id: Mutex<Option<i64>>,
    /// Row id saved as `completed` by `stop()`.
    pub saved_id: Mutex<Option<i64>>,
    /// Capture buffers, once created.
    pub buffers: Mutex<Option<SessionBuffers>>,
    /// Gemini Live produced the transcript (any mode).
    pub live_gemini_active: AtomicBool,
    /// Gemini Live was translating (the transcript is final).
    pub live_translate_active: AtomicBool,
    /// Tokens spent by every Gemini path during this meeting.
    pub usage: Mutex<crate::ai_usage::MeetingUsage>,
    /// Reason every finalize request failed, if they all did.
    pub finalize_error: Mutex<Option<String>>,
    /// Title resolved from the calendar / meeting window.
    pub title: Mutex<Option<String>>,
    /// Last speech frame from either VAD (prolonged-silence auto-end).
    pub silence_anchor: Mutex<Instant>,
    /// Any VAD speech frame was seen this session.
    pub voiced: AtomicBool,
    /// The transcript changed since the last incremental persist.
    pub persist_dirty: AtomicBool,
    /// A buffer write error was already reported to the UI.
    pub buffer_error_reported: AtomicBool,
    /// The capture thread.
    pub worker: Mutex<Option<std::thread::JoinHandle<()>>>,
    #[cfg(target_os = "macos")]
    pub subtitles: Mutex<crate::meeting::live::SubtitleFeed>,
}

impl Session {
    pub fn new(generation: u64, started_at_ms: i64) -> Self {
        Self {
            generation,
            started_at_ms,
            stop_signal: AtomicBool::new(false),
            shutting_down: AtomicBool::new(false),
            transcript: Mutex::new(TranscriptBuf::default()),
            meeting_id: Mutex::new(None),
            saved_id: Mutex::new(None),
            buffers: Mutex::new(None),
            live_gemini_active: AtomicBool::new(false),
            live_translate_active: AtomicBool::new(false),
            usage: Mutex::new(crate::ai_usage::MeetingUsage::default()),
            finalize_error: Mutex::new(None),
            title: Mutex::new(None),
            silence_anchor: Mutex::new(Instant::now()),
            voiced: AtomicBool::new(false),
            persist_dirty: AtomicBool::new(false),
            buffer_error_reported: AtomicBool::new(false),
            worker: Mutex::new(None),
            #[cfg(target_os = "macos")]
            subtitles: Mutex::new(Default::default()),
        }
    }

    pub fn transcript(&self) -> MutexGuard<'_, TranscriptBuf> {
        self.transcript.lock().unwrap()
    }

    pub fn text(&self) -> String {
        self.transcript().text().to_string()
    }

    pub fn meeting_id(&self) -> Option<i64> {
        *self.meeting_id.lock().unwrap()
    }

    pub fn saved_id(&self) -> Option<i64> {
        *self.saved_id.lock().unwrap()
    }

    pub fn buffers(&self) -> Option<SessionBuffers> {
        self.buffers.lock().unwrap().clone()
    }

    pub fn title(&self) -> Option<String> {
        self.title.lock().unwrap().clone()
    }

    pub fn is_stopping(&self) -> bool {
        self.stop_signal.load(Ordering::SeqCst)
    }

    /// Speech was just observed by either source's VAD.
    pub fn note_speech(&self) {
        self.voiced.store(true, Ordering::Relaxed);
        *self.silence_anchor.lock().unwrap() = Instant::now();
    }

    /// Restart the silence timer without claiming speech was heard (the user
    /// answered "keep going").
    pub fn reset_silence_timer(&self) {
        *self.silence_anchor.lock().unwrap() = Instant::now();
    }

    /// Milliseconds since this session started.
    pub fn elapsed_ms(&self) -> u64 {
        (super::manager::now_epoch_ms() - self.started_at_ms).max(0) as u64
    }

    /// Fold one call's token usage into the session total.
    pub fn record_usage(&self, model: &str, input: u64, output: u64, audio_seconds: u64) {
        record_usage(&self.usage, model, input, output, audio_seconds);
    }
}

/// Fold one call's token usage into `tally`. Shared by live sessions and the
/// exclusive jobs (import, re-transcription, recovery), which keep their own.
pub fn record_usage(
    tally: &Mutex<crate::ai_usage::MeetingUsage>,
    model: &str,
    input_tokens: u64,
    output_tokens: u64,
    audio_seconds: u64,
) {
    if input_tokens == 0 && output_tokens == 0 && audio_seconds == 0 {
        return;
    }
    log::debug!(
        "usage: {} +{} in / +{} out tokens",
        model,
        input_tokens,
        output_tokens
    );
    tally
        .lock()
        .unwrap()
        .add_with_audio(model, input_tokens, output_tokens, audio_seconds);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meeting::manager::TranscriptSource;

    fn seg(text: &str, ts: u64) -> TranscriptSegment {
        TranscriptSegment {
            text: text.to_string(),
            timestamp_ms: ts,
            source: TranscriptSource::Mic,
            translation: None,
            speaker: None,
        }
    }

    #[test]
    fn in_order_segments_append_to_the_cached_text() {
        let mut t = TranscriptBuf::default();
        t.push(seg("one", 0));
        t.push(seg("two", 10));
        t.push(seg("three", 10));
        assert_eq!(t.text(), "one two three");
        assert_eq!(t.segments().len(), 3);
    }

    #[test]
    fn a_late_segment_lands_in_timestamp_order() {
        let mut t = TranscriptBuf::default();
        t.push(seg("first", 0));
        t.push(seg("third", 30));
        t.push(seg("second", 20));
        assert_eq!(t.text(), "first second third");
        let order: Vec<u64> = t.segments().iter().map(|s| s.timestamp_ms).collect();
        assert_eq!(order, vec![0, 20, 30]);
    }

    #[test]
    fn replace_sorts_and_skips_empty_text() {
        let mut t = TranscriptBuf::default();
        t.push(seg("live", 0));
        t.replace(vec![seg("b", 20), seg("", 5), seg("a", 10)]);
        assert_eq!(t.text(), "a b");
    }

    #[test]
    fn state_wire_names_match_the_frontend_contract() {
        assert_eq!(MeetingState::Idle.as_str(), "idle");
        assert_eq!(MeetingState::Running.as_str(), "running");
        assert_eq!(MeetingState::Finalizing.as_str(), "finalizing");
    }
}
