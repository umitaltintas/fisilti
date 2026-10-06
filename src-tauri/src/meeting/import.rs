// Importing a recording made elsewhere (a phone voice memo, a conference
// recording) as a meeting.
//
// This module only turns a file into what the meeting pipeline already works
// with — 16 kHz mono f32 samples plus a best guess at when it was recorded and
// what to call it. Transcription, persistence and the summary stay in
// `MeetingManager`, so an imported meeting is indistinguishable from a captured
// one once it is saved.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::{MetadataOptions, MetadataRevision, StandardTagKey};
use symphonia::core::probe::Hint;

use crate::audio_toolkit::audio::FrameResampler;
use crate::audio_toolkit::constants::WHISPER_SAMPLE_RATE;

/// Extensions the file picker offers. Everything here decodes through the
/// symphonia features enabled in Cargo.toml; Opus (WhatsApp voice notes) is
/// deliberately absent because symphonia has no Opus decoder.
pub const SUPPORTED_EXTENSIONS: &[&str] = &[
    "m4a", "mp4", "aac", "mp3", "wav", "aif", "aiff", "caf", "flac", "ogg", "oga", "mka", "mkv",
    "webm",
];

/// Longest recording we accept. The whole recording is held in memory at
/// 16 kHz (~230 MB per hour), and nobody's conference talk is longer.
pub const MAX_IMPORT_SECS: u64 = 4 * 60 * 60;

/// A decoded recording, ready for the meeting pipeline.
pub struct DecodedRecording {
    /// 16 kHz mono samples.
    pub samples: Vec<f32>,
    /// Recording time from the file's own metadata, when it carries one
    /// (iPhone Voice Memos and most recorders write a creation date).
    pub recorded_at_ms: Option<i64>,
}

impl DecodedRecording {
    pub fn duration_ms(&self) -> i64 {
        (self.samples.len() as u64 * 1000 / WHISPER_SAMPLE_RATE as u64) as i64
    }
}

/// Decode `path` to 16 kHz mono. `progress` receives 0..1 as the file is read
/// (only when the container reports its length); `cancel` is checked between
/// packets.
pub fn decode_file(
    path: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(f32),
) -> Result<DecodedRecording, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("Could not open the file: {}", e))?;
    let stream = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let mut probed = symphonia::default::get_probe()
        .format(
            &hint,
            stream,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|_| "This file format is not supported.".to_string())?;

    // Metadata may sit in the container header or in a probe-level revision
    // (ID3 tags in front of an MP3); check both.
    let mut recorded_at_ms = probed
        .metadata
        .get()
        .and_then(|m| m.current().and_then(recording_date));
    let mut format = probed.format;
    if recorded_at_ms.is_none() {
        recorded_at_ms = format.metadata().current().and_then(recording_date);
    }

    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| "The file contains no audio track.".to_string())?;
    let track_id = track.id;
    let sample_rate = track
        .codec_params
        .sample_rate
        .ok_or_else(|| "The file does not report its sample rate.".to_string())?;
    let total_frames = track.codec_params.n_frames;
    if let Some(frames) = total_frames {
        if frames / sample_rate as u64 > MAX_IMPORT_SECS {
            return Err(too_long_error());
        }
    }

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|_| "The audio codec in this file is not supported.".to_string())?;

    let max_samples = (MAX_IMPORT_SECS * WHISPER_SAMPLE_RATE as u64) as usize;
    let mut resampler = FrameResampler::new(
        sample_rate as usize,
        WHISPER_SAMPLE_RATE as usize,
        Duration::from_millis(30),
    );
    let mut samples: Vec<f32> = Vec::new();
    let mut mono: Vec<f32> = Vec::new();
    let mut buffer: Option<SampleBuffer<f32>> = None;
    let mut last_reported = -1.0f32;

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(CANCELLED.to_string());
        }
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break
            }
            // A chained stream changed parameters mid-file; what we have is
            // the recording.
            Err(SymphoniaError::ResetRequired) => break,
            Err(e) => return Err(format!("Failed to read the recording: {}", e)),
        };
        if packet.track_id() != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            // One corrupt packet should cost a few milliseconds, not the file.
            Err(SymphoniaError::DecodeError(e)) => {
                log::debug!("meeting import: skipping undecodable packet: {}", e);
                continue;
            }
            Err(e) => return Err(format!("Failed to decode the recording: {}", e)),
        };

        let spec = *decoded.spec();
        let channels = spec.channels.count().max(1);
        let needed = decoded.capacity() as u64;
        let buf = match &mut buffer {
            Some(b) if b.capacity() as u64 >= needed * channels as u64 => b,
            _ => buffer.insert(SampleBuffer::<f32>::new(needed, spec)),
        };
        buf.copy_interleaved_ref(decoded);
        downmix(buf.samples(), channels, &mut mono);
        resampler.push(&mono, |frame| samples.extend_from_slice(frame));

        if samples.len() > max_samples {
            return Err(too_long_error());
        }
        if let Some(total) = total_frames.filter(|&t| t > 0) {
            let done = (packet.ts() + packet.dur()) as f32 / total as f32;
            let done = done.clamp(0.0, 1.0);
            // ~100 updates over the whole file is plenty for a progress bar.
            if done - last_reported >= 0.01 {
                last_reported = done;
                progress(done);
            }
        }
    }
    resampler.finish(|frame| samples.extend_from_slice(frame));
    progress(1.0);

    Ok(DecodedRecording {
        samples,
        recorded_at_ms,
    })
}

