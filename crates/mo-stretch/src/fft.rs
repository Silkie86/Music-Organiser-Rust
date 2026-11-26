//! The Fourier transform the stretcher works in: a real one, "half-bin shifted" (Signalsmith Linear's `RealFFT` with
//! `halfBinShift`, from its `fft.h`).
//!
//! A plain transform of `n` samples gives bands at 0, 1, 2 ... cycles per `n` samples, with one at 0 (the DC) and one
//! at the very top (Nyquist) that have no phase. This one puts its `n / 2` bands half way between those: at 0.5, 1.5,
//! 2.5 ... cycles. Every band then has a phase, and none is a special case, which keeps the stretcher's sums simple.
//!
//! It's worked out with a complex transform of half the size (`rustfft`), the usual trick for a real one: even
//! samples as the real part, odd ones as the imaginary part, then untangled.

use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};
use std::f64::consts::PI;
use std::sync::Arc;

/// The smallest size at or above `size` that the transform is quick at: a power of two times 1 to 8 (7 is skipped),
/// as Signalsmith Linear's `SplitFFT::fastSizeAbove`. Keeping its sizes keeps the stretcher's bands where the
/// original's are.
fn complex_size_above(size: usize) -> usize {
    let mut pow2 = 1;
    while pow2 < 16 && pow2 < size {
        pow2 *= 2;
    }
    while pow2 * 8 < size {
        pow2 *= 2;
    }
    let mut multiple = size.div_ceil(pow2);
    if multiple == 7 {
        multiple += 1;
    }
    multiple * pow2
}

/// The real transform's quick size at or above `size` (Signalsmith Linear's `RealFFT::fastSizeAbove`): always even.
pub(crate) fn size_above(size: usize) -> usize {
    complex_size_above(size.div_ceil(2)) * 2
}

/// A real transform of `size` samples into `size / 2` half-bin-shifted bands, and back.
pub(crate) struct RealFft {
    forward: Arc<dyn Fft<f32>>,
    inverse: Arc<dyn Fft<f32>>,
    /// The half-size complex buffer the work is done in.
    work: Vec<Complex32>,
    scratch: Vec<Complex32>,
    /// Untangling the even and odd halves, with the half-band shift and a turn by -i baked in.
    twiddles: Vec<Complex32>,
    /// The half-band shift put on the samples before the transform.
    twists: Vec<Complex32>,
}

impl RealFft {
    /// A transform of `size` samples: `size` must be a multiple of 4 ([`size_above`] gives one, times 2).
    pub(crate) fn new(size: usize) -> Self {
        let half = size / 2;
        let mut planner = FftPlanner::new();
        let forward = planner.plan_fft_forward(half);
        let inverse = planner.plan_fft_inverse(half);
        let scratch_len = forward.get_inplace_scratch_len().max(inverse.get_inplace_scratch_len());
        let n = size as f64;
        let twiddles = (0..=half / 2).map(|i| polar((i as f64 + 0.5) * (-2.0 * PI / n) - PI / 2.0)).collect();
        let twists = (0..half).map(|i| polar(-2.0 * PI * i as f64 / n)).collect();
        Self {
            forward,
            inverse,
            work: vec![Complex32::new(0.0, 0.0); half],
            scratch: vec![Complex32::new(0.0, 0.0); scratch_len],
            twiddles,
            twists,
        }
    }

    /// `time` (`size` samples) into `freq` (`size / 2` bands). Not scaled: a band's height is the sum over the samples.
    pub(crate) fn forward(&mut self, time: &[f32], freq: &mut [Complex32]) {
        let half = self.work.len();
        for (i, (w, twist)) in self.work.iter_mut().zip(&self.twists).enumerate() {
            let (tr, ti) = (time[2 * i], time[2 * i + 1]);
            *w = Complex32::new(tr * twist.re - ti * twist.im, ti * twist.re + tr * twist.im);
        }
        self.forward.process_with_scratch(&mut self.work, &mut self.scratch);
        let f = &self.work;
        for i in 0..=half / 2 {
            let conj_i = half - 1 - i;
            let twiddle = self.twiddles[i];
            let odd = (f[i] + f[conj_i].conj()) * 0.5;
            let even_i = (f[i] - f[conj_i].conj()) * 0.5;
            let even_rot = Complex32::new(
                even_i.re * twiddle.re - even_i.im * twiddle.im,
                even_i.im * twiddle.re + even_i.re * twiddle.im,
            );
            freq[i] = odd + even_rot;
            freq[conj_i] = Complex32::new(odd.re - even_rot.re, even_rot.im - odd.im);
        }
    }

