// Raw capture buffers: the full per-source session audio streamed to disk as
// little-endian f32 (16 kHz mono), and the readers that consume it without
// ever loading a whole track. A forgotten seven-hour session is 1.6 GB per
// track; reading that whole, plus a copy for the encoder, is how stop used to
// need several times the audio's size in RAM.

use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use tauri::AppHandle;

const BYTES_PER_SAMPLE: u64 = std::mem::size_of::<f32>() as u64;

/// Directory the capture buffers live in: `{app_data}/meeting_buffers`.
///
/// Not `$TMPDIR`: macOS purges it on reboot (and periodically), which made
/// "recover after a crash" fail exactly when the crash was a reboot. Falls
/// back to the temp dir only when the app data dir cannot be resolved.
pub fn buffer_dir(app: &AppHandle) -> PathBuf {
    match crate::portable::app_data_dir(app) {
        Ok(dir) => {
            let dir = dir.join("meeting_buffers");
            match std::fs::create_dir_all(&dir) {
                Ok(()) => dir,
                Err(e) => {
                    log::warn!(
                        "meeting: cannot create {:?} ({}); buffering to the temp dir",
                        dir,
                        e
                    );
                    std::env::temp_dir()
                }
            }
        }
        Err(e) => {
            log::warn!(
                "meeting: app data dir unavailable ({}); buffering to the temp dir",
                e
            );
            std::env::temp_dir()
        }
    }
}

/// Number of samples in a raw buffer file, from its size alone.
pub fn sample_count(path: &Path) -> u64 {
    std::fs::metadata(path)
        .map(|m| m.len() / BYTES_PER_SAMPLE)
        .unwrap_or(0)
}

/// Appends raw little-endian f32 samples to a file. Remembers the FIRST write
/// error instead of dropping it: a full disk used to silently truncate the
/// recording, and the user found out only when the transcript stopped midway.
pub struct RawF32Writer {
    inner: BufWriter<File>,
    scratch: Vec<u8>,
    error: Option<String>,
    error_taken: bool,
}

impl RawF32Writer {
    pub fn create(path: &Path) -> std::io::Result<Self> {
        Ok(Self {
            inner: BufWriter::new(File::create(path)?),
            scratch: Vec::new(),
            error: None,
            error_taken: false,
        })
    }

    pub fn write(&mut self, samples: &[f32]) {
        if self.error.is_some() || samples.is_empty() {
            return;
        }
        self.scratch.clear();
        self.scratch.reserve(samples.len() * 4);
        for &s in samples {
            self.scratch.extend_from_slice(&s.to_le_bytes());
        }
        if let Err(e) = self.inner.write_all(&self.scratch) {
            self.error = Some(e.to_string());
        }
    }

    /// Push buffered bytes to the OS, so a crash loses at most what arrived
    /// since the last flush.
    pub fn flush(&mut self) {
        if self.error.is_some() {
            return;
        }
        if let Err(e) = self.inner.flush() {
            self.error = Some(e.to_string());
        }
    }

    /// The first write error, returned once.
    pub fn take_new_error(&mut self) -> Option<String> {
        if self.error_taken {
            return None;
        }
        let error = self.error.clone()?;
        self.error_taken = true;
        Some(error)
    }
}

/// 16 kHz mono audio to transcribe: an imported recording held in memory, or
/// a capture buffer on disk read a window at a time.
pub enum AudioSource<'a> {
    Memory(&'a [f32]),
    File { file: File, len: usize },
}

