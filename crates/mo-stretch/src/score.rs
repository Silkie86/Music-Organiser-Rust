//! How good a stretch is, in numbers: the 1.1 plan's "how we'll know it's better", before the ear test.
//!
//! Each score compares a sound with its stretched copy (same pitch, another length), mono:
//!
//! - **Ghost**: how much a hit is heard before it starts (pre-echo, the phase vocoder's best-known fault: a drum's
//!   attack smeared into the few milliseconds before it). The sound 30 to 5 ms before each hit, against the hit's
//!   first 10 ms, in dB, less the same for the original. 0 is as clean as the original; more is worse.
//! - **Sharpness**: how fast each hit rises (the steepest climb of its loudness, in dB per millisecond), against the
//!   original's. 1 is as sharp; under 1 is softer.
//! - **Timing**: how far each hit's peak lands from where the new tempo puts it, in milliseconds, on average.
//! - **Distance**: how far the stretched sound's spectrum is from the original's at the same point in the music,
//!   in dB on average over the bands that are heard (log-spectral distance). 0 is the same; smaller is closer.
//!
//! Hits are found in the original by a finder of their own (spectral flux: where the spectrum's bands suddenly get
//! louder, then the steepest climb of the loudness there), not the drum stretch's: a stretch isn't scored at the very
//! samples it aimed for. Loudness is measured over 2 ms every 1 ms. Samples that aren't numbers count as silence.

use rustfft::FftPlanner;
use rustfft::num_complex::Complex32;

/// The scores of one stretch: see the module notes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scores {
    /// More ghost before the hits than the original has, in dB (0 as the original, more is worse; under 0 is quieter
    /// before the hits than the original).
    pub ghost_db: f32,
    /// How sharp the hits are against the original's (1 the same, under 1 softer).
    pub sharpness: f32,
    /// How far the hits are from where they should be, in ms on average.
    pub timing_ms: f32,
    /// How far the spectrum is from the original's, in dB on average.
    pub distance_db: f32,
    /// How many of the original's hits were scored (ghost, sharpness and timing are 0, 1 and 0 when none was: a hit
    /// in the first 35 ms isn't).
    pub hits: usize,
}

/// The loudness of `sound` in dB, worked out over 2 ms every 1 ms.
struct Loudness {
    db: Vec<f32>,
    /// Samples per step (1 ms).
    hop: usize,
}

/// A sample as it's scored: one that isn't a number (a damaged file) is silence.
fn clean(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}

impl Loudness {
    fn of(sound: &[f32], sample_rate: f32) -> Self {
        let hop = ((sample_rate / 1000.0) as usize).max(1);
        let width = hop * 2;
        let db = (0..sound.len() / hop)
            .map(|i| {
                let start = i * hop;
                let end = (start + width).min(sound.len());
                let part = &sound[start..end];
                let power = part.iter().map(|&s| clean(s) * clean(s)).sum::<f32>() / part.len().max(1) as f32;
                10.0 * (power + 1e-12).log10()
            })
            .collect();
        Self { db, hop }
    }

    /// The mean power from step `from` up to `to` (clipped to the sound), as dB.
    fn mean_db(&self, from: isize, to: isize) -> f32 {
        let from = from.clamp(0, self.db.len() as isize) as usize;
        let to = to.clamp(0, self.db.len() as isize) as usize;
        if to <= from {
            return -120.0;
        }
        let power: f32 = self.db[from..to].iter().map(|d| 10f32.powf(d / 10.0)).sum::<f32>() / (to - from) as f32;
        10.0 * (power + 1e-12).log10()
    }

    /// The steepest climb in dB per step from step `from` up to `to`.
    fn steepest(&self, from: isize, to: isize) -> f32 {
        let from = from.clamp(1, self.db.len() as isize) as usize;
        let to = to.clamp(0, self.db.len() as isize) as usize;
        (from..to).map(|i| self.db[i] - self.db[i - 1]).fold(0.0, f32::max)
    }

    /// The loudest step from `from` up to `to`.
    fn peak(&self, from: isize, to: isize) -> Option<usize> {
        let from = from.clamp(0, self.db.len() as isize) as usize;
        let to = to.clamp(0, self.db.len() as isize) as usize;
        (from..to).max_by(|&a, &b| self.db[a].total_cmp(&self.db[b]))
    }
}

