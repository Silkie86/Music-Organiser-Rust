//! Hear in the curve editor: the curve as the volume of a tone, going round, at a speed you choose, with a metronome
//! when you want one (Solomon, 14 Nov 2026: "there should be an option to hear a metronome and change the speed",
//! and what's heard follows the edits while it plays). No screen here: the tone is made from the points, and the
//! place in it is carried over when the tone is made again.

use mo_formats::bitwig::curve::CurvePoint;

use crate::curve_groups;

/// The speed a curve is heard at unless another is chosen (Solomon, 13 Nov: "preview the curve maybe as a volume
/// curve on a sinewave").
pub const BPM_USUAL: f64 = 120.0;
/// The slowest and fastest speeds, and how far a click on minus or plus goes.
pub const BPM_LOW: f64 = 40.0;
pub const BPM_HIGH: f64 = 240.0;
pub const BPM_STEP: f64 = 5.0;
/// The longest stretch heard: 32 bars (a minute at 120); a longer curve is heard from its start.
pub const MOST_BEATS: f64 = 128.0;
/// Beats in a bar: the metronome's first click of each is the higher one.
const BAR: usize = 4;

/// The tone's pitch (A below middle C) and loudness at the curve's top (half: -6 dB).
const TONE_HZ: f64 = 220.0;
const TONE_LEVEL: f64 = 0.5;
const TONE_RATE: u32 = 44_100;
/// How fast the tone's volume follows a jump in the curve, in seconds: quick, but with no click.
const TONE_GLIDE: f64 = 0.002;
/// The metronome's click: its pitch on a bar's first beat and on the others, how loud, and how long it rings.
const CLICK_BAR_HZ: f64 = 1_568.0;
const CLICK_BEAT_HZ: f64 = 1_046.5;
const CLICK_LEVEL: f64 = 0.4;
const CLICK_SECONDS: f64 = 0.04;

/// How a curve is heard: how fast, and whether the metronome clicks on each beat.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hear {
    pub bpm: f64,
    pub click: bool,
}

impl Default for Hear {
    fn default() -> Hear {
        Hear { bpm: BPM_USUAL, click: false }
    }
}

impl Hear {
    /// The speed, kept to what can be heard (a speed that isn't a number is the usual one).
    pub fn speed(self) -> f64 {
        speed_of(self.bpm)
    }
}

/// A speed kept to what can be heard (one that isn't a number is the usual one).
fn speed_of(bpm: f64) -> f64 {
    if bpm.is_finite() { bpm.clamp(BPM_LOW, BPM_HIGH) } else { BPM_USUAL }
}

/// A curve's length in beats (a bar when it isn't known), and how many of them are heard.
fn beats_of(length_beats: f64) -> (f64, f64) {
    let beats = if length_beats.is_finite() && length_beats > 0.0 { length_beats } else { BAR as f64 };
    (beats, beats.min(MOST_BEATS))
}

/// The speed `by` clicks up (or down) from `bpm`: onto the next multiple of [`BPM_STEP`], never past the ends.
pub fn stepped(bpm: f64, by: i32) -> f64 {
    let bpm = speed_of(bpm);
    let on = bpm / BPM_STEP;
    let next = match by.cmp(&0) {
        std::cmp::Ordering::Greater => (on + 1e-9).floor() + f64::from(by),
        std::cmp::Ordering::Less => (on - 1e-9).ceil() + f64::from(by),
        std::cmp::Ordering::Equal => on,
    };
    (next * BPM_STEP).clamp(BPM_LOW, BPM_HIGH)
}

/// The speed in words: "120 BPM".
pub fn speed_words(bpm: f64) -> String {
    let bpm = speed_of(bpm);
    if (bpm - bpm.round()).abs() < 0.05 { format!("{bpm:.0} BPM") } else { format!("{bpm:.1} BPM") }
}

/// How many beats of a curve `length_beats` long are heard (a bar when its length isn't known).
pub fn heard_beats(length_beats: f64) -> f64 {
    beats_of(length_beats).1
}

