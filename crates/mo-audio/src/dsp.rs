//! Signal-processing building blocks: FFT, windows, spectrograms, mel filters and resampling. A port of the C# app's
//! `Sounds/Dsp.cs`, step for step, so an analysis comes out the same as the C# one (the twin test compares them).

use std::f64::consts::PI;

/// In-place radix-2 complex FFT. `re` and `im` must be the same length, a power of two; anything else is a mistake in
/// the caller (a debug build stops on it; a release build leaves the numbers as they are).
pub fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    debug_assert!(
        n == im.len() && (n == 0 || n.is_power_of_two()),
        "fft needs two lists of the same power-of-two length"
    );
    if n != im.len() || !n.is_power_of_two() {
        return;
    }
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * PI / len as f64;
        let (wr, wi) = (ang.cos(), ang.sin());
        for i in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0, 0.0);
            for k in 0..len / 2 {
                let a = i + k;
                let b = a + len / 2;
                let tr = re[b] * cr - im[b] * ci;
                let ti = re[b] * ci + im[b] * cr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let nr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = nr;
            }
        }
        len <<= 1;
    }
}

/// Periodic Hann window (as numpy, librosa and transformers use for spectrograms).
pub fn hann(n: usize) -> Vec<f64> {
    (0..n).map(|i| 0.5 - 0.5 * (2.0 * PI * i as f64 / n as f64).cos()).collect()
}

/// Power spectrogram: one row of `n_fft / 2 + 1` bins per frame. With `center`, frames are centred with reflect
/// padding, matching librosa and Hugging Face feature extractors. `n_fft` must be a power of two.
pub fn power_spectrogram(x: &[f32], n_fft: usize, hop: usize, center: bool) -> Vec<Vec<f32>> {
    let window = hann(n_fft);
    let padded;
    let x = if center {
        padded = reflect_pad(x, n_fft / 2);
        padded.as_slice()
    } else {
        x
    };
    let hop = hop.max(1);
    let frames = if x.len() < n_fft { 0 } else { 1 + (x.len() - n_fft) / hop };
    let bins = n_fft / 2 + 1;
    let mut re = vec![0.0; n_fft];
    let mut im = vec![0.0; n_fft];
    let mut spec = Vec::with_capacity(frames);
    for f in 0..frames {
        let frame = &x[f * hop..f * hop + n_fft];
        for ((r, &s), w) in re.iter_mut().zip(frame).zip(&window) {
            *r = f64::from(s) * w;
        }
        im.fill(0.0);
        fft(&mut re, &mut im);
        spec.push(re.iter().zip(&im).take(bins).map(|(r, i)| (r * r + i * i) as f32).collect());
    }
    spec
}

fn reflect_pad(x: &[f32], pad: usize) -> Vec<f32> {
    let mut y = vec![0.0; x.len() + 2 * pad];
    y[pad..pad + x.len()].copy_from_slice(x);
    if x.len() < 2 {
        return y;
    }
    let n = x.len() as i64;
    for i in 0..pad {
        y[pad - 1 - i] = x[reflect(i as i64 + 1, n)];
        y[pad + x.len() + i] = x[reflect(n - 2 - i as i64, n)];
    }
    y
}

fn reflect(i: i64, n: i64) -> usize {
    let period = 2 * (n - 1);
    let i = i.rem_euclid(period);
    (if i < n { i } else { period - i }) as usize
}

/// How hertz map to mels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MelScale {
    Htk,
    Slaney,
}

const MIN_LOG_HZ: f64 = 1000.0;
const MIN_LOG_MEL: f64 = 15.0;
const LOG_STEP: f64 = 27.0;

pub fn hz_to_mel(hz: f64, scale: MelScale) -> f64 {
    match scale {
        MelScale::Htk => 2595.0 * (1.0 + hz / 700.0).log10(),
        MelScale::Slaney => {
            let step_log = 6.4f64.ln() / LOG_STEP;
            if hz >= MIN_LOG_HZ { MIN_LOG_MEL + (hz / MIN_LOG_HZ).ln() / step_log } else { 3.0 * hz / 200.0 }
        }
    }
}

