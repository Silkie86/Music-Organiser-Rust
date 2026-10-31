//! Fitting how the answer is given (the design's E12): the chance each kind of answer needs at each length, and the
//! share of notes in one scale a key needs.
//!
//! - **Thresholds.** At each knot (1, 2, 4, 8, 16 and 32 bars; whole songs take the last), a kind's threshold is the
//!   smallest chance on a 0.01 grid at which the answers of that kind are wrong no more often than its target: one key
//!   15% at 1 and 2 bars, 10% at 4 and 8, 8% at 16, 5% at 32 and for songs; a pair, the key labelled outside it, 10%;
//!   a home note, the wrong tonic, 10%. It's judged on the answers of that kind alone, given by at least
//!   [`MIN_SONGS`] songs, so a few answers that happen to be right can't set it. One key is fitted first, then the
//!   pair on the cases left, then the home note. A threshold never rises as the music gets longer: where a longer
//!   knot needs more, the shorter ones are raised to it.
//!
//!   The design judged "no more often" by the upper end of the error's likely range. On POP909-CL that never let one
//!   key be given at any length (the surest answers are wrong a little under each target, and the range adds two or
//!   three points), so the error itself is used, and the guard against answers that only look sure is the bench's:
//!   each fold is graded with thresholds fitted on the other folds, so the report's "right" for each kind is measured
//!   on songs the thresholds never saw. The upper end is still printed beside each threshold.
//! - **Notes in one scale.** The share below which notes have no key stays as built in (0.75, the design's start).
//!   Its 0.5th percentile over the cases with enough notes is printed: on POP909-CL it's above the design's range
//!   (0.70 to 0.80), and taking it would refuse a fifth of the songs that change key, whose notes span two scales.

use mo_harmony::Key;
use mo_harmony::keyfind::{MIN_NOTES, THRESHOLD_KNOTS, Thresholds, ranked};

use crate::metrics;

/// Most often a key given alone may be wrong, at 1, 2, 4, 8, 16 and 32 bars (whole songs take the last).
pub const ONE_TARGET: [f64; THRESHOLD_KNOTS] = [0.15, 0.15, 0.10, 0.10, 0.08, 0.05];
/// Most often the key labelled may be outside a pair given ("C minor or Eb major").
pub const PAIR_TARGET: f64 = 0.10;
/// Most often a home note given may be the wrong tonic.
pub const HOME_TARGET: f64 = 0.10;
/// The percentile of the cases' shares of notes in one scale that's printed beside the share a key needs.
const NOT_TONAL_PERCENTILE: f64 = 0.005;
/// A threshold no answer can reach: that kind isn't given at that knot.
const NEVER: f64 = 1.0;
/// The fewest songs whose answers a threshold may be judged on.
pub const MIN_SONGS: usize = 30;

/// A case to fit on: its song (for the likely ranges), the key labelled, the knot its length is nearest, its notes
/// and their share in one scale, and its chances.
pub struct Graded {
    pub song: usize,
    pub truth: Key,
    pub knot: usize,
    pub notes: u32,
    pub diatonic_fit: f64,
    pub chances: [f64; 24],
}

/// The knot nearest this many bars heard, on a log scale; whole songs take the last.
pub fn knot_of(bars: u32, whole_song: bool) -> usize {
    if whole_song {
        return THRESHOLD_KNOTS - 1;
    }
    (f64::from(bars.max(1)).log2().round() as usize).min(THRESHOLD_KNOTS - 1)
}

/// What was fitted, and the lines that say how it came out.
pub struct Fitted {
    pub thresholds: Thresholds,
    pub not_tonal_below: f64,
    pub lines: Vec<String>,
}

/// A case as each kind of answer sees it, as (chance, wrong): one key when the first key is ahead of the second, a pair
/// when the second is the first's relative, a home note always; none when the first key isn't clear (a tie that can't
/// be broken, never given a key).
struct Seen {
    song: usize,
    knot: usize,
    one: Option<(f64, bool)>,
    pair: Option<(f64, bool)>,
    home: Option<(f64, bool)>,
}

