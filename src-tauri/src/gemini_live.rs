//! Live speech processing of a meeting audio stream via the Gemini Live API.
//!
//! The Live API is a stateful WebSocket (`BidiGenerateContent`) that takes raw
//! 16 kHz mono PCM and streams text back. That input format is exactly what the
//! meeting capture loop already produces, so a session is a thin sink the loop
//! can push frames into.
//!
//! Two models are supported, selected by [`LiveMode`]:
//!
//! * **Translate** (`gemini-3.5-live-translate`) speaks the meeting back in a
//!   target language, and reports *both* what it heard and what it said — the
//!   "original + translation" pair the transcript shows side by side.
//! * **Transcribe** (`gemini-3.5-transcribe-live`) is transcription only. It is
//!   several times cheaper because its output is text rather than speech, so it
//!   is the right choice for anyone who wants an accurate cloud transcript and
//!   no translation.
//!
//! Two constraints shape the design:
//!
//! 1. **Sessions expire.** Audio sessions cap out around 15 minutes and the
//!    WebSocket itself around 10, while meetings run for hours. The server
//!    hands out resumption handles and warns with `goAway` before cutting the
//!    connection, so [`LiveSession`] reconnects transparently and resumes from
//!    the last handle. Audio pushed during a reconnect is dropped rather than
//!    queued — falling behind real time is worse than a gap, since the model is
//!    following a live conversation.
//! 2. **The capture loop must never block.** It runs a hard real-time mix/VAD
//!    loop, so `push_audio` is a non-blocking send into a bounded channel that
//!    drops on overflow.
//!
//! This module is ADDITIVE: nothing here runs unless the user picks a live mode
//! and supplies a Gemini API key.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

/// Default Live API model for speech-to-speech translation. Overridable from
/// settings so a newer preview can be selected without a rebuild.
pub const DEFAULT_LIVE_TRANSLATE_MODEL: &str = "gemini-3.5-live-translate-preview";

/// Default Live API model for transcription-only streaming.
pub const DEFAULT_LIVE_TRANSCRIBE_MODEL: &str = "gemini-3.5-transcribe-live";

/// What a live session should do with the audio it is fed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LiveMode {
    /// Translate speech into `target_language`, reporting original and
    /// translation separately.
    Translate { target_language: String },
    /// Transcribe only, no translation.
    Transcribe {
        /// BCP-47 hints; empty means auto-detect.
        language_codes: Vec<String>,
        /// Ask the model to clean disfluencies and format the text.
        smart: bool,
    },
}

impl LiveMode {
    /// The model id this mode expects when the caller has no override.
    pub fn default_model(&self) -> &'static str {
        match self {
            LiveMode::Translate { .. } => DEFAULT_LIVE_TRANSLATE_MODEL,
            LiveMode::Transcribe { .. } => DEFAULT_LIVE_TRANSCRIBE_MODEL,
        }
    }

    /// Short tag for logs.
    fn tag(&self) -> &'static str {
        match self {
            LiveMode::Translate { .. } => "translate",
            LiveMode::Transcribe { .. } => "transcribe",
        }
    }
}

const LIVE_API_HOST: &str = "generativelanguage.googleapis.com";
const LIVE_API_PATH: &str =
    "/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent";

/// How many audio chunks may sit in the channel before the capture loop starts
/// dropping them. At ~100 ms per chunk this is ~3 s of slack, enough to ride
/// out a reconnect without letting the stream drift behind the conversation.
const AUDIO_QUEUE_CHUNKS: usize = 32;

/// Reconnect backoff after a failed connection attempt. Deliberately short: a
/// meeting is happening right now, and every second offline is lost speech.
const RECONNECT_DELAY_MS: u64 = 750;

/// A transcript fragment produced by the Live API.
///
/// The API reports the two directions separately and incrementally: `original`
/// is what it heard (in the speaker's language), `translation` is what it said
/// back (in the target language). A single message usually carries one or the
/// other, so both are optional.
#[derive(Clone, Debug, Default)]
pub struct LiveTranscript {
    /// Transcript of the INPUT audio, in the detected source language.
    pub original: Option<String>,
    /// Transcript of the model's translated speech, in the target language.
    pub translation: Option<String>,
    /// BCP-47 code the model detected for the input, when reported.
    pub source_language: Option<String>,
    /// True on the message that closes a turn. Transcripts stream in as partial
    /// fragments, so consumers accumulate until they see this — otherwise a
    /// sentence lands in the meeting as a dozen word-sized segments.
    pub turn_complete: bool,
}

