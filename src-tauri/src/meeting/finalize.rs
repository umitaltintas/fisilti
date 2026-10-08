// The on-stop finalize pass and its building blocks, shared with recovery,
// imports and re-transcription: re-transcribe the FULL per-source audio
// (Gemini batch when Gemini is the meeting model, the local final model in
// VAD-bounded windows otherwise) for a higher-quality transcript that
// replaces the live preview.
//
// Audio is read from the capture buffers a window (or a ≤50-minute Gemini
// piece) at a time, never whole.

use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Mutex;

use crate::audio_toolkit::constants::WHISPER_SAMPLE_RATE;

use super::buffers::AudioSource;
use super::manager::{MeetingManager, TranscriptSegment, TranscriptSource};
use super::session::Session;

/// Tally of transcription failures seen during a pass, used to tell "the
/// meeting was silent" apart from "every transcription call failed". Only the
/// first error message is kept — the requests all fail for the same reason (no
/// API balance, network down, model missing) and one line is what the UI shows.
#[derive(Default)]
pub(super) struct FinalizeErrors {
    pub count: usize,
    pub first: Option<String>,
}

impl FinalizeErrors {
    pub fn record(&mut self, err: String) {
        self.count += 1;
        if self.first.is_none() {
            self.first = Some(err);
        }
    }
}

/// What a transcription pass over one or more sources produced.
#[derive(Default)]
pub(super) struct PassResult {
    pub segments: Vec<TranscriptSegment>,
    pub errors: FinalizeErrors,
    /// Some source actually held audio.
    pub had_audio: bool,
}

impl PassResult {
    pub fn has_text(&self) -> bool {
        self.segments.iter().any(|s| !s.text.trim().is_empty())
    }

    /// The failure to report when the pass produced nothing usable: every
    /// request failed. `None` when it simply heard nothing.
    pub fn total_failure(&self) -> Option<String> {
        if self.has_text() || self.errors.count == 0 {
            return None;
        }
        Some(
            self.errors
                .first
                .clone()
                .unwrap_or_else(|| "transcription failed".to_string()),
        )
    }
}

/// One source to transcribe.
pub(super) struct SourceInput<'a> {
    pub source: TranscriptSource,
    pub audio: AudioSource<'a>,
    /// For logs and upload display names ("you", "others", "import").
    pub label: &'a str,
}

impl<'a> SourceInput<'a> {
    /// Open a capture buffer; a missing or unreadable file is skipped (logged).
    pub fn from_file(
        path: Option<&Path>,
        source: TranscriptSource,
        label: &'a str,
    ) -> Option<Self> {
        let path = path?;
        match AudioSource::open(path) {
            Ok(audio) => Some(Self {
                source,
                audio,
                label,
            }),
            Err(e) => {
                log::warn!("meeting finalize: failed to read {:?}: {}", path, e);
                None
            }
        }
    }
}

/// Turns the API's raw speaker tags into display labels, numbered by the order
/// each speaker first talks.
///
/// Deliberately ignores whatever number the tag itself carries. The documented
/// shape is `spk_1`, but the API actually returns `spk:0` — different separator
/// AND zero-based — so any attempt to reuse its numbering is a guess that has
/// already been wrong once. Order of first appearance is derived from data we
/// can see, always starts at 1, and survives whatever the tag looks like next.
#[derive(Default)]
pub(super) struct SpeakerLabels {
    seen: Vec<String>,
}

impl SpeakerLabels {
    pub fn label(&mut self, raw: &str) -> String {
        let index = match self.seen.iter().position(|s| s == raw) {
            Some(i) => i,
            None => {
                self.seen.push(raw.to_string());
                self.seen.len() - 1
            }
        };
        format!("Speaker {}", index + 1)
    }
}

/// BCP-47 hints for the Gemini paths: the pinned meeting language, or empty
/// (auto-detect) when the user left it on automatic.
pub(super) fn meeting_language_hints(settings: &crate::settings::AppSettings) -> Vec<String> {
    let language = settings.meeting_language.trim();
    if language.is_empty() || language == "auto" {
        Vec::new()
    } else {
        vec![language.to_string()]
    }
}

