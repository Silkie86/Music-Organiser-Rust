//! The stretcher for sounds with hits in them (drum loops, plucks, anything with a sharp start): the 1.1 plan's
//! "our own" stretch, built on [`Stretch`].
//!
//! A phase vocoder (Signalsmith Stretch too) is good on tones but softens a drum's attack and smears a little of it
//! into the 50 ms or so before it (pre-echo). So the sound is stretched in three parts that are put back together:
//!
//! 1. **The bed**: the whole sound through Signalsmith Stretch. Below `low_hz` (120 Hz) it goes through a stretcher
//!    with windows twice as long, which holds a kick's or an 808's low note steadier (the idea of the `timestretch`
//!    crate's low-end lock); above, the usual windows.
//! 2. **Pre-echo redrawn**: in the 70 ms before each hit, the bed is turned down to no louder than the original was
//!    at that point, a little at a time (5 ms steps): the ghost of the hit goes, the sound before it stays (Fierro and
//!    others, 2023, redraw the volume shape before each hit the same way).
//! 3. **Hits kept untouched**: each hit is put back as it was, where the new tempo puts it, from just before its
//!    attack to `hit_ms` after it, faded back into the bed where the two line up best (SELEBI, 2026, keeps the hits
//!    out of the stretch for the same reason; a DAW's beat slicing does it too).
//!
//! The hits come from [`hits`]. A sound with none comes out as the bed alone.

use crate::Stretch;

/// How the drum stretch is set: see the module notes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrumOptions {
    /// How long each hit is kept untouched after its attack, at most (ms). Longer keeps more of the hit's own body;
    /// shorter leaves more to the stretch.
    pub hit_ms: f32,
    /// Below this the bed is stretched with long windows (Hz); 0 for one stretch of the whole sound.
    pub low_hz: f32,
    /// How much of the pre-echo is taken away, 0 to 1.
    pub redraw: f32,
}

impl Default for DrumOptions {
    fn default() -> Self {
        Self { hit_ms: 40.0, low_hz: 120.0, redraw: 1.0 }
    }
}

/// A sample as it's stretched: one that isn't a number (a damaged file) is silence.
fn clean(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}

/// Where the hits are in `sound` (one channel, at `rate`): the samples their attacks start at, in order. A hit is a
/// sudden rise of the sound's highs (above 1 kHz: a drum's attack, and not a low note's own swing, which a short
/// window sees rise and fall every cycle), at least 30 ms after the one before.
pub fn hits(sound: &[f32], rate: f32) -> Vec<usize> {
    let highs = {
        let hp = Biquad::butterworth(1000.0, rate.max(1.0), true);
        hp.run(&hp.run(&sound.iter().copied().map(clean).collect::<Vec<_>>()))
    };
    let rate = rate.max(1.0) as usize;
    let hop = (rate / 375).max(16); // 128 samples at 48 kHz, under 3 ms
    let width = hop * 2;
    let levels: Vec<f64> = (0..highs.len().div_ceil(hop))
        .map(|k| {
            let part = &highs[k * hop..(k * hop + width).min(highs.len())];
            let e: f64 = part.iter().map(|&x| f64::from(x) * f64::from(x)).sum();
            10.0 * (e / part.len().max(1) as f64 + 1e-12).log10()
        })
        .collect();
    let loudest = levels.iter().copied().fold(f64::MIN, f64::max);
    let gap = (rate * 30 / 1000).div_ceil(hop).max(1);
    let mut found: Vec<usize> = Vec::new();
    let mut k = 0;
    while k < levels.len() {
        // Before the sound starts counts as silence.
        let before = match k {
            0 => -240.0,
            _ => levels[k.saturating_sub(4)..k].iter().copied().fold(f64::MAX, f64::min),
        };
        // 12 dB up on the level just before, and no quieter than 40 dB under the loudest moment (nor near silence).
        if levels[k] - before > 12.0 && levels[k] > (loudest - 40.0).max(-90.0) {
            let top = (k..(k + 3).min(levels.len())).max_by(|&a, &b| levels[a].total_cmp(&levels[b])).unwrap_or(k);
            found.push(attack_start(sound, k.saturating_sub(1) * hop, (top + 1) * hop, rate));
            k = top + gap;
        } else {
            k += 1;
        }
    }
    found.dedup();
    found
}