impl LiveTranscript {
    /// Nothing to accumulate AND no turn boundary to act on.
    fn is_empty(&self) -> bool {
        self.original.is_none() && self.translation.is_none() && !self.turn_complete
    }
}

/// Everything needed to open a live session.
#[derive(Clone, Debug)]
pub struct LiveConfig {
    /// Gemini API key.
    pub api_key: String,
    /// Bare model id (without the `models/` prefix).
    pub model: String,
    /// Translate or transcribe.
    pub mode: LiveMode,
}

impl LiveConfig {
    fn model_resource(&self) -> String {
        if self.model.starts_with("models/") {
            self.model.clone()
        } else {
            format!("models/{}", self.model)
        }
    }

    /// The setup frame sent immediately after the socket opens.
    ///
    /// The two modes need genuinely different frames. Translate is a
    /// speech-to-speech model with no text-only mode, so `responseModalities`
    /// must be `["AUDIO"]`; we never play that audio, and it is the two
    /// transcription flags that turn the session into the "original +
    /// translation" text pair we actually consume. Transcribe is the mirror
    /// image: `["TEXT"]` is required, there is no output audio to transcribe,
    /// and the transcription config carries the language hints and mode.
    ///
    /// `sessionResumption` and `contextWindowCompression` are what let a
    /// meeting outlive the per-session limits, and apply to both.
    fn setup_message(&self, resumption_handle: Option<&str>) -> Value {
        let session_resumption = match resumption_handle {
            Some(handle) => json!({ "handle": handle }),
            None => json!({}),
        };

        let mut setup = json!({
            "model": self.model_resource(),
            "sessionResumption": session_resumption,
            "contextWindowCompression": {
                "slidingWindow": {},
            },
        });

        match &self.mode {
            LiveMode::Translate { target_language } => {
                setup["generationConfig"] = json!({ "responseModalities": ["AUDIO"] });
                setup["inputAudioTranscription"] = json!({});
                setup["outputAudioTranscription"] = json!({});
                setup["translationConfig"] = json!({
                    "targetLanguageCode": target_language,
                    // Keep translating even when the speaker already uses the
                    // target language, so the transcript never silently gaps.
                    "echoTargetLanguage": true,
                });
            }
            LiveMode::Transcribe {
                language_codes,
                smart,
            } => {
                setup["generationConfig"] = json!({ "responseModalities": ["TEXT"] });
                setup["inputAudioTranscription"] = json!({
                    "languageCodes": language_codes,
                    "mode": if *smart { "SMART" } else { "VERBATIM" },
                });
            }
        }

        json!({ "setup": setup })
    }
}

/// A running live session. Dropping it stops the worker.
pub struct LiveSession {
    audio_tx: mpsc::Sender<Vec<f32>>,
    stop: Arc<AtomicBool>,
    /// Chunks the capture loop had to drop because the worker was behind.
    /// Reported once at shutdown rather than logged per drop.
    dropped: Arc<AtomicU64>,
    label: String,
}

impl LiveSession {
    /// Open a session and start streaming. `on_transcript` is invoked from the
    /// worker task for every transcript fragment; keep it cheap and
    /// non-blocking.
    ///
    /// Returns immediately — the connection is established in the background,
    /// so a slow or failing connect never delays the meeting from starting.
    pub fn start<F>(config: LiveConfig, label: impl Into<String>, on_transcript: F) -> Self
    where
        F: Fn(LiveTranscript) + Send + Sync + 'static,
    {
        let label = label.into();
        let (audio_tx, audio_rx) = mpsc::channel::<Vec<f32>>(AUDIO_QUEUE_CHUNKS);
        let stop = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicU64::new(0));

        let worker_stop = stop.clone();
        let worker_label = label.clone();
        tauri::async_runtime::spawn(async move {
            run_session_loop(
                config,
                worker_label,
                audio_rx,
                worker_stop,
                Arc::new(on_transcript),
            )
            .await;
        });

