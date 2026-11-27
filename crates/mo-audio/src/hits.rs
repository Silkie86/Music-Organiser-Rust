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
//! A low note's level comes and goes with its wave: a step is shorter than half a wave of anything under 190 Hz, so
//! read step by step, a kick's tail, a bass note or a creaky voice was a row of hits. Three things keep a wave from
//! being one:
//!
//! - a step's biggest jumps are left out of its change ([`JUMPS_LEFT_OUT`]): a saw or a square bass jumps once or
//!   twice a wave and barely moves between;
//! - each level is read **held**: the most it has been in the last [`HOLD_SECONDS`]. Held that long, a wave no
//!   longer shows, and an attack still shows at once;
//! - and each is read **followed**: held, but let go as the sound dies away ([`HIGHS_LET_GO_DB`],
//!   [`OWN_LET_GO_DB`]). A rise is measured in the followed level, and counts in full only where the held level
//!   rose with it ([`HELD_RISE_DB`]).
//!
//! The held level alone would do for a wave, but it runs a hold behind a sound that's dying away: a hit that rings
//! into the next one (open hats, a roll, a fast break) still stood at its full level when the next came, which then
//! rose by too little to be a hit. The followed level has died away by then, as the sound has; and the crests of a
//! wave, which do rise in the followed level, never rise in the held one.
//!
//! A rise is counted from the quietest of the three steps before it, or from the noise floor when that's louder
//! (50 dB under the loudest moment): so the start of a sound out of silence rises by how loud it is, and quiet room
//! noise at a file's start is no hit next to the attack after it. Each hit's frame is where its attack starts
//! (`attack_start`), [`LEAD_SECONDS`] early, so a cut there keeps the whole attack, even after `edit::part`'s fade
//! in.

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

/// How long a level is held: half a wave of 20 Hz, the lowest note there is. A step is shorter than half a wave of
/// anything under 190 Hz, so a low note read step by step is all but silent at each crossing of its wave and full
/// again a few milliseconds on: a kick's 50 Hz tail "rose" by 12 dB a hundred times a second, and got a hit every
/// 70 ms. A buzzing low note's highs come and go the same way, once a wave. Held this long, a wave no longer shows.
/// Longer would hide the hits of a fast roll (one every 36 ms) behind each other; so a buzz slower than some 36
/// pulses a second (the creakiest of voices) can't be told from a roll, and reads as a row of hits.
pub const HOLD_SECONDS: f64 = 0.025;

/// How far the followed level of a sound's change (its highs) is let go in one hold, in dB. Less, and a hat in a
/// fast roll (one every 40 ms) still stands on the hat before it; more, and each pulse of a buzzing voice rises
/// from where the last was let go to, by enough to pass for a hit.
pub const HIGHS_LET_GO_DB: f64 = 14.0;

/// How far the followed level of a sound's own energy is let go in one hold, in dB: less than its highs are, since
/// a low note's own energy does come and go, twice a wave, and each crest rises by as far as the last was let go
/// (2 dB for a 40 Hz note: far from a hit). More, and a kick's tail beating against a bass note is a row of weak
/// hits; less, and a kick over a bass note is found less often.
pub const OWN_LET_GO_DB: f64 = 4.5;

/// How far the held level must rise for a rise of the followed level to count in full, in dB. Where it rose by
/// less, the rise counts for that share (half of it at 1.5 dB), and for nothing where the held level didn't rise:
/// that's a wave's crest, or a sound that only stopped dying away.
pub const HELD_RISE_DB: f64 = 3.0;

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

/// Where an attack starts is looked for by level first, in stretches a step long, this many to a step: so to a
/// third of a millisecond.
const ATTACK_STRETCHES_A_STEP: usize = 8;

/// An attack is on by the first stretch whose level is this far over the most it was in the hold before (dB): more
/// than a steady hiss wavers by.
const ATTACK_OVER_BEFORE_DB: f64 = 1.5;

/// ...and no further than this under the most it gets to (dB): a fifth of the attack's biggest jump, as its first
/// frame is found by. A bass note swelling a little under a hat is far further under the hat than this.
const ATTACK_UNDER_MOST_DB: f64 = 14.0;

/// How long before that stretch's end the attack's first frame is looked for, in steps: an attack that's only just
/// louder than what came before fills this much of the stretch before the stretch is over it.
const ATTACK_BACK_STEPS: f64 = 0.75;

/// One hit: the frame its attack starts at (a millisecond early) and how big its rise was, in dB, from
/// [`LEAST_RISE_DB`] to [`MOST_RISE_DB`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub at: usize,
    pub rise_db: f32,
}

/// The two energies of a stretch of sound: of its sample-to-sample change (its highs) without its biggest jumps, and
/// its own.
#[derive(Clone, Copy, Debug, Default)]
struct Energy {
    change: f64,
    own: f64,
}

