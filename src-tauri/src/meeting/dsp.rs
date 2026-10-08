// Mic conditioning, echo mitigation and the level-meter waveform.
//
// Pure signal processing on 16 kHz mono frames; no I/O, no app state.

// ---- Mic conditioning + echo mitigation (Items 3 & 5) ----------------------
//
// These run on the MIC frames ONLY (16 kHz mono), in this order each tick:
//   1. Echo duck  (Item 3): when output = speakers AND system audio is loud,
//      attenuate the mic so the remote party (already cleanly captured by the
//      system tap) isn't re-captured + duplicated in the transcript.
//   2. High-pass  (Item 5): ~80 Hz one-pole HPF to remove rumble/DC before
//      loudness measurement.
//   3. Loudness   (Item 5): EBU R128 shortterm normalization toward -23 LUFS.
// System audio and the saved mix are NOT touched by any of these.

/// Mic attenuation applied while ducking (echo-prone speaker output + loud
/// system audio). -15 dB ≈ ×0.178 linear. Chosen to strongly suppress leakage
/// without fully gating, so a person talking OVER the remote audio (double-talk)
/// is still partially captured rather than dropped entirely.
#[cfg(target_os = "macos")]
const ECHO_DUCK_GAIN_DB: f32 = -15.0;
/// System-audio running-RMS threshold above which we consider remote audio to
/// be "actively playing" and enable ducking. ~0.02 RMS on the 16 kHz system
/// stream — above ambient tap noise/silence, below normal speech level.
#[cfg(target_os = "macos")]
const ECHO_DUCK_SYS_RMS_THRESHOLD: f32 = 0.02;
/// Smoothing factor for the system running RMS (per mic-frame batch). Closer to
/// 1.0 = slower/steadier; 0.2 reacts within ~5 ticks (~0.5 s).
#[cfg(target_os = "macos")]
const ECHO_DUCK_RMS_SMOOTH: f32 = 0.2;
/// Target integrated loudness for mic normalization (EBU R128). -23 LUFS is the
/// EBU broadcast reference; Whisper was trained on roughly this level of speech.
#[cfg(target_os = "macos")]
const MIC_TARGET_LUFS: f64 = -23.0;
/// Clamp the normalization gain so a near-silent block isn't amplified into
/// noise (and a hot block isn't over-attenuated). ±12 dB.
#[cfg(target_os = "macos")]
const MIC_NORM_MAX_GAIN_DB: f64 = 12.0;
/// High-pass cutoff applied to the mic before normalization (Hz).
#[cfg(target_os = "macos")]
pub(super) const MIC_HIGHPASS_HZ: f32 = 80.0;

/// One-pole high-pass filter (DC/rumble removal). Stateful across frames.
#[cfg(target_os = "macos")]
pub(super) struct HighPass {
    alpha: f32,
    prev_in: f32,
    prev_out: f32,
}

#[cfg(target_os = "macos")]
impl HighPass {
    pub(super) fn new(cutoff_hz: f32, sample_rate: f32) -> Self {
        // Standard one-pole HPF coefficient.
        let rc = 1.0 / (2.0 * std::f32::consts::PI * cutoff_hz);
        let dt = 1.0 / sample_rate;
        let alpha = rc / (rc + dt);
        Self {
            alpha,
            prev_in: 0.0,
            prev_out: 0.0,
        }
    }

    pub(super) fn process(&mut self, samples: &mut [f32]) {
        for s in samples.iter_mut() {
            let x = *s;
            let y = self.alpha * (self.prev_out + x - self.prev_in);
            self.prev_in = x;
            self.prev_out = y;
            *s = y;
        }
    }
}

/// EBU R128 shortterm loudness normalization toward `MIC_TARGET_LUFS`. We feed
/// every mic frame into the meter, read the shortterm (3 s) loudness, and apply
/// a clamped gain. Using shortterm keeps it adaptive to a moving talker without
/// pumping on every sample.
#[cfg(target_os = "macos")]
pub(super) struct MicLoudnessNorm {
    meter: ebur128::EbuR128,
}

#[cfg(target_os = "macos")]
impl MicLoudnessNorm {
    pub(super) fn new(sample_rate: u32) -> Option<Self> {
        match ebur128::EbuR128::new(1, sample_rate, ebur128::Mode::S) {
            Ok(meter) => Some(Self { meter }),
            Err(e) => {
                log::warn!(
                    "meeting: failed to init EBU R128 meter: {}; mic norm disabled",
                    e
                );
                None
            }
        }
    }

    /// Feed + normalize a block of mic samples in place.
    pub(super) fn process(&mut self, samples: &mut [f32]) {
        if samples.is_empty() {
            return;
        }
        if self.meter.add_frames_f32(samples).is_err() {
            return;
        }
        // shortterm loudness needs ~3 s of audio; returns -inf / error early on.
        let loudness = match self.meter.loudness_shortterm() {
            Ok(l) if l.is_finite() => l,
            _ => return,
        };
        // Gain (dB) to reach target, clamped, then linearized.
        let gain_db =
            (MIC_TARGET_LUFS - loudness).clamp(-MIC_NORM_MAX_GAIN_DB, MIC_NORM_MAX_GAIN_DB);
        let gain = 10f64.powf(gain_db / 20.0) as f32;
        for s in samples.iter_mut() {
            *s = (*s * gain).clamp(-1.0, 1.0);
        }
    }
}

