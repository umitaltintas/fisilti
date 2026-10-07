// MP3 encoding for meeting audio: the saved playback copy and the audio sent
// to cloud transcription. Speech at 16 kHz mono loses nothing a listener or a
// recognizer cares about at these bitrates, and the files are a tenth the size
// of the 16-bit WAV (a twentieth of the old 32-bit float WAV) — which is what
// made long meetings slow to open and slow to upload.

use std::mem::MaybeUninit;
use std::path::Path;

use mp3lame_encoder::{Bitrate, Builder, FlushNoGap, MonoPcm, Quality};

/// Bitrate of the saved playback copy: ~22 MB per hour of meeting.
pub const STORAGE_BITRATE: Bitrate = Bitrate::Kbps48;

/// Bitrate of audio uploaded for transcription. A notch above storage so the
/// recognizer never works from a noticeably degraded signal; still ~5x smaller
/// than the 16-bit WAV it replaces.
pub const UPLOAD_BITRATE: Bitrate = Bitrate::Kbps64;

/// Samples handed to LAME per call; bounds the scratch buffer for long audio.
const ENCODE_CHUNK: usize = 16_000 * 10;

/// Encode mono `samples` (nominally -1..1) at `sample_rate` into an MP3.
pub fn encode_mp3(samples: &[f32], sample_rate: u32, bitrate: Bitrate) -> Result<Vec<u8>, String> {
    if samples.is_empty() {
        return Ok(Vec::new());
    }
    let mut builder = Builder::new().ok_or("Failed to create the MP3 encoder")?;
    builder
        .set_num_channels(1)
        .map_err(|e| format!("MP3 encoder: channels: {}", e))?;
    builder
        .set_sample_rate(sample_rate)
        .map_err(|e| format!("MP3 encoder: sample rate: {}", e))?;
    builder
        .set_brate(bitrate)
        .map_err(|e| format!("MP3 encoder: bitrate: {}", e))?;
    builder
        .set_quality(Quality::Good)
        .map_err(|e| format!("MP3 encoder: quality: {}", e))?;
    let mut encoder = builder
        .build()
        .map_err(|e| format!("Failed to initialize the MP3 encoder: {}", e))?;

    // ~bitrate/8 bytes per second, plus slack for the last frames.
    let estimate = samples.len() / sample_rate.max(1) as usize * (bitrate as usize * 125) + 8192;
    let mut out: Vec<u8> = Vec::with_capacity(estimate);
    let mut scratch: Vec<MaybeUninit<u8>> = Vec::new();
    for chunk in samples.chunks(ENCODE_CHUNK) {
        scratch.resize(
            mp3lame_encoder::max_required_buffer_size(chunk.len()),
            MaybeUninit::uninit(),
        );
        let written = encoder
            .encode(MonoPcm(chunk), &mut scratch)
            .map_err(|e| format!("MP3 encoding failed: {}", e))?;
        out.extend(
            scratch[..written]
                .iter()
                .map(|b| unsafe { b.assume_init() }),
        );
    }
    scratch.resize(8192, MaybeUninit::uninit());
    let written = encoder
        .flush::<FlushNoGap>(&mut scratch)
        .map_err(|e| format!("MP3 encoding failed: {}", e))?;
    out.extend(
        scratch[..written]
            .iter()
            .map(|b| unsafe { b.assume_init() }),
    );
    Ok(out)
}

/// Encode `samples` and write them to `path` atomically (temp file + rename),
/// so a crash mid-write never leaves a truncated file at the final path.
pub fn write_mp3_file(
    path: &Path,
    samples: &[f32],
    sample_rate: u32,
    bitrate: Bitrate,
) -> Result<(), String> {
    let bytes = encode_mp3(samples, sample_rate, bitrate)?;
    let tmp = path.with_extension("mp3.tmp");
    std::fs::write(&tmp, &bytes).map_err(|e| format!("Failed to write {:?}: {}", tmp, e))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("Failed to move {:?} into place: {}", path, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_of_tone_encodes_to_a_small_valid_mp3() {
        let samples: Vec<f32> = (0..16_000)
            .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / 16_000.0).sin() * 0.5)
            .collect();
        let mp3 = encode_mp3(&samples, 16_000, STORAGE_BITRATE).unwrap();
        // MPEG frame sync: 11 set bits at the start of the first frame.
        assert_eq!(mp3[0], 0xFF);
        assert_eq!(mp3[1] & 0xE0, 0xE0);
        // 48 kbps ≈ 6 KB/s; far below the 32 KB of 16-bit PCM.
        assert!(mp3.len() < 10_000, "{} bytes", mp3.len());
    }

    #[test]
    fn empty_input_is_not_an_error() {
        assert!(encode_mp3(&[], 16_000, STORAGE_BITRATE).is_ok());
    }
}
