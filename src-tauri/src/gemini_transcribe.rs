//! Batch speech-to-text via the Gemini Interactions API (`gemini-3.5-transcribe`).
//!
//! This is the *finalize-pass* counterpart to [`crate::gemini_live`]: instead of
//! streaming a live conversation, it takes the complete audio a meeting captured
//! and transcribes it in one request. That buys three things the local Whisper
//! path cannot give us:
//!
//! * **Speaker diarization** — words come back tagged `spk_1`, `spk_2`, … so a
//!   call with several remote participants stops being one undifferentiated
//!   "others" block.
//! * **Word-level timestamps**, which is what lets us rebuild real per-speaker
//!   segments with honest start offsets instead of window boundaries.
//! * **Smart mode**, which strips disfluencies ("um", "uh", false starts) and
//!   formats the text — though see the constraint below: it cannot be combined
//!   with the first two.
//!
//! Three API constraints shape the code:
//!
//! 1. Audio is passed **by URI**, not inline, so every request is really two:
//!    a resumable upload to the Files API, then the transcription itself.
//! 2. With diarization or timestamps enabled the model accepts at most
//!    [`DIARIZATION_MAX_SECS`] of audio (an hour otherwise). Callers must check
//!    duration and degrade rather than let the request fail — see
//!    [`supports_diarization`].
//! 3. Diarization and word timestamps are **verbatim-only**. Asking for either
//!    alongside smart mode is a 400, so [`BatchTranscribeConfig::request_body`]
//!    resolves the conflict in favour of speaker attribution.
//!
//! Everything here is blocking (`reqwest::blocking`) because the finalize pass
//! is a blocking pipeline. [`transcribe_samples`] therefore does its work on a
//! dedicated thread, since a blocking client panics if built inside a tokio
//! runtime.

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::time::Duration;

/// Default batch transcription model. Overridable from settings so a newer
/// preview can be selected without a rebuild.
pub const DEFAULT_BATCH_TRANSCRIBE_MODEL: &str = "gemini-3.5-transcribe";

/// Longest audio the API accepts once diarization or word timestamps are on.
/// (Plain transcription allows an hour.) Past this the request is rejected
/// outright, so callers downgrade instead.
pub const DIARIZATION_MAX_SECS: u64 = 30 * 60;

const FILES_UPLOAD_URL: &str = "https://generativelanguage.googleapis.com/upload/v1beta/files";
const INTERACTIONS_URL: &str = "https://generativelanguage.googleapis.com/v1beta/interactions";

/// Generous: a 30-minute meeting is a lot of audio to upload and then process.
const REQUEST_TIMEOUT_SECS: u64 = 15 * 60;

/// The API caps custom vocabulary at 1,000 terms; the docs note accuracy is best
/// around 100, so we trim rather than let a pasted wall of text degrade results.
const MAX_VOCABULARY_TERMS: usize = 100;

/// Whether `duration_secs` of audio still fits the diarization/timestamp limit.
pub fn supports_diarization(duration_secs: u64) -> bool {
    duration_secs <= DIARIZATION_MAX_SECS
}

/// A completed batch transcription: what was said, and what it cost.
pub struct BatchTranscribeResult {
    pub segments: Vec<DiarizedSegment>,
    /// `(input_tokens, output_tokens)` the API reported, or zeros when it
    /// reported nothing. Usage is strictly an extra on top of the transcript
    /// and must never be able to fail the call that produced it.
    pub usage: (u64, u64),
}

/// One contiguous run of speech attributed to a single speaker.
#[derive(Clone, Debug, PartialEq)]
pub struct DiarizedSegment {
    /// The words of this run, joined back into a sentence.
    pub text: String,
    /// Raw speaker label from the API (`"spk_1"`, …), or `None` when
    /// diarization was off or the model did not attribute this run.
    pub speaker: Option<String>,
    /// Start offset within the submitted audio.
    pub start_ms: u64,
}

/// How the model should render what it hears.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TranscriptionMode {
    /// Literal, including disfluencies. Best when the transcript is evidence.
    Verbatim,
    /// Cleans filler words and false starts and applies formatting. Best for
    /// meeting notes, which is our default.
    Smart,
}