/// Echo / double-capture mitigation. On speaker output, the mic re-captures the
/// remote party (already captured by the system tap) → duplicated transcript.
/// We track a smoothed RMS of the SYSTEM frames; when output = speakers and the
/// system is loud, we attenuate the mic by `ECHO_DUCK_GAIN_DB`.
///
/// Double-talk tradeoff: when both the local user and remote audio are loud at
/// once, the local user's mic is also attenuated (~15 dB), so very quiet local
/// interjections over loud playback may be missed. This is the accepted cost of
/// preventing the (worse) duplicated/echoed transcript. Headphone output is
/// detected separately and skips ducking entirely.
#[cfg(target_os = "macos")]
pub(super) struct EchoDuck {
    /// Whether the current output route is echo-prone (speakers/unknown).
    enabled: bool,
    /// Smoothed system RMS.
    sys_rms: f32,
    duck_gain: f32,
}

#[cfg(target_os = "macos")]
impl EchoDuck {
    pub(super) fn new(route: crate::audio_toolkit::audio::OutputRoute) -> Self {
        use crate::audio_toolkit::audio::OutputRoute;
        let enabled = !matches!(route, OutputRoute::Headphones);
        log::info!(
            "meeting: echo duck {} (output route: {:?})",
            if enabled {
                "ENABLED"
            } else {
                "disabled (headphones)"
            },
            route
        );
        Self {
            enabled,
            sys_rms: 0.0,
            duck_gain: 10f32.powf(ECHO_DUCK_GAIN_DB / 20.0),
        }
    }

    /// Update the smoothed system RMS from this tick's system frames.
    pub(super) fn observe_system(&mut self, sys_frames: &[f32]) {
        if sys_frames.is_empty() {
            // Decay toward zero so a gap in system audio releases the duck.
            self.sys_rms *= 1.0 - ECHO_DUCK_RMS_SMOOTH;
            return;
        }
        let sum_sq: f32 = sys_frames.iter().map(|s| s * s).sum();
        let rms = (sum_sq / sys_frames.len() as f32).sqrt();
        self.sys_rms = ECHO_DUCK_RMS_SMOOTH * rms + (1.0 - ECHO_DUCK_RMS_SMOOTH) * self.sys_rms;
    }

    /// Whether the mic should currently be ducked.
    fn is_ducking(&self) -> bool {
        self.enabled && self.sys_rms > ECHO_DUCK_SYS_RMS_THRESHOLD
    }

    /// Attenuate mic samples in place if ducking is active. Returns whether the
    /// mic was ducked this tick (caller may skip feeding the mic VAD).
    pub(super) fn apply(&self, mic_frames: &mut [f32]) -> bool {
        if self.is_ducking() {
            for s in mic_frames.iter_mut() {
                *s *= self.duck_gain;
            }
            true
        } else {
            false
        }
    }
}

/// Downsample a window of mixed samples into a fixed-length oscilloscope trace
/// (averaging strided chunks, values in -1..1) and the peak absolute amplitude
/// (0..1). Returns a flat zero trace when there are no samples.
pub(super) fn downsample_wave(samples: &[f32], points: usize) -> (Vec<f32>, f32) {
    if samples.is_empty() || points == 0 {
        return (vec![0.0; points], 0.0);
    }
    let mut wave = Vec::with_capacity(points);
    let mut peak = 0.0f32;
    let len = samples.len();
    for p in 0..points {
        let start = p * len / points;
        let end = ((p + 1) * len / points).max(start + 1).min(len);
        let mut sum = 0.0f32;
        let mut count = 0u32;
        for &s in &samples[start..end] {
            sum += s;
            let a = s.abs();
            if a > peak {
                peak = a;
            }
            count += 1;
        }
        let avg = if count > 0 { sum / count as f32 } else { 0.0 };
        wave.push(avg.clamp(-1.0, 1.0));
    }
    (wave, peak.min(1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downsample_wave_empty_is_flat() {
        let (wave, peak) = downsample_wave(&[], 96);
        assert_eq!(wave.len(), 96);
        assert!(wave.iter().all(|&v| v == 0.0));
        assert_eq!(peak, 0.0);
    }

    #[test]
    fn downsample_wave_fixed_length_and_peak() {
        // 480 samples -> 96 points, peak should be the max abs amplitude.
        let samples: Vec<f32> = (0..480)
            .map(|i| if i % 2 == 0 { 0.5 } else { -0.5 })
            .collect();
        let (wave, peak) = downsample_wave(&samples, 96);
        assert_eq!(wave.len(), 96);
        assert!((peak - 0.5).abs() < 1e-6);
        // Averaging alternating +/-0.5 over each chunk -> near zero.
        assert!(wave.iter().all(|&v| v.abs() <= 0.5));
    }

    #[test]
    fn downsample_wave_clamps_and_bounds_peak() {
        let samples = vec![5.0f32, -5.0, 2.0, -2.0];
        let (wave, peak) = downsample_wave(&samples, 4);
        assert_eq!(wave.len(), 4);
        assert!(wave.iter().all(|&v| (-1.0..=1.0).contains(&v)));
        assert_eq!(peak, 1.0); // clamped to 1.0
    }

    #[test]
    fn downsample_wave_more_points_than_samples() {
        // Should not panic when points > samples.
        let samples = vec![0.1f32, 0.2, 0.3];
        let (wave, peak) = downsample_wave(&samples, 96);
        assert_eq!(wave.len(), 96);
        assert!(peak > 0.0);
    }
}
