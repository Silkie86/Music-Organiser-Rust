//! The ear: on a low-priority thread, sums up the newest audio in the ring as one [`Heard`].

use std::sync::Arc;

use mo_audio::dsp;

use crate::hits::{Hit, Hits};
use crate::ring::{RING, Ring};

/// The FFT's length in samples: 186 ms at 44.1 kHz, long enough to tell low notes a semitone apart (as the Sounds
/// page's chroma does).
pub const FFT: usize = 8192;
/// Where low ends and mid begins, and where mid ends and high begins, in Hz.
pub const BANDS_HZ: [f64; 2] = [250.0, 4000.0];
/// Below this level (the FFT window's RMS, about -80 dB below full scale) no notes are heard: the chroma stays empty.
pub const QUIET: f32 = 1e-4;

/// What the ear heard since it last listened.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Heard {
    /// The stream sample this summary goes up to (not including it): the count the tap has been given, so it lines up
    /// with the host's play position.
    pub samples_to: u64,
    /// How much new audio it covers, in seconds (at most the ring's length).
    pub seconds: f64,
    /// The new audio's RMS level and its peak, as fractions of full scale.
    pub rms: f32,
    pub peak: f32,
    /// How the newest FFT window's sound splits between low, mid and high ([`BANDS_HZ`]), as shares adding up to 1;
    /// all 0 in silence.
    pub bands: [f32; 3],
    /// The 12 notes heard in the newest FFT window (C first), the loudest as 1; all 0 below [`QUIET`].
    pub chroma: [f32; 12],
    /// The hits in the new audio, oldest first.
    pub hits: Vec<Hit>,
}

/// The low-priority half of a listener: call [`Ear::hear`] about every 100 ms. Keeps its working room between calls,
/// so listening takes no new memory after the first few.
pub struct Ear {
    ring: Arc<Ring>,
    rate: f64,
    heard_to: u64,
    window: Vec<f64>,
    re: Vec<f64>,
    im: Vec<f64>,
    power: Vec<f32>,
    mag: Vec<f64>,
    frame: Vec<f32>,
    fresh: Vec<f32>,
    hits: Hits,
}

impl Ear {
    pub(crate) fn new(ring: Arc<Ring>, sample_rate: f64) -> Ear {
        let rate = if sample_rate.is_finite() && sample_rate > 0.0 { sample_rate } else { 48_000.0 };
        Ear {
            ring,
            rate,
            heard_to: 0,
            window: dsp::hann(FFT),
            re: vec![0.0; FFT],
            im: vec![0.0; FFT],
            power: vec![0.0; FFT / 2 + 1],
            mag: vec![0.0; FFT / 2 + 1],
            frame: Vec::with_capacity(FFT),
            fresh: Vec::with_capacity(RING),
            hits: Hits::new(rate),
        }
    }

    /// The sample rate it listens at, in Hz.
    pub fn sample_rate(&self) -> f64 {
        self.rate
    }

    /// Sums up the audio that came since the last call; `None` when none came (the host stopped calling, or the
    /// track sleeps in silence).
    pub fn hear(&mut self) -> Option<Heard> {
        let to = self.ring.written();
        if to == self.heard_to {
            return None;
        }
        let from = self.ring.read(self.heard_to, to, &mut self.fresh);
        self.heard_to = to;
        let mut heard = Heard { samples_to: to, seconds: self.fresh.len() as f64 / self.rate, ..Heard::default() };
        let mut sum = 0f64;
        for &s in &self.fresh {
            sum += f64::from(s) * f64::from(s);
            heard.peak = heard.peak.max(s.abs());
        }
        heard.rms = (sum / self.fresh.len().max(1) as f64).sqrt() as f32;
        self.hits.feed(from, &self.fresh, &mut heard.hits);

        self.ring.newest(to, FFT, &mut self.frame);
        let mut window_power = 0f64;
        for ((r, &s), w) in self.re.iter_mut().zip(&self.frame).zip(&self.window) {
            *r = f64::from(s) * w;
            window_power += f64::from(s) * f64::from(s);
        }
        self.im.fill(0.0);
        dsp::fft(&mut self.re, &mut self.im);
        for ((p, r), i) in self.power.iter_mut().zip(&self.re).zip(&self.im) {
            *p = (r * r + i * i) as f32;
        }
        let bin_hz = self.rate / FFT as f64;
        heard.bands = bands(&self.power, bin_hz);
        if (window_power / FFT as f64).sqrt() >= f64::from(QUIET) {
            let chroma = dsp::chroma_of(&self.power, bin_hz, &mut self.mag);
            let loudest = chroma.iter().copied().fold(0.0, f64::max);
            if loudest > 0.0 {
                for (out, c) in heard.chroma.iter_mut().zip(chroma) {
                    *out = (c / loudest) as f32;
                }
            }
        }
        Some(heard)
    }
}

/// Shares of the power below, between and above [`BANDS_HZ`] (the DC bin left out).
fn bands(power: &[f32], bin_hz: f64) -> [f32; 3] {
    let mut sums = [0f64; 3];
    for (k, p) in power.iter().enumerate().skip(1) {
        let hz = k as f64 * bin_hz;
        let band = if hz < BANDS_HZ[0] {
            0
        } else if hz < BANDS_HZ[1] {
            1
        } else {
            2
        };
        sums[band] += f64::from(*p);
    }
    let total: f64 = sums.iter().sum();
    if total <= 0.0 || !total.is_finite() {
        return [0.0; 3];
    }
    sums.map(|s| (s / total) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_split_by_frequency() {
        let bin_hz = 48_000.0 / FFT as f64;
        let mut power = vec![0f32; FFT / 2 + 1];
        power[(100.0 / bin_hz) as usize] = 1.0;
        power[(1000.0 / bin_hz) as usize] = 2.0;
        power[(8000.0 / bin_hz) as usize] = 1.0;
        assert_eq!(bands(&power, bin_hz), [0.25, 0.5, 0.25]);
        assert_eq!(bands(&vec![0.0; FFT / 2 + 1], bin_hz), [0.0; 3]);
    }
}
