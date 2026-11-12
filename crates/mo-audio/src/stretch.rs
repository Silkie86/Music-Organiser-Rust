//! A loop played at another tempo without its pitch changing (the `timestretch` library, MIT, written in Rust).
//!
//! It's for hearing a loop at a DAW's tempo: the sound is made again, a little longer or shorter, and played as it
//! is. Nothing is written: the file stays as it is.
//!
//! The library is a phase vocoder: good on tones, but it smears a drum's attack into the few milliseconds before it
//! and moves it a little. So each hit is then put back as it was, where the new tempo puts it, and the stretch fills
//! the time between hits: the idea of harmonic-percussive stretching (Driedger, Müller and Ewert, 2014: tones
//! through a phase vocoder, hits kept short) and of a DAW's beat slicing, done at the hits found in the sound.

use crate::AudioData;

/// The widest a loop is stretched, either way: past this (a 70 BPM loop at 175) it's played as if at half or twice
/// its own tempo (140 BPM's worth here), as a DAW's double-time and half-time do. Within it (85 at 125) it's stretched
/// as it is, so it keeps its feel.
const MOST: f64 = 1.5;

/// The tempo `from` is best stretched to for `to`: its own, halved or doubled until it's within [`MOST`] of `to`
/// either way. `None` when either isn't a real tempo.
pub fn nearest(from: f64, to: f64) -> Option<f64> {
    if !(from.is_finite() && to.is_finite() && from > 0.0 && to > 0.0) {
        return None;
    }
    let mut from = from;
    for _ in 0..8 {
        if to / from > MOST {
            from *= 2.0;
        } else if from / to > MOST {
            from /= 2.0;
        } else {
            return Some(from);
        }
    }
    None
}

/// `audio`, a loop at `from` BPM, played at `to` BPM with its pitch kept: longer when `to` is slower. Mono and
/// stereo only (a sound with more channels plays its first two). The longer the sound the longer it takes (about a
/// tenth of a second for a few bars, more for a minute): call it off any thread that mustn't wait.
pub fn to_tempo(audio: &AudioData, from: f64, to: f64) -> Result<AudioData, String> {
    stretch(audio, from, to, true)
}

/// [`to_tempo`], with the hits put back or not (`hits`: only the tests' measure turns it off).
fn stretch(audio: &AudioData, from: f64, to: f64, hits: bool) -> Result<AudioData, String> {
    let from = nearest(from, to).ok_or("That isn't a tempo a loop can be played at")?;
    let channels = audio.channels.len().clamp(1, 2);
    let frames = audio.channels.first().map_or(0, Vec::len);
    if frames == 0 {
        return Ok(audio.clone());
    }
    let mut interleaved = Vec::with_capacity(frames * channels);
    for i in 0..frames {
        for c in 0..channels {
            let sample = audio.channels.get(c).and_then(|ch| ch.get(i)).copied().unwrap_or(0.0);
            interleaved.push(if sample.is_finite() { sample } else { 0.0 });
        }
    }
    let tempo = timestretch::TempoRate::from_bpm(from, to).map_err(|e| format!("Can't play it at {to:.0} BPM: {e}"))?;
    let config = timestretch::SessionConfig {
        source_sample_rate: audio.sample_rate,
        output_sample_rate: audio.sample_rate,
        channels,
        rate_policy: timestretch::RatePolicy::AllowConversion,
        parameters: timestretch::Parameters::keylock(tempo),
        ..timestretch::SessionConfig::default()
    };
    let out = mo_catch::catch(|| timestretch::render(&interleaved, config))
        .map_err(|_| "The stretch stopped on something it didn't expect".to_owned())?
        .map_err(|e| format!("Couldn't stretch it: {e}"))?;
    let split = deinterleave(&out.samples, channels);
    // Each hit put back as it was, where the new tempo puts it: the stretch smears a drum's attack and moves it a few
    // milliseconds. A hit that can't be put back leaves the stretch's own sound there.
    let original = audio.channels.get(..channels).unwrap_or_default();
    let split = if !hits {
        split
    } else {
        // The app's own code, so a panic in it is a bug, logged in full.
        match mo_catch::catch_crash(move || keep_hits(original, split, from / to, audio.sample_rate)) {
            Ok(kept) => kept,
            // Made again from the stretch alone.
            Err(_) => deinterleave(&out.samples, channels),
        }
    };
    // The fields one by one: `..audio.clone()` would copy every sample only to drop them.
    Ok(AudioData {
        sample_rate: audio.sample_rate,
        channels: split,
        file_seconds: audio.file_seconds * from / to,
        embedded_tempo: audio.embedded_tempo,
        embedded_root_note: audio.embedded_root_note,
        embedded_is_loop: audio.embedded_is_loop,
    })
}

