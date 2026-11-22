//! Qwen3-ASR (speech-LLM) over three ONNX graphs, in the layout of the
//! `onnx-asr` "speech-llm" export (e.g. `OpenVoiceOS/qwen3-asr-1.7b-onnx`):
//!
//! - `encoder[_int8].onnx`: 128-mel features → audio embeddings already
//!   projected into the LM's embedding space.
//! - `embed_tokens[_int8].onnx`: token ids → embeddings.
//! - `decoder[_int8].onnx`: merged prefill/decode Qwen3 LM with a KV cache
//!   (`past_key_values.{i}.key|value` → `present.{i}.key|value`).
//! - `config.json`: prompt token ids, EOS ids and cache geometry.
//! - `vocab.json`: byte-level BPE vocabulary (decode only).
//!
//! The prompt is the Qwen chat template with the audio embeddings spliced
//! between `<|audio_start|>` and `<|audio_end|>`; the model answers
//! `language X<asr_text>transcript`. Forcing a language appends
//! `language X<asr_text>` to the assistant turn, as the reference
//! implementation does.

mod features;
mod tokenizer;

use std::path::{Path, PathBuf};
use std::time::Instant;

use ort::session::{Session, SessionInputValue};
use ort::value::{DynValue, Tensor};
use serde::Deserialize;

use self::tokenizer::Vocab;
use super::Quantization;
use crate::{
    ModelCapabilities, SpeechModel, TranscribeError, TranscribeOptions, TranscriptionResult,
};

/// The encoder export accepts at most 3000 feature frames (30 s).
const MAX_FEATURE_FRAMES: usize = 3000;

/// Longest audio piece sent through the model in one pass. The int8 export
/// is unreliable on longer inputs (it can return an empty transcript), so
/// longer audio is cut at the quietest point before this length.
const MAX_CHUNK_SECS: f32 = 15.0;
/// Cut points are searched in the last part of each chunk, from this
/// fraction of `MAX_CHUNK_SECS` onwards.
const CUT_SEARCH_FROM: f32 = 0.5;

const NEG_INF: f32 = -3.402_823_5e38;

/// Language codes (BCP-47 base) Qwen3-ASR names in its prompt, with the
/// English name it expects after `language `.
const LANGUAGES: &[(&str, &str)] = &[
    ("ar", "Arabic"),
    ("yue", "Cantonese"),
    ("zh", "Chinese"),
    ("cs", "Czech"),
    ("da", "Danish"),
    ("nl", "Dutch"),
    ("en", "English"),
    ("fil", "Filipino"),
    ("fi", "Finnish"),
    ("fr", "French"),
    ("de", "German"),
    ("el", "Greek"),
    ("hi", "Hindi"),
    ("hu", "Hungarian"),
    ("id", "Indonesian"),
    ("it", "Italian"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("mk", "Macedonian"),
    ("ms", "Malay"),
    ("fa", "Persian"),
    ("pl", "Polish"),
    ("pt", "Portuguese"),
    ("ro", "Romanian"),
    ("ru", "Russian"),
    ("es", "Spanish"),
    ("sv", "Swedish"),
    ("th", "Thai"),
    ("tr", "Turkish"),
    ("vi", "Vietnamese"),
];

const LANGUAGE_CODES: &[&str] = &[
    "ar", "yue", "zh", "cs", "da", "nl", "en", "fil", "fi", "fr", "de", "el", "hi", "hu", "id",
    "it", "ja", "ko", "mk", "ms", "fa", "pl", "pt", "ro", "ru", "es", "sv", "th", "tr", "vi",
];

/// Map a language setting to the supported base code (`zh-Hans` → `zh`).
fn language_code(code: &str) -> Option<&'static str> {
    let base = match code {
        "zh-Hans" | "zh-Hant" | "zh-CN" | "zh-TW" => "zh",
        "tl" => "fil",
        other => other.split(['-', '_']).next().unwrap_or(other),
    };
    LANGUAGES.iter().find(|(c, _)| *c == base).map(|(c, _)| *c)
}