/// Where to go on from, in seconds, in a tone made again: the beat that was playing (`seconds` into a tone at
/// `was`), in the new tone at `now`, `beats` long. So an edit, another speed or the metronome doesn't start the
/// curve again.
pub fn carried(seconds: f64, was: Hear, now: Hear, beats: f64) -> f64 {
    let beats = heard_beats(beats);
    let beat =
        if seconds.is_finite() { (seconds.max(0.0) * was.speed() / 60.0).rem_euclid(beats.max(1e-6)) } else { 0.0 };
    beat * 60.0 / now.speed()
}

/// The beat playing, `seconds` into a tone of a curve `length_beats` long heard as `hear`.
pub fn beat_at(seconds: f64, hear: Hear, length_beats: f64) -> f64 {
    (seconds.max(0.0) * hear.speed() / 60.0).rem_euclid(heard_beats(length_beats).max(1e-6))
}

/// A curve heard on its own: a sine tone whose volume follows it, as long as the curve is at the speed chosen, made
/// to go round and round with no gap or click where it starts again; with the metronome on, a click on each beat
/// (higher on each bar's first).
pub fn tone(points: &[CurvePoint], length_beats: f64, hear: Hear) -> mo_audio::AudioData {
    let (beats, heard) = beats_of(length_beats);
    let seconds = heard * 60.0 / hear.speed();
    let rate = f64::from(TONE_RATE);
    let frames = ((seconds * rate).round() as usize).max(1);
    // The part of the curve heard (all of it, unless it's longer than the most heard), stretched across, with the
    // first point past it kept for the line to it; drawn at each sample.
    let stretch = beats / heard;
    let mut part: Vec<CurvePoint> = Vec::new();
    for p in points {
        part.push(CurvePoint { x: p.x * stretch, ..*p });
        if p.x * stretch > 1.0 {
            break;
        }
    }
    let drawn = curve_groups::draw(&part, frames);
    // A whole number of the tone's waves in the loop, so where it starts again joins up.
    let waves = (TONE_HZ * seconds).round().max(1.0);
    let step = std::f64::consts::TAU * waves / frames as f64;
    let follow = 1.0 - (-1.0 / (TONE_GLIDE * rate)).exp();
    let mut level = drawn.first().copied().unwrap_or(0.0).clamp(0.0, 1.0);
    let mut samples: Vec<f32> = drawn
        .iter()
        .take(frames)
        .enumerate()
        .map(|(i, v)| {
            level += (v.clamp(0.0, 1.0) - level) * follow;
            ((i as f64 * step).sin() * level * TONE_LEVEL) as f32
        })
        .collect();
    if hear.click {
        click_into(&mut samples, heard, hear.speed());
    }
    mo_audio::AudioData {
        sample_rate: TONE_RATE,
        // One channel: the player plays it to both sides, and a long curve's tone is half the size.
        channels: vec![samples],
        file_seconds: seconds,
        ..Default::default()
    }
}