fn seen(c: &Graded) -> Seen {
    let (ranked, clear) = ranked(&c.chances);
    let [(k1, p1), (k2, p2), ..] = ranked;
    let parallel = ranked.iter().find(|(k, _)| *k == k1.parallel()).map_or(0.0, |(_, p)| *p);
    Seen {
        song: c.song,
        knot: c.knot,
        one: (clear && p1 > p2).then_some((p1, k1 != c.truth)),
        pair: (clear && k2 == k1.relative()).then_some((p1 + p2, c.truth != k1 && c.truth != k2)),
        home: clear.then_some((p1 + parallel, c.truth.tonic != k1.tonic)),
    }
}

/// The smallest threshold on the 0.01 grid at which the answers (each song, chance and whether it's wrong) given at
/// or above it, by at least [`MIN_SONGS`] songs, are wrong no more often than `target`; [`NEVER`] when none is, and
/// `None` when there's nothing to judge by (no answers at all).
fn smallest(answers: &[(usize, f64, bool)], target: f64) -> Option<f64> {
    if answers.is_empty() {
        return None;
    }
    let mut sorted: Vec<&(usize, f64, bool)> = answers.iter().collect();
    sorted.sort_by(|a, b| b.1.total_cmp(&a.1));
    let songs = answers.iter().map(|a| a.0).max().map_or(0, |m| m + 1);
    let mut per_song = vec![(0.0, 0.0); songs];
    let mut next = sorted.iter().peekable();
    let mut best = NEVER;
    for step in (1..100).rev() {
        let threshold = f64::from(step) / 100.0;
        while let Some((song, _, wrong)) = next.next_if(|a| a.1 >= threshold) {
            if let Some((n, e)) = per_song.get_mut(*song) {
                *n += 1.0;
                *e += f64::from(u8::from(*wrong));
            }
        }
        let (n, errors) = per_song.iter().fold((0.0, 0.0), |(a, b), (n, e)| (a + n, b + e));
        let songs = per_song.iter().filter(|(n, _)| *n > 0.0).count();
        if songs >= MIN_SONGS && errors <= target * n {
            best = threshold;
        }
    }
    Some(best)
}

/// The thresholds at every knot from those fitted (`None` where nothing could be judged): each raised to the largest
/// at any longer knot, so a threshold never rises with more music; a knot with nothing to judge by takes the next
/// shorter knot's (or, with none shorter, the next longer one's).
fn never_rising(fitted: [Option<f64>; THRESHOLD_KNOTS]) -> [f64; THRESHOLD_KNOTS] {
    let mut t = fitted;
    let mut longest: Option<f64> = None;
    for slot in t.iter_mut().rev() {
        if let Some(v) = slot {
            let raised = longest.map_or(*v, |l| v.max(l));
            *slot = Some(raised);
            longest = Some(raised);
        }
    }
    let mut out = [NEVER; THRESHOLD_KNOTS];
    for (j, slot) in out.iter_mut().enumerate() {
        let shorter = t.get(..j).and_then(|s| s.iter().rev().find_map(|v| *v));
        let longer = t.get(j..).and_then(|s| s.iter().find_map(|v| *v));
        *slot = t.get(j).copied().flatten().or(shorter).or(longer).unwrap_or(NEVER);
    }
    out
}

/// The cases' shares of notes in one scale at [`NOT_TONAL_PERCENTILE`], and how many fall below `below`.
fn in_one_scale(cases: &[Graded], below: f64) -> (f64, f64) {
    let mut shares: Vec<f64> = cases.iter().filter(|c| c.notes >= MIN_NOTES).map(|c| c.diatonic_fit).collect();
    shares.sort_by(f64::total_cmp);
    let at = ((shares.len() as f64 * NOT_TONAL_PERCENTILE) as usize).min(shares.len().saturating_sub(1));
    let refused = shares.iter().filter(|s| **s < below).count() as f64 / shares.len().max(1) as f64;
    (shares.get(at).copied().unwrap_or(0.0), refused)
}