#[derive(Debug, Deserialize)]
struct Config {
    hidden_size: usize,
    num_layers: usize,
    num_key_value_heads: usize,
    head_dim: usize,
    #[serde(default = "default_n_window")]
    n_window: usize,
    #[serde(default = "default_n_window_infer")]
    n_window_infer: usize,
    eos_token_ids: Vec<i64>,
    #[serde(default = "default_max_sequence_length")]
    max_sequence_length: usize,
    text_start_token_id: i64,
    prompt_prefix_ids: Vec<i64>,
    prompt_suffix_ids: Vec<i64>,
}

fn default_n_window() -> usize {
    50
}
fn default_n_window_infer() -> usize {
    800
}
fn default_max_sequence_length() -> usize {
    512
}

/// Per-call parameters for [`Qwen3AsrModel::transcribe_with`].
#[derive(Debug, Clone, Default)]
pub struct Qwen3AsrParams {
    /// Language code to force (e.g. "tr", "en"). `None` or an unsupported
    /// code lets the model detect the language.
    pub language: Option<String>,
}

pub struct Qwen3AsrModel {
    encoder: Session,
    embed: Session,
    decoder: Session,
    vocab: Vocab,
    config: Config,
    /// Ids of `language` + ` {Name}` per supported code, resolved at load.
    language_tokens: Vec<(&'static str, Vec<i64>)>,
}

fn resolve(
    dir: &Path,
    stem: &str,
    quantization: &Quantization,
) -> Result<PathBuf, TranscribeError> {
    if *quantization == Quantization::Int8 {
        let p = dir.join(format!("{stem}_int8.onnx"));
        if p.exists() {
            return Ok(p);
        }
        log::warn!("{stem}_int8.onnx not found, falling back to {stem}.onnx");
    }
    let p = dir.join(format!("{stem}.onnx"));
    if p.exists() {
        Ok(p)
    } else {
        Err(TranscribeError::ModelNotFound(p))
    }
}

impl Qwen3AsrModel {
    pub fn load(model_dir: &Path, quantization: &Quantization) -> Result<Self, TranscribeError> {
        if !model_dir.exists() {
            return Err(TranscribeError::ModelNotFound(model_dir.to_path_buf()));
        }
        let start = Instant::now();

        let config_path = model_dir.join("config.json");
        if !config_path.exists() {
            return Err(TranscribeError::ModelNotFound(config_path));
        }
        let config: Config = serde_json::from_str(&std::fs::read_to_string(&config_path)?)?;
        let vocab = Vocab::load(&model_dir.join("vocab.json"))?;

        let encoder =
            super::session::create_session(&resolve(model_dir, "encoder", quantization)?)?;
        let embed =
            super::session::create_session(&resolve(model_dir, "embed_tokens", quantization)?)?;
        let decoder =
            super::session::create_session(&resolve(model_dir, "decoder", quantization)?)?;

        let past_inputs = decoder
            .inputs()
            .iter()
            .filter(|i| i.name().starts_with("past_key_values."))
            .count();
        if past_inputs != config.num_layers * 2 {
            return Err(TranscribeError::Config(format!(
                "decoder has {past_inputs} cache inputs, config expects {}",
                config.num_layers * 2
            )));
        }

        let language_tokens = LANGUAGES
            .iter()
            .filter_map(|(code, name)| {
                let ids = vocab.encode_greedy(&format!("language {name}"))?;
                Some((*code, ids))
            })
            .collect();

        log::info!("Qwen3-ASR loaded in {:.2?}", start.elapsed());
        Ok(Self {
            encoder,
            embed,
            decoder,
            vocab,
            config,
            language_tokens,
        })
    }