        Self {
            audio_tx,
            stop,
            dropped,
            label,
        }
    }

    /// Hand a chunk of 16 kHz mono audio to the session. Never blocks: if the
    /// worker is behind (reconnecting, or the network stalled) the chunk is
    /// dropped, because delaying the real-time capture loop would be worse.
    pub fn push_audio(&self, frames: &[f32]) {
        if frames.is_empty() || self.stop.load(Ordering::Relaxed) {
            return;
        }
        if self.audio_tx.try_send(frames.to_vec()).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Signal the worker to close the socket and stop reconnecting.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        let dropped = self.dropped.load(Ordering::Relaxed);
        if dropped > 0 {
            log::warn!(
                "gemini-live[{}]: dropped {} audio chunk(s) while the worker was behind",
                self.label,
                dropped
            );
        }
    }
}

impl Drop for LiveSession {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Reconnect loop: keeps a session alive across the API's connection lifetime
/// limits by resuming from the last handle the server issued.
async fn run_session_loop<F>(
    config: LiveConfig,
    label: String,
    mut audio_rx: mpsc::Receiver<Vec<f32>>,
    stop: Arc<AtomicBool>,
    on_transcript: Arc<F>,
) where
    F: Fn(LiveTranscript) + Send + Sync + 'static,
{
    let mut resumption_handle: Option<String> = None;
    let mut consecutive_failures: u32 = 0;

    while !stop.load(Ordering::Relaxed) {
        match run_one_connection(
            &config,
            &label,
            &mut audio_rx,
            &stop,
            &on_transcript,
            resumption_handle.as_deref(),
        )
        .await
        {
            Ok(next_handle) => {
                consecutive_failures = 0;
                // Keep the previous handle when the server didn't issue a new
                // one, so a clean close still resumes rather than restarting.
                if next_handle.is_some() {
                    resumption_handle = next_handle;
                }
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                log::info!("gemini-live[{}]: reconnecting to continue session", label);
            }
            Err(e) => {
                consecutive_failures += 1;
                log::warn!(
                    "gemini-live[{}]: connection failed ({}); attempt {}",
                    label,
                    e,
                    consecutive_failures
                );
                // Back off linearly, capped, so a persistent failure (bad key,
                // no network) doesn't hammer the endpoint for a whole meeting.
                let delay = RECONNECT_DELAY_MS * u64::from(consecutive_failures.min(8));
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            }
        }
    }
    log::info!("gemini-live[{}]: session loop ended", label);
}

/// One WebSocket connection's lifetime. Returns the most recent resumption
/// handle so the caller can continue the session on the next connection.
async fn run_one_connection<F>(
    config: &LiveConfig,
    label: &str,
    audio_rx: &mut mpsc::Receiver<Vec<f32>>,
    stop: &Arc<AtomicBool>,
    on_transcript: &Arc<F>,
    resumption_handle: Option<&str>,
) -> Result<Option<String>>
where
    F: Fn(LiveTranscript) + Send + Sync + 'static,
{
    let url = format!(
        "wss://{}{}?key={}",
        LIVE_API_HOST, LIVE_API_PATH, config.api_key
    );
    let (ws, _response) = tokio_tungstenite::connect_async(&url)
        .await
        .map_err(|e| anyhow!("connect: {}", e))?;
    let (mut writer, mut reader) = ws.split();

    writer
        .send(Message::Text(
            config.setup_message(resumption_handle).to_string().into(),
        ))
        .await
        .map_err(|e| anyhow!("setup: {}", e))?;
    log::info!(
        "gemini-live[{}]: connected (mode={}, resumed={})",
        label,
        config.mode.tag(),
        resumption_handle.is_some()
    );

    let mut handle: Option<String> = resumption_handle.map(str::to_string);
    // Set when the server warns it is about to close, so we stop feeding audio
    // into a socket that is going away and reconnect promptly.
    let mut going_away = false;

    loop {
        if stop.load(Ordering::Relaxed) {
            // Tell the server the microphone closed so it flushes any pending
            // transcript, then let the socket close.
            let _ = writer
                .send(Message::Text(
                    json!({ "realtimeInput": { "audioStreamEnd": true } })
                        .to_string()
                        .into(),
                ))
                .await;
            let _ = writer.send(Message::Close(None)).await;
            return Ok(handle);
        }

        tokio::select! {
            // Outbound: audio from the capture loop.
            chunk = audio_rx.recv(), if !going_away => {
                match chunk {
                    Some(frames) => {
                        let payload = json!({
                            "realtimeInput": {
                                "audio": {
                                    "data": BASE64.encode(pcm16_bytes(&frames)),
                                    "mimeType": "audio/pcm;rate=16000",
                                }
                            }
                        });
                        if let Err(e) = writer.send(Message::Text(payload.to_string().into())).await {
                            return Err(anyhow!("send audio: {}", e));
                        }
                    }
                    // The sender was dropped: the meeting ended.
                    None => {
                        let _ = writer.send(Message::Close(None)).await;
                        return Ok(handle);
                    }
                }
            }

            // Inbound: transcripts and session-control messages.
            incoming = reader.next() => {
                let Some(message) = incoming else {
                    // Stream ended — reconnect and resume.
                    return Ok(handle);
                };
                let message = message.map_err(|e| anyhow!("read: {}", e))?;
                let text = match message {
                    Message::Text(t) => t.to_string(),
                    // The API also frames responses as binary JSON.
                    Message::Binary(b) => match String::from_utf8(b.to_vec()) {
                        Ok(t) => t,
                        Err(_) => continue,
                    },
                    Message::Close(_) => return Ok(handle),
                    _ => continue,
                };
                let Ok(value) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };

                if let Some(new_handle) = resumption_handle_of(&value) {
                    handle = Some(new_handle);
                }
                if let Some(time_left) = go_away_of(&value) {
                    log::info!(
                        "gemini-live[{}]: server going away in {}; will resume",
                        label,
                        time_left
                    );
                    going_away = true;
                }

                let transcript = transcript_of(&value, &config.mode);
                if !transcript.is_empty() {
                    on_transcript(transcript);
                }

                if going_away {
                    let _ = writer.send(Message::Close(None)).await;
                    return Ok(handle);
                }
            }
        }
    }
}

