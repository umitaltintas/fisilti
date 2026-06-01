// Audio mixer for meeting mode (Step 2), macOS-only.
//
// Combines a microphone stream and the Step-1 system-audio stream into a single
// 16 kHz mono f32 stream suitable for Fısıltı's transcription seam
// (`TranscriptionManager::transcribe(Vec<f32>)`).
//
// Design: resample-then-mix. Both sources are independently resampled to 16 kHz
// mono BEFORE entering the ring buffer, then mixed at 16 kHz. This is simpler
// than meetily's mix-at-48k-then-downsample and keeps the mixer rate-agnostic.
//
// The two structs below (`AudioMixerRingBuffer` and `ProfessionalAudioMixer`)
// are a trimmed port of meetily's `audio/pipeline.rs`. They depend only on std
// `VecDeque`/`Vec<f32>`, so they port cleanly. The clipping prevention is a
// proportional SOFT-SCALE (`sum / sum.abs()` when `|sum| > 1.0`), NOT a hard
// clamp, matching meetily.
//
// This file is isolated from Fısıltı's dictation flow; it never touches the
// `AudioRecordingManager` / `RecordingState` / `TranscriptionCoordinator`
// singletons. The mic is captured via an independent cpal input stream.

use std::collections::VecDeque;

use crate::audio_toolkit::constants::WHISPER_SAMPLE_RATE;

/// Mixing window length in milliseconds (at the 16 kHz mix rate).
const MIX_WINDOW_MS: f32 = 100.0;
/// Safety multiplier: max buffered audio per source before dropping oldest.
const MAX_BUFFER_MULTIPLIER: usize = 8;

/// Ring buffer for synchronized audio mixing.
///
/// Accumulates samples from the mic and system streams (already at the mix rate)
/// until aligned fixed-size windows can be extracted. Short windows are
/// zero-padded (silence) rather than held, and the oldest samples are dropped on
/// overflow. Ported from meetily's `AudioMixerRingBuffer`.
pub struct AudioMixerRingBuffer {
    mic_buffer: VecDeque<f32>,
    system_buffer: VecDeque<f32>,
    window_size_samples: usize,
    max_buffer_size: usize,
}

/// Which source a batch of samples belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MixSource {
    Microphone,
    System,
}

impl AudioMixerRingBuffer {
    /// Create a ring buffer sized for `sample_rate` (the mix rate, 16 kHz here).
    pub fn new(sample_rate: u32) -> Self {
        let window_size_samples = (sample_rate as f32 * MIX_WINDOW_MS / 1000.0) as usize;
        let max_buffer_size = window_size_samples * MAX_BUFFER_MULTIPLIER;

        log::info!(
            "Mixer ring buffer: window={}ms ({} samples), max={} samples @ {} Hz",
            MIX_WINDOW_MS,
            window_size_samples,
            max_buffer_size,
            sample_rate
        );

        Self {
            mic_buffer: VecDeque::with_capacity(max_buffer_size),
            system_buffer: VecDeque::with_capacity(max_buffer_size),
            window_size_samples,
            max_buffer_size,
        }
    }

    /// Push a batch of already-resampled mono samples for the given source.
    pub fn add_samples(&mut self, source: MixSource, samples: &[f32]) {
        match source {
            MixSource::Microphone => self.mic_buffer.extend(samples.iter().copied()),
            MixSource::System => self.system_buffer.extend(samples.iter().copied()),
        }

        // Drop oldest on overflow (keep only the most recent max_buffer_size).
        while self.mic_buffer.len() > self.max_buffer_size {
            self.mic_buffer.pop_front();
        }
        while self.system_buffer.len() > self.max_buffer_size {
            self.system_buffer.pop_front();
        }
    }

    /// True when at least one source has a full window available.
    pub fn can_mix(&self) -> bool {
        self.mic_buffer.len() >= self.window_size_samples
            || self.system_buffer.len() >= self.window_size_samples
    }

    /// Extract one aligned `(mic, system)` window, zero-padding the short side.
    /// Returns `None` if neither source has a full window yet.
    pub fn extract_window(&mut self) -> Option<(Vec<f32>, Vec<f32>)> {
        if !self.can_mix() {
            return None;
        }

        let mic_window = Self::drain_window(&mut self.mic_buffer, self.window_size_samples);
        let sys_window = Self::drain_window(&mut self.system_buffer, self.window_size_samples);

        Some((mic_window, sys_window))
    }

    /// Drain up to `window` samples, zero-padding (silence) if short.
    fn drain_window(buf: &mut VecDeque<f32>, window: usize) -> Vec<f32> {
        if buf.len() >= window {
            buf.drain(0..window).collect()
        } else if !buf.is_empty() {
            let available: Vec<f32> = buf.drain(..).collect();
            let mut padded = Vec::with_capacity(window);
            padded.extend_from_slice(&available);
            padded.resize(window, 0.0);
            padded
        } else {
            vec![0.0; window]
        }
    }