impl TranscriptionMode {
    fn as_str(self) -> &'static str {
        match self {
            TranscriptionMode::Verbatim => "verbatim",
            TranscriptionMode::Smart => "smart",
        }
    }
}

/// Everything needed for one batch transcription request.
#[derive(Clone, Debug)]
pub struct BatchTranscribeConfig {
    /// Gemini API key.
    pub api_key: String,
    /// Bare model id, e.g. `"gemini-3.5-transcribe"`.
    pub model: String,
    /// BCP-47 hints. Empty means "detect automatically", which is what a
    /// multilingual meeting wants.
    pub language_codes: Vec<String>,
    /// Domain terms, names, product names the model would otherwise mangle.
    pub custom_vocabulary: Vec<String>,
    /// Verbatim vs. cleaned-up output.
    pub mode: TranscriptionMode,
    /// Attribute speech to distinct speakers. Requires audio within
    /// [`DIARIZATION_MAX_SECS`]; the caller is responsible for that check.
    pub diarize: bool,
}

impl BatchTranscribeConfig {
    /// The request body for `POST /v1beta/interactions`.
    ///
    /// Word-level timestamps are requested whenever diarization is on, and only
    /// then: they carry the same length penalty, the docs warn they can cost a
    /// little accuracy, and without diarization we have nothing to place them
    /// against — the whole transcript is one speaker.
    ///
    /// The API allows `diarization_mode` and `timestamp_granularities` ONLY
    /// alongside `type: "verbatim"`; combining either with `"smart"` is
    /// rejected outright (`Unknown parameter 'diarization_mode'`). Speaker
    /// attribution is the entire reason to use this model over the local one,
    /// so it wins and the request downgrades to verbatim — losing the
    /// disfluency cleanup, which the summary step largely redoes anyway.
    fn request_body(&self, file_uri: &str, mime_type: &str) -> Value {
        let mode_type = if self.diarize {
            TranscriptionMode::Verbatim
        } else {
            self.mode
        };
        let mut mode = json!({ "type": mode_type.as_str() });
        if self.diarize {
            mode["diarization_mode"] = json!("speaker");
            mode["timestamp_granularities"] = json!(["word"]);
        }

        let vocabulary: Vec<&String> = self
            .custom_vocabulary
            .iter()
            .take(MAX_VOCABULARY_TERMS)
            .collect();

        json!({
            "model": self.model,
            "input": [{
                "type": "audio",
                "uri": file_uri,
                "mime_type": mime_type,
            }],
            "generation_config": {
                "transcription_config": {
                    "language_codes": self.language_codes,
                    "custom_vocabulary": vocabulary,
                    "mode": mode,
                }
            }
        })
    }
}

/// Transcribe 16 kHz mono `samples`, returning speaker-attributed segments.
///
/// Runs the whole upload-then-transcribe exchange on a dedicated OS thread so
/// `reqwest::blocking` never sees an ambient tokio runtime. The thread is
/// scoped, so the samples are borrowed rather than copied — a 50-minute piece
/// is ~190 MB of f32 and a second copy of it bought nothing.
pub fn transcribe_samples(
    config: &BatchTranscribeConfig,
    samples: &[f32],
    sample_rate: u32,
    display_name: &str,
) -> Result<BatchTranscribeResult> {
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                // MP3 uploads are ~5x smaller than 16-bit WAV: an hour of
                // meeting is ~29 MB instead of ~115 MB, which dominated the
                // wait on slow uplinks.
                let audio = crate::audio_toolkit::mp3::encode_mp3(
                    samples,
                    sample_rate,
                    crate::audio_toolkit::mp3::UPLOAD_BITRATE,
                )
                .map_err(|e| anyhow!(e))?;
                run_exchange(config, audio, display_name)
            })
            .join()
            .map_err(|_| anyhow!("Gemini transcription worker thread panicked"))?
    })
}

/// Attempts per HTTP request before giving up. Overload responses (429/503)
/// are routine on preview models and usually clear within seconds.
const MAX_ATTEMPTS: u32 = 4;
/// First retry delay; doubles per attempt.
const RETRY_BASE_DELAY: Duration = Duration::from_secs(2);
/// Never wait longer than this between attempts, whatever `Retry-After` says.
const RETRY_MAX_DELAY: Duration = Duration::from_secs(30);

