// MP3 encoding for meeting audio: the saved playback copy and the audio sent
// to cloud transcription. Speech at 16 kHz mono loses nothing a listener or a
// recognizer cares about at these bitrates, and the files are a tenth the size
// of the 16-bit WAV (a twentieth of the old 32-bit float WAV) — which is what
// made long meetings slow to open and slow to upload.

use std::io::Write;
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

/// Incremental MP3 encoder: feed mono samples in pieces, get MP3 bytes out.
/// Lets long audio be converted without holding all of it in memory.
pub struct Mp3Stream {
    encoder: mp3lame_encoder::Encoder,
    scratch: Vec<MaybeUninit<u8>>,
}

impl Mp3Stream {
    pub fn new(sample_rate: u32, bitrate: Bitrate) -> Result<Self, String> {
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
        let encoder = builder
            .build()
            .map_err(|e| format!("Failed to initialize the MP3 encoder: {}", e))?;
        Ok(Self {
            encoder,
            scratch: Vec::new(),
        })
    }

    /// Encode `samples` (mono, nominally -1..1) and append the bytes to `out`.
    pub fn push(&mut self, samples: &[f32], out: &mut impl Write) -> Result<(), String> {
        for chunk in samples.chunks(ENCODE_CHUNK) {
            self.scratch.resize(
                mp3lame_encoder::max_required_buffer_size(chunk.len()),
                MaybeUninit::uninit(),
            );
            let written = self
                .encoder
                .encode(MonoPcm(chunk), &mut self.scratch)
                .map_err(|e| format!("MP3 encoding failed: {}", e))?;
            self.emit(written, out)?;
        }
        Ok(())
    }

    /// Flush the encoder's last frames into `out`.
    pub fn finish(mut self, out: &mut impl Write) -> Result<(), String> {
        self.scratch.resize(8192, MaybeUninit::uninit());
        let written = self
            .encoder
            .flush::<FlushNoGap>(&mut self.scratch)
            .map_err(|e| format!("MP3 encoding failed: {}", e))?;
        self.emit(written, out)
    }

    fn emit(&self, written: usize, out: &mut impl Write) -> Result<(), String> {
        // SAFETY: LAME initialised the first `written` bytes.
        let bytes: Vec<u8> = self.scratch[..written]
            .iter()
            .map(|b| unsafe { b.assume_init() })
            .collect();
        out.write_all(&bytes)
            .map_err(|e| format!("Failed to write MP3 data: {}", e))
    }
}

/// Encode mono `samples` (nominally -1..1) at `sample_rate` into an MP3.
pub fn encode_mp3(samples: &[f32], sample_rate: u32, bitrate: Bitrate) -> Result<Vec<u8>, String> {
    if samples.is_empty() {
        return Ok(Vec::new());
    }
    // ~bitrate/8 bytes per second, plus slack for the last frames.
    let estimate = samples.len() / sample_rate.max(1) as usize * (bitrate as usize * 125) + 8192;
    let mut out: Vec<u8> = Vec::with_capacity(estimate);
    let mut stream = Mp3Stream::new(sample_rate, bitrate)?;
    stream.push(samples, &mut out)?;
    stream.finish(&mut out)?;
    Ok(out)
}

/// Convert a WAV file to MP3 a few seconds at a time, so even a
/// multi-hour recording never sits in memory whole. Multi-channel input is
/// averaged to mono. Written atomically (temp file + rename).
pub fn transcode_wav_file(wav: &Path, mp3: &Path, bitrate: Bitrate) -> Result<(), String> {
    let mut reader =
        hound::WavReader::open(wav).map_err(|e| format!("Cannot read {:?}: {}", wav, e))?;
    let spec = reader.spec();
    let channels = spec.channels.max(1) as usize;
    let tmp = mp3.with_extension("mp3.tmp");
    let file =
        std::fs::File::create(&tmp).map_err(|e| format!("Cannot create {:?}: {}", tmp, e))?;
    let mut out = std::io::BufWriter::new(file);
    let mut stream = Mp3Stream::new(spec.sample_rate, bitrate)?;

    let frame_budget = spec.sample_rate as usize * 30 * channels;
    let mut interleaved: Vec<f32> = Vec::with_capacity(frame_budget);
    let mut mono: Vec<f32> = Vec::with_capacity(frame_budget / channels);
    let mut flush = |interleaved: &mut Vec<f32>,
                     stream: &mut Mp3Stream,
                     out: &mut std::io::BufWriter<std::fs::File>| {
        mono.clear();
        mono.extend(
            interleaved
                .chunks_exact(channels)
                .map(|frame| frame.iter().sum::<f32>() / channels as f32),
        );
        interleaved.clear();
        stream.push(&mono, out)
    };

    match spec.sample_format {
        hound::SampleFormat::Float => {
            for sample in reader.samples::<f32>() {
                interleaved.push(sample.map_err(|e| format!("Corrupt WAV {:?}: {}", wav, e))?);
                if interleaved.len() >= frame_budget {
                    flush(&mut interleaved, &mut stream, &mut out)?;
                }
            }
        }
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample.max(1) - 1)) as f32;
            for sample in reader.samples::<i32>() {
                let value = sample.map_err(|e| format!("Corrupt WAV {:?}: {}", wav, e))?;
                interleaved.push(value as f32 * scale);
                if interleaved.len() >= frame_budget {
                    flush(&mut interleaved, &mut stream, &mut out)?;
                }
            }
        }
    }
    flush(&mut interleaved, &mut stream, &mut out)?;
    stream.finish(&mut out)?;
    out.flush()
        .map_err(|e| format!("Failed to write {:?}: {}", tmp, e))?;
    drop(out);
    std::fs::rename(&tmp, mp3).map_err(|e| format!("Failed to move {:?} into place: {}", mp3, e))
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
    fn a_float_wav_transcodes_to_mp3_on_disk() {
        let dir = std::env::temp_dir().join(format!("fisilti-mp3-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("in.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(&wav, spec).unwrap();
        for i in 0..48_000 {
            writer.write_sample((i as f32 * 0.05).sin() * 0.3).unwrap();
        }
        writer.finalize().unwrap();
        let mp3 = dir.join("out.mp3");
        transcode_wav_file(&wav, &mp3, STORAGE_BITRATE).unwrap();
        let size = std::fs::metadata(&mp3).unwrap().len();
        // 3 s at 48 kbps ≈ 18 KB, versus 192 KB of float WAV.
        assert!(size > 10_000 && size < 30_000, "{} bytes", size);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_input_is_not_an_error() {
        assert!(encode_mp3(&[], 16_000, STORAGE_BITRATE).is_ok());
    }
}