    pub fn transcribe_with(
        &mut self,
        samples: &[f32],
        params: &Qwen3AsrParams,
    ) -> Result<TranscriptionResult, TranscribeError> {
        let start = Instant::now();
        let forced = params
            .language
            .as_deref()
            .and_then(language_code)
            .and_then(|code| self.language_tokens.iter().find(|(c, _)| *c == code))
            .map(|(_, ids)| ids.clone());

        let mut parts = Vec::new();
        for (from, to) in chunk_bounds(samples) {
            let text = self.transcribe_chunk(&samples[from..to], forced.as_deref())?;
            if !text.is_empty() {
                parts.push(text);
            }
        }
        let text = join_chunks(&parts);
        log::info!(
            "Qwen3-ASR transcribed {:.1}s in {:.2?}",
            samples.len() as f32 / features::SAMPLE_RATE as f32,
            start.elapsed()
        );
        Ok(TranscriptionResult {
            text,
            segments: None,
        })
    }

    fn transcribe_chunk(
        &mut self,
        samples: &[f32],
        forced_language: Option<&[i64]>,
    ) -> Result<String, TranscribeError> {
        let hidden = self.config.hidden_size;

        // --- audio → embeddings -------------------------------------------
        let feature_len = samples
            .len()
            .div_ceil(features::HOP_LENGTH)
            .clamp(1, MAX_FEATURE_FRAMES);
        let chunk_len = self.config.n_window * 2;
        let padded_frames = feature_len.div_ceil(chunk_len) * chunk_len;
        let mut padded = samples[..samples.len().min(feature_len * features::HOP_LENGTH)].to_vec();
        padded.resize(padded_frames * features::HOP_LENGTH, 0.0);
        let mel = features::log_mel(&padded);
        let (n_mels, n_frames) = mel.dim();
        let mel_tensor = Tensor::from_array((
            vec![1i64, n_mels as i64, n_frames as i64],
            mel.as_standard_layout()
                .iter()
                .cloned()
                .collect::<Vec<f32>>()
                .into_boxed_slice(),
        ))?;

        let windows = audio_windows(feature_len, chunk_len, self.config.n_window_infer);
        let valid = Tensor::from_array((
            vec![windows.valid_indices.len() as i64],
            windows.valid_indices.clone().into_boxed_slice(),
        ))?;
        let total = windows.valid_indices.len();
        let enc_bias = Tensor::from_array((
            vec![1i64, 1, total as i64, total as i64],
            windows.attn_bias.into_boxed_slice(),
        ))?;

        let audio_embeds: Vec<f32> = {
            let out = self.encoder.run(ort::inputs![
                "input_features" => mel_tensor,
                "valid_indices" => valid,
                "attn_bias" => enc_bias
            ])?;
            let (_, data) = out[0].try_extract_tensor::<f32>()?;
            data.to_vec()
        };
        let audio_len = audio_embeds.len() / hidden;

        // --- prompt embeddings ----------------------------------------------
        let mut suffix = self.config.prompt_suffix_ids.clone();
        if let Some(lang) = forced_language {
            suffix.extend_from_slice(lang);
            suffix.push(self.config.text_start_token_id);
        }
        let prefix = self.embed_ids(&self.config.prompt_prefix_ids.clone())?;
        let suffix_emb = self.embed_ids(&suffix)?;

        let mut inputs_embeds = prefix;
        inputs_embeds.extend_from_slice(&audio_embeds);
        inputs_embeds.extend_from_slice(&suffix_emb);
        let prompt_len = inputs_embeds.len() / hidden;
        debug_assert_eq!(
            prompt_len,
            self.config.prompt_prefix_ids.len() + audio_len + suffix.len()
        );

        // --- greedy decode with KV cache --------------------------------------
        let layers = self.config.num_layers;
        let kv_heads = self.config.num_key_value_heads;
        let head_dim = self.config.head_dim;
        let mut cache: Vec<DynValue> = Vec::with_capacity(layers * 2);
        for _ in 0..layers * 2 {
            // ndarray, not raw data: ort rejects zero-length dims from slices.
            let empty = ndarray::Array4::<f32>::zeros((1, kv_heads, 0, head_dim));
            cache.push(Tensor::from_array(empty)?.into_dyn());
        }

        let mut generated: Vec<i64> = Vec::new();
        let mut step_embeds = inputs_embeds;
        let mut past = 0usize;
        for _ in 0..self.config.max_sequence_length {
            let s = step_embeds.len() / hidden;
            let embeds = Tensor::from_array((
                vec![1i64, s as i64, hidden as i64],
                std::mem::take(&mut step_embeds).into_boxed_slice(),
            ))?;
            let positions = Tensor::from_array((
                vec![1i64, s as i64],
                (past as i64..(past + s) as i64)
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            ))?;
            let bias = Tensor::from_array((
                vec![1i64, 1, s as i64, (past + s) as i64],
                causal_bias(s, past).into_boxed_slice(),
            ))?;

            let mut inputs: Vec<(String, SessionInputValue)> = Vec::with_capacity(3 + layers * 2);
            inputs.push(("inputs_embeds".into(), embeds.into()));
            inputs.push(("attn_bias".into(), bias.into()));
            inputs.push(("position_ids".into(), positions.into()));
            for (idx, value) in std::mem::take(&mut cache).into_iter().enumerate() {
                let kind = if idx % 2 == 0 { "key" } else { "value" };
                inputs.push((
                    format!("past_key_values.{}.{}", idx / 2, kind),
                    value.into(),
                ));
            }

            let mut out = self.decoder.run(inputs)?;
            let next = {
                let (shape, logits) = out["logits"].try_extract_tensor::<f32>()?;
                let vocab_size = shape[2] as usize;
                let last = &logits[logits.len() - vocab_size..];
                argmax(last) as i64
            };
            for layer in 0..layers {
                for kind in ["key", "value"] {
                    let name = format!("present.{layer}.{kind}");
                    cache.push(out.remove(&name).ok_or_else(|| {
                        TranscribeError::Inference(format!("decoder output {name} missing"))
                    })?);
                }
            }
            drop(out);
            past += s;

            if self.config.eos_token_ids.contains(&next) {
                break;
            }
            generated.push(next);
            step_embeds = self.embed_ids(&[next])?;
        }

        let text_ids = match generated
            .iter()
            .position(|&t| t == self.config.text_start_token_id)
        {
            Some(i) => &generated[i + 1..],
            None if forced_language.is_some() => &generated[..],
            // No marker and no forced language: the model produced only the
            // language preamble (silence) or nothing usable.
            None => {
                let raw = self.vocab.decode(&generated);
                return Ok(strip_language_preamble(&raw).to_string());
            }
        };
        Ok(self.vocab.decode(text_ids))
    }

