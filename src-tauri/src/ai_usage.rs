//! Token accounting for the Gemini paths, and the cost estimate built from it.
//!
//! Two decisions shape this module.
//!
//! **We store tokens, not money.** Prices change — every model here is a
//! preview — and a stored dollar figure would quietly become a lie about a past
//! meeting. Token counts are a fact about what happened; the price table is a
//! snapshot applied at display time, so history stays honest when prices move.
//!
//! **The numbers come from the API, not from a stopwatch.** Both the Live API
//! and the Interactions API report `usageMetadata`, so we bill from what the
//! server says it processed rather than multiplying wall-clock minutes by a
//! published per-minute figure — which is itself only Google's estimate.
//!
//! What this produces is still an ESTIMATE, and callers must present it as one:
//! the price table is a snapshot, preview pricing moves, and free-tier quota,
//! taxes and billing-account discounts are invisible from here.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use specta::Type;

/// Tokens consumed by one model, split by direction.
///
/// The modality split is kept even though current pricing does not vary by it
/// within a single model: it costs nothing to record, and it is the difference
/// between "we can explain this number" and "trust us" when a translate session
/// (audio out) and a transcribe session (text out) sit side by side.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, Type)]
pub struct ModelUsage {
    /// Bare model id the work was spent on.
    pub model: String,
    /// Tokens the model consumed, when it reported them.
    pub input_tokens: u64,
    /// Tokens the model produced, when it reported them.
    pub output_tokens: u64,
    /// Seconds of audio streamed to this model.
    ///
    /// Measured on our side, and for the Live API it is the ONLY basis
    /// available: `gemini-3.5-transcribe-live` sends no `usageMetadata` at all
    /// — verified by enumerating every distinct message shape across whole
    /// meetings — so a token-only design silently reported nothing.
    #[serde(default)]
    pub audio_seconds: u64,
}

/// Everything a meeting spent, one entry per model involved.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, Type)]
pub struct MeetingUsage {
    pub entries: Vec<ModelUsage>,
}

impl MeetingUsage {
    /// Fold in another model's usage, merging into an existing entry for the
    /// same model so a meeting that reconnected six times still reads as one
    /// line per model.
    pub fn add(&mut self, model: &str, input_tokens: u64, output_tokens: u64) {
        self.add_with_audio(model, input_tokens, output_tokens, 0);
    }