impl Energy {
    /// Of `stretch`, which the sample `last` comes before, counted as `frames` long: where it's shorter (at the
    /// sound's start), the rest is the silence before it. `jumps` is room to work in.
    fn of(stretch: &[f32], mut last: f32, frames: usize, jumps: &mut Vec<f64>) -> Self {
        let mut own = 0.0f64;
        jumps.clear();
        for &x in stretch {
            let x = clean(x);
            let jump = f64::from(x - last);
            jumps.push(jump * jump);
            own += f64::from(x) * f64::from(x);
            last = x;
        }
        let frames = frames.max(stretch.len()).max(1);
        let left_out = ((frames as f64 * JUMPS_LEFT_OUT).round() as usize).min(frames - 1);
        let kept = jumps.len().saturating_sub(left_out);
        if kept > 0 && kept < jumps.len() {
            // The smallest `kept` of them come first.
            jumps.select_nth_unstable_by(kept - 1, f64::total_cmp);
        }
        let change: f64 = jumps[..kept].iter().sum();
        Self { change: change / frames as f64, own: own / frames as f64 }
    }

    /// The whole sound's energy: its highs and a quarter of its own.
    fn whole(self) -> f64 {
        self.change + 0.25 * self.own
    }

    /// Its highs' energy: its own counts next to nothing.
    fn highs(self) -> f64 {
        self.change + LOWS_IN_HIGHS * self.own
    }

    /// The whole sound's energy, or its highs'.
    fn by(self, highs: bool) -> f64 {
        if highs { self.highs() } else { self.whole() }
    }

    /// The bigger of each energy.
    fn most(self, other: Self) -> Self {
        Self { change: self.change.max(other.change), own: self.own.max(other.own) }
    }

    /// The smaller of each energy.
    fn least(self, other: Self) -> Self {
        Self { change: self.change.min(other.change), own: self.own.min(other.own) }
    }

    /// Halfway between the two.
    fn with(self, other: Self) -> Self {
        Self { change: (self.change + other.change) / 2.0, own: (self.own + other.own) / 2.0 }
    }
}

/// The hits in `audio` (its channels mixed to one), in order.
pub fn find(audio: &AudioData) -> Vec<Hit> {
    find_in(&audio.mono(), audio.sample_rate)
}

/// The hits in one channel of sound at `rate` frames a second, in order.
pub fn find_in(sound: &[f32], rate: u32) -> Vec<Hit> {
    let rate = rate.max(1) as usize;
    let hop = (rate / 375).max(16); // 128 frames at 48 kHz, under 3 ms
    let mut jumps: Vec<f64> = Vec::with_capacity(hop);
    let energies: Vec<Energy> = sound
        .chunks(hop)
        .scan(0.0f32, |last, step| {
            let energy = Energy::of(step, *last, step.len(), &mut jumps);
            *last = step.last().copied().map_or(*last, clean);
            Some(energy)
        })
        .collect();
    let (rises_whole, rises_highs) = rises(&energies, rate, hop);
    // A step's rise: the bigger of the whole sound's and its highs'.
    let rises: Vec<f64> = rises_whole.iter().zip(&rises_highs).map(|(w, h)| w.max(*h)).collect();
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
        let span = first.saturating_sub(1).max(after_last) * hop..((k + 2) * hop).min(sound.len());
        let at = attack_start(sound, span, (k + 1) * hop, rises_highs[k] > rises_whole[k], rate, hop, &mut jumps);
        // Two hits never share a frame, and they stay in order (an attack found early can't pass the one before).
        if found.last().is_some_and(|h| h.at >= at) {
            continue;
        }
        found.push(Hit { at, rise_db: rise as f32 });
    }
    found
}