/// Error text for a cancelled import; the UI treats it as "no error".
pub const CANCELLED: &str = "Import cancelled.";

fn too_long_error() -> String {
    format!(
        "The recording is longer than {} hours, which is more than can be imported.",
        MAX_IMPORT_SECS / 3600
    )
}

/// Average interleaved channels into one.
fn downmix(interleaved: &[f32], channels: usize, out: &mut Vec<f32>) {
    out.clear();
    if channels == 1 {
        out.extend_from_slice(interleaved);
        return;
    }
    let scale = 1.0 / channels as f32;
    out.extend(
        interleaved
            .chunks_exact(channels)
            .map(|frame| frame.iter().sum::<f32>() * scale),
    );
}

/// The recording date tag, when present and parseable.
fn recording_date(revision: &MetadataRevision) -> Option<i64> {
    revision
        .tags()
        .iter()
        .filter(|tag| {
            matches!(
                tag.std_key,
                Some(StandardTagKey::Date) | Some(StandardTagKey::OriginalDate)
            )
        })
        .find_map(|tag| parse_date_tag(&tag.value.to_string()))
}

/// Parse the date formats recorders actually write: RFC 3339
/// (`2026-10-06T09:15:00Z`), a space-separated variant, or a bare date. A bare
/// year is useless for "when was this meeting", so it is rejected.
fn parse_date_tag(raw: &str) -> Option<i64> {
    use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, TimeZone};
    let raw = raw.trim();
    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        return Some(dt.timestamp_millis());
    }
    for fmt in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M"] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(raw, fmt) {
            return Local
                .from_local_datetime(&naive)
                .earliest()
                .map(|dt| dt.timestamp_millis());
        }
    }
    let date = NaiveDate::parse_from_str(raw, "%Y-%m-%d").ok()?;
    Local
        .from_local_datetime(&date.and_hms_opt(0, 0, 0)?)
        .earliest()
        .map(|dt| dt.timestamp_millis())
}