pub fn mel_to_hz(mel: f64, scale: MelScale) -> f64 {
    match scale {
        MelScale::Htk => 700.0 * (10f64.powf(mel / 2595.0) - 1.0),
        MelScale::Slaney => {
            let step_log = 6.4f64.ln() / LOG_STEP;
            if mel >= MIN_LOG_MEL { MIN_LOG_HZ * (step_log * (mel - MIN_LOG_MEL)).exp() } else { 200.0 * mel / 3.0 }
        }
    }
}

/// A triangular mel filter bank (Hugging Face `mel_filter_bank`): each mel band's weights over the spectrum's bins.
#[derive(Clone, Debug, PartialEq)]
pub struct MelFilters {
    pub bins: usize,
    pub mels: usize,
    /// For each band, the first bin it covers and its weights from there; the bins outside weigh nothing.
    pub bands: Vec<(usize, Vec<f64>)>,
}

impl MelFilters {
    pub fn new(
        bins: usize,
        mels: usize,
        f_min: f64,
        f_max: f64,
        sample_rate: u32,
        scale: MelScale,
        slaney_norm: bool,
    ) -> MelFilters {
        let mel_min = hz_to_mel(f_min, scale);
        let mel_max = hz_to_mel(f_max, scale);
        let freqs: Vec<f64> = (0..mels + 2)
            .map(|i| mel_to_hz(mel_min + (mel_max - mel_min) * i as f64 / (mels + 1) as f64, scale))
            .collect();
        // C# divides the rate by 2 in whole numbers first.
        let nyquist = f64::from(sample_rate / 2);
        let bands = (0..mels)
            .map(|m| {
                let weights: Vec<f64> = (0..bins)
                    .map(|k| {
                        let f = nyquist * k as f64 / bins.saturating_sub(1).max(1) as f64;
                        let down = (f - freqs[m]) / (freqs[m + 1] - freqs[m]);
                        let up = (freqs[m + 2] - f) / (freqs[m + 2] - freqs[m + 1]);
                        let v = down.min(up).max(0.0);
                        if slaney_norm { v * 2.0 / (freqs[m + 2] - freqs[m]) } else { v }
                    })
                    .collect();
                // Kept from the first bin that weighs anything to the last, so applying it skips the rest.
                let first = weights.iter().position(|w| *w != 0.0).unwrap_or(weights.len());
                let last = weights.iter().rposition(|w| *w != 0.0).map_or(first, |l| l + 1);
                (first, weights[first..last].to_vec())
            })
            .collect();
        MelFilters { bins, mels, bands }
    }

    /// A bin's weight in a band.
    pub fn weight(&self, bin: usize, band: usize) -> f64 {
        self.bands
            .get(band)
            .and_then(|(first, w)| bin.checked_sub(*first).and_then(|i| w.get(i)))
            .copied()
            .unwrap_or(0.0)
    }

    /// Applies the bank to a power spectrogram: one row of `mels` values per frame. The bins are added in order and
    /// the ones weighing nothing skipped, as C# does, so the sums come out the same.
    pub fn apply(&self, spec: &[Vec<f32>]) -> Vec<Vec<f32>> {
        spec.iter()
            .map(|s| {
                self.bands
                    .iter()
                    .map(|(first, weights)| {
                        let mut acc = 0.0;
                        for (w, p) in weights.iter().zip(s.iter().skip(*first)) {
                            if *w != 0.0 {
                                acc += w * f64::from(*p);
                            }
                        }
                        acc as f32
                    })
                    .collect()
            })
            .collect()
    }
}