    /// Fold in usage that also carries measured audio duration.
    pub fn add_with_audio(
        &mut self,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
        audio_seconds: u64,
    ) {
        if input_tokens == 0 && output_tokens == 0 && audio_seconds == 0 {
            return;
        }
        match self.entries.iter_mut().find(|e| e.model == model) {
            Some(entry) => {
                entry.input_tokens += input_tokens;
                entry.output_tokens += output_tokens;
                entry.audio_seconds += audio_seconds;
            }
            None => self.entries.push(ModelUsage {
                model: model.to_string(),
                input_tokens,
                output_tokens,
                audio_seconds,
            }),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Estimated cost in US dollars, and whether every model was priced.
    ///
    /// An unknown model contributes nothing rather than a guess, and flips
    /// `complete` to false so the UI can say the figure is partial instead of
    /// quietly under-reporting.
    pub fn estimate_usd(&self) -> CostEstimate {
        let mut usd = 0.0;
        let mut complete = true;
        for entry in &self.entries {
            match price_for(&entry.model) {
                Some(price) => {
                    // Prefer reported tokens; fall back to measured audio when
                    // the model reports none. Never both — that would bill the
                    // same audio twice.
                    if entry.input_tokens > 0 || entry.output_tokens > 0 {
                        usd += entry.input_tokens as f64 / 1_000_000.0 * price.input_per_mtok;
                        usd += entry.output_tokens as f64 / 1_000_000.0 * price.output_per_mtok;
                    } else {
                        let minutes = entry.audio_seconds as f64 / 60.0;
                        usd += minutes * (price.input_per_min + price.output_per_min);
                    }
                }
                None => complete = false,
            }
        }
        CostEstimate { usd, complete }
    }
}

/// A cost figure plus whether it covers everything that was spent.
#[derive(Clone, Debug, PartialEq, Serialize, Type)]
pub struct CostEstimate {
    pub usd: f64,
    /// False when at least one model had no entry in the price table, so the
    /// real cost is higher than `usd`.
    pub complete: bool,
}

/// US dollars per million tokens.
struct Price {
    input_per_mtok: f64,
    output_per_mtok: f64,
    /// Google's published per-minute-of-audio figures, used when the model
    /// reports no tokens.
    input_per_min: f64,
    output_per_min: f64,
}

/// Published paid-tier pricing, per million tokens.
///
/// Matched by PREFIX so a `-preview` or dated suffix on a model id still
/// prices. Getting a new model silently priced as $0 would be worse than
/// getting it slightly wrong: the estimate would look complete and be low.
fn price_for(model: &str) -> Option<Price> {
    let model = model.trim_start_matches("models/");
    // Longest-prefix first, so "…-live-translate" is not captured by a shorter
    // entry that happens to also match.
    // (prefix, $/Mtok in, $/Mtok out, $/min in, $/min out)
    const TABLE: &[(&str, f64, f64, f64, f64)] = &[
        ("gemini-3.5-live-translate", 3.50, 21.00, 0.0053, 0.0315),
        ("gemini-3.5-transcribe-live", 3.50, 21.00, 0.005, 0.004),
        ("gemini-3.5-transcribe", 2.00, 12.00, 0.003, 0.002),
    ];
    TABLE
        .iter()
        .find(|(prefix, ..)| model.starts_with(prefix))
        .map(
            |&(_, input_per_mtok, output_per_mtok, input_per_min, output_per_min)| Price {
                input_per_mtok,
                output_per_mtok,
                input_per_min,
                output_per_min,
            },
        )
}

/// Read `usageMetadata` out of an API message.
///
/// Returns `(input_tokens, output_tokens)`. `promptTokenCount` covers the audio
/// we sent; `responseTokenCount` what came back. Missing fields count as zero
/// rather than failing — usage is a nice-to-have on top of transcription, and
/// must never be able to break it.
pub fn usage_of(value: &Value) -> Option<(u64, u64)> {
    // Seen at the top level on some messages; checked inside `serverContent`
    // too because this API has already placed two other fields somewhere other
    // than where its own examples show them.
    let usage = value
        .get("usageMetadata")
        .or_else(|| value["serverContent"].get("usageMetadata"))?;
    let input = usage["promptTokenCount"].as_u64().unwrap_or(0);
    let output = usage["responseTokenCount"]
        .as_u64()
        // The Interactions API names the same idea differently.
        .or_else(|| usage["candidatesTokenCount"].as_u64())
        .unwrap_or(0);
    if input == 0 && output == 0 {
        return None;
    }
    Some((input, output))
}

/// Accumulates `usageMetadata` from a stream whose reporting style is unknown.
///
/// The Live API docs do not say whether `usageMetadata` is cumulative for the
/// session or per-message, and the two differ by orders of magnitude. Summing a
/// cumulative stream inflates the total quadratically; taking the maximum of a
/// per-message stream reports one message. So this decides from the data: a
/// counter that never decreases is treated as cumulative (keep the latest), and
/// one that fluctuates is treated as per-message (sum it).
#[derive(Debug, Default)]
pub struct UsageAccumulator {
    /// Running sum, used if the stream turns out to be per-message.
    summed_input: u64,
    summed_output: u64,
    /// Highest value seen, used if the stream is cumulative.
    peak_input: u64,
    peak_output: u64,
    /// Set once any reported value is lower than one already seen, which a
    /// cumulative counter cannot do.
    looks_per_message: bool,
    seen: bool,
}

impl UsageAccumulator {
    pub fn record(&mut self, input: u64, output: u64) {
        if self.seen && (input < self.peak_input || output < self.peak_output) {
            self.looks_per_message = true;
        }
        self.seen = true;
        self.summed_input += input;
        self.summed_output += output;
        self.peak_input = self.peak_input.max(input);
        self.peak_output = self.peak_output.max(output);
    }

    /// `(input, output)` for the whole stream.
    pub fn totals(&self) -> (u64, u64) {
        if self.looks_per_message {
            (self.summed_input, self.summed_output)
        } else {
            (self.peak_input, self.peak_output)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn usage_merges_into_one_entry_per_model() {
        let mut usage = MeetingUsage::default();
        // A meeting that reconnected several times must not read as several
        // separate charges for the same model.
        usage.add("gemini-3.5-transcribe-live", 100, 10);
        usage.add("gemini-3.5-transcribe-live", 50, 5);
        usage.add("gemini-3.5-transcribe", 200, 20);
        assert_eq!(usage.entries.len(), 2);
        assert_eq!(usage.entries[0].input_tokens, 150);
        assert_eq!(usage.entries[0].output_tokens, 15);
    }

    #[test]
    fn zero_usage_adds_nothing() {
        let mut usage = MeetingUsage::default();
        usage.add("gemini-3.5-transcribe", 0, 0);
        usage.add_with_audio("gemini-3.5-transcribe", 0, 0, 0);
        assert!(usage.is_empty());
    }

    #[test]
    fn cost_uses_per_million_token_pricing() {
        let mut usage = MeetingUsage::default();
        usage.add("gemini-3.5-transcribe", 1_000_000, 1_000_000);
        let estimate = usage.estimate_usd();
        assert!((estimate.usd - 14.00).abs() < 1e-9, "got {}", estimate.usd);
        assert!(estimate.complete);
    }

    #[test]
    fn a_preview_suffix_still_prices() {
        // Model ids carry -preview and dated suffixes; an exact-match table
        // would silently price the real model at zero.
        let mut usage = MeetingUsage::default();
        usage.add("gemini-3.5-live-translate-preview", 1_000_000, 0);
        let estimate = usage.estimate_usd();
        assert!((estimate.usd - 3.50).abs() < 1e-9, "got {}", estimate.usd);
        assert!(estimate.complete);
    }

    #[test]
    fn the_longer_prefix_wins() {
        // "gemini-3.5-transcribe-live" must not be priced by the cheaper
        // "gemini-3.5-transcribe" entry.
        let mut usage = MeetingUsage::default();
        usage.add("gemini-3.5-transcribe-live", 1_000_000, 0);
        assert!((usage.estimate_usd().usd - 3.50).abs() < 1e-9);
    }

    #[test]
    fn audio_duration_prices_a_model_that_reports_no_tokens() {
        // gemini-3.5-transcribe-live sends no usageMetadata at all, so without
        // this the whole feature silently reported nothing.
        let mut usage = MeetingUsage::default();
        usage.add_with_audio("gemini-3.5-transcribe-live", 0, 0, 600);
        // 10 minutes at $0.005 + $0.004 per minute.
        let estimate = usage.estimate_usd();
        assert!((estimate.usd - 0.09).abs() < 1e-9, "got {}", estimate.usd);
        assert!(estimate.complete);
    }

    #[test]
    fn reported_tokens_win_so_audio_is_never_billed_twice() {
        let mut usage = MeetingUsage::default();
        usage.add_with_audio("gemini-3.5-transcribe", 1_000_000, 0, 600);
        // Tokens only: $2.00, with no per-minute charge added on top.
        assert!((usage.estimate_usd().usd - 2.00).abs() < 1e-9);
    }

    #[test]
    fn audio_seconds_accumulate_across_reconnects() {
        let mut usage = MeetingUsage::default();
        usage.add_with_audio("gemini-3.5-transcribe-live", 0, 0, 300);
        usage.add_with_audio("gemini-3.5-transcribe-live", 0, 0, 300);
        assert_eq!(usage.entries.len(), 1);
        assert_eq!(usage.entries[0].audio_seconds, 600);
    }

    #[test]
    fn an_unpriced_model_marks_the_estimate_incomplete() {
        let mut usage = MeetingUsage::default();
        usage.add("gemini-3.5-transcribe", 1_000_000, 0);
        usage.add("some-future-model", 1_000_000, 0);
        let estimate = usage.estimate_usd();
        // It contributes nothing rather than a guess...
        assert!((estimate.usd - 2.00).abs() < 1e-9);
        // ...but the shortfall must be visible.
        assert!(!estimate.complete);
    }

    #[test]
    fn usage_metadata_is_read_from_either_field_name() {
        assert_eq!(
            usage_of(&json!({ "usageMetadata": {
                "promptTokenCount": 120, "responseTokenCount": 8
            }})),
            Some((120, 8))
        );
        // The Interactions API names the response side differently.
        assert_eq!(
            usage_of(&json!({ "usageMetadata": {
                "promptTokenCount": 5, "candidatesTokenCount": 3
            }})),
            Some((5, 3))
        );
        // Nested under serverContent, which is where some messages carry it.
        assert_eq!(
            usage_of(&json!({ "serverContent": { "usageMetadata": {
                "promptTokenCount": 7, "responseTokenCount": 2
            }}})),
            Some((7, 2))
        );
        assert_eq!(usage_of(&json!({})), None);
        assert_eq!(
            usage_of(&json!({ "usageMetadata": { "promptTokenCount": 0 } })),
            None
        );
    }

    #[test]
    fn a_cumulative_stream_is_not_double_counted() {
        // Values that only ever grow are one running total being restated.
        let mut acc = UsageAccumulator::default();
        acc.record(100, 10);
        acc.record(250, 25);
        acc.record(400, 40);
        assert_eq!(acc.totals(), (400, 40));
    }

    #[test]
    fn a_per_message_stream_is_summed() {
        // A counter that drops cannot be cumulative, so these are increments.
        let mut acc = UsageAccumulator::default();
        acc.record(100, 10);
        acc.record(80, 8);
        acc.record(120, 12);
        assert_eq!(acc.totals(), (300, 30));
    }

    #[test]
    fn a_single_report_is_taken_at_face_value() {
        let mut acc = UsageAccumulator::default();
        acc.record(42, 7);
        assert_eq!(acc.totals(), (42, 7));
        assert_eq!(UsageAccumulator::default().totals(), (0, 0));
    }
}