/// The first sample of an attack found between `from` and `to`: where the sound first jumps by a fifth of the
/// attack's biggest jump, half a millisecond before it.
fn attack_start(sound: &[f32], from: usize, to: usize, rate: usize) -> usize {
    let span = sound.get(from..to.min(sound.len())).unwrap_or_default();
    let jumps = || span.windows(2).map(|w| (clean(w[1]) - clean(w[0])).abs());
    let most = jumps().fold(0.0f32, f32::max);
    let first = jumps().position(|j| j >= most * 0.2).unwrap_or(0);
    (from + first).saturating_sub(rate / 2000)
}

/// A two-pole filter (one biquad), run forward.
#[derive(Clone, Copy)]
struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
}

impl Biquad {
    /// A Butterworth low or high pass at `freq` Hz (Q = 1/√2): two in a row make a Linkwitz-Riley crossover, whose
    /// low and high parts add back up to the sound with every frequency as loud as it was.
    fn butterworth(freq: f32, rate: f32, high: bool) -> Self {
        let w = 2.0 * std::f32::consts::PI * (freq / rate).clamp(1e-5, 0.49);
        let (sin, cos) = w.sin_cos();
        let alpha = sin / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
        let a0 = 1.0 + alpha;
        let b = if high {
            [(1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0]
        } else {
            [(1.0 - cos) / 2.0, 1.0 - cos, (1.0 - cos) / 2.0]
        };
        Self { b: [b[0] / a0, b[1] / a0, b[2] / a0], a: [-2.0 * cos / a0, (1.0 - alpha) / a0] }
    }

    fn run(&self, sound: &[f32]) -> Vec<f32> {
        let (mut x1, mut x2, mut y1, mut y2) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        sound
            .iter()
            .map(|&x| {
                let y = self.b[0] * x + self.b[1] * x1 + self.b[2] * x2 - self.a[0] * y1 - self.a[1] * y2;
                (x2, x1, y2, y1) = (x1, x, y1, y);
                y
            })
            .collect()
    }
}

/// `sound` split at `freq` into its low and high parts, which add back up to it exactly: the low part is a
/// Butterworth low pass run forwards and then backwards (so nothing in it is moved in time, 24 dB an octave), the
/// high part is the rest.
fn split(sound: &[f32], freq: f32, rate: f32) -> (Vec<f32>, Vec<f32>) {
    let low = Biquad::butterworth(freq, rate, false);
    let mut low_part = low.run(sound);
    low_part.reverse();
    let mut low_part = low.run(&low_part);
    low_part.reverse();
    let high_part = sound.iter().zip(&low_part).map(|(x, l)| x - l).collect();
    (low_part, high_part)
}

/// The loudness of `sound` over `width` samples around each sample (as power), found every `step` samples.
fn envelope(sound: &[f32], width: usize, step: usize) -> Vec<f32> {
    let width = width.max(1);
    (0..sound.len().div_ceil(step))
        .map(|i| {
            let centre = i * step;
            let from = centre.saturating_sub(width / 2);
            let to = (centre + width / 2 + 1).min(sound.len());
            let part = &sound[from..to];
            part.iter().map(|s| s * s).sum::<f32>() / part.len().max(1) as f32
        })
        .collect()
}

/// A smooth 0 to 1 over 0 to 1 (a raised cosine).
fn smooth(t: f32) -> f32 {
    0.5 - 0.5 * (t.clamp(0.0, 1.0) * std::f32::consts::PI).cos()
}

/// `channels` stretched to `length` samples with [`Stretch`] alone, its windows `scale` times the default's.
fn bed(channels: &[Vec<f32>], rate: f32, length: usize, scale: f32) -> Option<Vec<Vec<f32>>> {
    let input_samples = channels.iter().map(Vec::len).min().unwrap_or(0);
    let mut stretch =
        Stretch::with_sizes(channels.len(), (rate * 0.12 * scale) as usize, (rate * 0.03 * scale) as usize);
    let mut out = vec![vec![0.0f32; length]; channels.len()];
    let inputs: Vec<&[f32]> = channels.iter().map(Vec::as_slice).collect();
    let mut outputs: Vec<&mut [f32]> = out.iter_mut().map(Vec::as_mut_slice).collect();
    stretch.exact(&inputs, input_samples, &mut outputs, length).then_some(out)
}

/// `channels` (one slice per channel, all as long) stretched to `length` samples the drum way (see the module
/// notes). `None` when the sound is too short for the stretch's windows, as [`crate::to_length`].
pub fn to_length(channels: &[&[f32]], rate: f32, length: usize, options: DrumOptions) -> Option<Vec<Vec<f32>>> {
    let input_samples = channels.iter().map(|c| c.len()).min()?;
    if input_samples == 0 || length == 0 {
        return None;
    }
    let cleaned: Vec<Vec<f32>> =
        channels.iter().map(|c| c[..input_samples].iter().copied().map(clean).collect()).collect();
    let ratio = length as f64 / input_samples as f64;

    // 1. The bed. A sound too short for the long windows is stretched whole with the usual ones.
    let split_bed = || {
        let (lows, highs): (Vec<_>, Vec<_>) = cleaned.iter().map(|c| split(c, options.low_hz, rate)).unzip();
        let low_bed = bed(&lows, rate, length, 2.0)?;
        let mut high_bed = bed(&highs, rate, length, 1.0)?;
        for (h, l) in high_bed.iter_mut().zip(&low_bed) {
            for (x, y) in h.iter_mut().zip(l) {
                *x += y;
            }
        }
        Some(high_bed)
    };
    let mut out = match (options.low_hz > 0.0).then(split_bed).flatten() {
        Some(out) => out,
        None => bed(&cleaned, rate, length, 1.0)?,
    };
    let source = cleaned;

    let mono: Vec<f32> = (0..input_samples).map(|i| source.iter().map(|c| c[i]).sum()).collect();
    let found = hits(&mono, rate);
    if found.is_empty() {
        return Some(out);
    }
    let ms = |n: f32| ((rate * n / 1000.0) as usize).max(1);
    let (pre, fade_in, fade_out, longest) = (ms(2.0), ms(1.0), ms(8.0), ms(options.hit_ms.max(1.0)));
    // Where each hit goes, and the room it has before the next (in both sounds); `None` for a hit that can't be put
    // back (too near the next one or the end), which keeps the bed's own attack and so isn't turned down before.
    let places: Vec<Option<(usize, usize)>> = found
        .iter()
        .enumerate()
        .map(|(n, &h)| {
            let at = (h as f64 * ratio).round() as usize;
            let next_h = found.get(n + 1).copied().unwrap_or(input_samples);
            let next_at = found.get(n + 1).map_or(length, |&q| (q as f64 * ratio).round() as usize);
            let room = next_h.saturating_sub(h).min(next_at.saturating_sub(at)).saturating_sub(pre + fade_out);
            (room >= ms(5.0) && at < length).then_some((at, room))
        })
        .collect();

    // 2. Pre-echo: in the 70 ms before each hit, the bed no louder than the original was at that point in the music.
    if options.redraw > 0.0 {
        let step = ms(1.0);
        let width = ms(5.0);
        let before = ms(70.0);
        let out_mono: Vec<f32> = (0..length).map(|i| out.iter().map(|c| c[i]).sum()).collect();
        let env_in = envelope(&mono, width, step);
        let env_out = envelope(&out_mono, width, step);
        let mut gain = vec![1.0f32; length];
        for (n, (&h, place)) in found.iter().zip(&places).enumerate() {
            if place.is_none() {
                continue;
            }
            let at = (h as f64 * ratio).round() as usize;
            let previous = n.checked_sub(1).map_or(0, |p| (found[p] as f64 * ratio).round() as usize + ms(10.0));
            let from = at.saturating_sub(before).max(previous);
            for (j, g) in gain.iter_mut().enumerate().take(at.min(length)).skip(from) {
                let source_at = ((j as f64 / ratio) as usize).min(input_samples - 1);
                let want = env_in[source_at / step];
                let have = env_out[j / step];
                if have > want && have > 0.0 {
                    let full = (want / have).sqrt().max(0.01);
                    *g = g.min(1.0 - options.redraw.clamp(0.0, 1.0) * (1.0 - full));
                }
            }
        }
        // Smoothed both ways over 2 ms, so the turn-down never clicks.
        let smoothing = 1.0 / ms(2.0) as f32;
        let mut g = 1.0f32;
        for x in gain.iter_mut() {
            g += (*x - g) * smoothing;
            *x = x.min(g);
        }
        g = 1.0;
        for x in gain.iter_mut().rev() {
            g += (*x - g) * smoothing;
            *x = x.min(g);
        }
        for channel in &mut out {
            for (x, g) in channel.iter_mut().zip(&gain) {
                *x *= g;
            }
        }
    }

    // 3. Each hit put back as it was.
    for (&h, place) in found.iter().zip(&places) {
        let Some((at, room)) = *place else { continue };
        let pre = pre.min(h).min(at);
        let fade_in = fade_in.min(pre);
        let keep = best_crossing(&mono, &out, h, at, ms(10.0).min(room), longest.min(room), fade_out);
        for (orig, channel) in source.iter().zip(out.iter_mut()) {
            let from = at - pre;
            let to = (at + keep + fade_out).min(channel.len());
            for (j, y) in channel.iter_mut().enumerate().take(to).skip(from) {
                let Some(&x) = orig.get(j + h - at) else { break };
                let w = if j < from + fade_in {
                    smooth((j - from) as f32 / fade_in as f32)
                } else if j >= at + keep {
                    1.0 - smooth((j - at - keep) as f32 / fade_out as f32)
                } else {
                    1.0
                };
                *y = x * w + *y * (1.0 - w);
            }
        }
    }
    Some(out)
}

/// How long after the attack (between `least` and `most` samples) the original and the stretch line up best over the
/// fade back (`window` samples), all channels together: the highest correlation, tried every 16 samples.
fn best_crossing(
    mono: &[f32],
    stretched: &[Vec<f32>],
    h: usize,
    at: usize,
    least: usize,
    most: usize,
    window: usize,
) -> usize {
    let mut best = (f64::MIN, least);
    for k in (least..=most.max(least)).step_by(16) {
        let Some(a) = mono.get(h + k..h + k + window) else { break };
        if stretched.iter().any(|c| c.len() < at + k + window) {
            break;
        }
        let (mut ab, mut aa, mut bb) = (0.0f64, 1e-12f64, 1e-12f64);
        for (i, &x) in a.iter().enumerate() {
            let y: f32 = stretched.iter().map(|c| c[at + k + i]).sum();
            let (x, y) = (f64::from(x), f64::from(y));
            ab += x * y;
            aa += x * x;
            bb += y * y;
        }
        let score = ab / (aa * bb).sqrt();
        if score > best.0 {
            best = (score, k);
        }
    }
    best.1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::score::score;

    const RATE: f32 = 44_100.0;

    /// Two bars of drums at 120 BPM: a kick on each beat, a snare on 2 and 4, closed hats on the eighths, over a
    /// soft chord.
    fn drums() -> Vec<f32> {
        let beat = 22_050usize;
        let n = beat * 8;
        let mut s: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / RATE;
                0.05 * ((2.0 * std::f32::consts::PI * 220.0 * t).sin() + (2.0 * std::f32::consts::PI * 329.6 * t).sin())
            })
            .collect();
        let mut seed = 7u32;
        let mut noise = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
        };
        for e in 0..16 {
            let at = e * beat / 2;
            for i in 0..beat / 2 {
                let t = i as f32 / RATE;
                let mut v = 0.25 * (-t * 300.0).exp() * noise(); // hat
                if e % 2 == 0 {
                    let phase = 2.0 * std::f32::consts::PI * (45.0 * t + 1.5 * (1.0 - (-t * 30.0).exp()));
                    v += 0.8 * (-t * 8.0).exp() * phase.sin(); // kick
                }
                if e % 4 == 2 {
                    v += 0.5 * (-t * 25.0).exp() * noise(); // snare
                }
                if let Some(x) = s.get_mut(at + i) {
                    *x += v;
                }
            }
        }
        s
    }

    #[test]
    fn hits_are_found_where_they_are() {
        let sound = drums();
        let found = hits(&sound, RATE);
        assert_eq!(found.len(), 16, "one on each eighth: {found:?}");
        for (n, &h) in found.iter().enumerate() {
            let want = n * 11_025;
            assert!(h.abs_diff(want) < 60, "hit {n} at {h}, wanted {want}");
        }
    }

    #[test]
    fn the_crossover_adds_back_up() {
        let tone: Vec<f32> = (0..44_100).map(|i| (i as f32 * 0.004).sin() + (i as f32 * 0.3).sin() * 0.5).collect();
        let (low, high) = split(&tone, 120.0, RATE);
        let sum: Vec<f32> = low.iter().zip(&high).map(|(a, b)| a + b).collect();
        let power = |s: &[f32]| s[4410..].iter().map(|x| x * x).sum::<f32>() / (s.len() - 4410) as f32;
        let worst = sum.iter().zip(&tone).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(worst < 1e-5, "the parts add up to the sound, give or take {worst}");
        assert!(power(&low) > 0.9 * 0.5 && power(&high) < 0.6 * 0.25, "the slow tone is in the low part");
    }

    /// The point of it all: on a drum loop the hits come out sharper and with less ghost than Signalsmith alone,
    /// without the rest moving further from the original.
    #[test]
    fn hits_come_out_better_than_signalsmith_alone() {
        let sound = drums();
        for ratio in [0.8f32, 1.25, 1.6] {
            let length = (sound.len() as f32 * ratio) as usize;
            let plain = crate::to_length(&[&sound], RATE, length, 0.0).unwrap();
            let ours = to_length(&[&sound], RATE, length, DrumOptions::default()).unwrap();
            let a = score(&sound, &plain[0], RATE);
            let b = score(&sound, &ours[0], RATE);
            eprintln!("{ratio}x\n  Signalsmith {a:?}\n  ours        {b:?}");
            assert!(b.ghost_db < a.ghost_db - 0.5 || b.ghost_db < 0.5, "{ratio}x ghost: {b:?} vs {a:?}");
            // Sharpness is a rough measure (the steepest 1 ms climb): close to the original's is what counts.
            assert!((0.75..1.35).contains(&b.sharpness), "{ratio}x sharpness: {b:?} vs {a:?}");
            assert!(b.timing_ms <= a.timing_ms + 0.5, "{ratio}x timing: {b:?} vs {a:?}");
            assert!(b.distance_db < a.distance_db + 0.5, "{ratio}x distance: {b:?} vs {a:?}");
            assert!(ours[0].iter().all(|s| s.is_finite() && s.abs() < 3.0));
        }
    }
}