    /// `freq` (`size / 2` bands) back into `time` (`size` samples). Not scaled: [`forward`](Self::forward) then this
    /// gives the samples `size` times over.
    pub(crate) fn inverse(&mut self, freq: &[Complex32], time: &mut [f32]) {
        let half = self.work.len();
        for i in 0..=half / 2 {
            let conj_i = half - 1 - i;
            let twiddle = self.twiddles[i];
            let odd = freq[i] + freq[conj_i].conj();
            let even_rot = freq[i] - freq[conj_i].conj();
            let even_i = Complex32::new(
                even_rot.re * twiddle.re + even_rot.im * twiddle.im,
                even_rot.im * twiddle.re - even_rot.re * twiddle.im,
            );
            self.work[i] = odd + even_i;
            self.work[conj_i] = Complex32::new(odd.re - even_i.re, even_i.im - odd.im);
        }
        self.inverse.process_with_scratch(&mut self.work, &mut self.scratch);
        for (i, (t, twist)) in self.work.iter().zip(&self.twists).enumerate() {
            time[2 * i] = t.re * twist.re + t.im * twist.im;
            time[2 * i + 1] = t.im * twist.re - t.re * twist.im;
        }
    }
}

/// A turn by `phase` radians, worked out in f64 as the original does before it's kept as f32.
fn polar(phase: f64) -> Complex32 {
    Complex32::new(phase.cos() as f32, phase.sin() as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_the_originals() {
        // The default preset at 44.1 kHz: a 5292-sample block gives a 6144-sample transform.
        assert_eq!(size_above(5292_usize.div_ceil(2)) * 2, 6144);
        assert_eq!(complex_size_above(7), 8);
        assert_eq!(complex_size_above(100), 128, "7 times 16 is skipped");
        assert_eq!(complex_size_above(90), 96);
        assert_eq!(complex_size_above(1323), 1536);
        assert_eq!(complex_size_above(1793), 2048, "7 times a power of two is skipped");
    }

    /// The bands match a slow, plain sum at the half-band frequencies, and the way back gives the samples `n` times.
    #[test]
    fn matches_the_plain_sum_and_comes_back() {
        let n = 48;
        let time: Vec<f32> = (0..n).map(|i| ((i * 37 % 11) as f32 - 5.0) / 7.0 + (i as f32 * 0.3).sin()).collect();
        let mut fft = RealFft::new(n);
        let mut freq = vec![Complex32::new(0.0, 0.0); n / 2];
        fft.forward(&time, &mut freq);
        for (k, band) in freq.iter().enumerate() {
            let mut sum = (0.0f64, 0.0f64);
            for (i, &x) in time.iter().enumerate() {
                let phase = -2.0 * PI * i as f64 * (k as f64 + 0.5) / n as f64;
                sum.0 += x as f64 * phase.cos();
                sum.1 += x as f64 * phase.sin();
            }
            assert!((band.re as f64 - sum.0).abs() < 1e-3, "band {k}: {} vs {}", band.re, sum.0);
            assert!((band.im as f64 - sum.1).abs() < 1e-3, "band {k}: {} vs {}", band.im, sum.1);
        }
        let mut back = vec![0.0; n];
        fft.inverse(&freq, &mut back);
        for (a, b) in time.iter().zip(&back) {
            assert!((a * n as f32 - b).abs() < 1e-3, "{a} x {n} vs {b}");
        }
    }
}