/// `samples` (frames of `channels` samples one after another) as one list per channel.
fn deinterleave(samples: &[f32], channels: usize) -> Vec<Vec<f32>> {
    let mut split: Vec<Vec<f32>> = (0..channels).map(|_| Vec::with_capacity(samples.len() / channels)).collect();
    for frame in samples.chunks_exact(channels) {
        for (ch, s) in split.iter_mut().zip(frame) {
            ch.push(*s);
        }
    }
    split
}

/// A sample as it's stretched: one that isn't a number (a damaged file) is silence.
fn clean(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}

/// Where the hits are in `sound` (one channel, at `rate`): the frames their attacks start at, in order. A hit is
/// a sudden rise of the sound's level, weighted to its highs (a drum's attack), at least 30 ms after the one before.
pub fn hits(sound: &[f32], rate: u32) -> Vec<usize> {
    let hop = (rate as usize / 375).max(16); // 128 frames at 48 kHz, under 3 ms
    let levels: Vec<f64> = sound
        .chunks(hop)
        .scan(0.0f32, |last, chunk| {
            let mut e = 0.0f64;
            for &x in chunk {
                let x = clean(x);
                let rise = f64::from(x - *last);
                e += rise * rise + 0.25 * f64::from(x) * f64::from(x);
                *last = x;
            }
            Some(10.0 * (e / chunk.len() as f64 + 1e-12).log10())
        })
        .collect();
    let loudest = levels.iter().copied().fold(f64::MIN, f64::max);
    let gap = (rate as usize * 30 / 1000).div_ceil(hop).max(1);
    let mut found: Vec<usize> = Vec::new();
    let mut k = 0;
    while k < levels.len() {
        // Before the sound starts counts as silence.
        let before = match k {
            0 => -240.0,
            _ => levels[k.saturating_sub(3)..k].iter().copied().fold(f64::MAX, f64::min),
        };
        // 12 dB up on the level just before, and no quieter than 50 dB under the loudest moment (nor near silence).
        if levels[k] - before > 12.0 && levels[k] > (loudest - 50.0).max(-90.0) {
            // The rise's steepest hop, within the next two.
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

/// The first frame of an attack found between `from` and `to`: where the sound first jumps by a fifth of the
/// attack's biggest jump (sample to sample: a low tail before it barely moves), a little before it (half a
/// millisecond).
fn attack_start(sound: &[f32], from: usize, to: usize, rate: u32) -> usize {
    let span = sound.get(from..to.min(sound.len())).unwrap_or_default();
    let jumps = || span.windows(2).map(|w| (clean(w[1]) - clean(w[0])).abs());
    let most = jumps().fold(0.0f32, f32::max);
    let first = jumps().position(|j| j >= most * 0.2).unwrap_or(0);
    (from + first).saturating_sub(rate as usize / 2000)
}

/// Puts each hit of `original` back into `stretched` (made `d` times as long) where its new time is, as it was:
/// from 12 ms before the attack (the quiet the stretch smeared the attack into) to up to 40 ms after it, faded in and
/// out of the stretch. The way back to the stretch is at the spot within that where the two sounds line up best, so
/// nothing cancels as they cross.
fn keep_hits(original: &[Vec<f32>], mut stretched: Vec<Vec<f32>>, d: f64, rate: u32) -> Vec<Vec<f32>> {
    let Some(first) = original.first() else { return stretched };
    // The hits found in both channels together (a mono sound as it is, unless a sample needs cleaning).
    let mono: std::borrow::Cow<[f32]> = if original.len() == 1 && first.iter().all(|x| x.is_finite()) {
        std::borrow::Cow::Borrowed(first.as_slice())
    } else {
        (0..first.len()).map(|i| original.iter().map(|ch| ch.get(i).copied().map_or(0.0, clean)).sum()).collect()
    };
    let found = hits(&mono, rate);
    let ms = |n: usize| rate as usize * n / 1000;
    let (before, fade_in, fade_out, longest) = (ms(12), ms(2).max(1), ms(8).max(1), ms(40));
    let out_len = stretched.first().map_or(0, Vec::len);
    for (n, &o) in found.iter().enumerate() {
        let p = (o as f64 * d).round() as usize;
        // Room up to the next hit, in both sounds (that one is put back at its own place).
        let next_o = found.get(n + 1).copied().unwrap_or(first.len());
        let next_p = found.get(n + 1).map_or(out_len, |&q| (q as f64 * d).round() as usize);
        let room = next_o.saturating_sub(o).min(next_p.saturating_sub(p)).saturating_sub(before + fade_out);
        let pre = before.min(o).min(p);
        // A hit at the very start has no quiet before it to fade in over: it's taken whole from its first frame.
        let fade_in = fade_in.min(pre);
        if room < ms(5) || p >= out_len {
            continue;
        }
        let keep = best_crossing(&mono, &stretched, o, p, ms(10).min(room), longest.min(room), fade_out);
        for (orig, out) in original.iter().zip(stretched.iter_mut()) {
            let from = p - pre;
            let to = (p + keep + fade_out).min(out.len());
            for j in from..to {
                let Some(&x) = orig.get(j + o - p) else { break };
                let x = clean(x);
                let w = if j < from + fade_in {
                    smooth((j - from) as f32 / fade_in as f32)
                } else if j >= p + keep {
                    1.0 - smooth((j - p - keep) as f32 / fade_out as f32)
                } else {
                    1.0
                };
                if let Some(y) = out.get_mut(j) {
                    *y = x * w + *y * (1.0 - w);
                }
            }
        }
    }
    stretched
}

/// How long after the attack (between `least` and `most` frames) the original and the stretch line up best over the
/// fade back (`window` frames), both channels together: the highest correlation, found every 16 frames.
fn best_crossing(
    mono: &[f32],
    stretched: &[Vec<f32>],
    o: usize,
    p: usize,
    least: usize,
    most: usize,
    window: usize,
) -> usize {
    let mut best = (f64::MIN, least);
    for k in (least..=most.max(least)).step_by(16) {
        let Some(a) = mono.get(o + k..o + k + window) else { break };
        if stretched.iter().any(|ch| ch.len() < p + k + window) {
            break;
        }
        let (mut ab, mut aa, mut bb) = (0.0f64, 1e-12f64, 1e-12f64);
        for (i, x) in a.iter().enumerate() {
            let y: f32 = stretched.iter().map(|ch| ch[p + k + i]).sum();
            let (x, y) = (f64::from(*x), f64::from(y));
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

/// A smooth 0 to 1 over 0 to 1 (a raised cosine).
fn smooth(t: f32) -> f32 {
    0.5 - 0.5 * (t.clamp(0.0, 1.0) * std::f32::consts::PI).cos()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tempo_far_off_is_halved_or_doubled_first() {
        assert_eq!(nearest(120.0, 128.0), Some(120.0));
        assert_eq!(nearest(70.0, 175.0), Some(140.0));
        assert_eq!(nearest(174.0, 87.0), Some(87.0));
        assert_eq!(nearest(85.0, 125.0), Some(85.0), "within the widest stretch: kept as it is");
        assert_eq!(nearest(0.0, 120.0), None);
        assert_eq!(nearest(f64::NAN, 120.0), None);
    }

    /// Two bars at 120 BPM, 48 kHz: a click on every beat over a quiet tone.
    fn loop_at_120() -> AudioData {
        let rate = 48_000_usize;
        let beat = rate / 2;
        let samples: Vec<f32> = (0..beat * 8)
            .map(|i| {
                let t = (i % beat) as f32 / rate as f32;
                let click = if i % beat < 200 { 0.8 * (-t * 200.0).exp() } else { 0.0 };
                click + 0.1 * (i as f32 * 220.0 * std::f32::consts::TAU / rate as f32).sin()
            })
            .collect();
        AudioData {
            sample_rate: 48_000,
            channels: vec![samples.clone(), samples],
            file_seconds: 4.0,
            embedded_tempo: None,
            embedded_root_note: None,
            embedded_is_loop: Some(true),
        }
    }

    #[test]
    fn a_loop_at_another_tempo_is_as_long_as_that_tempo_says() {
        let audio = loop_at_120();
        for to in [100.0, 128.0, 140.0] {
            let out = to_tempo(&audio, 120.0, to).unwrap();
            let want = (8.0 * 24_000.0) * 120.0 / to;
            let got = out.channels[0].len() as f64;
            assert!((got - want).abs() / want < 0.02, "{to} BPM: {got} frames, wanted about {want}");
            assert_eq!(out.channels.len(), 2);
            assert!(out.channels[0].iter().all(|s| s.is_finite()));
            assert!(out.channels[0].iter().any(|s| s.abs() > 0.3), "{to} BPM: the clicks are still there");
            // Each beat's click lands where that tempo puts the beat, give or take 10 ms.
            let beat = 24_000.0 * 120.0 / to;
            for n in 1..7 {
                let (a, e) = (((n as f64 - 0.25) * beat) as usize, ((n as f64 + 0.25) * beat) as usize);
                let loudest = (a..e.min(out.channels[0].len()))
                    .max_by(|&x, &y| out.channels[0][x].abs().total_cmp(&out.channels[0][y].abs()))
                    .unwrap();
                let off = (loudest as f64 - n as f64 * beat).abs() / 48.0;
                assert!(off < 10.0, "{to} BPM: beat {n} is {off:.1} ms off");
            }
        }
    }
}

#[cfg(test)]
mod measure {
    use super::*;

    /// Two bars of drums at 120 BPM, 48 kHz: kicks, snares, eighth-note hats. Onsets in frames.
    pub fn drums() -> (AudioData, Vec<usize>) {
        let rate = 48_000.0_f32;
        let beat = 24_000usize;
        let n = beat * 8;
        let mut s = vec![0.0f32; n];
        let mut seed = 1u32;
        let mut noise = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
        };
        let mut onsets = Vec::new();
        for e in 0..16 {
            let at = e * beat / 2;
            onsets.push(at);
            let kind = if e % 4 == 0 {
                0
            } else if e % 4 == 2 {
                1
            } else {
                2
            };
            let len = [12_000, 9_000, 2_000][kind];
            let mut phase = 0.0f32;
            for i in 0..len.min(n - at) {
                let t = i as f32 / rate;
                let v = match kind {
                    0 => {
                        phase += (50.0 + 120.0 * (-t * 30.0).exp()) * std::f32::consts::TAU / rate;
                        0.9 * phase.sin() * (-t * 8.0).exp()
                    }
                    1 => {
                        0.6 * noise() * (-t * 25.0).exp()
                            + 0.3 * (t * 190.0 * std::f32::consts::TAU).sin() * (-t * 30.0).exp()
                    }
                    _ => 0.3 * noise() * (-t * 120.0).exp(),
                };
                s[at + i] += v;
            }
        }
        let audio = AudioData {
            sample_rate: 48_000,
            channels: vec![s.clone(), s],
            file_seconds: 4.0,
            embedded_tempo: None,
            embedded_root_note: None,
            embedded_is_loop: Some(true),
        };
        (audio, onsets)
    }

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>() / x.len().max(1) as f64).sqrt()
    }

    /// Per hit: the level just before it (smear ahead of the hit, in dB over the original's) and the attack's
    /// peak (dB under the original's). Averages over the hits.
    pub fn smear(orig: &[f32], out: &[f32], onsets: &[usize], d: f64) -> (f64, f64) {
        let (mut pre, mut peak, mut k) = (0.0, 0.0, 0.0);
        for &o in onsets.iter().skip(1) {
            let p = (o as f64 * d).round() as usize;
            if p + 480 > out.len() || o + 480 > orig.len() {
                continue;
            }
            let pk = |x: &[f32]| x.iter().fold(0.0f32, |m, v| m.max(v.abs())) as f64;
            let before_out = rms(&out[p - 480..p - 48]);
            pre += 20.0 * ((before_out + 1e-6) / pk(&orig[o..o + 240])).log10();
            peak += 20.0 * (pk(&out[p..p + 240]) / pk(&orig[o..o + 240])).log10();
            k += 1.0;
        }
        (pre / k, peak / k)
    }

    /// How far the stretched sound's level (5 ms at a time) is from the original's moved to the new tempo: the mean
    /// difference in dB where the original is no quieter than 40 dB under its loudest.
    pub fn envelope(orig: &[f32], out: &[f32], d: f64) -> f64 {
        let w = 240;
        let loudest = orig.chunks(w).map(rms).fold(0.0, f64::max);
        let (mut sum, mut n) = (0.0, 0.0);
        for (i, chunk) in out.chunks(w).enumerate() {
            let at = ((i * w) as f64 / d) as usize;
            let len = ((w as f64) / d) as usize;
            let Some(src) = orig.get(at..at + len) else { break };
            let (a, b) = (rms(src), rms(chunk));
            if a > loudest * 0.01 {
                sum += (20.0 * ((b + 1e-9) / a).log10()).abs();
                n += 1.0;
            }
        }
        sum / n
    }

    /// Where each hit starts in `out`, less where the new tempo puts it, in frames: the first jump sample to sample
    /// of a fifth of the hit's biggest, looked for from 2 ms before (9999 when there's none).
    fn hit_offsets(orig: &[f32], out: &[f32], onsets: &[usize], d: f64) -> Vec<i64> {
        onsets
            .iter()
            .skip(1)
            .map(|&on| {
                let p = (on as f64 * d) as usize;
                let peak = orig[on..on + 240].windows(2).fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()));
                // From 2 ms before: a louder hit's tail runs into a quiet hat's time.
                let from = p - 96;
                let Some(first) = (from..p + 720).find(|&i| (out[i] - out[i - 1]).abs() > peak * 0.05) else {
                    return 9999;
                };
                let first_orig = (on - 96..on + 720).find(|&i| (orig[i] - orig[i - 1]).abs() > peak * 0.2).unwrap();
                first as i64 - p as i64 - (first_orig as i64 - on as i64)
            })
            .collect()
    }

    #[test]
    fn drum_hits_stay_sharp_and_on_time() {
        let (audio, onsets) = drums();
        let found = hits(&audio.channels[0], 48_000);
        assert_eq!(found.len(), onsets.len(), "every hit found once: {found:?}");
        for (f, o) in found.iter().zip(&onsets) {
            assert!(f.abs_diff(*o) < 48, "found at {f}, made at {o}");
        }
        let orig = &audio.channels[0];
        for to in [90.0, 110.0, 140.0, 160.0] {
            let d = 120.0 / to;
            let out = to_tempo(&audio, 120.0, to).unwrap();
            let plain = stretch(&audio, 120.0, to, false).unwrap();
            let (before, peak) = smear(orig, &out.channels[0], &onsets, d);
            let (before_plain, _) = smear(orig, &plain.channels[0], &onsets, d);
            assert!(
                before < -60.0,
                "{to} BPM: {before:.1} dB just before the hits (the stretch alone: {before_plain:.1})"
            );
            assert!(peak.abs() < 1.0, "{to} BPM: the attacks' peaks {peak:+.1} dB");
            let late = hit_offsets(orig, &out.channels[0], &onsets, d);
            assert!(late.iter().all(|f| f.abs() <= 24), "{to} BPM: hits off by {late:?} frames (half a ms is 24)");
            let (level, level_plain) = (envelope(orig, &out.channels[0], d), envelope(orig, &plain.channels[0], d));
            assert!(
                level < level_plain + 0.5,
                "{to} BPM: level off by {level:.2} dB, the stretch alone {level_plain:.2}"
            );
            assert_eq!(out.channels[0].len(), plain.channels[0].len());
            // The first hit, at the very start, is the original's from its first frame.
            assert_eq!(&out.channels[0][..240], &orig[..240], "{to} BPM: the downbeat kept whole");
            assert!(out.channels.iter().flatten().all(|s| s.is_finite()));
        }
    }

    #[test]
    fn hits_stay_on_time_to_the_end_of_a_long_loop_and_damage_is_silence() {
        // Sixteen bars (32 s): any lag or drift in the stretch would show by the last hits.
        let (bars, onsets) = drums();
        let len = bars.channels[0].len();
        let long: Vec<f32> = bars.channels[0].iter().cycle().take(len * 8).copied().collect();
        let all: Vec<usize> = (0..8).flat_map(|r| onsets.iter().map(move |o| o + r * len)).collect();
        let mut audio = AudioData { channels: vec![long.clone(), long], file_seconds: 32.0, ..bars };
        let out = to_tempo(&audio, 120.0, 140.0).unwrap();
        let late = hit_offsets(&audio.channels[0], &out.channels[0], &all[all.len() - 9..], 120.0 / 140.0);
        assert!(late.iter().all(|f| f.abs() <= 24), "the last hits off by {late:?} frames");
        // A damaged sample in a hit: the stretch and the hits put back are all numbers.
        audio.channels[1][24_000 + 10] = f32::NAN;
        audio.channels[0][48_000 + 10] = f32::INFINITY;
        let out = to_tempo(&audio, 120.0, 100.0).unwrap();
        assert!(out.channels.iter().flatten().all(|s| s.is_finite()));
    }

    #[test]
    fn a_tone_that_swells_has_no_hits_but_its_start() {
        // A pad: in at once, then swelling slowly up and down. Only its start is a hit.
        let tone: Vec<f32> =
            (0..96_000).map(|i| (0.2 + 0.15 * (i as f32 / 9_000.0).sin()) * (i as f32 * 0.03).sin()).collect();
        let found = hits(&tone, 48_000);
        assert!(found.len() <= 1 && found.iter().all(|&f| f < 2_400), "{found:?}");
        assert!(hits(&[], 48_000).is_empty());
        assert!(hits(&[f32::NAN; 500], 48_000).is_empty());
    }
}
