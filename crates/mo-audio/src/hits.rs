//! Where the hits are in a sound, and how strong each one is: for snapping a picked part's edges to hits, and for
//! cutting a sound into slices at its hits (the 1.1 plan's light sample edits).
//!
//! A hit is a sudden rise of the sound's level (a drum's attack, a pluck, a sung word's start). Every rise of at
//! least [`LEAST_RISE_DB`] that is the biggest within [`GAP_SECONDS`] each side is a hit, with how big its rise was.
//! A sensitivity is then only a smallest rise to keep ([`at_least`]): turning it up or down never moves a hit, it
//! only shows or hides the weaker ones, so a slider can change it live without looking again.
//!
//! The level is measured every 2.7 ms, twice over, and a step's rise is the bigger of the two:
//!
//! - **the whole sound**: the energy of its sample-to-sample change (its highs) plus a quarter of its own, much as
//!   `mo_audio::stretch::hits` measures it. A kick, a snare, a note;
//! - **its highs**: the same, with its own energy counting next to nothing ([`LOWS_IN_HIGHS`]). A hat over a long
//!   808 or a bass note adds little to the whole sound, and is a hit all the same.
//!
//! Both are held for [`HOLD_SECONDS`], and a step's biggest jumps are left out of its change
//! ([`JUMPS_LEFT_OUT`]): a low note's level comes and goes with its wave, and without these a kick's tail, a bass
//! note or a creaky voice was a row of hits.
//!
//! A rise is counted from the quietest of the three steps before it, or from the noise floor when that's louder
//! (50 dB under the loudest moment): so the start of a sound out of silence rises by how loud it is, and quiet room
//! noise at a file's start is no hit next to the attack after it. Each hit's frame is where its attack starts,
//! [`LEAD_SECONDS`] early, so a cut there keeps the whole attack, even after `edit::part`'s fade in.

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

/// How long a step's energy counts for: half a wave of 20 Hz, the lowest note there is. A step is shorter than half
/// a wave of anything under 190 Hz, so a low note read step by step is all but silent at each crossing of its wave
/// and full again a few milliseconds on: a kick's 50 Hz tail "rose" by 12 dB a hundred times a second, and got a hit
/// every 70 ms. A buzzing low note's highs come and go the same way, once a wave. Held this long, a wave no longer
/// shows, and an attack still shows at once. (Longer would hide the second of two hats 40 ms apart behind the first.)
pub const HOLD_SECONDS: f64 = 0.025;

/// How much of a sound's own energy counts in the level of its highs (a quarter of it counts in the whole sound's):
/// 30 dB down. The ear is some 20 to 25 dB keener to a hat's highs than to a bass note's lows, so a hat that sounds
/// as loud as the bass under it has that much less energy, and would add nothing to see to the whole sound's level.
/// With the bass counting this little, such a hat rises by 6 to 9 dB over the bass at its loudest, and by more as
/// the bass rings out; and with it counting more than nothing, a faint hiss far under a loud bass is still no hit.
pub const LOWS_IN_HIGHS: f64 = 1.0 / 1024.0;

