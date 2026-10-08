// Text-level clean-up of finalize output: de-duplicating the overlap between
// consecutive windows, and dropping mic segments that are only acoustic echo
// of the system stream.

use super::manager::{TranscriptSegment, TranscriptSource};

/// Normalize a string for fuzzy text comparison: lowercase, strip punctuation,
/// collapse whitespace. Used by `dedup_overlap` so casing/punctuation drift
/// between two whisper passes over the same audio doesn't defeat the match.
#[cfg(target_os = "macos")]
fn normalize_for_match(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = true;
    for c in s.chars() {
        if c.is_alphanumeric() {
            for lc in c.to_lowercase() {
                out.push(lc);
            }
            last_space = false;
        } else if c.is_whitespace() || !c.is_alphanumeric() {
            if !last_space {
                out.push(' ');
                last_space = true;
            }
        }
    }
    out.trim().to_string()
}

/// Split text into sentences on `. ! ? …` boundaries, keeping the delimiter with
/// the sentence. Trailing fragment (no terminal punctuation) is its own piece.
#[cfg(target_os = "macos")]
fn split_sentences(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in s.chars() {
        cur.push(c);
        if matches!(c, '.' | '!' | '?' | '…') {
            let trimmed = cur.trim();
            if !trimmed.is_empty() {
                out.push(trimmed.to_string());
            }
            cur.clear();
        }
    }
    let trimmed = cur.trim();
    if !trimmed.is_empty() {
        out.push(trimmed.to_string());
    }
    out
}

/// De-duplicate the overlapping region between two consecutive finalize windows
/// (Item 6). `prev` is the previous window's full text, `curr` the current
/// window's full text; the windows overlap by ~`FINALIZE_OVERLAP_SAMPLES`, so
/// `curr`'s leading sentence(s) often repeat `prev`'s trailing sentence(s).
///
/// Strategy: take the last few sentences of `prev` as a "tail set" (normalized)
/// and drop leading sentences of `curr` while they match something in that set.
/// Falls back to a word-level longest-common-prefix trim when sentence matching
/// finds nothing (e.g. one long unpunctuated window). Conservative: when in
/// doubt it keeps text (a rare duplicate is less bad than dropping real words).
#[cfg(target_os = "macos")]
pub(super) fn dedup_overlap(prev: &str, curr: &str) -> String {
    let prev_sentences = split_sentences(prev);
    let curr_sentences = split_sentences(curr);
    if prev_sentences.is_empty() || curr_sentences.is_empty() {
        return curr.to_string();
    }

    // Normalized trailing sentences of `prev` (look back a handful).
    let tail_n = prev_sentences.len().min(4);
    let prev_tail_norm: Vec<String> = prev_sentences[prev_sentences.len() - tail_n..]
        .iter()
        .map(|s| normalize_for_match(s))
        .filter(|s| !s.is_empty())
        .collect();

    // Drop leading `curr` sentences that match any normalized prev-tail sentence.
    let mut start_idx = 0;
    for (i, sent) in curr_sentences.iter().enumerate() {
        let norm = normalize_for_match(sent);
        if norm.is_empty() {
            start_idx = i + 1;
            continue;
        }
        let is_dup = prev_tail_norm
            .iter()
            .any(|p| p == &norm || (norm.len() > 8 && p.contains(&norm)));
        if is_dup {
            start_idx = i + 1;
        } else {
            break;
        }
    }

    if start_idx > 0 {
        return curr_sentences[start_idx..].join(" ");
    }

    // No sentence-level match: try a word-level common-prefix trim against the
    // prev tail (handles long unpunctuated windows). Only trims when a
    // reasonably long run matches, to avoid eating distinct repeated words.
    let prev_norm_words: Vec<&str> = prev.split_whitespace().collect::<Vec<_>>();
    let prev_tail_words: Vec<String> = prev_norm_words
        .iter()
        .rev()
        .take(40)
        .rev()
        .map(|w| normalize_for_match(w))
        .filter(|w| !w.is_empty())
        .collect();
    let curr_words: Vec<&str> = curr.split_whitespace().collect();
    let curr_norm: Vec<String> = curr_words.iter().map(|w| normalize_for_match(w)).collect();

    // Find the longest k such that curr's first k words appear as a contiguous
    // run at the end of prev's tail.
    let mut best_k = 0;
    let max_k = curr_norm.len().min(prev_tail_words.len());
    for k in (4..=max_k).rev() {
        let head = &curr_norm[..k];
        if prev_tail_words.len() >= k && &prev_tail_words[prev_tail_words.len() - k..] == head {
            best_k = k;
            break;
        }
    }
    if best_k > 0 {
        return curr_words[best_k..].join(" ");
    }

    curr.to_string()
}

