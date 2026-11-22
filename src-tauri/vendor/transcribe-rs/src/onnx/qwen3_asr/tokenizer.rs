//! Byte-level BPE vocabulary for Qwen3-ASR: decoding generated ids back to
//! text, plus the few prompt tokens that are looked up by string.

use std::collections::HashMap;
use std::path::Path;

use crate::TranscribeError;

/// Ids at or above this are Qwen special tokens (`<|im_end|>`, `<asr_text>`, …)
/// and never part of the transcript.
const FIRST_SPECIAL_ID: i64 = 151_643;

pub struct Vocab {
    by_id: HashMap<i64, String>,
    by_token: HashMap<String, i64>,
    byte_of_char: HashMap<char, u8>,
}

impl Vocab {
    pub fn load(path: &Path) -> Result<Self, TranscribeError> {
        if !path.exists() {
            return Err(TranscribeError::ModelNotFound(path.to_path_buf()));
        }
        let raw = std::fs::read_to_string(path)?;
        let by_token: HashMap<String, i64> = serde_json::from_str(&raw)?;
        let by_id = by_token.iter().map(|(t, &i)| (i, t.clone())).collect();
        let byte_of_char = bytes_to_unicode()
            .into_iter()
            .map(|(b, c)| (c, b))
            .collect();
        Ok(Self {
            by_id,
            by_token,
            byte_of_char,
        })
    }

    /// Decode generated ids, skipping special tokens.
    pub fn decode(&self, ids: &[i64]) -> String {
        let mut bytes = Vec::new();
        for &id in ids {
            if id >= FIRST_SPECIAL_ID {
                continue;
            }
            let Some(token) = self.by_id.get(&id) else {
                continue;
            };
            for ch in token.chars() {
                match self.byte_of_char.get(&ch) {
                    Some(&b) => bytes.push(b),
                    None => {
                        let mut tmp = [0u8; 4];
                        bytes.extend_from_slice(ch.encode_utf8(&mut tmp).as_bytes());
                    }
                }
            }
        }
        String::from_utf8_lossy(&bytes).trim().to_string()
    }

    /// Encode a short ASCII string by greedy longest-match over the vocabulary.
    /// Not the canonical BPE segmentation, but every piece is a real token and
    /// the sequence decodes to the same text, which is all a prompt hint needs.
    pub fn encode_greedy(&self, text: &str) -> Option<Vec<i64>> {
        let mapped: String = {
            let table = bytes_to_unicode();
            let lookup: HashMap<u8, char> = table.into_iter().collect();
            text.bytes().map(|b| lookup[&b]).collect()
        };
        let chars: Vec<char> = mapped.chars().collect();
        let mut out = Vec::new();
        let mut pos = 0;
        while pos < chars.len() {
            let mut found = None;
            for end in (pos + 1..=chars.len()).rev() {
                let piece: String = chars[pos..end].iter().collect();
                if let Some(&id) = self.by_token.get(&piece) {
                    found = Some((id, end));
                    break;
                }
            }
            let (id, end) = found?;
            out.push(id);
            pos = end;
        }
        Some(out)
    }
}

/// GPT-2's reversible byte → printable-unicode table.
fn bytes_to_unicode() -> Vec<(u8, char)> {
    let mut bs: Vec<u32> = (b'!' as u32..=b'~' as u32)
        .chain(0xA1..=0xAC)
        .chain(0xAE..=0xFF)
        .collect();
    let mut cs = bs.clone();
    let mut n = 0;
    for b in 0..256u32 {
        if !bs.contains(&b) {
            bs.push(b);
            cs.push(256 + n);
            n += 1;
        }
    }
    bs.into_iter()
        .zip(cs)
        .map(|(b, c)| (b as u8, char::from_u32(c).unwrap_or('?')))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_table_is_a_bijection() {
        let t = bytes_to_unicode();
        assert_eq!(t.len(), 256);
        let chars: std::collections::HashSet<char> = t.iter().map(|&(_, c)| c).collect();
        assert_eq!(chars.len(), 256);
        assert!(t.contains(&(b' ', 'Ġ')));
    }

    #[test]
    fn decodes_multibyte_text_and_skips_specials() {
        let by_token: HashMap<String, i64> = [
            ("Mer".to_string(), 1),
            ("haba".to_string(), 2),
            // "ı" (U+0131, bytes C4 B1) in byte-level form
            ("Ä±".to_string(), 3),
            ("Ġ".to_string(), 4),
        ]
        .into_iter()
        .collect();
        let vocab = Vocab {
            by_id: by_token.iter().map(|(t, &i)| (i, t.clone())).collect(),
            by_token,
            byte_of_char: bytes_to_unicode()
                .into_iter()
                .map(|(b, c)| (c, b))
                .collect(),
        };
        assert_eq!(vocab.decode(&[1, 2, 4, 3, 151_645]), "Merhaba ı");
        assert_eq!(vocab.encode_greedy("Merhaba"), Some(vec![1, 2]));
    }
}