/// The scores of `stretched` against `original` (both mono, at `sample_rate`): the stretch is their lengths' ratio.
pub fn score(original: &[f32], stretched: &[f32], sample_rate: f32) -> Scores {
    let ratio = stretched.len() as f64 / original.len().max(1) as f64;
    let a = Loudness::of(original, sample_rate);
    let b = Loudness::of(stretched, sample_rate);
    let starts = onsets(original, &a, sample_rate);

    let (mut ghost, mut sharp, mut timing, mut counted) = (0.0f64, 0.0f64, 0.0f64, 0usize);
    for &start in &starts {
        // A hit in the first 35 ms has too little before it to tell a ghost by (a sound's own start).
        if start < 35 {
            continue;
        }
        let s = start as isize;
        let t = (start as f64 * ratio).round() as isize;
        // The stretched hit's own start, near where it should be: so a hit a little late isn't all ghost.
        let Some(peak_a) = a.peak(s, s + 20) else { continue };
        let Some(peak_b) = b.peak(t - 20, t + 20 + (peak_a as isize - s)) else { continue };
        let lead = peak_a as isize - s;
        let tb = peak_b as isize - lead;
        let ghost_a = a.mean_db(s - 30, s - 5) - a.mean_db(s, s + 10);
        let ghost_b = b.mean_db(tb - 30, tb - 5) - b.mean_db(tb, tb + 10);
        let steep_a = a.steepest(s - 2, peak_a as isize + 1);
        let steep_b = b.steepest(tb - 10, peak_b as isize + 1);
        if steep_a <= 0.0 {
            continue;
        }
        ghost += (ghost_b - ghost_a) as f64;
        sharp += (steep_b / steep_a) as f64;
        timing += (peak_b as f64 - (peak_a as f64 * ratio)).abs() * b.hop as f64 * 1000.0 / sample_rate as f64;
        counted += 1;
    }
    let (ghost_db, sharpness, timing_ms) = if counted == 0 {
        (0.0, 1.0, 0.0)
    } else {
        let n = counted as f64;
        ((ghost / n) as f32, (sharp / n) as f32, (timing / n) as f32)
    };

    Scores { ghost_db, sharpness, timing_ms, distance_db: distance(original, stretched, ratio), hits: counted }
}

/// Where the hits start in `sound`, as steps of its `loudness`: frames (1024 samples every 256) whose bands get
/// louder all together far more than the frames around them do, at least 30 ms apart; each placed at the steepest
/// climb of the loudness within its frame.
fn onsets(sound: &[f32], loudness: &Loudness, sample_rate: f32) -> Vec<usize> {
    const SIZE: usize = 1024;
    const HOP: usize = 256;
    if sound.len() < SIZE {
        return Vec::new();
    }
    let fft = FftPlanner::new().plan_fft_forward(SIZE);
    let window: Vec<f32> =
        (0..SIZE).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / SIZE as f32).cos()).collect();
    let mut buffer = vec![Complex32::new(0.0, 0.0); SIZE];
    let mut previous = vec![0.0f32; SIZE / 2];
    let frames = (sound.len() - SIZE) / HOP + 1;
    let mut flux = Vec::with_capacity(frames);
    for f in 0..frames {
        for (i, b) in buffer.iter_mut().enumerate() {
            *b = Complex32::new(clean(sound[f * HOP + i]) * window[i], 0.0);
        }
        fft.process(&mut buffer);
        let mut rise = 0.0f32;
        for (p, c) in previous.iter_mut().zip(&buffer[..SIZE / 2]) {
            let level = (1.0 + 100.0 * c.norm()).ln();
            rise += (level - *p).max(0.0);
            *p = level;
        }
        flux.push(if f == 0 { 0.0 } else { rise });
    }
    let mean = flux.iter().sum::<f32>() / flux.len() as f32;
    let gap = ((sample_rate * 0.03) as usize).div_ceil(HOP).max(1);
    let mut found: Vec<usize> = Vec::new();
    let mut last: Option<usize> = None;
    for f in 1..flux.len().saturating_sub(1) {
        let around = &flux[f.saturating_sub(8)..(f + 9).min(flux.len())];
        let local = around.iter().sum::<f32>() / around.len() as f32;
        let peak = flux[f] >= flux[f - 1] && flux[f] > flux[f + 1];
        if peak && flux[f] > 1.5 * local + 0.2 * mean && last.is_none_or(|l| f >= l + gap) {
            last = Some(f);
            // The new part of this frame is its last hop or so; the steepest climb in the whole frame is the start.
            let from = (f * HOP / loudness.hop) as isize;
            let to = ((f * HOP + SIZE) / loudness.hop) as isize;
            let from_c = from.clamp(1, loudness.db.len() as isize) as usize;
            let to_c = to.clamp(0, loudness.db.len() as isize) as usize;
            if let Some(step) = (from_c..to_c).max_by(|&a, &b| {
                (loudness.db[a] - loudness.db[a - 1]).total_cmp(&(loudness.db[b] - loudness.db[b - 1]))
            }) && found.last().is_none_or(|&l| step > l + 20)
            {
                found.push(step - 1);
            }
        }
    }
    found
}

