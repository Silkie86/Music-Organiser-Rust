//! Where the hits are in a sound, and how strong each one is: for snapping a picked part's edges to hits, and for
//! cutting a sound into slices at its hits (the 1.1 plan's light sample edits).
//!
//! A hit is a sudden rise of the sound's level, weighted to its highs (a drum's attack, a pluck, a sung word's
//! start). Every rise of at least [`LEAST_RISE_DB`] that is the biggest within [`GAP_SECONDS`] each side is a hit,
//! with how big its rise was. A sensitivity is then only a smallest rise to keep ([`at_least`]): turning it up or down
//! never moves a hit, it only shows or hides the weaker ones, so a slider can change it live without looking again.
//!
//! The level is measured as `mo_audio::stretch::hits` measures it (the energy of the sound's sample-to-sample change
//! plus a quarter of its own, every 2.7 ms). A rise is counted from the quietest of the three steps before it, or from
//! the noise floor when that's louder (50 dB under the loudest moment): so the start of a sound out of silence rises
//! by how loud it is, and quiet room noise at a file's start is no hit next to the attack after it. Each hit's frame
//! is where its attack starts, [`LEAD_SECONDS`] early, so a cut there keeps the whole attack, even after
//! `edit::part`'s fade in.

use crate::AudioData;

/// The smallest rise of the level (dB, against the quietest of the three steps before) that is a hit at all.
pub const LEAST_RISE_DB: f32 = 6.0;

/// The rise a sensitivity in the middle keeps: what the app's other hit finders take as a hit.
pub const USUAL_RISE_DB: f32 = 12.0;

/// The most a rise is counted as: the start of a sound out of silence, however loud, counts as this.
pub const MOST_RISE_DB: f32 = 60.0;

/// The shortest time between two hits, either side: a hit is the biggest rise within this of it.
pub const GAP_SECONDS: f64 = 0.030;

/// How long before its attack a hit's frame is: as long as a cut's fade in (`edit::EDGE_IN_SECONDS`), so the fade is
/// over before the attack starts.
pub const LEAD_SECONDS: f64 = crate::edit::EDGE_IN_SECONDS;

/// How far back an attack is looked for, before the step that rose most: a slow attack (a pluck, a sung word) rises
/// over several steps.
const ATTACK_LOOK_BACK_SECONDS: f64 = 0.025;

/// How far under the sound's loudest moment a hit may be (dB), and the quietest level that's a hit at all: quieter is
/// noise.
const FLOOR_UNDER_LOUDEST_DB: f64 = 50.0;
const FLOOR_DB: f64 = -90.0;

/// One hit: the frame its attack starts at (half a millisecond early) and how big its rise was, in dB, from
/// [`LEAST_RISE_DB`] to [`MOST_RISE_DB`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub at: usize,
    pub rise_db: f32,
}

/// The hits in `audio` (its channels mixed to one), in order.
pub fn find(audio: &AudioData) -> Vec<Hit> {
    find_in(&audio.mono(), audio.sample_rate)
}

/// The hits in one channel of sound at `rate` frames a second, in order.
pub fn find_in(sound: &[f32], rate: u32) -> Vec<Hit> {
    let rate = rate.max(1) as usize;
    let hop = (rate / 375).max(16); // 128 frames at 48 kHz, under 3 ms
    let levels: Vec<f64> = sound
        .chunks(hop)
        .scan(0.0f32, |last, chunk| {
            let mut e = 0.0f64;
            for &x in chunk {
                let x = clean(x);
                let change = f64::from(x - *last);
                e += change * change + 0.25 * f64::from(x) * f64::from(x);
                *last = x;
            }
            Some(10.0 * (e / chunk.len() as f64 + 1e-12).log10())
        })
        .collect();
    let Some(loudest) = levels.iter().copied().reduce(f64::max) else { return Vec::new() };
    let floor = (loudest - FLOOR_UNDER_LOUDEST_DB).max(FLOOR_DB);
    // How much each step rose on the quietest of the three before it, or on the floor when that's louder (and at the
    // very start, where there's nothing before).
    let rises: Vec<f64> = (0..levels.len())
        .map(|k| {
            if levels[k] <= floor {
                return 0.0;
            }
            let before = levels[k.saturating_sub(3)..k].iter().copied().reduce(f64::min).unwrap_or(floor).max(floor);
            (levels[k] - before).min(f64::from(MOST_RISE_DB))
        })
        .collect();
    let look_back = (rate as f64 * ATTACK_LOOK_BACK_SECONDS / hop as f64).ceil() as usize;
    let gap = (rate as f64 * GAP_SECONDS / hop as f64).ceil().max(1.0) as usize;
    let mut found: Vec<Hit> = Vec::new();
    for k in 0..rises.len() {
        let rise = rises[k];
        if rise < f64::from(LEAST_RISE_DB) {
            continue;
        }
        // The biggest within the gap each side; of equal ones, the first.
        let window = k.saturating_sub(gap)..(k + gap + 1).min(rises.len());
        let biggest = window.clone().all(|j| if j < k { rises[j] < rise } else { rises[j] <= rise });
        if !biggest {
            continue;
        }
        // A slow attack started rising some steps before its biggest rise: back to the first step of that run (never
        // past the hit before), so the attack is looked for from where it started.
        let after_last = found.last().map_or(0, |h| h.at / hop + 1);
        let mut first = k;
        while first > after_last && k - first < look_back && rises[first - 1] >= f64::from(LEAST_RISE_DB) {
            first -= 1;
        }
        let at = attack_start(sound, first.saturating_sub(1).max(after_last) * hop, (k + 2) * hop, rate);
        // Two hits never share a frame, and they stay in order (an attack found early can't pass the one before).
        if found.last().is_some_and(|h| h.at >= at) {
            continue;
        }
        found.push(Hit { at, rise_db: rise as f32 });
    }
    found
}