/// HTTP statuses worth retrying: the request was fine, the service was not.
fn is_retryable_status(status: u16) -> bool {
    matches!(status, 408 | 429 | 500 | 502 | 503 | 504)
}

/// Delay before retry number `attempt` (1-based): the server's `Retry-After`
/// when it sent one, else exponential backoff — capped either way.
fn backoff_delay(attempt: u32, retry_after: Option<Duration>) -> Duration {
    let exponential = RETRY_BASE_DELAY.saturating_mul(1u32 << attempt.saturating_sub(1).min(8));
    retry_after.unwrap_or(exponential).min(RETRY_MAX_DELAY)
}

/// A failed attempt, and whether trying again could help.
struct AttemptError {
    error: anyhow::Error,
    retryable: bool,
    retry_after: Option<Duration>,
}

impl AttemptError {
    fn fatal(error: anyhow::Error) -> Self {
        Self {
            error,
            retryable: false,
            retry_after: None,
        }
    }

    /// A transport failure (connect/timeout/reset) is worth another try.
    fn transport(context: &str, e: reqwest::Error) -> Self {
        Self {
            retryable: e.is_timeout() || e.is_connect() || e.is_request(),
            error: anyhow!("{}: {}", context, e),
            retry_after: None,
        }
    }
}

/// Classify a non-success response: its body carries the message, its
/// `Retry-After` header the delay.
fn status_error(context: &str, response: reqwest::blocking::Response) -> AttemptError {
    let status = response.status();
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(Duration::from_secs);
    let body = response.text().unwrap_or_default();
    AttemptError {
        error: anyhow!("{} ({}): {}", context, status, truncate(&body, 400)),
        retryable: is_retryable_status(status.as_u16()),
        retry_after,
    }
}

/// Run `attempt` until it succeeds, fails for good, or runs out of attempts.
fn with_retries<T>(
    what: &str,
    mut attempt: impl FnMut() -> std::result::Result<T, AttemptError>,
) -> Result<T> {
    let mut tries = 0;
    loop {
        tries += 1;
        match attempt() {
            Ok(value) => return Ok(value),
            Err(e) if e.retryable && tries < MAX_ATTEMPTS => {
                let delay = backoff_delay(tries, e.retry_after);
                log::warn!(
                    "gemini-transcribe: {} attempt {}/{} failed ({}); retrying in {:?}",
                    what,
                    tries,
                    MAX_ATTEMPTS,
                    e.error,
                    delay
                );
                std::thread::sleep(delay);
            }
            Err(e) => return Err(e.error),
        }
    }
}

/// Upload the audio, ask for a transcript, parse the result.
fn run_exchange(
    config: &BatchTranscribeConfig,
    audio: Vec<u8>,
    display_name: &str,
) -> Result<BatchTranscribeResult> {
    const MIME: &str = "audio/mpeg";

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .build()
        .map_err(|e| anyhow!("Failed to build HTTP client: {}", e))?;

    let byte_len = audio.len();
    let file_uri = upload_file(&client, &config.api_key, audio, MIME, display_name)?;
    log::info!(
        "gemini-transcribe: uploaded {} ({} bytes) as {}",
        display_name,
        byte_len,
        file_uri
    );

    let request_body = config.request_body(&file_uri, MIME);
    let body = with_retries("transcription", || {
        let response = client
            .post(INTERACTIONS_URL)
            .header("x-goog-api-key", &config.api_key)
            .json(&request_body)
            .send()
            .map_err(|e| AttemptError::transport("transcription request failed", e))?;
        if !response.status().is_success() {
            return Err(status_error("Gemini transcription failed", response));
        }
        response
            .text()
            .map_err(|e| AttemptError::transport("failed to read transcription response", e))
    })?;

    let value: Value = serde_json::from_str(&body)
        .map_err(|e| anyhow!("malformed transcription response: {}", e))?;
    let segments = parse_response(&value);
    if segments.is_empty() {
        // A 2xx with nothing we recognise means the response shape moved, not
        // that the user was silent. Without this the failure is a silent empty
        // transcript with no way to tell the two apart.
        log::warn!(
            "gemini-transcribe: no transcript found in a successful response; body was {}",
            truncate(&body, 1200)
        );
    }
    Ok(BatchTranscribeResult {
        segments,
        // A batch call is one request, so there is no cumulative-vs-incremental
        // ambiguity to resolve here: whatever it reports is the whole cost.
        usage: crate::ai_usage::usage_of(&value).unwrap_or((0, 0)),
    })
}

