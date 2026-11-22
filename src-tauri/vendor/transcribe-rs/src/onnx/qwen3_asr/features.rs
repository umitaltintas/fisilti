//! Whisper-style 128-bin log-mel front end, as used by Qwen3-ASR
//! (`WhisperFeatureExtractor`: centred reflect-padded STFT, periodic Hann
//! window, Slaney mel filters, log10, clamp to max − 8, `(x + 4) / 4`).

use ndarray::Array2;
use rustfft::{num_complex::Complex, FftPlanner};
use std::f32::consts::PI;

pub const SAMPLE_RATE: usize = 16_000;
pub const N_FFT: usize = 400;
pub const HOP_LENGTH: usize = 160;
pub const N_MELS: usize = 128;

/// Compute log-mel features of shape `[N_MELS, samples.len() / HOP_LENGTH]`.
///
/// Matches Whisper, which drops the final STFT frame, so a buffer whose length
/// is a multiple of `HOP_LENGTH` yields exactly `len / HOP_LENGTH` frames.
pub fn log_mel(samples: &[f32]) -> Array2<f32> {
    let n_frames = samples.len() / HOP_LENGTH;
    if n_frames == 0 {
        return Array2::zeros((N_MELS, 0));
    }

    let padded = reflect_pad(samples, N_FFT / 2);
    let window: Vec<f32> = (0..N_FFT)
        .map(|i| 0.5 * (1.0 - (2.0 * PI * i as f32 / N_FFT as f32).cos()))
        .collect();
    let filters = slaney_mel_filters();
    let n_bins = N_FFT / 2 + 1;

    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(N_FFT);
    let mut buf = vec![Complex::new(0.0f32, 0.0); N_FFT];
    let mut power = vec![0.0f32; n_bins];

    let mut mel = Array2::<f32>::zeros((N_MELS, n_frames));
    for t in 0..n_frames {
        let start = t * HOP_LENGTH;
        for i in 0..N_FFT {
            buf[i] = Complex::new(padded[start + i] * window[i], 0.0);
        }
        fft.process(&mut buf);
        for (p, c) in power.iter_mut().zip(buf.iter()) {
            *p = c.norm_sqr();
        }
        for m in 0..N_MELS {
            let row = filters.row(m);
            let e: f32 = row.iter().zip(power.iter()).map(|(w, p)| w * p).sum();
            mel[[m, t]] = e.max(1e-10).log10();
        }
    }

    let max = mel.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let floor = max - 8.0;
    mel.mapv_inplace(|v| (v.max(floor) + 4.0) / 4.0);
    mel
}

/// numpy `pad(mode="reflect")` (edge sample not repeated). Falls back to zero
/// padding for inputs too short to reflect.
fn reflect_pad(samples: &[f32], pad: usize) -> Vec<f32> {
    let n = samples.len();
    let mut out = Vec::with_capacity(n + 2 * pad);
    for i in (1..=pad).rev() {
        out.push(if i < n { samples[i] } else { 0.0 });
    }
    out.extend_from_slice(samples);
    for i in 0..pad {
        out.push(if n >= 2 + i { samples[n - 2 - i] } else { 0.0 });
    }
    out
}

fn hz_to_mel(f: f64) -> f64 {
    let f_sp = 200.0 / 3.0;
    let min_log_hz = 1000.0;
    let min_log_mel = min_log_hz / f_sp;
    let logstep = (6.4f64).ln() / 27.0;
    if f >= min_log_hz {
        min_log_mel + (f / min_log_hz).ln() / logstep
    } else {
        f / f_sp
    }
}

fn mel_to_hz(m: f64) -> f64 {
    let f_sp = 200.0 / 3.0;
    let min_log_hz = 1000.0;
    let min_log_mel = min_log_hz / f_sp;
    let logstep = (6.4f64).ln() / 27.0;
    if m >= min_log_mel {
        min_log_hz * (logstep * (m - min_log_mel)).exp()
    } else {
        f_sp * m
    }
}

/// librosa `mel(sr=16000, n_fft=400, n_mels=128, fmin=0, fmax=8000,
/// htk=False, norm="slaney")`, shape `[N_MELS, N_FFT / 2 + 1]`.
fn slaney_mel_filters() -> Array2<f32> {
    let n_bins = N_FFT / 2 + 1;
    let fmax = SAMPLE_RATE as f64 / 2.0;
    let fft_freqs: Vec<f64> = (0..n_bins)
        .map(|i| i as f64 * fmax / (n_bins - 1) as f64)
        .collect();
    let mel_max = hz_to_mel(fmax);
    let mel_f: Vec<f64> = (0..N_MELS + 2)
        .map(|i| mel_to_hz(mel_max * i as f64 / (N_MELS + 1) as f64))
        .collect();

    let mut weights = Array2::<f32>::zeros((N_MELS, n_bins));
    for m in 0..N_MELS {
        let lower_w = mel_f[m + 1] - mel_f[m];
        let upper_w = mel_f[m + 2] - mel_f[m + 1];
        let enorm = 2.0 / (mel_f[m + 2] - mel_f[m]);
        for (k, &f) in fft_freqs.iter().enumerate() {
            let lower = (f - mel_f[m]) / lower_w;
            let upper = (mel_f[m + 2] - f) / upper_w;
            let w = lower.min(upper).max(0.0);
            weights[[m, k]] = (w * enorm) as f32;
        }
    }
    weights
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_count_drops_the_last_stft_frame() {
        let mel = log_mel(&vec![0.0; 16_000]);
        assert_eq!(mel.dim(), (N_MELS, 100));
    }

    #[test]
    fn silence_is_flat_after_normalisation() {
        let mel = log_mel(&vec![0.0; 3_200]);
        let first = mel[[0, 0]];
        assert!(mel.iter().all(|&v| (v - first).abs() < 1e-6));
    }

    #[test]
    fn filters_cover_the_spectrum() {
        let f = slaney_mel_filters();
        for m in 0..N_MELS {
            assert!(f.row(m).iter().any(|&w| w > 0.0), "empty mel band {m}");
        }
    }

    #[test]
    fn reflect_pad_matches_numpy() {
        assert_eq!(
            reflect_pad(&[1.0, 2.0, 3.0, 4.0], 2),
            vec![3.0, 2.0, 1.0, 2.0, 3.0, 4.0, 3.0, 2.0]
        );
    }
}