/// The log-spectral distance between `stretched` and `original` at the same point in the music, in dB: over 2048-
/// sample windows every 512 of the stretched sound, against the original's window at the same point, over the
/// bands within 60 dB of the loudest.
fn distance(original: &[f32], stretched: &[f32], ratio: f64) -> f32 {
    const SIZE: usize = 2048;
    const HOP: usize = 512;
    if original.len() < SIZE || stretched.len() < SIZE {
        return 0.0;
    }
    let fft = FftPlanner::new().plan_fft_forward(SIZE);
    let window: Vec<f32> =
        (0..SIZE).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / SIZE as f32).cos()).collect();
    let mut buffer = vec![Complex32::new(0.0, 0.0); SIZE];
    let mut spectrum = |sound: &[f32], start: usize| -> Vec<f32> {
        for (i, b) in buffer.iter_mut().enumerate() {
            *b = Complex32::new(clean(sound[start + i]) * window[i], 0.0);
        }
        fft.process(&mut buffer);
        buffer[..SIZE / 2].iter().map(|c| 10.0 * (c.norm_sqr() + 1e-12).log10()).collect()
    };
    let (mut total, mut count) = (0.0f64, 0usize);
    let mut start = 0;
    while start + SIZE <= stretched.len() {
        let source = ((start as f64 / ratio).round() as usize).min(original.len() - SIZE);
        let a = spectrum(original, source);
        let b = spectrum(stretched, start);
        let loudest = a.iter().chain(&b).copied().fold(f32::NEG_INFINITY, f32::max);
        for (x, y) in a.iter().zip(&b) {
            if x.max(*y) > loudest - 60.0 {
                // A band near silence in one of them counts as near silence, not as 120 dB apart.
                total += (x.max(loudest - 60.0) - y.max(loudest - 60.0)).abs() as f64;
                count += 1;
            }
        }
        start += HOP;
    }
    if count == 0 { 0.0 } else { (total / count as f64) as f32 }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = 44_100.0;

    /// Two bars of a made-up beat: a noisy hit every quarter second over a soft tone.
    fn beat(seconds: f32) -> Vec<f32> {
        let mut seed = 1u32;
        (0..(RATE * seconds) as usize)
            .map(|i| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0;
                let since = (i % 11_025) as f32;
                0.1 * (i as f32 * 0.03).sin() + 0.7 * (-since / 300.0).exp() * noise
            })
            .collect()
    }

    #[test]
    fn a_sound_against_itself_scores_perfectly() {
        let sound = beat(2.0);
        let s = score(&sound, &sound, RATE);
        assert_eq!(s.hits, 7, "the first hit, at the very start, isn't scored");
        assert!(s.ghost_db.abs() < 1e-3 && (s.sharpness - 1.0).abs() < 1e-3 && s.timing_ms < 1e-3, "{s:?}");
        assert!(s.distance_db < 1e-3, "{s:?}");
    }

    #[test]
    fn ghosts_and_soft_hits_are_seen() {
        let sound = beat(2.0);
        // Each hit's start heard in the 30 ms before it, at a third of its loudness: pre-echo.
        let mut ghosted = sound.clone();
        for hit in 1..8 {
            let at = hit * 11_025;
            for i in 0..1323 {
                ghosted[at - 1323 + i] += sound[at + i % 300] * 0.33;
            }
        }
        let s = score(&sound, &ghosted, RATE);
        assert!(s.ghost_db > 3.0, "{s:?}");

        // Each hit faded in over 8 ms: softer.
        let mut soft = sound.clone();
        for hit in 0..8 {
            for i in 0..353 {
                soft[hit * 11_025 + i] *= i as f32 / 353.0;
            }
        }
        let s = score(&sound, &soft, RATE);
        assert!(s.sharpness < 0.7, "{s:?}");
    }

    #[test]
    fn the_stretcher_as_it_stands() {
        // Where Signalsmith Stretch starts on a drum loop at 0.8x and 1.25x: what the drum work should improve.
        let sound = beat(2.0);
        for ratio in [0.8f32, 1.25] {
            let length = (sound.len() as f32 * ratio) as usize;
            let out = crate::to_length(&[&sound], RATE, length, 0.0).unwrap();
            let s = score(&sound, &out[0], RATE);
            assert_eq!(s.hits, 7, "the first hit, at the very start, isn't scored");
            assert!(s.timing_ms < 5.0, "{ratio}x: {s:?}");
            assert!(s.distance_db < 12.0, "{ratio}x: {s:?}");
            eprintln!("Signalsmith at {ratio}x: {s:?}");
        }
    }
}