/// Maximum time gap (ms) between a mic segment and a system segment for the mic
/// one to be considered a possible acoustic echo of the system one. Finalize
/// windows are ~25-30 s and the two sources are segmented independently, so the
/// same spoken passage can land in windows whose start timestamps differ by up
/// to roughly one window length.
#[cfg(target_os = "macos")]
const CROSS_ECHO_MAX_GAP_MS: u64 = 35_000;
/// Minimum normalized-token count for a mic segment to be eligible for
/// cross-channel echo removal. Short utterances (e.g. "evet", "tamam") are NOT
/// dropped even if they appear on both sides — a genuine local agreement with
/// the remote party shouldn't be erased; only substantial verbatim copies are.
#[cfg(target_os = "macos")]
const CROSS_ECHO_MIN_TOKENS: usize = 6;
/// Fraction of a mic segment's tokens that must also appear (in order, as a
/// contiguous run) inside a nearby system segment for it to count as echo.
#[cfg(target_os = "macos")]
const CROSS_ECHO_CONTAINMENT: f32 = 0.8;

/// CROSS-CHANNEL echo removal (complements the live echo duck + buffer zeroing).
/// On speaker output the mic re-captures the remote party; the duck attenuates
/// and the finalize buffer is zeroed while ducking, but residual leakage can
/// survive around the duck's attack/release edges (and entirely when the output
/// route is misdetected). This is a TEXT-level safety net: drop any `Mic`
/// ("you") segment whose normalized tokens are largely contained, as a
/// contiguous run, in a `System` ("others") segment occurring within
/// `CROSS_ECHO_MAX_GAP_MS`. The system tap is the clean source, so the mic copy
/// is the echo and is the one removed. Conservative by design (high containment
/// threshold + minimum token count + time gate) so genuine local speech that
/// merely echoes a phrase isn't erased.
#[cfg(target_os = "macos")]
pub(super) fn drop_cross_channel_echo(segments: Vec<TranscriptSegment>) -> Vec<TranscriptSegment> {
    // Pre-tokenize the system ("others") segments once.
    let system: Vec<(u64, Vec<String>)> = segments
        .iter()
        .filter(|s| s.source == TranscriptSource::System)
        .map(|s| {
            (
                s.timestamp_ms,
                normalize_for_match(&s.text)
                    .split_whitespace()
                    .map(|w| w.to_string())
                    .collect::<Vec<_>>(),
            )
        })
        .collect();

    segments
        .into_iter()
        .filter(|seg| {
            if seg.source != TranscriptSource::Mic {
                return true; // never drop system segments
            }
            let mic_tokens: Vec<String> = normalize_for_match(&seg.text)
                .split_whitespace()
                .map(|w| w.to_string())
                .collect();
            if mic_tokens.len() < CROSS_ECHO_MIN_TOKENS {
                return true; // too short to confidently call echo
            }
            // Keep the mic segment unless some nearby system segment contains a
            // long-enough contiguous run of its tokens.
            let is_echo = system.iter().any(|(sys_ts, sys_tokens)| {
                let gap = seg.timestamp_ms.abs_diff(*sys_ts);
                gap <= CROSS_ECHO_MAX_GAP_MS
                    && longest_contiguous_run(&mic_tokens, sys_tokens) as f32
                        >= CROSS_ECHO_CONTAINMENT * mic_tokens.len() as f32
            });
            !is_echo
        })
        .collect()
}