/// How much each step rose (dB), in the whole sound's level and in its highs': the rise of its followed level,
/// counted in full where its held level rose by [`HELD_RISE_DB`] with it, for less where that rose by less, and for
/// nothing where it didn't.
fn rises(energies: &[Energy], rate: usize, hop: usize) -> (Vec<f64>, Vec<f64>) {
    let hold = (rate as f64 * HOLD_SECONDS / hop as f64).ceil().max(1.0) as usize;
    let level = |energy: f64| 10.0 * (energy + 1e-12).log10();
    // What's left of an energy one step on, let go by so many dB a hold.
    let left = |db: f64| 10f64.powf(-db / hold as f64 / 10.0);
    let (left_of_change, left_of_own) = (left(HIGHS_LET_GO_DB), left(OWN_LET_GO_DB));
    let steps = energies.len();
    let (mut followed_whole, mut followed_highs) = (Vec::with_capacity(steps), Vec::with_capacity(steps));
    let (mut held_whole, mut held_highs) = (Vec::with_capacity(steps), Vec::with_capacity(steps));
    let mut let_go = Energy::default();
    for k in 0..steps {
        let since = (k + 1).saturating_sub(hold);
        // Held: the most of the hold, each step taken with the one before it. A short pulse (a creaky voice's, the
        // edge of a saw's wave) falls now in one step, now across two; step by step, its level rose by up to 3 dB
        // each time it fell in one.
        let held = (since..=k).fold(Energy::default(), |most, j| {
            most.most(if j == 0 { energies[0] } else { energies[j].with(energies[j - 1]) })
        });
        // Followed: the most of the hold too, but no more than what the loudest step so far has been let go to.
        let most = energies[since..=k].iter().fold(Energy::default(), |most, e| most.most(*e));
        let_go = Energy {
            change: energies[k].change.max(let_go.change * left_of_change),
            own: energies[k].own.max(let_go.own * left_of_own),
        };
        let followed = most.least(let_go);
        followed_whole.push(level(followed.whole()));
        followed_highs.push(level(followed.highs()));
        held_whole.push(level(held.whole()));
        held_highs.push(level(held.highs()));
    }
    let counted = |followed: &[f64], held: &[f64]| -> Vec<f64> {
        rises_of(followed)
            .into_iter()
            .zip(rises_of(held))
            .map(|(rise, held)| rise * (held / HELD_RISE_DB).clamp(0.0, 1.0))
            .collect()
    };
    (counted(&followed_whole, &held_whole), counted(&followed_highs, &held_highs))
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

/// The first frame of the attack found in `span`, [`LEAD_SECONDS`] before it. `risen` is where the step that rose
/// most ends (the attack is on by then), and `highs` says the hit was found by its highs, not by the whole sound.
///
/// It's looked for in two goes. First by level, in stretches a step long, an eighth of a step apart: the attack is
/// on by the first stretch whose level is over the most it was in the hold before `span` by
/// [`ATTACK_OVER_BEFORE_DB`], and within [`ATTACK_UNDER_MOST_DB`] of the most it gets to. Then by the frame, in the
/// three quarters of a step before that stretch's end: the first that jumps by a fifth of the biggest jump there (a
/// low tail before an attack barely moves from sample to sample).
///
/// The second go alone, over the whole span, put a hit on the edge of a bass's wave: a saw or a square bass under
/// the drums jumps as far as a hat does, so a hit's frame landed up to 5 ms before the drum, or after it. And where
/// the hit before still rang, on that: a few milliseconds early. The first go isn't fooled by either: a wave's edges
/// are among the jumps left out, what's left of them was there in the hold before, and a tail is quieter than it
/// was. Where no stretch passes by `risen` (a weak hit, soon after a louder sound), the whole span is looked in: a
/// frame found there is early, never late.
fn attack_start(
    sound: &[f32],
    span: std::ops::Range<usize>,
    risen: usize,
    highs: bool,
    rate: usize,
    hop: usize,
    jumps: &mut Vec<f64>,
) -> usize {
    let (from, to) = (span.start, span.end.min(sound.len()));
    let stride = (hop / ATTACK_STRETCHES_A_STEP).max(1);
    // The energy of the stretch that ends at `end`.
    let mut ending = |end: usize| {
        let start = end.saturating_sub(hop);
        let last = if start == 0 { 0.0 } else { clean(sound[start - 1]) };
        Energy::of(&sound[start..end], last, hop, jumps).by(highs)
    };
    // The most the level was in the hold before the span.
    let hold = (rate as f64 * HOLD_SECONDS).ceil() as usize;
    let mut before = 0.0f64;
    let mut end = from;
    while end > 0 && from - end <= hold {
        before = before.max(ending(end));
        end = end.saturating_sub(stride);
    }
    let ends: Vec<usize> = (1..).map(|j| from + j * stride).take_while(|&end| end <= to).collect();
    let stretches: Vec<f64> = ends.iter().map(|&end| ending(end)).collect();
    let most = stretches.iter().copied().fold(0.0f64, f64::max);
    let ratio = |db: f64| 10f64.powf(db / 10.0);
    let least = (before * ratio(ATTACK_OVER_BEFORE_DB)).max(most * ratio(-ATTACK_UNDER_MOST_DB));
    let on = stretches.iter().position(|&energy| energy > 0.0 && energy >= least).filter(|&on| ends[on] <= risen);
    let looked_in = match on {
        Some(on) => ends[on].saturating_sub((hop as f64 * ATTACK_BACK_STEPS) as usize).max(from)..ends[on],
        None => from..to,
    };
    let jumped = || sound[looked_in.clone()].windows(2).map(|w| (clean(w[1]) - clean(w[0])).abs());
    let biggest = jumped().fold(0.0f32, f32::max);
    let first = jumped().position(|jump| jump >= biggest * 0.2).unwrap_or(0);
    let lead = (rate as f64 * LEAD_SECONDS).round() as usize;
    (looked_in.start + first).saturating_sub(lead)
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
        // A creaky low voice: a pulse every 18 ms, each ringing for a few milliseconds; and every 22 and 14 ms,
        // where a pulse falls now in one step, now across two.
        for every in [794, 980, 630] {
            let pulse = |i: usize| {
                let t = (i % every) as f32 / 44_100.0;
                0.4 * (-t / 0.003).exp()
                    * ((std::f32::consts::TAU * 700.0 * t).sin() + 0.6 * (std::f32::consts::TAU * 1_200.0 * t).sin())
            };
            let voice: Vec<f32> = (0..88_200).map(pulse).collect();
            in_place(&find_in(&voice, 44_100), &[0]);
        }
    }

    /// Made-up hits at 44.1 kHz that ring: noise (hiss when `hiss`, else a snare's noise and tone) from each frame of
    /// `at`, dying away to a third in `decay` frames.
    fn ringing(len: usize, at: &[usize], decay: f32, hiss: bool) -> Vec<f32> {
        let mut sound = vec![0.0f32; len];
        let (mut seed, mut last) = (77u32, 0.0f32);
        for &from in at {
            for (i, s) in sound[from..].iter_mut().take((decay * 8.0) as usize).enumerate() {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let white = (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0;
                let voice = if hiss {
                    (white - last) * 0.5
                } else {
                    0.7 * white + 0.3 * (i as f32 * 190.0 * std::f32::consts::TAU / 44_100.0).sin()
                };
                *s += 0.5 * (-(i as f32) / decay).exp() * voice;
                last = white;
            }
        }
        sound
    }

    #[test]
    fn hits_that_ring_into_each_other_are_all_hits() {
        // Sixteen hits as loud as each other, each still ringing when the next comes: open hats a 16th apart, a
        // snare roll, a fast roll of hats. (frames between them, frames to die away to a third, hiss)
        for (every, decay, hiss) in
            [(3_660, 2_205.0, false), (2_756, 1_544.0, false), (2_381, 882.0, false), (1_764, 662.0, true)]
        {
            let at: Vec<usize> = (0..16).map(|k| 2_000 + k * every).collect();
            let sound = ringing(every * 18 + 44_100, &at, decay, hiss);
            let found = find_in(&sound, 44_100);
            assert_eq!(found.len(), at.len(), "every {every}, decay {decay}: {found:?}");
            for (h, &want) in found.iter().zip(&at) {
                assert!(h.at <= want && want - h.at <= 110, "every {every}: {} for {want}", h.at);
            }
            // Each rises by what the usual sensitivity keeps: it died away that far before the next.
            assert!(found.iter().all(|h| h.rise_db >= USUAL_RISE_DB), "every {every}, decay {decay}: {found:?}");
        }
    }

    #[test]
    fn drums_over_a_buzzing_bass_start_where_the_drums_do() {
        // Hats an 8th apart at 120, each a little off the grid (so it starts anywhere in a step and in the bass's
        // wave), over a saw bass, a square one, and a saw with its highs taken off: a bass's wave jumps once or
        // twice a wave, as far as a hat does or further.
        let at: Vec<usize> = (1..16).map(|k| k * 11_025 + (k * 37) % 113).collect();
        // A bass: its name, its note (Hz), its wave by how far through it is (0 to 1), and how much of its highs is
        // left (1: all).
        type Bass = (&'static str, f32, fn(f32) -> f32, f32);
        let basses: [Bass; 4] = [
            ("a saw", 55.0, |phase| 2.0 * phase - 1.0, 1.0),
            ("a higher saw", 164.8, |phase| 2.0 * phase - 1.0, 1.0),
            ("a square", 82.4, |phase| if phase < 0.5 { 1.0 } else { -1.0 }, 1.0),
            ("a duller saw", 55.0, |phase| 2.0 * phase - 1.0, 0.35),
        ];
        for (name, hz, shape, open) in basses {
            let mut sound = hats(11_025 * 17, &at, 0.5);
            let mut dull = (0.0f32, 0.0f32);
            for (i, s) in sound.iter_mut().enumerate() {
                let raw = shape((i as f32 * hz / 44_100.0).fract());
                dull.0 += open * (raw - dull.0);
                dull.1 += open * (dull.0 - dull.1);
                *s += 0.3 * dull.1;
            }
            let found = find_in(&sound, 44_100);
            // The bass's own start, then the hats: each within 2.5 ms before the hat, never after it.
            assert_eq!(found.len(), at.len() + 1, "{name}: {found:?}");
            assert_eq!(found[0].at, 0, "{name}");
            for (h, &want) in found[1..].iter().zip(&at) {
                assert!(h.at <= want && want - h.at <= 110, "{name}: {} for {want}", h.at);
            }
        }
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