/// Band-limited (windowed-sinc) resampling from `from` to `to` samples a second. Good enough for analysis and kit
/// exports.
pub fn resample(x: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || x.is_empty() {
        return x.to_vec();
    }
    // No rate to go to (or from): nothing comes out. C# gives none for `to` 0 too; for `from` 0 it went wrong.
    if from == 0 || to == 0 {
        return Vec::new();
    }
    let ratio = f64::from(to) / f64::from(from);
    let out_len = (x.len() as f64 * ratio).ceil() as usize;
    // The fraction of the input's highest frequency to keep.
    let cutoff = ratio.min(1.0);
    const HALF_TAPS: f64 = 16.0;
    let support = HALF_TAPS / cutoff;
    let last = x.len() as i64 - 1;
    (0..out_len)
        .map(|i| {
            let t = i as f64 / ratio;
            let left = (t - support).floor() as i64 + 1;
            let right = (t + support).floor() as i64;
            let (mut acc, mut wsum) = (0.0, 0.0);
            for j in left.max(0)..=right.min(last) {
                let d = (t - j as f64) * cutoff;
                let sinc = if d.abs() < 1e-9 { 1.0 } else { (PI * d).sin() / (PI * d) };
                let u = (t - j as f64) / support;
                let win = 0.42 + 0.5 * (PI * u).cos() + 0.08 * (2.0 * PI * u).cos();
                let w = sinc * win;
                acc += f64::from(x[j as usize]) * w;
                wsum += w;
            }
            if wsum > 1e-9 { (acc / wsum) as f32 } else { 0.0 }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fft_finds_a_sine_in_its_bin() {
        let n = 64;
        let mut re: Vec<f64> = (0..n).map(|i| (2.0 * PI * 5.0 * i as f64 / n as f64).sin()).collect();
        let mut im = vec![0.0; n];
        fft(&mut re, &mut im);
        let mag: Vec<f64> = re.iter().zip(&im).map(|(r, i)| (r * r + i * i).sqrt()).collect();
        assert!((mag[5] - 32.0).abs() < 1e-9);
        assert!(mag[4] < 1e-9 && mag[6] < 1e-9);
    }

    #[test]
    fn spectrogram_frames_and_reflect_padding_match_librosa() {
        assert_eq!(reflect_pad(&[1.0, 2.0, 3.0, 4.0], 2), vec![3.0, 2.0, 1.0, 2.0, 3.0, 4.0, 3.0, 2.0]);
        let x = vec![0.5f32; 4096];
        let spec = power_spectrogram(&x, 2048, 512, true);
        assert_eq!(spec.len(), 1 + 4096 / 512);
        assert_eq!(spec[0].len(), 1025);
        assert!(power_spectrogram(&[0.0; 10], 2048, 512, false).is_empty());
    }

    #[test]
    fn mel_scales_go_there_and_back() {
        for hz in [0.0, 440.0, 1000.0, 8000.0] {
            for scale in [MelScale::Htk, MelScale::Slaney] {
                assert!((mel_to_hz(hz_to_mel(hz, scale), scale) - hz).abs() < 1e-6);
            }
        }
        assert!((hz_to_mel(1000.0, MelScale::Slaney) - 15.0).abs() < 1e-12);
    }

    #[test]
    fn mel_filters_are_triangles_that_cover_the_band() {
        let fb = MelFilters::new(1025, 40, 20.0, 16_000.0, 44_100, MelScale::Slaney, true);
        assert_eq!(fb.bands.len(), 40);
        for m in 0..40 {
            assert!((0..1025).any(|k| fb.weight(k, m) > 0.0), "band {m} is empty");
            assert!((0..1025).all(|k| fb.weight(k, m) >= 0.0));
        }
        // Applying it is the plain sum over every bin.
        let spec = vec![(0..1025).map(|k| (k % 7) as f32).collect::<Vec<f32>>()];
        let got = fb.apply(&spec);
        for (m, got) in got[0].iter().enumerate() {
            let mut want = 0.0;
            for (k, p) in spec[0].iter().enumerate() {
                let w = fb.weight(k, m);
                if w != 0.0 {
                    want += w * f64::from(*p);
                }
            }
            assert_eq!(*got, want as f32);
        }
    }

    #[test]
    fn resampling_keeps_a_steady_level_and_the_length() {
        let x = vec![0.25f32; 1000];
        let y = resample(&x, 48_000, 22_050);
        assert_eq!(y.len(), (1000.0f64 * 22_050.0 / 48_000.0).ceil() as usize);
        assert!(y.iter().all(|v| (v - 0.25).abs() < 1e-5));
        assert_eq!(resample(&x, 44_100, 44_100), x);
        assert!(resample(&x, 44_100, 0).is_empty());
    }
}