/// Adds the metronome to a loop `heard` beats long at `bpm`: a short ring on each beat that starts in it, going on
/// round the loop's end into its start.
fn click_into(samples: &mut [f32], heard: f64, bpm: f64) {
    let rate = f64::from(TONE_RATE);
    let frames = samples.len();
    if frames == 0 {
        return;
    }
    let ring = (CLICK_SECONDS * rate) as usize;
    let mut beat = 0usize;
    while (beat as f64) < heard - 1e-9 {
        let from = (beat as f64 * 60.0 / bpm * rate).round() as usize;
        let hz = if beat.is_multiple_of(BAR) { CLICK_BAR_HZ } else { CLICK_BEAT_HZ };
        for i in 0..ring.min(frames) {
            let t = i as f64 / rate;
            // Dies away to a thousandth by its end, so it never stops with a tick of its own.
            let fade = (-6.9 * t / CLICK_SECONDS).exp();
            let s = (std::f64::consts::TAU * hz * t).sin() * fade * CLICK_LEVEL;
            if let Some(out) = samples.get_mut((from + i) % frames) {
                *out = (f64::from(*out) + s).clamp(-1.0, 1.0) as f32;
            }
        }
        beat += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(y: f64) -> Vec<CurvePoint> {
        vec![CurvePoint { x: 0.0, y, bend: 0.0 }, CurvePoint { x: 1.0, y, bend: 0.0 }]
    }

    #[test]
    fn a_curve_is_heard_as_a_tone_that_loops() {
        let fall = [CurvePoint { x: 0.0, y: 1.0, bend: 0.0 }, CurvePoint { x: 1.0, y: 0.0, bend: 0.0 }];
        let tone = tone(&fall, 4.0, Hear::default());
        assert_eq!((tone.sample_rate, tone.channels.len(), tone.len()), (44_100, 1, 88_200), "a bar at 120 is 2 s");
        let loudest = |from: usize, to: usize| tone.channels[0][from..to].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(loudest(0, 2_000) > 0.45, "loud at the top");
        assert!(loudest(80_000, 88_200) < 0.05, "quiet at the bottom");
        // An unknown length is heard as a bar; a very long one only its first 32 bars.
        assert_eq!(super::tone(&fall, 0.0, Hear::default()).len(), 88_200);
        assert_eq!(super::tone(&fall, 1_000.0, Hear::default()).len(), 44_100 * 64);
    }

    #[test]
    fn a_slower_speed_makes_a_longer_tone() {
        let usual = tone(&flat(1.0), 4.0, Hear::default());
        let slow = tone(&flat(1.0), 4.0, Hear { bpm: 60.0, click: false });
        assert!((usual.file_seconds - 2.0).abs() < 1e-9);
        assert!((slow.file_seconds - 4.0).abs() < 1e-9);
        assert_eq!(slow.channels[0].len(), 2 * usual.channels[0].len());
    }

    #[test]
    fn the_metronome_clicks_on_each_beat_and_nowhere_else() {
        // A silent curve: all that's heard is the metronome.
        let quiet = tone(&flat(0.0), 4.0, Hear::default());
        assert!(quiet.channels[0].iter().all(|s| s.abs() < 1e-6));
        let clicked = tone(&flat(0.0), 4.0, Hear { bpm: 120.0, click: true });
        let s = &clicked.channels[0];
        let beat = TONE_RATE as usize / 2;
        for b in 0..4 {
            let near = &s[b * beat..b * beat + 400];
            assert!(near.iter().any(|v| v.abs() > 0.1), "a click on beat {b}");
            let between = &s[b * beat + beat / 2..(b + 1) * beat - 10];
            assert!(between.iter().all(|v| v.abs() < 1e-3), "quiet after beat {b}");
        }
    }

    #[test]
    fn the_place_is_carried_to_a_tone_made_again() {
        let was = Hear::default();
        // Beat 3 of a bar at 120 is 1.5 s in; at 60 it's 3 s in.
        assert!((carried(1.5, was, Hear { bpm: 60.0, click: false }, 4.0) - 3.0).abs() < 1e-9);
        // Round a second time, the same beat; and a curve cut shorter wraps.
        assert!((carried(3.5, was, was, 4.0) - 1.5).abs() < 1e-9);
        assert!((carried(1.5, was, was, 2.0) - 0.5).abs() < 1e-9);
        assert!((beat_at(1.5, was, 4.0) - 3.0).abs() < 1e-9);
    }

    #[test]
    fn the_speed_steps_in_fives_and_stays_in_range() {
        assert_eq!(stepped(120.0, 1), 125.0);
        assert_eq!(stepped(120.0, -1), 115.0);
        assert_eq!(stepped(122.0, 1), 125.0);
        assert_eq!(stepped(122.0, -1), 120.0);
        assert_eq!(stepped(240.0, 1), 240.0);
        assert_eq!(stepped(40.0, -1), 40.0);
        assert_eq!(stepped(f64::NAN, 1), 125.0);
        assert_eq!(speed_words(120.0), "120 BPM");
        assert_eq!(speed_words(87.5), "87.5 BPM");
    }
}