/// Fits the thresholds on these cases; notes with less than `not_tonal_below` of their time in one scale have no key.
pub fn fit(cases: &[Graded], not_tonal_below: f64) -> Fitted {
    let (percentile, refused) = in_one_scale(cases, not_tonal_below);
    let seen: Vec<Seen> =
        cases.iter().filter(|c| c.notes >= MIN_NOTES && c.diatonic_fit >= not_tonal_below).map(seen).collect();
    let at = |j: usize| seen.iter().filter(move |s| s.knot == j);
    let answers = |j: usize, kind: &dyn Fn(&Seen) -> Option<(f64, bool)>| -> Vec<(usize, f64, bool)> {
        at(j).filter_map(|s| kind(s).map(|(p, wrong)| (s.song, p, wrong))).collect()
    };

    let one_raw: [Option<f64>; THRESHOLD_KNOTS] =
        std::array::from_fn(|j| smallest(&answers(j, &|s| s.one), ONE_TARGET.get(j).copied().unwrap_or(0.05)));
    let one = never_rising(one_raw);
    let given_one = |s: &Seen| s.one.is_some_and(|(p, _)| p >= one.get(s.knot).copied().unwrap_or(NEVER));

    let pair_raw: [Option<f64>; THRESHOLD_KNOTS] =
        std::array::from_fn(|j| smallest(&answers(j, &|s| if given_one(s) { None } else { s.pair }), PAIR_TARGET));
    let pair = never_rising(pair_raw);
    let given_pair = |s: &Seen| s.pair.is_some_and(|(p, _)| p >= pair.get(s.knot).copied().unwrap_or(NEVER));

    let home_raw: [Option<f64>; THRESHOLD_KNOTS] = std::array::from_fn(|j| {
        smallest(&answers(j, &|s| if given_one(s) || given_pair(s) { None } else { s.home }), HOME_TARGET)
    });
    let home = never_rising(home_raw);
    let given_home = |s: &Seen| s.home.is_some_and(|(p, _)| p >= home.get(s.knot).copied().unwrap_or(NEVER));

    let mut lines = vec![format!(
        "Notes in one scale a key needs: {not_tonal_below:.3}, as built in; {:.2}% of these cases have less (the \
         {:.1}th percentile is {percentile:.3}).",
        100.0 * refused,
        NOT_TONAL_PERCENTILE * 100.0
    )];
    let knot_names = ["1 bar", "2 bars", "4 bars", "8 bars", "16 bars", "32 bars and songs"];
    for (j, name) in knot_names.iter().enumerate() {
        let cases: Vec<&Seen> = at(j).collect();
        if cases.is_empty() {
            continue;
        }
        let n = cases.len() as f64;
        let kind = |given: &dyn Fn(&Seen) -> bool, wrong: &dyn Fn(&Seen) -> bool, t: f64, raw: Option<f64>| {
            let mut per_song = vec![(0.0, 0.0); cases.iter().map(|s| s.song).max().map_or(0, |m| m + 1)];
            let mut count = 0.0;
            for s in cases.iter().filter(|s| given(s)) {
                count += 1.0;
                if let Some((g, e)) = per_song.get_mut(s.song) {
                    *g += 1.0;
                    *e += f64::from(u8::from(wrong(s)));
                }
            }
            let errors: f64 = per_song.iter().map(|(_, e)| e).sum();
            let moved = match raw {
                Some(raw) if (t - raw).abs() > 1e-9 => format!(", raised from {raw:.2}"),
                _ => String::new(),
            };
            if t >= NEVER {
                "never".to_owned()
            } else {
                format!(
                    "from {t:.2}{moved}: {:.1}% given, {:.1}% wrong (up to {:.1}%)",
                    100.0 * count / n,
                    100.0 * errors / count.max(1.0),
                    100.0 * metrics::error_upper(&per_song)
                )
            }
        };
        let (t1, t2, t3) = (
            one.get(j).copied().unwrap_or(NEVER),
            pair.get(j).copied().unwrap_or(NEVER),
            home.get(j).copied().unwrap_or(NEVER),
        );
        let (r1, r2, r3) =
            (one_raw.get(j).copied().flatten(), pair_raw.get(j).copied().flatten(), home_raw.get(j).copied().flatten());
        let only_pair = |s: &Seen| !given_one(s) && given_pair(s);
        let only_home = |s: &Seen| !given_one(s) && !given_pair(s) && given_home(s);
        lines.push(format!(
            "  {name} ({} cases): one key {}; key or its relative {}; home note {}.",
            cases.len(),
            kind(&given_one, &|s| s.one.is_some_and(|(_, w)| w), t1, r1),
            kind(&only_pair, &|s| s.pair.is_some_and(|(_, w)| w), t2, r2),
            kind(&only_home, &|s| s.home.is_some_and(|(_, w)| w), t3, r3),
        ));
    }
    Fitted { thresholds: Thresholds { one, pair, home }, not_tonal_below, lines }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mo_harmony::PitchClass;

    const C_MAJOR: Key = Key::major(PitchClass::C);

    /// A case at a knot whose first key has chance `p`, the rest shared evenly.
    fn case(song: usize, knot: usize, truth: Key, p: f64) -> Graded {
        let mut chances = [(1.0 - p) / 23.0; 24];
        chances[C_MAJOR.index()] = p;
        Graded { song, truth, knot, notes: 20, diatonic_fit: 0.95, chances }
    }

    #[test]
    fn lengths_go_to_the_nearest_knot() {
        let knots: Vec<usize> = [1, 2, 3, 4, 5, 6, 7, 8, 11, 12, 16, 23, 24, 64].map(|b| knot_of(b, false)).to_vec();
        assert_eq!(knots, [0, 1, 2, 2, 2, 3, 3, 3, 3, 4, 4, 5, 5, 5]);
        assert_eq!(knot_of(1, true), THRESHOLD_KNOTS - 1);
    }

    #[test]
    fn a_threshold_is_the_smallest_that_keeps_its_answers_right_often_enough() {
        // At 8 bars: 400 songs whose answers at 0.9 are right, 400 at 0.6 right one time in two.
        let mut cases: Vec<Graded> = (0..400).map(|s| case(s, 3, C_MAJOR, 0.9)).collect();
        let wrong = Key::major(PitchClass::D);
        cases.extend((400..800).map(|s| case(s, 3, if s % 2 == 0 { C_MAJOR } else { wrong }, 0.6)));
        let fitted = fit(&cases, 0.75);
        // From 0.61 up only the sure ones are given; at 0.6 the half-wrong ones would be too (25% wrong). The knots
        // with no cases take it too.
        assert_eq!(fitted.thresholds.one, [0.61; THRESHOLD_KNOTS]);
    }

    #[test]
    fn a_few_lucky_answers_dont_set_a_threshold() {
        // Every answer right, but from fewer songs than a threshold may be judged on.
        let cases: Vec<Graded> = (0..MIN_SONGS - 1).map(|s| case(s, 5, C_MAJOR, 0.7)).collect();
        assert_eq!(fit(&cases, 0.75).thresholds.one.get(5), Some(&NEVER));
    }

    #[test]
    fn thresholds_never_rise_with_more_music() {
        let t = never_rising([Some(0.6), Some(0.9), Some(0.7), Some(0.8), Some(0.5), Some(0.75)]);
        assert_eq!(t, [0.9, 0.9, 0.8, 0.8, 0.75, 0.75]);
        // A knot with nothing to judge by takes the next shorter one's, or the next longer one's at the start.
        let t = never_rising([None, Some(0.8), None, Some(0.7), None, None]);
        assert_eq!(t, [0.8, 0.8, 0.8, 0.7, 0.7, 0.7]);
        assert_eq!(never_rising([None; THRESHOLD_KNOTS]), [NEVER; THRESHOLD_KNOTS]);
    }
}