impl MeetingManager {
    /// HYBRID TRANSCRIPTION. On stop, re-transcribe the FULL mic and system
    /// audio buffered during the session, producing a higher-quality labeled
    /// transcript that REPLACES the live rough preview.
    ///
    /// Each source is split into TIME-ORDERED ~25-30 s windows that close
    /// preferentially at VAD silence boundaries (so words aren't cut), each
    /// keeping its real start offset; mic + system windows are merged and
    /// sorted by timestamp.
    ///
    /// Emits `"meeting-finalizing"` (true) before and (false) after. If no
    /// buffers were captured (e.g. capture failed early), leaves the live
    /// transcript untouched.
    pub(super) fn finalize_session(&self, session: &Session) {
        // Gemini batch transcription, when configured, replaces the local window
        // pass entirely — it is the only path that can attribute speech to
        // individual participants.
        let gemini = self.gemini_finalize_config();

        // A Gemini Live stream already produced a transcript, so the question is
        // only what may overwrite it. The batch pass may: it re-reads the same
        // audio with a stronger model and adds speaker attribution, which is a
        // strict upgrade. The local Whisper pass may not — that would swap a
        // cloud transcript for a weaker local one the user did not ask for. And
        // a TRANSLATED transcript is final either way: any re-transcription
        // loses the translation.
        if session.live_gemini_active.load(Ordering::Relaxed)
            && (session.live_translate_active.load(Ordering::Relaxed) || gemini.is_none())
        {
            log::info!("meeting finalize: skipped (gemini live produced the transcript)");
            return;
        }

        let Some(buffers) = session.buffers() else {
            return;
        };

        self.emit_finalizing(true);
        let result = self.transcribe_capture_buffers(
            Some(&buffers.mic),
            Some(&buffers.system),
            gemini.as_ref(),
            &session.usage,
        );
        self.emit_finalizing(false);

        if !result.had_audio {
            log::warn!("meeting finalize: no captured audio to transcribe");
            return;
        }

        // Only replace the live transcript if the finalize pass produced
        // something; otherwise keep the live preview as-is.
        if result.has_text() {
            log::info!(
                "meeting finalize: produced {} segment(s)",
                result.segments.len()
            );
            if result.errors.count > 0 {
                // Some requests failed but others did not: the transcript has
                // a hole, which the user should hear about.
                let reason = result.errors.first.clone().unwrap_or_default();
                log::warn!(
                    "meeting finalize: {} request(s) failed; keeping the partial transcript ({})",
                    result.errors.count,
                    reason
                );
                self.emit_error(&format!(
                    "Part of the meeting could not be transcribed: {}",
                    reason
                ));
            }
            self.replace_transcript(session, result.segments);
            return;
        }

        // Every request failing is a FAILURE, not a silent meeting. Record it so
        // `persist_session` keeps the recording instead of discarding it, and
        // tell the UI why the transcript is empty rather than failing silently.
        match result.total_failure() {
            Some(reason) => {
                log::error!(
                    "meeting finalize: all {} request(s) failed; first error: {}",
                    result.errors.count,
                    reason
                );
                *session.finalize_error.lock().unwrap() = Some(reason.clone());
                self.emit_error(&reason);
            }
            None => log::warn!(
                "meeting finalize: full re-transcription yielded no text; keeping live transcript"
            ),
        }
    }

    /// Transcribe a session's mic + system capture buffers through Gemini (when
    /// `gemini` is set) or the local final model. Shared by the on-stop
    /// finalize pass and crash recovery, so a recovered Gemini meeting is
    /// re-transcribed the way the user chose rather than by a local model.
    pub(super) fn transcribe_capture_buffers(
        &self,
        mic: Option<&Path>,
        system: Option<&Path>,
        gemini: Option<&crate::gemini_transcribe::BatchTranscribeConfig>,
        usage: &Mutex<crate::ai_usage::MeetingUsage>,
    ) -> PassResult {
        let mut sources: Vec<SourceInput<'static>> = [
            SourceInput::from_file(mic, TranscriptSource::Mic, "you"),
            SourceInput::from_file(system, TranscriptSource::System, "others"),
        ]
        .into_iter()
        .flatten()
        .collect();