    /// Drain whatever remains in both buffers as a final (possibly short,
    /// zero-padded to equal length) window. Used to flush at end of capture.
    pub fn drain_remaining(&mut self) -> Option<(Vec<f32>, Vec<f32>)> {
        if self.mic_buffer.is_empty() && self.system_buffer.is_empty() {
            return None;
        }
        let len = self.mic_buffer.len().max(self.system_buffer.len());
        let mut mic: Vec<f32> = self.mic_buffer.drain(..).collect();
        let mut sys: Vec<f32> = self.system_buffer.drain(..).collect();
        mic.resize(len, 0.0);
        sys.resize(len, 0.0);
        Some((mic, sys))
    }
}

/// Simple audio mixer with proportional soft-scaling clipping prevention.
/// Ported from meetily's `ProfessionalAudioMixer::mix_window`.
pub struct ProfessionalAudioMixer;

impl ProfessionalAudioMixer {
    pub fn new() -> Self {
        Self
    }

    /// Sum mic + system per sample, then soft-scale to keep within ±1.0.
    /// If `|sum| > 1.0`, scale proportionally (`sum / sum.abs()`) instead of a
    /// hard clamp, which avoids the "radio break" distortion of hard clipping.
    pub fn mix_window(&self, mic_window: &[f32], sys_window: &[f32]) -> Vec<f32> {
        let max_len = mic_window.len().max(sys_window.len());
        let mut mixed = Vec::with_capacity(max_len);

        for i in 0..max_len {
            let mic = mic_window.get(i).copied().unwrap_or(0.0);
            let sys = sys_window.get(i).copied().unwrap_or(0.0);

            let sum = mic + sys;
            let sum_abs = sum.abs();
            let mixed_sample = if sum_abs > 1.0 { sum / sum_abs } else { sum };

            mixed.push(mixed_sample);
        }

        mixed
    }
}

impl Default for ProfessionalAudioMixer {
    fn default() -> Self {
        Self::new()
    }
}

/// Convenience wrapper bundling the ring buffer + mixer at 16 kHz.
pub struct MeetingMixer {
    ring: AudioMixerRingBuffer,
    mixer: ProfessionalAudioMixer,
}

impl MeetingMixer {
    /// Create a mixer producing output at Fısıltı's 16 kHz transcription rate.
    pub fn new() -> Self {
        Self {
            ring: AudioMixerRingBuffer::new(WHISPER_SAMPLE_RATE),
            mixer: ProfessionalAudioMixer::new(),
        }
    }

    /// Feed already-16kHz-resampled mono samples for a source.
    pub fn push(&mut self, source: MixSource, samples: &[f32]) {
        self.ring.add_samples(source, samples);
    }

    /// Drain all currently-available aligned windows, mixing each, and append
    /// the mixed samples to `out`.
    pub fn drain_into(&mut self, out: &mut Vec<f32>) {
        while self.ring.can_mix() {
            if let Some((mic, sys)) = self.ring.extract_window() {
                out.extend(self.mixer.mix_window(&mic, &sys));
            } else {
                break;
            }
        }
    }

    /// Flush any remaining buffered audio (end of capture) into `out`.
    pub fn flush_into(&mut self, out: &mut Vec<f32>) {
        self.drain_into(out);
        if let Some((mic, sys)) = self.ring.drain_remaining() {
            out.extend(self.mixer.mix_window(&mic, &sys));
        }
    }
}

impl Default for MeetingMixer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soft_scale_keeps_within_unit() {
        let mixer = ProfessionalAudioMixer::new();
        let out = mixer.mix_window(&[0.8, -0.9], &[0.8, -0.9]);
        assert!(out.iter().all(|&s| s.abs() <= 1.0 + 1e-6));
        // 0.8 + 0.8 = 1.6 -> 1.6/1.6 = 1.0
        assert!((out[0] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn no_scaling_within_unit() {
        let mixer = ProfessionalAudioMixer::new();
        let out = mixer.mix_window(&[0.2, 0.1], &[0.3, -0.2]);
        assert!((out[0] - 0.5).abs() < 1e-6);
        assert!((out[1] - (-0.1)).abs() < 1e-6);
    }

    #[test]
    fn zero_pads_short_window() {
        let mut rb = AudioMixerRingBuffer::new(16000); // window = 1600
        rb.add_samples(MixSource::Microphone, &[0.5; 1600]);
        rb.add_samples(MixSource::System, &[0.25; 100]);
        let (mic, sys) = rb.extract_window().unwrap();
        assert_eq!(mic.len(), 1600);
        assert_eq!(sys.len(), 1600);
        assert_eq!(sys[100], 0.0); // padded
        assert_eq!(sys[0], 0.25);
    }
}