/// Files API resumable upload. Returns the `uri` the Interactions API expects.
///
/// The protocol is two requests: a `start` that reserves an upload URL (returned
/// in the `x-goog-upload-url` *header*, not the body), then the bytes plus a
/// `finalize` command. A failed attempt restarts the whole upload: a fresh
/// upload URL is simpler to reason about than resuming a half-sent one.
fn upload_file(
    client: &reqwest::blocking::Client,
    api_key: &str,
    bytes: Vec<u8>,
    mime_type: &str,
    display_name: &str,
) -> Result<String> {
    let body = with_retries("upload", || {
        let start = client
            .post(FILES_UPLOAD_URL)
            .header("x-goog-api-key", api_key)
            .header("X-Goog-Upload-Protocol", "resumable")
            .header("X-Goog-Upload-Command", "start")
            .header("X-Goog-Upload-Header-Content-Length", bytes.len())
            .header("X-Goog-Upload-Header-Content-Type", mime_type)
            .json(&json!({ "file": { "display_name": display_name } }))
            .send()
            .map_err(|e| AttemptError::transport("upload start failed", e))?;
        if !start.status().is_success() {
            return Err(status_error("Gemini file upload could not start", start));
        }

        let upload_url = start
            .headers()
            .get("x-goog-upload-url")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| {
                AttemptError::fatal(anyhow!("Gemini file upload returned no upload URL"))
            })?
            .to_string();

        let finalize = client
            .post(&upload_url)
            .header("Content-Length", bytes.len())
            .header("X-Goog-Upload-Offset", "0")
            .header("X-Goog-Upload-Command", "upload, finalize")
            .body(bytes.clone())
            .send()
            .map_err(|e| AttemptError::transport("upload failed", e))?;
        if !finalize.status().is_success() {
            return Err(status_error("Gemini file upload failed", finalize));
        }
        finalize
            .text()
            .map_err(|e| AttemptError::transport("failed to read upload response", e))
    })?;

    let value: Value =
        serde_json::from_str(&body).map_err(|e| anyhow!("malformed upload response: {}", e))?;
    file_uri_of(&value).ok_or_else(|| anyhow!("Gemini upload response carried no file URI"))
}

/// The URI may sit at `file.uri` or, defensively, at the top level.
fn file_uri_of(value: &Value) -> Option<String> {
    for candidate in [&value["file"]["uri"], &value["uri"]] {
        if let Some(uri) = candidate.as_str() {
            if !uri.is_empty() {
                return Some(uri.to_string());
            }
        }
    }
    None
}

/// Turn an interaction response into speaker-attributed segments.
///
/// Preferred path is the `word_info` annotations, which carry speaker and
/// timing. When they are absent — diarization off, or the model chose not to
/// annotate — we fall back to `output_text` as a single untimed segment, so a
/// successful request never yields an empty transcript just because the shape
/// differed from what we expected.
fn parse_response(value: &Value) -> Vec<DiarizedSegment> {
    let interaction = if value.get("interaction").is_some() {
        &value["interaction"]
    } else {
        value
    };

    let words = collect_word_info(interaction);
    if !words.is_empty() {
        return group_words(&words);
    }

    // No word annotations: diarization was off, or the model chose not to
    // annotate. The transcript then arrives as plain text, and which field
    // carries it depends on the shape the API answered with — so try each
    // known one rather than assuming a single spelling.
    for text in [
        interaction["output_text"].as_str(),
        interaction["text"].as_str(),
    ]
    .into_iter()
    .flatten()
    {
        if !text.trim().is_empty() {
            return vec![DiarizedSegment {
                text: text.trim().to_string(),
                speaker: None,
                start_ms: 0,
            }];
        }
    }

    let joined = collect_plain_text(interaction);
    if joined.trim().is_empty() {
        return Vec::new();
    }
    vec![DiarizedSegment {
        text: joined.trim().to_string(),
        speaker: None,
        start_ms: 0,
    }]
}