impl<'a> AudioSource<'a> {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let file = File::open(path)?;
        let len = (file.metadata()?.len() / BYTES_PER_SAMPLE) as usize;
        Ok(AudioSource::File { file, len })
    }

    /// Total samples.
    pub fn len(&self) -> usize {
        match self {
            AudioSource::Memory(samples) => samples.len(),
            AudioSource::File { len, .. } => *len,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Samples `[start, end)`, clamped to the source.
    pub fn read(&mut self, start: usize, end: usize) -> std::io::Result<Vec<f32>> {
        let end = end.min(self.len());
        let start = start.min(end);
        match self {
            AudioSource::Memory(samples) => Ok(samples[start..end].to_vec()),
            AudioSource::File { file, .. } => {
                file.seek(SeekFrom::Start(start as u64 * BYTES_PER_SAMPLE))?;
                let mut bytes = vec![0u8; (end - start) * BYTES_PER_SAMPLE as usize];
                file.read_exact(&mut bytes)?;
                Ok(bytes
                    .chunks_exact(4)
                    .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                    .collect())
            }
        }
    }

    /// Visit the whole source in consecutive chunks of at most `chunk`
    /// samples, in order. Stops at the first error.
    pub fn for_each_chunk(
        &mut self,
        chunk: usize,
        mut visit: impl FnMut(&[f32]),
    ) -> std::io::Result<()> {
        let len = self.len();
        let mut start = 0;
        while start < len {
            let end = (start + chunk.max(1)).min(len);
            let samples = self.read(start, end)?;
            visit(&samples);
            start = end;
        }
        Ok(())
    }
}

/// Encode a raw capture buffer as MP3 at `mp3`, streaming a few seconds at a
/// time. Written atomically (temp file + rename). Returns `Ok(false)` when
/// the buffer holds no audio (nothing is written).
pub fn write_mp3_from_raw(
    raw: &Path,
    mp3: &Path,
    bitrate: mp3lame_encoder::Bitrate,
) -> Result<bool, String> {
    use crate::audio_toolkit::constants::WHISPER_SAMPLE_RATE;
    use crate::audio_toolkit::mp3::Mp3Stream;

    let mut source = AudioSource::open(raw).map_err(|e| format!("Cannot read {:?}: {}", raw, e))?;
    if source.is_empty() {
        return Ok(false);
    }
    let tmp = mp3.with_extension("mp3.tmp");
    let result = (|| {
        let file = File::create(&tmp).map_err(|e| format!("Cannot create {:?}: {}", tmp, e))?;
        let mut out = BufWriter::new(file);
        let mut stream = Mp3Stream::new(WHISPER_SAMPLE_RATE, bitrate)?;
        let mut encode_error: Option<String> = None;
        source
            .for_each_chunk(WHISPER_SAMPLE_RATE as usize * 30, |chunk| {
                if encode_error.is_none() {
                    if let Err(e) = stream.push(chunk, &mut out) {
                        encode_error = Some(e);
                    }
                }
            })
            .map_err(|e| format!("Cannot read {:?}: {}", raw, e))?;
        if let Some(e) = encode_error {
            return Err(e);
        }
        stream.finish(&mut out)?;
        out.flush()
            .map_err(|e| format!("Failed to write {:?}: {}", tmp, e))?;
        drop(out);
        std::fs::rename(&tmp, mp3)
            .map_err(|e| format!("Failed to move {:?} into place: {}", mp3, e))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map(|()| true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "fisilti-buffers-test-{}-{}",
            std::process::id(),
            name
        ))
    }

    #[test]
    fn written_samples_read_back_by_range_and_chunk() {
        let path = temp_path("roundtrip.f32");
        let samples: Vec<f32> = (0..1000).map(|i| i as f32 / 1000.0).collect();
        let mut writer = RawF32Writer::create(&path).unwrap();
        writer.write(&samples[..400]);
        writer.write(&samples[400..]);
        writer.flush();
        assert!(writer.take_new_error().is_none());
        assert_eq!(sample_count(&path), 1000);

        let mut source = AudioSource::open(&path).unwrap();
        assert_eq!(source.len(), 1000);
        assert_eq!(source.read(10, 13).unwrap(), samples[10..13].to_vec());
        // Ranges past the end are clamped, not errors.
        assert_eq!(source.read(998, 5000).unwrap(), samples[998..].to_vec());

        let mut seen = Vec::new();
        source
            .for_each_chunk(300, |chunk| seen.extend_from_slice(chunk))
            .unwrap();
        assert_eq!(seen, samples);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn memory_and_file_sources_agree() {
        let samples = [0.5f32, -0.25, 0.125];
        let mut memory = AudioSource::Memory(&samples);
        assert_eq!(memory.read(1, 3).unwrap(), vec![-0.25, 0.125]);
        assert_eq!(memory.len(), 3);
    }

    #[test]
    fn an_empty_buffer_writes_no_mp3() {
        let raw = temp_path("empty.f32");
        std::fs::write(&raw, b"").unwrap();
        let mp3 = temp_path("empty.mp3");
        assert_eq!(
            write_mp3_from_raw(&raw, &mp3, crate::audio_toolkit::mp3::STORAGE_BITRATE),
            Ok(false)
        );
        assert!(!mp3.exists());
        let _ = std::fs::remove_file(&raw);
    }

    #[test]
    fn a_raw_buffer_streams_into_an_mp3() {
        let raw = temp_path("tone.f32");
        let mut writer = RawF32Writer::create(&raw).unwrap();
        let tone: Vec<f32> = (0..48_000).map(|i| (i as f32 * 0.05).sin() * 0.3).collect();
        writer.write(&tone);
        writer.flush();
        drop(writer);
        let mp3 = temp_path("tone.mp3");
        assert_eq!(
            write_mp3_from_raw(&raw, &mp3, crate::audio_toolkit::mp3::STORAGE_BITRATE),
            Ok(true)
        );
        let size = std::fs::metadata(&mp3).unwrap().len();
        assert!(size > 5_000 && size < 40_000, "{} bytes", size);
        let _ = std::fs::remove_file(&raw);
        let _ = std::fs::remove_file(&mp3);
    }
}