/// The hits with a rise of at least `rise_db`: a sensitivity.
pub fn at_least(hits: &[Hit], rise_db: f32) -> impl Iterator<Item = &Hit> + '_ {
    hits.iter().filter(move |h| h.rise_db >= rise_db)
}

/// The rise a sensitivity from 0 (only the strongest hits) to 1 (every hit found) keeps; 0.5 keeps
/// [`USUAL_RISE_DB`].
pub fn rise_for(sensitivity: f32) -> f32 {
    let s = if sensitivity.is_finite() { sensitivity.clamp(0.0, 1.0) } else { 0.5 };
    // 0 -> 30 dB, 0.5 -> 12 dB, 1 -> 6 dB: two straight lines meeting in the middle.
    if s <= 0.5 {
        30.0 - s / 0.5 * (30.0 - USUAL_RISE_DB)
    } else {
        USUAL_RISE_DB - (s - 0.5) / 0.5 * (USUAL_RISE_DB - LEAST_RISE_DB)
    }
}

/// Where an edge at frame `at` goes, snapped: to the nearest hit with a rise of at least `rise_db` no further than
/// `within` frames away, or where it is when there's none.
pub fn snap(at: usize, hits: &[Hit], rise_db: f32, within: usize) -> usize {
    at_least(hits, rise_db)
        .map(|h| h.at)
        .filter(|&h| h.abs_diff(at) <= within)
        .min_by_key(|&h| h.abs_diff(at))
        .unwrap_or(at)
}

/// The first frame of an attack found between `from` and `to`: where the sound first jumps by a fifth of the
/// attack's biggest jump (sample to sample: a low tail before it barely moves), [`LEAD_SECONDS`] before it, but never
/// before `from`'s step (the hit before's).
fn attack_start(sound: &[f32], from: usize, to: usize, rate: usize) -> usize {
    let span = sound.get(from..to.min(sound.len())).unwrap_or_default();
    let jumps = || span.windows(2).map(|w| (clean(w[1]) - clean(w[0])).abs());
    let most = jumps().fold(0.0f32, f32::max);
    let first = jumps().position(|j| j >= most * 0.2).unwrap_or(0);
    let lead = (rate as f64 * LEAD_SECONDS).round() as usize;
    (from + first).saturating_sub(lead)
}