/// Join every plain-text content part, in order.
///
/// Covers the `steps[].content[].text` shape (an interaction that answered with
/// ordinary content instead of word annotations) and the `candidates[]` shape
/// the generateContent family uses.
fn collect_plain_text(interaction: &Value) -> String {
    let mut parts: Vec<&str> = Vec::new();

    if let Some(steps) = interaction["steps"].as_array() {
        for step in steps {
            let Some(contents) = step["content"].as_array() else {
                continue;
            };
            for content in contents {
                if let Some(text) = content["text"].as_str() {
                    parts.push(text);
                }
            }
        }
    }

    if let Some(candidates) = interaction["candidates"].as_array() {
        for candidate in candidates {
            let Some(content_parts) = candidate["content"]["parts"].as_array() else {
                continue;
            };
            for part in content_parts {
                if let Some(text) = part["text"].as_str() {
                    parts.push(text);
                }
            }
        }
    }

    parts
        .into_iter()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// A single `word_info` annotation, flattened.
#[derive(Clone, Debug, PartialEq)]
struct WordInfo {
    text: String,
    speaker: Option<String>,
    start_ms: u64,
}

/// Walk `steps[].content[].annotations[]` and pull out every `word_info`.
fn collect_word_info(interaction: &Value) -> Vec<WordInfo> {
    let mut words = Vec::new();
    let Some(steps) = interaction["steps"].as_array() else {
        return words;
    };
    for step in steps {
        let Some(contents) = step["content"].as_array() else {
            continue;
        };
        for content in contents {
            let Some(annotations) = content["annotations"].as_array() else {
                continue;
            };
            for annotation in annotations {
                if annotation["type"].as_str() != Some("word_info") {
                    continue;
                }
                let Some(text) = annotation["text"].as_str() else {
                    continue;
                };
                if text.trim().is_empty() {
                    continue;
                }
                words.push(WordInfo {
                    text: text.to_string(),
                    speaker: annotation["speaker"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .map(str::to_string),
                    start_ms: parse_offset_ms(annotation["start_offset"].as_str()),
                });
            }
        }
    }
    words
}

/// Collapse the word stream into one segment per contiguous speaker run.
///
/// Word-per-segment would be unreadable and would swamp the transcript UI; one
/// segment per speaker turn is the unit a reader actually wants.
fn group_words(words: &[WordInfo]) -> Vec<DiarizedSegment> {
    let mut segments: Vec<DiarizedSegment> = Vec::new();
    for word in words {
        match segments.last_mut() {
            // Same speaker as the previous word: extend the run.
            Some(current) if current.speaker == word.speaker => {
                current.text.push(' ');
                current.text.push_str(word.text.trim());
            }
            _ => segments.push(DiarizedSegment {
                text: word.text.trim().to_string(),
                speaker: word.speaker.clone(),
                start_ms: word.start_ms,
            }),
        }
    }
    // Punctuation arrives attached to words, so naive joining leaves " ," style
    // artifacts only if the model ever emits bare punctuation tokens. Trim once
    // at the end rather than special-casing every token.
    for segment in &mut segments {
        segment.text = segment.text.trim().to_string();
    }
    segments.retain(|s| !s.text.is_empty());
    segments
}

/// Parse the API's `"1.250s"` offset format into milliseconds.
fn parse_offset_ms(offset: Option<&str>) -> u64 {
    let Some(offset) = offset else {
        return 0;
    };
    let seconds: f64 = offset.trim_end_matches('s').parse().unwrap_or(0.0);
    if seconds <= 0.0 {
        0
    } else {
        (seconds * 1000.0).round() as u64
    }
}

/// Keep error messages readable when the API returns a large HTML/JSON error.
///
/// Cuts on a character boundary: slicing bytes panicked whenever a multi-byte
/// character (a localized error page) straddled `max`.
fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// Split a user-entered vocabulary blob (one term per line, or comma-separated)
/// into clean terms.
pub fn parse_vocabulary(raw: &str) -> Vec<String> {
    raw.split(['\n', ','])
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(diarize: bool) -> BatchTranscribeConfig {
        BatchTranscribeConfig {
            api_key: "key".into(),
            model: DEFAULT_BATCH_TRANSCRIBE_MODEL.into(),
            language_codes: vec![],
            custom_vocabulary: vec!["Fisilti".into()],
            mode: TranscriptionMode::Smart,
            diarize,
        }
    }

    #[test]
    fn diarized_request_asks_for_speakers_and_word_timestamps() {
        let body = config(true).request_body("files/abc", "audio/wav");
        let mode = &body["generation_config"]["transcription_config"]["mode"];
        assert_eq!(mode["diarization_mode"], json!("speaker"));
        assert_eq!(mode["timestamp_granularities"], json!(["word"]));
        assert_eq!(body["input"][0]["uri"], json!("files/abc"));
        assert_eq!(
            body["generation_config"]["transcription_config"]["custom_vocabulary"],
            json!(["Fisilti"])
        );
    }

    #[test]
    fn diarization_forces_verbatim_because_smart_mode_rejects_it() {
        // The API answers `smart` + `diarization_mode` with 400 Unknown
        // parameter, so this combination must be impossible to construct.
        let mut cfg = config(true);
        cfg.mode = TranscriptionMode::Smart;
        let body = cfg.request_body("files/abc", "audio/wav");
        assert_eq!(
            body["generation_config"]["transcription_config"]["mode"]["type"],
            json!("verbatim")
        );
    }

    #[test]
    fn smart_mode_survives_when_diarization_is_off() {
        let mut cfg = config(false);
        cfg.mode = TranscriptionMode::Smart;
        let body = cfg.request_body("files/abc", "audio/wav");
        assert_eq!(
            body["generation_config"]["transcription_config"]["mode"]["type"],
            json!("smart")
        );
    }

    #[test]
    fn plain_request_omits_diarization_and_timestamps() {
        let body = config(false).request_body("files/abc", "audio/wav");
        let mode = &body["generation_config"]["transcription_config"]["mode"];
        assert!(mode.get("diarization_mode").is_none());
        assert!(mode.get("timestamp_granularities").is_none());
    }

    #[test]
    fn vocabulary_is_capped_so_a_pasted_wall_of_text_cannot_degrade_accuracy() {
        let mut cfg = config(true);
        cfg.custom_vocabulary = (0..500).map(|i| format!("term{}", i)).collect();
        let body = cfg.request_body("files/abc", "audio/wav");
        let sent = body["generation_config"]["transcription_config"]["custom_vocabulary"]
            .as_array()
            .unwrap()
            .len();
        assert_eq!(sent, MAX_VOCABULARY_TERMS);
    }

    #[test]
    fn word_annotations_group_into_one_segment_per_speaker_turn() {
        let response = json!({
            "output_text": "Hello there. Hi back.",
            "steps": [{
                "content": [{
                    "annotations": [
                        { "type": "word_info", "text": "Hello", "speaker": "spk_1", "start_offset": "0.100s" },
                        { "type": "word_info", "text": "there.", "speaker": "spk_1", "start_offset": "0.450s" },
                        { "type": "word_info", "text": "Hi", "speaker": "spk_2", "start_offset": "1.200s" },
                        { "type": "word_info", "text": "back.", "speaker": "spk_2", "start_offset": "1.500s" }
                    ]
                }]
            }]
        });
        let segments = parse_response(&response);
        assert_eq!(
            segments,
            vec![
                DiarizedSegment {
                    text: "Hello there.".into(),
                    speaker: Some("spk_1".into()),
                    start_ms: 100,
                },
                DiarizedSegment {
                    text: "Hi back.".into(),
                    speaker: Some("spk_2".into()),
                    start_ms: 1200,
                },
            ]
        );
    }

    #[test]
    fn a_speaker_returning_later_starts_a_new_segment() {
        let words = vec![
            WordInfo {
                text: "one".into(),
                speaker: Some("spk_1".into()),
                start_ms: 0,
            },
            WordInfo {
                text: "two".into(),
                speaker: Some("spk_2".into()),
                start_ms: 500,
            },
            WordInfo {
                text: "three".into(),
                speaker: Some("spk_1".into()),
                start_ms: 900,
            },
        ];
        let segments = group_words(&words);
        assert_eq!(segments.len(), 3);
        assert_eq!(segments[2].start_ms, 900);
        assert_eq!(segments[2].speaker.as_deref(), Some("spk_1"));
    }

    #[test]
    fn a_response_carrying_plain_content_parts_still_yields_a_transcript() {
        // What a request with diarization off can answer with: ordinary text
        // content and no word annotations, and no `output_text` either.
        let response = json!({
            "steps": [{
                "content": [
                    { "text": "  Merhaba, " },
                    { "text": "bu bir testtir.  " }
                ]
            }]
        });
        assert_eq!(
            parse_response(&response),
            vec![DiarizedSegment {
                text: "Merhaba, bu bir testtir.".into(),
                speaker: None,
                start_ms: 0,
            }]
        );
    }

    #[test]
    fn a_candidates_shaped_response_still_yields_a_transcript() {
        let response = json!({
            "candidates": [{
                "content": { "parts": [{ "text": "just the text" }] }
            }]
        });
        assert_eq!(
            parse_response(&response),
            vec![DiarizedSegment {
                text: "just the text".into(),
                speaker: None,
                start_ms: 0,
            }]
        );
    }

    #[test]
    fn response_without_annotations_falls_back_to_output_text() {
        let response = json!({ "output_text": "  just the text  " });
        let segments = parse_response(&response);
        assert_eq!(
            segments,
            vec![DiarizedSegment {
                text: "just the text".into(),
                speaker: None,
                start_ms: 0,
            }]
        );
    }

    #[test]
    fn response_nested_under_interaction_is_unwrapped() {
        let response = json!({ "interaction": { "output_text": "nested" } });
        assert_eq!(parse_response(&response)[0].text, "nested");
    }

    #[test]
    fn empty_response_yields_no_segments() {
        assert!(parse_response(&json!({ "output_text": "   " })).is_empty());
        assert!(parse_response(&json!({})).is_empty());
    }

    #[test]
    fn upload_uri_is_read_from_either_shape() {
        assert_eq!(
            file_uri_of(&json!({ "file": { "uri": "files/a" } })).as_deref(),
            Some("files/a")
        );
        assert_eq!(
            file_uri_of(&json!({ "uri": "files/b" })).as_deref(),
            Some("files/b")
        );
        assert!(file_uri_of(&json!({ "file": { "uri": "" } })).is_none());
    }

    #[test]
    fn offsets_parse_from_the_apis_seconds_string() {
        assert_eq!(parse_offset_ms(Some("0.100s")), 100);
        assert_eq!(parse_offset_ms(Some("12s")), 12_000);
        assert_eq!(parse_offset_ms(Some("1.2345s")), 1235);
        assert_eq!(parse_offset_ms(None), 0);
        assert_eq!(parse_offset_ms(Some("garbage")), 0);
    }

    #[test]
    fn vocabulary_accepts_lines_or_commas() {
        assert_eq!(
            parse_vocabulary("Fisilti\n Tauri ,, Gemini \n\n"),
            vec!["Fisilti", "Tauri", "Gemini"]
        );
        assert!(parse_vocabulary("   ").is_empty());
    }

    #[test]
    fn truncation_never_splits_a_multibyte_character() {
        // "ş" is two bytes; a byte cut at 3 would land inside the second one.
        assert_eq!(truncate("aşşa", 3), "aş…");
        assert_eq!(truncate("aşşa", 100), "aşşa");
        assert_eq!(truncate("çççç", 1), "…");
    }

    #[test]
    fn only_overload_and_server_errors_are_retried() {
        for status in [408, 429, 500, 502, 503, 504] {
            assert!(is_retryable_status(status), "{status}");
        }
        for status in [400, 401, 403, 404, 413] {
            assert!(!is_retryable_status(status), "{status}");
        }
    }

    #[test]
    fn backoff_doubles_honours_retry_after_and_is_capped() {
        assert_eq!(backoff_delay(1, None), Duration::from_secs(2));
        assert_eq!(backoff_delay(2, None), Duration::from_secs(4));
        assert_eq!(backoff_delay(3, None), Duration::from_secs(8));
        assert_eq!(
            backoff_delay(1, Some(Duration::from_secs(7))),
            Duration::from_secs(7)
        );
        assert_eq!(backoff_delay(10, None), RETRY_MAX_DELAY);
        assert_eq!(
            backoff_delay(1, Some(Duration::from_secs(600))),
            RETRY_MAX_DELAY
        );
    }

    #[test]
    fn diarization_limit_matches_the_documented_thirty_minutes() {
        assert!(supports_diarization(DIARIZATION_MAX_SECS));
        assert!(!supports_diarization(DIARIZATION_MAX_SECS + 1));
    }
}