/// Longest run of `needle` tokens that appears as a contiguous subsequence of
/// `haystack`. Used to detect a near-verbatim echo copy regardless of where in
/// the (longer) system segment it sits.
#[cfg(target_os = "macos")]
fn longest_contiguous_run(needle: &[String], haystack: &[String]) -> usize {
    if needle.is_empty() || haystack.is_empty() {
        return 0;
    }
    let mut best = 0usize;
    // For each possible alignment of needle's start within haystack, count how
    // far they match contiguously.
    for start in 0..haystack.len() {
        let mut run = 0usize;
        while run < needle.len()
            && start + run < haystack.len()
            && needle[run] == haystack[start + run]
        {
            run += 1;
        }
        if run > best {
            best = run;
            if best == needle.len() {
                break;
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "macos")]
    #[test]
    fn dedup_overlap_drops_repeated_leading_sentence() {
        use super::dedup_overlap;
        let prev = "Bugün hava çok güzel. Toplantıya başlayalım.";
        // curr repeats the trailing sentence of prev (with casing drift), then
        // adds new content.
        let curr = "toplantıya başlayalım. Gündem maddesi bir.";
        let out = dedup_overlap(prev, curr);
        assert!(
            !out.to_lowercase().contains("başlayalım"),
            "repeated sentence should be dropped, got: {out}"
        );
        assert!(
            out.contains("Gündem maddesi bir."),
            "new content kept: {out}"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn dedup_overlap_keeps_distinct_text() {
        use super::dedup_overlap;
        let prev = "Birinci konu tamamlandı.";
        let curr = "İkinci konuya geçiyoruz.";
        let out = dedup_overlap(prev, curr);
        assert_eq!(out, curr, "distinct text must be preserved");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn dedup_overlap_word_level_prefix_trim() {
        use super::dedup_overlap;
        // No sentence punctuation; word-level common run at boundary.
        let prev = "alpha beta gamma delta epsilon zeta eta theta";
        let curr = "epsilon zeta eta theta iota kappa lambda mu";
        let out = dedup_overlap(prev, curr);
        assert!(out.starts_with("iota"), "overlap words trimmed, got: {out}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn cross_channel_echo_drops_mic_copy_of_nearby_system_segment() {
        use super::{drop_cross_channel_echo, TranscriptSegment, TranscriptSource};
        let system_text =
            "bu çeyrekte gelirimiz beklentilerin üzerinde gerçekleşti ve büyümeye devam ediyoruz";
        let segs = vec![
            TranscriptSegment {
                text: system_text.to_string(),
                timestamp_ms: 1_000,
                source: TranscriptSource::System,
                translation: None,
                speaker: None,
            },
            // Mic picked up the same remote speech (slight ASR drift) ~2 s later.
            TranscriptSegment {
                text: "bu çeyrekte gelirimiz beklentilerin üzerinde gerçekleşti ve büyümeye devam ediyoruz".to_string(),
                timestamp_ms: 3_000,
                source: TranscriptSource::Mic,
                translation: None,
                speaker: None,
            },
        ];
        let out = drop_cross_channel_echo(segs);
        assert_eq!(out.len(), 1, "mic echo should be removed");
        assert_eq!(out[0].source, TranscriptSource::System);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn cross_channel_echo_keeps_genuine_local_speech() {
        use super::{drop_cross_channel_echo, TranscriptSegment, TranscriptSource};
        let segs = vec![
            TranscriptSegment {
                text: "satış rakamlarını üçüncü çeyrek için paylaşabilir misin lütfen".to_string(),
                timestamp_ms: 1_000,
                source: TranscriptSource::System,
                translation: None,
                speaker: None,
            },
            // Distinct local reply — not an echo, must be kept.
            TranscriptSegment {
                text: "tabii hemen ekranı paylaşıp grafikleri gösteriyorum".to_string(),
                timestamp_ms: 4_000,
                source: TranscriptSource::Mic,
                translation: None,
                speaker: None,
            },
        ];
        let out = drop_cross_channel_echo(segs);
        assert_eq!(out.len(), 2, "distinct local speech must be preserved");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn cross_channel_echo_keeps_short_shared_phrases() {
        use super::{drop_cross_channel_echo, TranscriptSegment, TranscriptSource};
        let segs = vec![
            TranscriptSegment {
                text: "evet kesinlikle".to_string(),
                timestamp_ms: 1_000,
                source: TranscriptSource::System,
                translation: None,
                speaker: None,
            },
            // Short agreement on both sides is below the token floor → kept.
            TranscriptSegment {
                text: "evet kesinlikle".to_string(),
                timestamp_ms: 2_000,
                source: TranscriptSource::Mic,
                translation: None,
                speaker: None,
            },
        ];
        let out = drop_cross_channel_echo(segs);
        assert_eq!(out.len(), 2, "short shared phrases must not be dropped");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn cross_channel_echo_keeps_distant_match() {
        use super::{drop_cross_channel_echo, TranscriptSegment, TranscriptSource};
        let text =
            "bu çeyrekte gelirimiz beklentilerin üzerinde gerçekleşti ve büyümeye devam ediyoruz";
        let segs = vec![
            TranscriptSegment {
                text: text.to_string(),
                timestamp_ms: 1_000,
                source: TranscriptSource::System,
                translation: None,
                speaker: None,
            },
            // Same words but far apart in time (>35 s) → treated as a real repeat,
            // not acoustic echo, so kept.
            TranscriptSegment {
                text: text.to_string(),
                timestamp_ms: 90_000,
                source: TranscriptSource::Mic,
                translation: None,
                speaker: None,
            },
        ];
        let out = drop_cross_channel_echo(segs);
        assert_eq!(
            out.len(),
            2,
            "matches outside the echo time window are kept"
        );
    }
}
