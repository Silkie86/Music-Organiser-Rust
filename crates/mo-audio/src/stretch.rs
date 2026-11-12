//! A loop played at another tempo without its pitch changing (the `timestretch` library, MIT, written in Rust).
//!
//! It's for hearing a loop at a DAW's tempo: the sound is made again, a little longer or shorter, and played as it
//! is. Nothing is written: the file stays as it is.

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
    let mut split: Vec<Vec<f32>> = (0..channels).map(|_| Vec::with_capacity(out.samples.len() / channels)).collect();
    for frame in out.samples.chunks_exact(channels) {
        for (c, s) in frame.iter().enumerate() {
            if let Some(ch) = split.get_mut(c) {
                ch.push(*s);
            }
        }
    }
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