/// When the recording STARTED, best guess first: the file's own date tag, else
/// the file's creation time, else its modification time minus the duration (a
/// recorder writes the file as it stops, so mtime marks the end).
pub fn recording_started_at(path: &Path, tagged_ms: Option<i64>, duration_ms: i64) -> i64 {
    if let Some(ms) = tagged_ms {
        return ms;
    }
    let to_ms = |t: std::time::SystemTime| {
        t.duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|d| d.as_millis() as i64)
    };
    let meta = std::fs::metadata(path).ok();
    let created = meta.as_ref().and_then(|m| m.created().ok()).and_then(to_ms);
    let modified = meta
        .as_ref()
        .and_then(|m| m.modified().ok())
        .and_then(to_ms);
    match (created, modified) {
        // A copied file gets a fresh creation time but often keeps the
        // original mtime, so the earlier of the two is the better witness.
        (Some(c), Some(m)) => c.min(m - duration_ms),
        (Some(c), None) => c,
        (None, Some(m)) => m - duration_ms,
        (None, None) => chrono::Utc::now().timestamp_millis() - duration_ms,
    }
}

/// A title for the imported meeting from its file name, or `None` when the
/// name is one a recorder made up ("New Recording 12", "REC_0042",
/// "20261006_101530") — those get the LLM auto-title instead.
pub fn title_from_path(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    let title = stem.replace(['_'], " ");
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() || is_generic_recording_name(&title) {
        None
    } else {
        Some(title)
    }
}

/// Words recorder apps put in default file names, in the languages we ship.
const GENERIC_NAME_WORDS: &[&str] = &[
    "new",
    "recording",
    "record",
    "rec",
    "audio",
    "aud",
    "voice",
    "memo",
    "note",
    "sound",
    "track",
    "untitled",
    "wa",
    "ptt",
    "yeni",
    "kayıt",
    "kaydı",
    "ses",
    "sesli",
    "not",
    "adsız",
    "nueva",
    "grabación",
    "nouvel",
    "enregistrement",
    "neue",
    "aufnahme",
];

fn is_generic_recording_name(name: &str) -> bool {
    name.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        // Digits carry no meaning in a recorder's name: counters, dates,
        // times, and suffixes like the "0001" in "WA0001".
        .map(|w| {
            w.chars()
                .filter(|c| !c.is_ascii_digit())
                .collect::<String>()
        })
        .filter(|w| !w.is_empty())
        .all(|w| GENERIC_NAME_WORDS.contains(&w.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn recorder_default_names_are_not_used_as_titles() {
        for name in [
            "New Recording 12.m4a",
            "Yeni Kayıt 3.m4a",
            "REC_0042.wav",
            "20261006_101530.mp3",
            "AUD-20261006-WA0001.m4a",
            "Voice Memo.m4a",
            "PTT-20261006-WA0007.ogg",
        ] {
            assert_eq!(title_from_path(&PathBuf::from(name)), None, "{}", name);
        }
    }

    #[test]
    fn a_meaningful_file_name_becomes_the_title() {
        assert_eq!(
            title_from_path(&PathBuf::from("/x/Rust Konferansı açılış.m4a")).as_deref(),
            Some("Rust Konferansı açılış")
        );
        assert_eq!(
            title_from_path(&PathBuf::from("team_sync_q4.mp3")).as_deref(),
            Some("team sync q4")
        );
    }

    #[test]
    fn date_tags_in_the_usual_shapes_parse() {
        assert_eq!(
            parse_date_tag("2026-10-06T09:15:00Z"),
            Some(1_791_278_100_000)
        );
        assert!(parse_date_tag("2026-10-06 09:15:00").is_some());
        assert!(parse_date_tag("2026-10-06").is_some());
        assert_eq!(parse_date_tag("2026"), None);
        assert_eq!(parse_date_tag("not a date"), None);
    }

    #[test]
    fn a_tagged_date_wins_over_file_times() {
        let path = PathBuf::from("/definitely/missing.m4a");
        assert_eq!(recording_started_at(&path, Some(42), 1000), 42);
    }

    #[test]
    fn stereo_is_averaged_to_mono() {
        let mut out = Vec::new();
        downmix(&[1.0, 0.0, 0.5, 0.5], 2, &mut out);
        assert_eq!(out, vec![0.5, 0.5]);
    }
}