        let mut result = match gemini {
            Some(config) => {
                self.transcribe_with_gemini(&mut sources, config, usage, &mut |_, _| true)
            }
            None => self.transcribe_locally(&mut sources, &mut |_, _| true),
        };
        if gemini.is_none() {
            // Cross-channel echo removal: drop "you" segments that merely echo
            // a nearby "others" segment (residual speaker leakage the live duck
            // didn't fully suppress).
            result.segments = super::text::drop_cross_channel_echo(result.segments);
        }
        result.segments.sort_by_key(|s| s.timestamp_ms);
        result
    }

    /// Re-transcribe `sources` through Gemini's batch model, in pieces of at
    /// most [`GEMINI_PIECE_SECS`] (one request takes about an hour at most, so
    /// a long meeting sent whole simply failed).
    ///
    /// The mic and system sources go up as SEPARATE requests, which is what
    /// keeps the "you" / "others" labeling intact — a single mixed upload would
    /// come back as anonymous `spk_N` with no way to tell which one is the
    /// user. Diarization is only asked for on the system stream (the mic is
    /// one person by definition), and only when the whole stream fits in one
    /// request: speaker numbers mean nothing across requests.
    ///
    /// `on_piece(index, total)` reports progress; returning false stops early.
    pub(super) fn transcribe_with_gemini(
        &self,
        sources: &mut [SourceInput<'_>],
        config: &crate::gemini_transcribe::BatchTranscribeConfig,
        usage: &Mutex<crate::ai_usage::MeetingUsage>,
        on_piece: &mut dyn FnMut(usize, usize) -> bool,
    ) -> PassResult {
        let mut result = PassResult::default();
        let total_pieces: usize = sources
            .iter()
            .map(|s| gemini_pieces(s.audio.len()).len())
            .sum();
        let mut done_pieces = 0;

        for input in sources.iter_mut() {
            let len = input.audio.len();
            if len == 0 {
                continue;
            }
            result.had_audio = true;

            let duration_secs = len as u64 / WHISPER_SAMPLE_RATE as u64;
            let mut request = config.clone();
            request.diarize = config.diarize
                && input.source == TranscriptSource::System
                && crate::gemini_transcribe::supports_diarization(duration_secs);
            if config.diarize && input.source == TranscriptSource::System && !request.diarize {
                log::info!(
                    "meeting finalize: {} min of audio exceeds the diarization limit; \
                     transcribing without speaker attribution",
                    duration_secs / 60
                );
            }

            // Numbering restarts per source, which is right: only the system
            // stream is ever diarized.
            let mut speakers = SpeakerLabels::default();
            let pieces = gemini_pieces(len);
            for (index, &(start, end)) in pieces.iter().enumerate() {
                if !on_piece(done_pieces, total_pieces) {
                    return result;
                }
                done_pieces += 1;
                let samples = match input.audio.read(start, end) {
                    Ok(samples) => samples,
                    Err(e) => {
                        log::warn!("meeting finalize: cannot read {} audio: {}", input.label, e);
                        result.errors.record(e.to_string());
                        continue;
                    }
                };
                let name = if pieces.len() > 1 {
                    format!("meeting-{}-{}", input.label, index + 1)
                } else {
                    format!("meeting-{}", input.label)
                };
                match crate::gemini_transcribe::transcribe_samples(
                    &request,
                    &samples,
                    WHISPER_SAMPLE_RATE,
                    &name,
                ) {
                    Ok(response) => {
                        let offset_ms = start as u64 * 1000 / WHISPER_SAMPLE_RATE as u64;
                        absorb_gemini_result(
                            usage,
                            &request.model,
                            response,
                            offset_ms,
                            input.source,
                            &mut speakers,
                            &mut result.segments,
                        );
                    }
                    Err(e) => {
                        log::warn!(
                            "meeting finalize: gemini {} piece {}/{} failed: {}",
                            input.label,
                            index + 1,
                            pieces.len(),
                            e
                        );
                        result.errors.record(e.to_string());
                    }
                }
            }
        }
        result
    }

    /// Re-transcribe `sources` with the local final model in VAD-bounded
    /// windows. Swaps the final model in for the duration and restores the
    /// previous one afterwards.
    ///
    /// `on_window(index, total)` reports progress; returning false stops early.
    pub(super) fn transcribe_locally(
        &self,
        sources: &mut [SourceInput<'_>],
        on_window: &mut dyn FnMut(usize, usize) -> bool,
    ) -> PassResult {
        let mut result = PassResult::default();
        if sources.iter().all(|s| s.audio.is_empty()) {
            return result;
        }

        // Swap in the stronger FINAL model (default "turbo") for the duration of
        // the pass, then restore the user's normal model. We never hold two
        // models resident. If the final model isn't downloaded / fails to load,
        // the currently-loaded model is used.
        let restore_model = self.swap_in_final_model();

        let vad_path = {
            use tauri::Manager;
            self.app_handle.path().resolve(
                "resources/models/silero_vad_v4.onnx",
                tauri::path::BaseDirectory::Resource,
            )
        };

        // Window every source first so progress can count all of them.
        let mut plans: Vec<Vec<(usize, usize)>> = Vec::new();
        for input in sources.iter_mut() {
            if input.audio.is_empty() {
                plans.push(Vec::new());
                continue;
            }
            result.had_audio = true;
            let windows = match &vad_path {
                Ok(p) => windows_for_source(&mut input.audio, p),
                Err(e) => {
                    log::warn!(
                        "meeting finalize: VAD path unresolved ({}); using fixed windows",
                        e
                    );
                    chunk_fixed(input.audio.len())
                }
            };
            plans.push(windows);
        }
        let total: usize = plans.iter().map(Vec::len).sum();
        let mut done = 0;
        let mut stopped = false;

        for (input, windows) in sources.iter_mut().zip(plans) {
            if stopped {
                break;
            }
            let mut offset_progress = |index: usize, _: usize| on_window(done + index, total);
            stopped = !self.transcribe_windows(
                &mut input.audio,
                &windows,
                input.source,
                &mut result.segments,
                &mut result.errors,
                &mut offset_progress,
            );
            done += windows.len();
        }

        self.restore_model(restore_model);
        result
    }

    /// Build the batch-transcription config, or `None` when this meeting is not
    /// running on Gemini.
    ///
    /// Keyed off the MEETING MODEL rather than a separate toggle. Picking
    /// Gemini in Settings → Models is the whole decision, and it routes
    /// finalize through this per-source path — which preserves the you/others
    /// labels and can diarize — instead of the generic single-blob cloud path
    /// that `transcribe_with_opts` would otherwise take.
    pub(super) fn gemini_finalize_config(
        &self,
    ) -> Option<crate::gemini_transcribe::BatchTranscribeConfig> {
        use crate::gemini_transcribe::{
            BatchTranscribeConfig, TranscriptionMode, DEFAULT_BATCH_TRANSCRIBE_MODEL,
        };
        use tauri::Manager;

        let settings = crate::settings::get_settings(&self.app_handle);
        let meeting_model = self
            .app_handle
            .try_state::<std::sync::Arc<crate::managers::model::ModelManager>>()
            .and_then(|mm| mm.get_model_info(settings.meeting_model_id()))?;
        if !matches!(
            meeting_model.engine_type,
            crate::managers::model::EngineType::Gemini
        ) {
            return None;
        }

        let api_key = settings.gemini_api_key.trim().to_string();
        if api_key.is_empty() {
            log::warn!("meeting: the Gemini model is selected but no Gemini API key is set");
            return None;
        }
        // The catalogue entry carries the bare model id in `filename`, same
        // slug-in-filename convention every cloud entry uses.
        let model = {
            let m = meeting_model.filename.trim();
            if m.is_empty() {
                DEFAULT_BATCH_TRANSCRIBE_MODEL.to_string()
            } else {
                m.to_string()
            }
        };
        Some(BatchTranscribeConfig {
            api_key,
            model,
            language_codes: meeting_language_hints(&settings),
            // One vocabulary for the whole app: the same terms that correct
            // dictation also prime the meeting model.
            custom_vocabulary: settings.custom_words.clone(),
            mode: if settings.meeting_gemini_smart {
                TranscriptionMode::Smart
            } else {
                TranscriptionMode::Verbatim
            },
            diarize: settings.meeting_gemini_diarize,
        })
    }

    /// Whether the meeting model is a cloud model. Cloud models are unsuitable
    /// for the per-segment LIVE pass — each VAD segment would be a separate
    /// network request — so meetings skip the live pass for them and let the
    /// finalize pass do the work in bounded windows.
    pub(super) fn meeting_model_is_cloud(&self) -> bool {
        use tauri::Manager;
        let settings = crate::settings::get_settings(&self.app_handle);
        self.app_handle
            .try_state::<std::sync::Arc<crate::managers::model::ModelManager>>()
            .and_then(|mm| mm.get_model_info(settings.meeting_model_id()))
            .is_some_and(|m| m.engine_type.is_cloud())
    }

    /// Load the configured `meeting_final_model` (default "turbo") for the
    /// finalize pass, returning the model id that should be restored afterwards
    /// (the model that was loaded before, or the configured meeting model).
    /// Returns `None` if no swap happened (already on the final model, or the
    /// final model couldn't be loaded — in which case the loaded model is kept).
    fn swap_in_final_model(&self) -> Option<String> {
        // Cloud selection: don't swap in the local final model. The finalize
        // transcription routes to the cloud model (keyed off the selected
        // model), so loading the local final model here would just load a model
        // the cloud path ignores.
        if self.meeting_model_is_cloud() {
            return None;
        }

        let settings = crate::settings::get_settings(&self.app_handle);
        let final_model = settings.meeting_final_model.trim().to_string();
        if final_model.is_empty() {
            return None;
        }

        let current = self
            .transcription_manager
            .get_current_model()
            .unwrap_or_else(|| settings.meeting_model_id().to_string());

        if current == final_model {
            return None;
        }

        match self.transcription_manager.load_model(&final_model) {
            Ok(()) => {
                log::info!(
                    "meeting finalize: swapped model {} -> {} for final pass",
                    current,
                    final_model
                );
                Some(current)
            }
            Err(e) => {
                log::warn!(
                    "meeting finalize: could not load final model '{}' ({}); using loaded model",
                    final_model,
                    e
                );
                None
            }
        }
    }

    /// Restore the model recorded by `swap_in_final_model`. No-op when `None`.
    fn restore_model(&self, restore: Option<String>) {
        if let Some(model_id) = restore {
            if let Err(e) = self.transcription_manager.load_model(&model_id) {
                log::warn!(
                    "meeting finalize: failed to restore model '{}': {}",
                    model_id,
                    e
                );
            }
        }
    }

    /// Transcribe each `[start, end)` window of `audio` into one timestamped,
    /// labeled segment via the meeting FINALIZE path
    /// (`transcribe_meeting_finalize`: forced meeting language + style prompt +
    /// anti-hallucination knobs + `no_context` so independent windows don't
    /// share decoder state). Each window is read from `audio` on demand.
    /// Returns false when `on_window` asked to stop.
    fn transcribe_windows(
        &self,
        audio: &mut AudioSource<'_>,
        windows: &[(usize, usize)],
        source: TranscriptSource,
        out: &mut Vec<TranscriptSegment>,
        errors: &mut FinalizeErrors,
        on_window: &mut dyn FnMut(usize, usize) -> bool,
    ) -> bool {
        // Tail text of the previous window for this source, used to de-dup the
        // overlapping region.
        let mut prev_tail: Option<String> = None;
        for (index, &(start, end)) in windows.iter().enumerate() {
            if !on_window(index, windows.len()) {
                return false;
            }
            let slice = match audio.read(start, end) {
                Ok(slice) => slice,
                Err(e) => {
                    log::warn!("meeting finalize: cannot read window: {}", e);
                    errors.record(e.to_string());
                    continue;
                }
            };
            match self
                .transcription_manager
                .transcribe_meeting_finalize(slice)
            {
                Ok(text) => {
                    let text = text.trim().to_string();
                    if !text.is_empty() {
                        let deduped = match &prev_tail {
                            Some(prev) => super::text::dedup_overlap(prev, &text),
                            None => text.clone(),
                        };
                        prev_tail = Some(text);
                        let deduped = deduped.trim().to_string();
                        if !deduped.is_empty() {
                            let timestamp_ms =
                                (start as u64).saturating_mul(1000) / WHISPER_SAMPLE_RATE as u64;
                            out.push(TranscriptSegment {
                                text: deduped,
                                timestamp_ms,
                                source,
                                translation: None,
                                speaker: None,
                            });
                        }
                    }
                }
                Err(e) => {
                    log::warn!(
                        "meeting finalize: transcription failed for {:?} window [{}..{}]: {}",
                        source,
                        start,
                        end,
                        e
                    );
                    errors.record(e.to_string());
                }
            }
        }
        true
    }
}

/// Record what a Gemini batch request cost and turn its segments into
/// transcript segments, shifted by `offset_ms` (non-zero for later pieces of a
/// long recording).
fn absorb_gemini_result(
    usage: &Mutex<crate::ai_usage::MeetingUsage>,
    model: &str,
    result: crate::gemini_transcribe::BatchTranscribeResult,
    offset_ms: u64,
    source: TranscriptSource,
    speakers: &mut SpeakerLabels,
    out: &mut Vec<TranscriptSegment>,
) {
    super::session::record_usage(usage, model, result.usage.0, result.usage.1, 0);
    for segment in result.segments {
        let text = segment.text.trim();
        if text.is_empty() {
            continue;
        }
        out.push(TranscriptSegment {
            text: text.to_string(),
            timestamp_ms: offset_ms + segment.start_ms,
            source,
            translation: None,
            speaker: segment.speaker.as_deref().map(|raw| speakers.label(raw)),
        });
    }
}

// ---- Windowing --------------------------------------------------------------
//
// Target window length the chunker aims for before it starts looking for a
// silence boundary to close on (~25 s).
pub(super) const FINALIZE_TARGET_SAMPLES: usize = WHISPER_SAMPLE_RATE as usize * 25;
// Hard cap: force-close a window here even mid-speech (~30 s, whisper's native
// window) so a continuous talker can't produce an unbounded window.
pub(super) const FINALIZE_MAX_SAMPLES: usize = WHISPER_SAMPLE_RATE as usize * 30;
// 30 ms frame for the VAD silence scan.
pub(super) const FINALIZE_FRAME_SAMPLES: usize = (WHISPER_SAMPLE_RATE as usize * 30) / 1000;
// Consecutive silent frames that mark a "safe" split point once past target
// (~150 ms). Long enough to be an inter-word/sentence gap, not a glottal stop.
const FINALIZE_SILENCE_SPLIT_FRAMES: usize = 5;
// Overlap between consecutive finalize windows (~4 s). The next window starts
// `FINALIZE_OVERLAP_SAMPLES` before the previous window's end so words cut at a
// `FINALIZE_MAX_SAMPLES` force-split aren't lost. The overlapping text is
// de-duplicated at merge time (see `dedup_overlap`). Kept comfortably under
// FINALIZE_TARGET_SAMPLES so windows still advance.
pub(super) const FINALIZE_OVERLAP_SAMPLES: usize = WHISPER_SAMPLE_RATE as usize * 4;

/// Longest piece of audio sent to Gemini in one request. The API takes an
/// hour of plain transcription per request; staying well under it leaves room
/// for its own accounting.
pub(super) const GEMINI_PIECE_SECS: usize = 50 * 60;

/// Split `len` samples into consecutive pieces of at most
/// `GEMINI_PIECE_SECS`. Pieces are cut at fixed points: at one cut per
/// 50 minutes, a clipped word is cheaper than another windowing heuristic.
pub(super) fn gemini_pieces(len: usize) -> Vec<(usize, usize)> {
    let piece = GEMINI_PIECE_SECS * WHISPER_SAMPLE_RATE as usize;
    (0..len)
        .step_by(piece)
        .map(|start| (start, (start + piece).min(len)))
        .collect()
}

/// Windows for one source: VAD-guided when the VAD loads, fixed otherwise.
/// Scans the audio in chunks, so a file source is never loaded whole.
fn windows_for_source(audio: &mut AudioSource<'_>, vad_path: &Path) -> Vec<(usize, usize)> {
    use crate::audio_toolkit::vad::VoiceActivityDetector;
    use crate::audio_toolkit::SileroVad;

    let len = audio.len();
    let mut vad = match SileroVad::new(vad_path, 0.3) {
        Ok(v) => v,
        Err(e) => {
            log::warn!(
                "meeting finalize: SileroVad init failed ({}); fixed windows",
                e
            );
            return chunk_fixed(len);
        }
    };
    let mut flags: Vec<bool> = Vec::with_capacity(len / FINALIZE_FRAME_SAMPLES + 1);
    // A whole number of frames per chunk keeps frame boundaries where a
    // single in-memory scan would put them.
    let chunk = FINALIZE_FRAME_SAMPLES * 2_000;
    let scanned = audio.for_each_chunk(chunk, |samples| {
        for frame in samples.chunks(FINALIZE_FRAME_SAMPLES) {
            // is_voice on the raw VAD is a per-frame decision (no hangover).
            flags.push(vad.is_voice(frame).unwrap_or(false));
        }
    });
    if let Err(e) = scanned {
        log::warn!("meeting finalize: VAD scan failed ({}); fixed windows", e);
        return chunk_fixed(len);
    }
    windows_from_voice(len, &flags)
}

/// Split `n` samples into time-ordered `[start, end)` windows from per-frame
/// voice decisions (`FINALIZE_FRAME_SAMPLES` per flag). A window closes once it
/// is past `FINALIZE_TARGET_SAMPLES` AND a run of
/// `FINALIZE_SILENCE_SPLIT_FRAMES` silent frames is seen (a natural pause), or
/// unconditionally at `FINALIZE_MAX_SAMPLES`. Windows with no speech at all
/// are dropped. Pure, so the windowing is testable without a VAD model.
pub(super) fn windows_from_voice(n: usize, flags: &[bool]) -> Vec<(usize, usize)> {
    let mut windows: Vec<(usize, usize)> = Vec::new();
    let mut win_start = 0usize;
    let mut silence_run = 0usize;
    let mut win_has_speech = false;

    for (i, &is_voice) in flags.iter().enumerate() {
        let pos = i * FINALIZE_FRAME_SAMPLES;
        if pos >= n {
            break;
        }
        let end = (pos + FINALIZE_FRAME_SAMPLES).min(n);
        if is_voice {
            win_has_speech = true;
            silence_run = 0;
        } else {
            silence_run += 1;
        }

        let win_len = end - win_start;
        let past_target = win_len >= FINALIZE_TARGET_SAMPLES;
        let safe_split = past_target && silence_run >= FINALIZE_SILENCE_SPLIT_FRAMES;
        let force_split = win_len >= FINALIZE_MAX_SAMPLES;

        if safe_split || force_split {
            if win_has_speech {
                windows.push((win_start, end));
            }
            // Start the next window before this one's end so a word cut at a
            // force-split is recovered. Guarded so win_start only advances.
            let next_start = end.saturating_sub(FINALIZE_OVERLAP_SAMPLES);
            win_start = next_start.max(win_start + 1).min(end);
            win_has_speech = false;
            silence_run = 0;
        }
    }

    if win_start < n && win_has_speech {
        windows.push((win_start, n));
    }
    windows
}

/// Fallback chunker: fixed `FINALIZE_MAX_SAMPLES`-sized windows with
/// `FINALIZE_OVERLAP_SAMPLES` overlap, no VAD.
pub(super) fn chunk_fixed(n: usize) -> Vec<(usize, usize)> {
    let mut windows = Vec::new();
    let mut start = 0usize;
    while start < n {
        let end = (start + FINALIZE_MAX_SAMPLES).min(n);
        windows.push((start, end));
        if end >= n {
            break;
        }
        start = end
            .saturating_sub(FINALIZE_OVERLAP_SAMPLES)
            .max(start + 1)
            .min(end);
    }
    windows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn long_audio_goes_to_gemini_in_pieces_that_cover_every_sample() {
        let piece = GEMINI_PIECE_SECS * 16_000;
        assert_eq!(gemini_pieces(10), vec![(0, 10)]);
        assert_eq!(
            gemini_pieces(piece * 2 + 5),
            vec![(0, piece), (piece, piece * 2), (piece * 2, piece * 2 + 5)]
        );
        assert!(gemini_pieces(0).is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn chunk_fixed_splits_into_overlapping_max_windows_covering_all_samples() {
        use super::{FINALIZE_MAX_SAMPLES, FINALIZE_OVERLAP_SAMPLES};
        // ~2.5 windows worth of audio.
        let n = FINALIZE_MAX_SAMPLES * 2 + FINALIZE_MAX_SAMPLES / 2;

        let windows = super::chunk_fixed(n);
        // First window starts at 0; last window ends at n; fully covering.
        assert_eq!(windows[0].0, 0);
        assert_eq!(windows[0].1, FINALIZE_MAX_SAMPLES);
        assert_eq!(windows.last().unwrap().1, n);
        for w in &windows {
            assert!(w.1 > w.0);
            assert!(w.1 - w.0 <= FINALIZE_MAX_SAMPLES);
        }
        // Consecutive windows overlap by FINALIZE_OVERLAP_SAMPLES (except the
        // final partial window which is clamped to n).
        for pair in windows.windows(2) {
            let (prev, next) = (pair[0], pair[1]);
            // next starts before prev ends => overlap.
            assert!(next.0 < prev.1, "windows should overlap: {prev:?} {next:?}");
            if next.1 - next.0 == FINALIZE_MAX_SAMPLES {
                assert_eq!(prev.1 - next.0, FINALIZE_OVERLAP_SAMPLES);
            }
        }
    }

    /// Flags for `secs` seconds of frames, voiced except where `silent` says.
    fn flags(secs: usize, silent: impl Fn(usize) -> bool) -> (usize, Vec<bool>) {
        let n = secs * 16_000;
        let frames = n.div_ceil(FINALIZE_FRAME_SAMPLES);
        (n, (0..frames).map(|i| !silent(i)).collect())
    }

    #[test]
    fn continuous_speech_is_force_split_with_overlap() {
        let (n, voice) = flags(70, |_| false);
        let windows = windows_from_voice(n, &voice);
        assert!(windows.len() >= 3);
        assert_eq!(windows[0].0, 0);
        assert_eq!(windows.last().unwrap().1, n);
        for w in &windows {
            // Frames stay on the 30 ms grid while an overlapped window start
            // need not, so a forced window may run up to one frame over.
            assert!(
                w.1 - w.0 <= FINALIZE_MAX_SAMPLES + FINALIZE_FRAME_SAMPLES,
                "{windows:?}"
            );
        }
        for pair in windows.windows(2) {
            assert_eq!(pair[0].1 - pair[1].0, FINALIZE_OVERLAP_SAMPLES);
        }
    }

    #[test]
    fn a_pause_past_the_target_closes_the_window_early() {
        // Silence from 26.0 s to 27.0 s: past the 25 s target, so the window
        // closes there instead of running to the 30 s cap.
        let frame_at = |secs: f64| (secs * 16_000.0) as usize / FINALIZE_FRAME_SAMPLES;
        let (n, voice) = flags(40, |i| i >= frame_at(26.0) && i < frame_at(27.0));
        let windows = windows_from_voice(n, &voice);
        let first_end = windows[0].1;
        assert!(first_end > FINALIZE_TARGET_SAMPLES && first_end < FINALIZE_MAX_SAMPLES);
    }

    #[test]
    fn silence_produces_no_windows() {
        let (n, voice) = flags(90, |_| true);
        assert!(windows_from_voice(n, &voice).is_empty());
        assert!(windows_from_voice(0, &[]).is_empty());
    }

    #[test]
    fn total_failure_only_when_every_request_failed() {
        let mut result = PassResult::default();
        assert_eq!(result.total_failure(), None, "silence is not a failure");
        result.errors.record("402".into());
        assert_eq!(result.total_failure().as_deref(), Some("402"));
        result.segments.push(TranscriptSegment {
            text: "partial".into(),
            timestamp_ms: 0,
            source: TranscriptSource::Mic,
            translation: None,
            speaker: None,
        });
        assert_eq!(result.total_failure(), None, "a partial transcript is kept");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn speakers_are_numbered_by_when_they_first_talk() {
        use super::SpeakerLabels;
        let mut labels = SpeakerLabels::default();
        // The API's own numbering is ignored: these arrive zero-based and with
        // a colon, and the first one to speak must still read "Speaker 1".
        assert_eq!(labels.label("spk:0"), "Speaker 1");
        assert_eq!(labels.label("spk:1"), "Speaker 2");
        // A speaker returning later keeps the label they were given.
        assert_eq!(labels.label("spk:0"), "Speaker 1");
        assert_eq!(labels.label("spk:5"), "Speaker 3");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn speaker_numbering_survives_an_unfamiliar_tag_format() {
        use super::SpeakerLabels;
        let mut labels = SpeakerLabels::default();
        assert_eq!(labels.label("spk_1"), "Speaker 1");
        assert_eq!(labels.label("SPEAKER_B"), "Speaker 2");
        assert_eq!(labels.label("spk_1"), "Speaker 1");
    }
}