/// The share of a step's jumps (sample to sample) left out of its change, the biggest ones: a saw or a square bass
/// jumps once or twice a wave and barely moves between, and those jumps alone made a low note's highs a row of hits,
/// each a wave after the last. A hat's or a snare's jumps are all much alike, so leaving a tenth out changes
/// nothing for them.
pub const JUMPS_LEFT_OUT: f64 = 0.1;

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
    // Each step's energy: of the sound's sample-to-sample change (its highs) without its biggest jumps, and its own.
    let mut jumps: Vec<f64> = Vec::with_capacity(hop);
    let energies: Vec<(f64, f64)> = sound
        .chunks(hop)
        .scan(0.0f32, |last, chunk| {
            let mut own = 0.0f64;
            jumps.clear();
            for &x in chunk {
                let x = clean(x);
                let jump = f64::from(x - *last);
                jumps.push(jump * jump);
                own += f64::from(x) * f64::from(x);
                *last = x;
            }
            let frames = chunk.len();
            let kept = frames - ((frames as f64 * JUMPS_LEFT_OUT).round() as usize).min(frames - 1);
            if kept < frames {
                // The smallest `kept` of them come first.
                jumps.select_nth_unstable_by(kept - 1, f64::total_cmp);
            }
            let change: f64 = jumps[..kept].iter().sum();
            Some((change / frames as f64, own / frames as f64))
        })
        .collect();
    // The two levels of each step, from the most its change and its own energy have been in the last `hold` steps.
    let hold = (rate as f64 * HOLD_SECONDS / hop as f64).ceil().max(1.0) as usize;
    let level = |energy: f64| 10.0 * (energy + 1e-12).log10();
    let (whole, highs): (Vec<f64>, Vec<f64>) = (0..energies.len())
        .map(|k| {
            let (change, own) = energies[(k + 1).saturating_sub(hold)..=k]
                .iter()
                .fold((0.0f64, 0.0f64), |most, e| (most.0.max(e.0), most.1.max(e.1)));
            (level(change + 0.25 * own), level(change + LOWS_IN_HIGHS * own))
        })
        .unzip();
    // A step's rise: the bigger of the whole sound's and its highs'.
    let rises: Vec<f64> = rises_of(&whole).into_iter().zip(rises_of(&highs)).map(|(w, h)| w.max(h)).collect();
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
        // past the hit before), so the attack is looked for from where it started. A step is in the run while it
        // rose by half of what makes a hit: an attack's first steps rise less than its steepest.
        let after_last = found.last().map_or(0, |h| h.at / hop + 1);
        let mut first = k;
        while first > after_last && k - first < look_back && rises[first - 1] >= f64::from(LEAST_RISE_DB) / 2.0 {
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

/// How much each step of `levels` (dB) rose on the quietest of the three before it, or on the floor when that's
/// louder (and at the very start, where there's nothing before); nothing for a step at the floor or under it.
fn rises_of(levels: &[f64]) -> Vec<f64> {
    let Some(loudest) = levels.iter().copied().reduce(f64::max) else { return Vec::new() };
    let floor = (loudest - FLOOR_UNDER_LOUDEST_DB).max(FLOOR_DB);
    (0..levels.len())
        .map(|k| {
            if levels[k] <= floor {
                return 0.0;
            }
            let before = levels[k.saturating_sub(3)..k].iter().copied().reduce(f64::min).unwrap_or(floor).max(floor);
            (levels[k] - before).min(f64::from(MOST_RISE_DB))
        })
        .collect()
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

    /// A made-up kick at 44.1 kHz, `i` frames in: a sweep down to 50 Hz that rings on for a quarter of a second.
    fn kick(i: usize) -> f32 {
        let t = i as f32 / 44_100.0;
        0.7 * (-t / 0.12).exp() * (std::f32::consts::TAU * (50.0 + 90.0 * (-t / 0.03).exp()) * t).sin()
    }

    /// Made-up hats at 44.1 kHz: a short burst of hiss (noise with its lows taken out), `loud` at its start, from each
    /// frame of `at`, in silence `len` long.
    fn hats(len: usize, at: &[usize], loud: f32) -> Vec<f32> {
        let mut sound = vec![0.0f32; len];
        let (mut seed, mut last) = (4_321u32, 0.0f32);
        for &from in at {
            for (i, s) in sound[from..].iter_mut().take(4_410).enumerate() {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let white = (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0;
                *s += loud * (-(i as f32) / 660.0).exp() * (white - last) * 0.5;
                last = white;
            }
        }
        sound
    }

    /// The hits' frames, each checked to be where one of `want` starts: no more than 2 ms before it, never after.
    fn in_place(found: &[Hit], want: &[usize]) {
        assert_eq!(found.len(), want.len(), "{found:?}");
        for (h, &want) in found.iter().zip(want) {
            assert!(h.at <= want && want - h.at <= 96, "{} for {want}: {found:?}", h.at);
        }
    }

    #[test]
    fn a_low_notes_wave_is_no_row_of_hits() {
        // Four kicks, half a second apart. Read step by step, each one's 50 Hz tail is all but silent at every
        // crossing of its wave and full again 5 ms on.
        let beat = 22_050;
        let kicks: Vec<f32> = (0..beat * 4).map(|i| kick(i % beat)).collect();
        let found = find_in(&kicks, 44_100);
        in_place(&found, &[0, beat, beat * 2, beat * 3]);
        assert!(found.iter().all(|h| h.rise_db >= 20.0), "{found:?}");
        // A bass note held for a second: its start, and nothing more. A 55 Hz saw, and one at 33 Hz, whose wave is
        // longer than the levels are held: its one jump a wave is left out of its highs.
        for hz in [55.0f32, 33.0] {
            let saw: Vec<f32> = (0..44_100).map(|i| (i as f32 * hz / 44_100.0).fract() - 0.5).collect();
            in_place(&find_in(&saw, 44_100), &[0]);
        }
        // A creaky low voice: a pulse every 18 ms, each ringing for a few milliseconds.
        let pulse = |i: usize| {
            let t = (i % 794) as f32 / 44_100.0;
            0.4 * (-t / 0.003).exp() * (std::f32::consts::TAU * 700.0 * t).sin()
        };
        let voice: Vec<f32> = (0..44_100).map(pulse).collect();
        in_place(&find_in(&voice, 44_100), &[0]);
    }

    #[test]
    fn a_hat_in_a_kicks_tail_is_a_hit() {
        // A kick on each beat and a hat between, quieter than it: eight hits.
        let beat = 22_050;
        let at: Vec<usize> = (0..8).map(|k| k * beat / 2).collect();
        let off_beats: Vec<usize> = at.iter().copied().filter(|a| a % beat != 0).collect();
        let mut sound = hats(beat * 4, &off_beats, 0.35);
        for (i, s) in sound.iter_mut().enumerate() {
            *s += kick(i % beat);
        }
        let found = find_in(&sound, 44_100);
        in_place(&found, &at);
        assert!(found.iter().all(|h| h.rise_db >= USUAL_RISE_DB), "{found:?}");
    }

    #[test]
    fn hats_over_a_long_bass_note_are_hits() {
        // An 808: a 45 Hz note that rings for over a second, with a hat every quarter of a second over it. A hat adds
        // next to nothing to the whole sound's level, and all the same it's a hit: by its highs.
        let quarter = 11_025;
        let at: Vec<usize> = (0..5).map(|k| k * quarter).collect();
        let mut sound = hats(quarter * 5, &at[1..], 0.3);
        for (i, s) in sound.iter_mut().enumerate() {
            let t = i as f32 / 44_100.0;
            *s += 0.8 * (-t / 0.5).exp() * (std::f32::consts::TAU * (45.0 + 80.0 * (-t / 0.02).exp()) * t).sin();
        }
        let found = find_in(&sound, 44_100);
        in_place(&found, &at);
        assert!(found.iter().all(|h| h.rise_db >= USUAL_RISE_DB), "{found:?}");
        // Without its highs, the bass note alone: its start.
        let bass: Vec<f32> = sound.iter().zip(hats(quarter * 5, &at[1..], 0.3)).map(|(s, h)| s - h).collect();
        in_place(&find_in(&bass, 44_100), &[0]);
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