/// A sample as it's measured: one that isn't a number (a damaged file) is silence.
fn clean(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A made-up drum part at 48 kHz: a decaying noisy hit at each `(frame, loudness)`, in silence `len` long.
    pub(crate) fn drums(len: usize, hits: &[(usize, f32)]) -> Vec<f32> {
        let mut sound = vec![0.0f32; len];
        let mut seed = 12_345u32;
        let mut noise = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
        };
        for &(at, loud) in hits {
            for (i, s) in sound[at..].iter_mut().enumerate() {
                let decay = (-(i as f32) / 2_400.0).exp();
                *s += loud * decay * (0.6 * noise() + 0.4 * (i as f32 * 0.07).sin());
            }
        }
        sound
    }

    #[test]
    fn hits_are_found_where_they_start_strongest_first() {
        let at = [4_800, 16_800, 28_800, 40_800, 52_800];
        let sound = drums(60_000, &[(at[0], 0.9), (at[1], 0.2), (at[2], 0.9), (at[3], 0.5), (at[4], 0.9)]);
        let found = find_in(&sound, 48_000);
        assert_eq!(found.len(), at.len(), "{found:?}");
        for (h, &want) in found.iter().zip(&at) {
            // Within two milliseconds before the attack, never after it (a cut there keeps the whole attack).
            assert!(h.at <= want && want - h.at <= 96, "{} for {want}", h.at);
        }
        // The first comes out of silence, as far as the floor (50 dB under the loudest); after it, the louder a hit,
        // the bigger its rise.
        assert!(found[0].rise_db >= 45.0, "{found:?}");
        let rise = |i: usize| found[i].rise_db;
        assert!(rise(1) < rise(3) && rise(3) < rise(2).min(rise(4)), "{found:?}");
    }

    #[test]
    fn a_quieter_hit_in_a_tail_is_weaker_and_a_sensitivity_hides_it() {
        // The second hit lands while the first still rings: it rises less on the level before it.
        let sound = drums(40_000, &[(2_000, 0.9), (12_000, 0.25), (26_000, 0.9)]);
        let found = find_in(&sound, 48_000);
        assert_eq!(found.len(), 3, "{found:?}");
        assert!(found[1].rise_db < found[0].rise_db && found[1].rise_db < found[2].rise_db, "{found:?}");
        let strong: Vec<usize> = at_least(&found, found[1].rise_db + 0.1).map(|h| h.at).collect();
        assert_eq!(strong, vec![found[0].at, found[2].at]);
        // The weak one is never moved by the sensitivity: it's there or hidden.
        let all: Vec<usize> = at_least(&found, LEAST_RISE_DB).map(|h| h.at).collect();
        assert_eq!(all, found.iter().map(|h| h.at).collect::<Vec<_>>());
    }

    #[test]
    fn two_rises_inside_the_gap_are_one_hit_the_bigger() {
        // A flam: two attacks 10 ms apart, the second louder.
        let sound = drums(20_000, &[(4_000, 0.3), (4_480, 0.9)]);
        let found = find_in(&sound, 48_000);
        assert_eq!(found.len(), 1, "{found:?}");
    }

    #[test]
    fn silence_noise_and_damage_have_no_hits() {
        assert!(find_in(&[], 48_000).is_empty());
        assert!(find_in(&[0.0; 10_000], 48_000).is_empty());
        let mut sound = drums(10_000, &[(2_000, 0.8)]);
        sound[100] = f32::NAN;
        sound[5_000] = f32::INFINITY;
        assert_eq!(find_in(&sound, 48_000).len(), 1);
        assert!(find_in(&[0.5; 3], 0).len() <= 1, "a rate of 0 doesn't stop it");
    }

    #[test]
    fn quiet_noise_before_the_first_hit_is_no_hit() {
        // Room noise 45 dB down, then a kick 15 ms in.
        let mut sound = drums(20_000, &[(720, 0.9)]);
        let mut seed = 7u32;
        for s in &mut sound {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *s += ((seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 0.01;
        }
        let found = find_in(&sound, 48_000);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].at <= 720 && 720 - found[0].at <= 96, "{found:?}");
    }

    #[test]
    fn a_slow_attack_is_found_where_it_starts() {
        // A 1760 Hz tone ringing at 0.3, then one that swells in over 10 ms from frame 24,000.
        let tone = |i: usize| (i as f32 * 1_760.0 * std::f32::consts::TAU / 48_000.0).sin();
        let sound: Vec<f32> = (0..36_000)
            .map(|i| {
                let swell = if i < 24_000 { 0.0 } else { ((i - 24_000) as f32 / 480.0).min(1.0) * 0.9 };
                0.3 * tone(i) + swell * tone(i * 3 / 2)
            })
            .collect();
        let found = find_in(&sound, 48_000);
        let near: Vec<&Hit> = found.iter().filter(|h| h.at.abs_diff(24_000) < 2_400).collect();
        assert_eq!(near.len(), 1, "{found:?}");
        assert!(near[0].at <= 24_000 + 48, "{found:?}: inside the attack");
    }

    #[test]
    fn a_sound_that_starts_loud_has_a_hit_at_its_start() {
        let sound = drums(10_000, &[(0, 0.8)]);
        assert_eq!(find_in(&sound, 48_000).first().map(|h| h.at), Some(0));
    }

    #[test]
    fn edges_snap_to_the_nearest_strong_enough_hit_close_by() {
        let hits =
            [Hit { at: 1_000, rise_db: 30.0 }, Hit { at: 2_000, rise_db: 8.0 }, Hit { at: 3_000, rise_db: 20.0 }];
        assert_eq!(snap(1_100, &hits, 6.0, 500), 1_000);
        assert_eq!(snap(1_900, &hits, 6.0, 500), 2_000);
        // The weak hit is hidden at a higher rise: nothing close enough, so it stays.
        assert_eq!(snap(1_900, &hits, 12.0, 500), 1_900);
        assert_eq!(snap(2_600, &hits, 12.0, 500), 3_000);
        assert_eq!(snap(10, &[], 6.0, 500), 10);
    }

    #[test]
    fn the_sensitivity_runs_from_strongest_only_to_every_hit() {
        assert_eq!(rise_for(0.0), 30.0);
        assert_eq!(rise_for(0.5), USUAL_RISE_DB);
        assert_eq!(rise_for(1.0), LEAST_RISE_DB);
        assert!(rise_for(0.25) > rise_for(0.75));
        assert_eq!(rise_for(f32::NAN), USUAL_RISE_DB);
        assert_eq!(rise_for(7.0), LEAST_RISE_DB);
    }
}