    fn embed_ids(&mut self, ids: &[i64]) -> Result<Vec<f32>, TranscribeError> {
        let t = Tensor::from_array((
            vec![1i64, ids.len() as i64],
            ids.to_vec().into_boxed_slice(),
        ))?;
        let out = self.embed.run(ort::inputs!["input_ids" => t])?;
        let (_, data) = out[0].try_extract_tensor::<f32>()?;
        Ok(data.to_vec())
    }
}

impl SpeechModel for Qwen3AsrModel {
    fn capabilities(&self) -> ModelCapabilities {
        ModelCapabilities {
            name: "Qwen3-ASR",
            engine_id: "qwen3_asr",
            sample_rate: 16000,
            languages: LANGUAGE_CODES,
            supports_timestamps: false,
            supports_translation: false,
            supports_streaming: false,
        }
    }

    fn transcribe(
        &mut self,
        samples: &[f32],
        options: &TranscribeOptions,
    ) -> Result<TranscriptionResult, TranscribeError> {
        self.transcribe_with(
            samples,
            &Qwen3AsrParams {
                language: options.language.clone(),
            },
        )
    }
}

struct AudioWindows {
    valid_indices: Vec<i64>,
    attn_bias: Vec<f32>,
}

fn post_cnn(len: usize) -> usize {
    let mut l = len;
    for _ in 0..3 {
        l = if l == 0 { 0 } else { (l - 1) / 2 + 1 };
    }
    l
}

/// The encoder runs its convolution on fixed `chunk_len`-frame chunks and
/// attends within windows of `n_window_infer` frames; this builds the gather
/// indices of the real (non-padding) outputs and the block-diagonal mask.
fn audio_windows(feature_len: usize, chunk_len: usize, n_window_infer: usize) -> AudioWindows {
    let num_chunks = feature_len.div_ceil(chunk_len).max(1);
    let frames_per_chunk = post_cnn(chunk_len);
    let out_lens: Vec<usize> = (0..num_chunks)
        .map(|i| post_cnn(feature_len.saturating_sub(i * chunk_len).min(chunk_len)))
        .collect();

    let mut valid_indices = Vec::new();
    for (i, &len) in out_lens.iter().enumerate() {
        valid_indices.extend((0..len).map(|j| (i * frames_per_chunk + j) as i64));
    }

    let total: usize = out_lens.iter().sum();
    let window = out_lens.iter().copied().max().unwrap_or(0) * (n_window_infer / chunk_len);
    let mut attn_bias = vec![NEG_INF; total * total];
    let mut start = 0;
    while start < total {
        let end = (start + window.max(1)).min(total);
        for r in start..end {
            for c in start..end {
                attn_bias[r * total + c] = 0.0;
            }
        }
        start = end;
    }
    AudioWindows {
        valid_indices,
        attn_bias,
    }
}

fn causal_bias(s: usize, past: usize) -> Vec<f32> {
    let cols = past + s;
    let mut bias = vec![0.0f32; s * cols];
    for r in 0..s {
        for c in (past + r + 1)..cols {
            bias[r * cols + c] = NEG_INF;
        }
    }
    bias
}

fn argmax(v: &[f32]) -> usize {
    let mut best = 0;
    let mut best_val = f32::NEG_INFINITY;
    for (i, &x) in v.iter().enumerate() {
        if x > best_val {
            best_val = x;
            best = i;
        }
    }
    best
}

/// Split long audio into pieces of at most `MAX_CHUNK_SECS`, cutting at the
/// quietest 100 ms in the second half of each piece so words stay whole.
fn chunk_bounds(samples: &[f32]) -> Vec<(usize, usize)> {
    let sr = features::SAMPLE_RATE;
    let max = (MAX_CHUNK_SECS * sr as f32) as usize;
    let win = sr / 10;
    let mut bounds = Vec::new();
    let mut start = 0;
    while samples.len() - start > max {
        let search_from = start + (max as f32 * CUT_SEARCH_FROM) as usize;
        let search_to = start + max - win;
        let mut best = start + max;
        let mut best_energy = f32::INFINITY;
        let mut pos = search_from;
        while pos <= search_to {
            let e: f32 = samples[pos..pos + win].iter().map(|x| x * x).sum();
            if e < best_energy {
                best_energy = e;
                best = pos + win / 2;
            }
            pos += win / 2;
        }
        bounds.push((start, best));
        start = best;
    }
    if start < samples.len() || bounds.is_empty() {
        bounds.push((start, samples.len()));
    }
    bounds
}

fn join_chunks(parts: &[String]) -> String {
    let mut out = String::new();
    for p in parts {
        if !out.is_empty() {
            let cjk = |c: char| {
                ('\u{3000}'..='\u{9fff}').contains(&c) || ('\u{ac00}'..='\u{d7af}').contains(&c)
            };
            let joins_cjk =
                out.chars().last().is_some_and(cjk) && p.chars().next().is_some_and(cjk);
            if !joins_cjk {
                out.push(' ');
            }
        }
        out.push_str(p);
    }
    out
}

/// "language English" with nothing after it means the model heard no speech.
fn strip_language_preamble(text: &str) -> &str {
    match text.strip_prefix("language ") {
        Some(rest) => rest
            .split_once(char::is_whitespace)
            .map_or("", |(_, t)| t.trim()),
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn post_cnn_matches_reference() {
        assert_eq!(post_cnn(100), 13);
        assert_eq!(post_cnn(1), 1);
        assert_eq!(post_cnn(0), 0);
        assert_eq!(post_cnn(50), 7);
    }

    #[test]
    fn windows_for_a_partial_last_chunk() {
        let w = audio_windows(250, 100, 800);
        // chunks of 100, 100, 50 frames → 13 + 13 + 7 outputs
        assert_eq!(w.valid_indices.len(), 33);
        assert_eq!(w.valid_indices[26], 26);
        assert_eq!(*w.valid_indices.last().unwrap(), 26 + 6);
        // a single 104-frame window covers all 33 outputs
        assert!(w.attn_bias.iter().all(|&b| b == 0.0));
    }

    #[test]
    fn windows_are_block_diagonal_past_eight_seconds() {
        let w = audio_windows(1000, 100, 800);
        let total = w.valid_indices.len();
        assert_eq!(total, 130);
        assert_eq!(w.attn_bias[0], 0.0);
        assert_eq!(w.attn_bias[103], 0.0);
        assert_eq!(w.attn_bias[104], NEG_INF);
        assert_eq!(w.attn_bias[104 * total + 104], 0.0);
    }

    #[test]
    fn causal_bias_offsets_by_past() {
        let b = causal_bias(2, 3);
        assert_eq!(
            b,
            vec![0.0, 0.0, 0.0, 0.0, NEG_INF, 0.0, 0.0, 0.0, 0.0, 0.0]
        );
    }

    #[test]
    fn short_audio_is_one_chunk() {
        assert_eq!(chunk_bounds(&vec![0.1; 16_000]), vec![(0, 16_000)]);
        assert_eq!(chunk_bounds(&[]), vec![(0, 0)]);
    }

    #[test]
    fn long_audio_is_cut_at_the_quiet_part() {
        let mut s = vec![0.5f32; 40 * 16_000];
        for x in &mut s[12 * 16_000..12 * 16_000 + 3_200] {
            *x = 0.0;
        }
        let b = chunk_bounds(&s);
        assert!(b.len() >= 3);
        assert!(b[0].1 > 12 * 16_000 && b[0].1 < 12 * 16_000 + 3_200);
        assert_eq!(b.last().unwrap().1, s.len());
        for (from, to) in &b {
            assert!(to - from <= 15 * 16_000);
        }
    }

    #[test]
    fn language_codes_resolve() {
        assert_eq!(language_code("tr"), Some("tr"));
        assert_eq!(language_code("zh-Hans"), Some("zh"));
        assert_eq!(language_code("pt-BR"), Some("pt"));
        assert_eq!(language_code("auto"), None);
        assert_eq!(LANGUAGES.len(), LANGUAGE_CODES.len());
    }

    #[test]
    fn preamble_only_output_is_empty() {
        assert_eq!(strip_language_preamble("language None"), "");
        assert_eq!(strip_language_preamble("language English hello"), "hello");
        assert_eq!(strip_language_preamble("hello"), "hello");
    }

    /// End-to-end run against a downloaded model:
    /// `QWEN3_ASR_DIR=… QWEN3_ASR_WAV=a.wav[:lang],b.wav [QWEN3_ASR_FP32=1] cargo test --features onnx
    ///  --lib qwen3_asr::tests::real_model -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_model() {
        let dir = std::env::var("QWEN3_ASR_DIR").expect("QWEN3_ASR_DIR");
        let quantization = if std::env::var("QWEN3_ASR_FP32").is_ok() {
            Quantization::FP32
        } else {
            Quantization::Int8
        };
        let mut model = Qwen3AsrModel::load(Path::new(&dir), &quantization).unwrap();
        for spec in std::env::var("QWEN3_ASR_WAV")
            .expect("QWEN3_ASR_WAV")
            .split(',')
        {
            let (path, lang) = match spec.split_once(':') {
                Some((p, l)) => (p, Some(l.to_string())),
                None => (spec, None),
            };
            let samples = crate::audio::read_wav_samples(Path::new(path)).unwrap();
            let t = Instant::now();
            let r = model
                .transcribe_with(
                    &samples,
                    &Qwen3AsrParams {
                        language: lang.clone(),
                    },
                )
                .unwrap();
            println!("[{path} {lang:?} {:.2?}] {}", t.elapsed(), r.text);
        }
    }

    #[test]
    fn chunks_join_with_spaces_except_cjk() {
        assert_eq!(join_chunks(&["a".into(), "b".into()]), "a b");
        assert_eq!(join_chunks(&["你好".into(), "世界".into()]), "你好世界");
    }
}