/// Convert 16 kHz mono f32 samples (-1..1) to the little-endian 16-bit PCM the
/// Live API expects.
fn pcm16_bytes(frames: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(frames.len() * 2);
    for &sample in frames {
        let clamped = sample.clamp(-1.0, 1.0);
        // i16::MIN..=i16::MAX is asymmetric; scaling by 32767 keeps +1.0 and
        // -1.0 symmetric and avoids wrapping at the positive extreme.
        let value = (clamped * 32767.0) as i16;
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

/// Pull the transcript fragments out of a `serverContent` message.
///
/// The two modes read the same envelope differently. Translate streams partial
/// fragments in both directions and marks the end of an exchange with
/// `turnComplete`, so the consumer accumulates until then. Transcribe has no
/// output audio to transcribe, and its `inputTranscription` is documented as
/// *already finalized* — emitted when the speaker pauses or the turn completes
/// — while `interimInputTranscription` carries the speculative partials we
/// deliberately ignore. So in transcribe mode every fragment we surface is
/// itself a complete unit and is marked as closing a turn; waiting for a
/// separate `turnComplete` that the model may never send would leave the last
/// sentence of a meeting stuck in the accumulator.
fn transcript_of(value: &Value, mode: &LiveMode) -> LiveTranscript {
    let content = &value["serverContent"];
    let input = non_empty(content["inputTranscription"]["text"].as_str());
    let source_language = non_empty(content["inputTranscription"]["languageCode"].as_str());
    // `turnComplete` closes the exchange; `generationComplete` fires when the
    // model stops producing output for this turn. Either is a safe flush point.
    let boundary = content["turnComplete"].as_bool().unwrap_or(false)
        || content["generationComplete"].as_bool().unwrap_or(false);

    match mode {
        LiveMode::Translate { .. } => LiveTranscript {
            translation: non_empty(content["outputTranscription"]["text"].as_str()),
            original: input,
            source_language,
            turn_complete: boundary,
        },
        LiveMode::Transcribe { .. } => LiveTranscript {
            turn_complete: boundary || input.is_some(),
            original: input,
            translation: None,
            source_language,
        },
    }
}

/// Extract a session-resumption handle, if this message carries a resumable one.
fn resumption_handle_of(value: &Value) -> Option<String> {
    let update = value.get("sessionResumptionUpdate")?;
    // A handle is only good if the server marked this point resumable.
    if !update["resumable"].as_bool().unwrap_or(false) {
        return None;
    }
    non_empty(update["newHandle"].as_str())
}

/// Extract the `goAway` warning's remaining time, if present.
fn go_away_of(value: &Value) -> Option<String> {
    let go_away = value.get("goAway")?;
    Some(
        go_away["timeLeft"]
            .as_str()
            .unwrap_or("unknown")
            .to_string(),
    )
}

fn non_empty(text: Option<&str>) -> Option<String> {
    let text = text?;
    if text.trim().is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn translate_mode() -> LiveMode {
        LiveMode::Translate {
            target_language: "tr".into(),
        }
    }

    fn transcribe_mode() -> LiveMode {
        LiveMode::Transcribe {
            language_codes: vec![],
            smart: true,
        }
    }

    #[test]
    fn translate_setup_carries_translation_and_both_transcription_flags() {
        let config = LiveConfig {
            api_key: "key".into(),
            model: DEFAULT_LIVE_TRANSLATE_MODEL.into(),
            mode: translate_mode(),
        };
        let setup = config.setup_message(None);
        let s = &setup["setup"];
        assert_eq!(
            s["model"],
            json!("models/gemini-3.5-live-translate-preview")
        );
        assert_eq!(
            s["generationConfig"]["responseModalities"],
            json!(["AUDIO"])
        );
        assert_eq!(s["translationConfig"]["targetLanguageCode"], json!("tr"));
        // Both transcription flags must be present, or the session returns only
        // audio and the meeting transcript stays empty.
        assert!(s["inputAudioTranscription"].is_object());
        assert!(s["outputAudioTranscription"].is_object());
        // Without these two a long meeting dies at the session limit.
        assert!(s["sessionResumption"].is_object());
        assert!(s["contextWindowCompression"]["slidingWindow"].is_object());
    }

    #[test]
    fn transcribe_setup_requests_text_and_never_translation() {
        let config = LiveConfig {
            api_key: "key".into(),
            model: DEFAULT_LIVE_TRANSCRIBE_MODEL.into(),
            mode: LiveMode::Transcribe {
                language_codes: vec!["tr-TR".into()],
                smart: true,
            },
        };
        let setup = config.setup_message(None);
        let s = &setup["setup"];
        assert_eq!(s["model"], json!("models/gemini-3.5-transcribe-live"));
        // TEXT is required for this model, and AUDIO would be rejected.
        assert_eq!(s["generationConfig"]["responseModalities"], json!(["TEXT"]));
        assert_eq!(
            s["inputAudioTranscription"]["languageCodes"],
            json!(["tr-TR"])
        );
        assert_eq!(s["inputAudioTranscription"]["mode"], json!("SMART"));
        // There is no output speech, so asking to transcribe it is meaningless.
        assert!(s.get("outputAudioTranscription").is_none());
        assert!(s.get("translationConfig").is_none());
        // Long meetings still need these.
        assert!(s["sessionResumption"].is_object());
        assert!(s["contextWindowCompression"]["slidingWindow"].is_object());
    }

    #[test]
    fn verbatim_transcribe_setup_asks_for_verbatim() {
        let config = LiveConfig {
            api_key: "key".into(),
            model: DEFAULT_LIVE_TRANSCRIBE_MODEL.into(),
            mode: LiveMode::Transcribe {
                language_codes: vec![],
                smart: false,
            },
        };
        assert_eq!(
            config.setup_message(None)["setup"]["inputAudioTranscription"]["mode"],
            json!("VERBATIM")
        );
    }

    #[test]
    fn each_mode_knows_its_own_default_model() {
        assert_eq!(
            translate_mode().default_model(),
            DEFAULT_LIVE_TRANSLATE_MODEL
        );
        assert_eq!(
            transcribe_mode().default_model(),
            DEFAULT_LIVE_TRANSCRIBE_MODEL
        );
    }

    #[test]
    fn setup_message_resumes_from_a_handle() {
        let config = LiveConfig {
            api_key: "key".into(),
            model: "models/custom-model".into(),
            mode: LiveMode::Translate {
                target_language: "en".into(),
            },
        };
        let setup = config.setup_message(Some("handle-123"));
        // An explicit models/ prefix must not be doubled.
        assert_eq!(setup["setup"]["model"], json!("models/custom-model"));
        assert_eq!(
            setup["setup"]["sessionResumption"]["handle"],
            json!("handle-123")
        );
    }

    #[test]
    fn parses_both_transcript_directions() {
        let message = json!({
            "serverContent": {
                "inputTranscription": { "text": "merhaba", "languageCode": "tr" },
                "outputTranscription": { "text": "hello" }
            }
        });
        let parsed = transcript_of(&message, &translate_mode());
        assert_eq!(parsed.original.as_deref(), Some("merhaba"));
        assert_eq!(parsed.translation.as_deref(), Some("hello"));
        assert_eq!(parsed.source_language.as_deref(), Some("tr"));
    }

    #[test]
    fn transcribe_mode_finalizes_every_fragment_it_surfaces() {
        let message = json!({
            "serverContent": {
                "inputTranscription": { "text": "merhaba", "languageCode": "tr" }
            }
        });
        let parsed = transcript_of(&message, &transcribe_mode());
        assert_eq!(parsed.original.as_deref(), Some("merhaba"));
        assert!(parsed.translation.is_none());
        // Without this the last sentence of a meeting would never be flushed.
        assert!(parsed.turn_complete);
    }

    #[test]
    fn transcribe_mode_ignores_speculative_partials() {
        // Interim hypotheses are rewritten as the speaker keeps talking; taking
        // them would duplicate text into the transcript.
        let message = json!({
            "serverContent": {
                "interimInputTranscription": { "text": "merh" }
            }
        });
        assert!(transcript_of(&message, &transcribe_mode()).is_empty());
    }

    #[test]
    fn reports_turn_boundaries() {
        let complete = json!({ "serverContent": { "turnComplete": true } });
        assert!(transcript_of(&complete, &translate_mode()).turn_complete);
        // A bare turn boundary carries no text but still must reach the
        // consumer, so it can flush what it accumulated.
        assert!(!transcript_of(&complete, &translate_mode()).is_empty());

        let generation_done = json!({ "serverContent": { "generationComplete": true } });
        assert!(transcript_of(&generation_done, &translate_mode()).turn_complete);

        let partial = json!({
            "serverContent": { "outputTranscription": { "text": "hel" } }
        });
        assert!(!transcript_of(&partial, &translate_mode()).turn_complete);
    }

    #[test]
    fn ignores_messages_without_transcripts() {
        // Audio-only turns arrive constantly; they must not produce empty
        // segments in the meeting transcript.
        let message = json!({
            "serverContent": {
                "modelTurn": { "parts": [{ "inlineData": { "data": "AAAA" } }] }
            }
        });
        assert!(transcript_of(&message, &translate_mode()).is_empty());
        assert!(transcript_of(&json!({}), &translate_mode()).is_empty());
        assert!(transcript_of(&json!({}), &transcribe_mode()).is_empty());
    }

    #[test]
    fn only_accepts_resumable_handles() {
        let resumable = json!({
            "sessionResumptionUpdate": { "newHandle": "h1", "resumable": true }
        });
        assert_eq!(resumption_handle_of(&resumable).as_deref(), Some("h1"));
        // Resuming from a non-resumable point would restart the conversation
        // mid-meeting, so it must be ignored.
        let not_resumable = json!({
            "sessionResumptionUpdate": { "newHandle": "h2", "resumable": false }
        });
        assert!(resumption_handle_of(&not_resumable).is_none());
        assert!(resumption_handle_of(&json!({})).is_none());
    }

    #[test]
    fn converts_samples_to_little_endian_pcm16() {
        let bytes = pcm16_bytes(&[0.0, 1.0, -1.0]);
        assert_eq!(bytes.len(), 6);
        assert_eq!(&bytes[0..2], &0i16.to_le_bytes());
        assert_eq!(&bytes[2..4], &32767i16.to_le_bytes());
        assert_eq!(&bytes[4..6], &(-32767i16).to_le_bytes());
        // Out-of-range input must clamp rather than wrap to the opposite sign.
        let clipped = pcm16_bytes(&[2.5, -2.5]);
        assert_eq!(&clipped[0..2], &32767i16.to_le_bytes());
        assert_eq!(&clipped[2..4], &(-32767i16).to_le_bytes());
    }
}
