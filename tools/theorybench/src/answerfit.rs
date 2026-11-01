//! Fitting how the answer is given (the design's E12): the chance each kind of answer needs at each length, and the
//! share of notes in one scale a key needs.
//!
//! - **Thresholds.** At each knot (1, 2, 4, 8, 16 and 32 bars; whole songs take the last), a kind's threshold is the
//!   smallest chance on a 0.01 grid, from an even chance up ([`LEAST`]), at which the answers of that kind are wrong
//!   no more often than its target: one key
//!   15% at 1 and 2 bars, 10% at 4 and 8, 8% at 16, 5% at 32 and for songs; a pair, the key labelled outside it, 10%;
//!   a home note, the wrong tonic, 10%. It's judged on the answers of that kind alone, given by at least
//!   [`MIN_SONGS`] songs, so a few answers that happen to be right can't set it. One key is fitted first, then the
//!   pair on the cases left, then the home note, each as `keyfind::grade` tries them (a test checks they agree). A
//!   threshold never rises as the music gets longer: where a longer knot needs more, the shorter ones are raised to it.
//! - **Lengths between knots.** The app draws a straight line (on a log scale of bars) between the knots either side,
//!   so a case is fitted at the knot at or above its length (a 3-bar case at 4 bars): with thresholds that never rise,
//!   the line there is never below that knot's, so the app is at least as strict there as the fit was.
//!
//!   The design judged "no more often" by the upper end of the error's likely range. On POP909-CL that never let one
//!   key be given at any length (the surest answers are wrong a little under each target, and the range adds two or
//!   three points), so the error itself is used. The guard against answers that only look sure is the bench's: each
//!   fold is graded with thresholds fitted on the other folds' chances. The upper end is still printed beside each
//!   threshold.
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
/// A threshold no answer can reach (`keyfind::grade` never gives a kind whose threshold is 1): that kind isn't given at
/// that knot.
const NEVER: f64 = 1.0;
/// No kind of answer is given below an even chance: what it names must be likelier than not. Without this floor the
/// pair's threshold fell to the grid's 0.01 from 8 bars, where nearly every pair the bench sees is right. The few below
/// 0.5 (about 0.2% of the cases from 5 bars up, all from a part heard alone) were wrong far more often, and music
/// unlike the songs fitted on could have been shown a pair at, say, 0.3.
pub const LEAST: f64 = 0.5;
/// The fewest songs whose answers a threshold may be judged on.
pub const MIN_SONGS: usize = 30;

/// A case to fit on: its song (for the likely ranges), the key labelled, the knot it's fitted at ([`knot_of`]), its
/// notes and their share in one scale, and its chances.
pub struct Graded {
    pub song: usize,
    pub truth: Key,
    pub knot: usize,
    pub notes: u32,
    pub diatonic_fit: f64,
    pub chances: [f64; 24],
}

/// The knot a case is fitted at: the first at or above this many bars heard (1, 2, 4, 8, 16, 32); whole songs, and
/// anything longer, take the last.
pub fn knot_of(bars: u32, whole_song: bool) -> usize {
    if whole_song {
        return THRESHOLD_KNOTS - 1;
    }
    (bars.max(1).next_power_of_two().trailing_zeros() as usize).min(THRESHOLD_KNOTS - 1)
}

/// What was fitted, and lines that say how it came out: one about the share of notes in one scale, one per knot.
pub struct Fitted {
    pub thresholds: Thresholds,
    pub not_tonal_below: f64,
    pub scale_line: String,
    pub knot_lines: Vec<String>,
}

/// The kinds of answer a threshold is fitted for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    One,
    Pair,
    Home,
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

impl Seen {
    /// The kind of answer given with these thresholds for one key, a pair and a home note, tried in that order as
    /// `keyfind::grade` tries them; `None` for no clear key.
    fn given(&self, one: f64, pair: f64, home: f64) -> Option<Kind> {
        let reaches = |kind: Option<(f64, bool)>, t: f64| t < NEVER && kind.is_some_and(|(p, _)| p >= t);
        if reaches(self.one, one) {
            Some(Kind::One)
        } else if reaches(self.pair, pair) {
            Some(Kind::Pair)
        } else if reaches(self.home, home) {
            Some(Kind::Home)
        } else {
            None
        }
    }

    /// The chance and whether it's wrong, as this kind of answer.
    fn as_kind(&self, kind: Kind) -> Option<(f64, bool)> {
        match kind {
            Kind::One => self.one,
            Kind::Pair => self.pair,
            Kind::Home => self.home,
        }
    }
}

/// Answers counted by song: how many, how many wrong, and from how many songs.
struct Tally {
    per_song: Vec<(f64, f64)>,
    n: f64,
    errors: f64,
    songs: usize,
}

impl Tally {
    fn new(songs: usize) -> Tally {
        Tally { per_song: vec![(0.0, 0.0); songs], n: 0.0, errors: 0.0, songs: 0 }
    }

    fn add(&mut self, song: usize, wrong: bool) {
        if let Some((n, e)) = self.per_song.get_mut(song) {
            if *n == 0.0 {
                self.songs += 1;
            }
            *n += 1.0;
            *e += f64::from(u8::from(wrong));
            self.n += 1.0;
            self.errors += f64::from(u8::from(wrong));
        }
    }
}

/// The smallest threshold on the 0.01 grid, [`LEAST`] or more, at which the answers (each song, chance and whether it's
/// wrong) given at or above it, by at least [`MIN_SONGS`] songs, are wrong no more often than `target`; [`NEVER`] when
/// none is. `None` when there's too little to judge by: fewer than [`MIN_SONGS`] songs even with every answer given.
fn smallest(answers: &[(usize, f64, bool)], target: f64) -> Option<f64> {
    let mut sorted: Vec<&(usize, f64, bool)> = answers.iter().collect();
    sorted.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut tally = Tally::new(answers.iter().map(|a| a.0).max().map_or(0, |m| m + 1));
    let mut next = sorted.iter().peekable();
    let mut best = NEVER;
    for step in (1..100).rev() {
        let threshold = f64::from(step) / 100.0;
        if threshold < LEAST {
            break;
        }
        while let Some((song, _, wrong)) = next.next_if(|a| a.1 >= threshold) {
            tally.add(*song, *wrong);
        }
        if tally.songs >= MIN_SONGS && tally.errors <= target * tally.n {
            best = threshold;
        }
    }
    // Every answer counted (the lowest chances too): still too few songs to judge by.
    for (song, _, wrong) in next {
        tally.add(*song, *wrong);
    }
    (tally.songs >= MIN_SONGS).then_some(best)
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
    // The answers of a kind at a knot, among the cases `left` keeps.
    let answers = |j: usize, kind: Kind, left: &dyn Fn(&Seen) -> bool| -> Vec<(usize, f64, bool)> {
        at(j).filter(|s| left(s)).filter_map(|s| s.as_kind(kind).map(|(p, wrong)| (s.song, p, wrong))).collect()
    };
    let knot = |t: &[f64; THRESHOLD_KNOTS], j: usize| t.get(j).copied().unwrap_or(NEVER);

    let one_raw: [Option<f64>; THRESHOLD_KNOTS] =
        std::array::from_fn(|j| smallest(&answers(j, Kind::One, &|_| true), knot(&ONE_TARGET, j)));
    let one = never_rising(one_raw);
    // The pair is fitted on the cases not given one key, the home note on those given neither.
    let pair_raw: [Option<f64>; THRESHOLD_KNOTS] = std::array::from_fn(|j| {
        let left = |s: &Seen| s.given(knot(&one, j), NEVER, NEVER).is_none();
        smallest(&answers(j, Kind::Pair, &left), PAIR_TARGET)
    });
    let pair = never_rising(pair_raw);
    let home_raw: [Option<f64>; THRESHOLD_KNOTS] = std::array::from_fn(|j| {
        let left = |s: &Seen| s.given(knot(&one, j), knot(&pair, j), NEVER).is_none();
        smallest(&answers(j, Kind::Home, &left), HOME_TARGET)
    });
    let home = never_rising(home_raw);

    let scale_line = format!(
        "Notes in one scale a key needs: {not_tonal_below:.3}, as built in; {:.2}% of these cases have less (the {:.1}th \
         percentile is {percentile:.3}).",
        100.0 * refused,
        NOT_TONAL_PERCENTILE * 100.0
    );
    let knot_names = ["1 bar", "2 bars", "3 to 4 bars", "5 to 8 bars", "9 to 16 bars", "17 bars or more, and songs"];
    let mut knot_lines = Vec::new();
    for (j, name) in knot_names.iter().enumerate() {
        let cases: Vec<&Seen> = at(j).collect();
        if cases.is_empty() {
            continue;
        }
        let songs = cases.iter().map(|s| s.song).max().map_or(0, |m| m + 1);
        let given = |s: &Seen| s.given(knot(&one, j), knot(&pair, j), knot(&home, j));
        let say = |kind: Kind, t: f64, raw: Option<f64>| {
            if t >= NEVER {
                return "never".to_owned();
            }
            let mut tally = Tally::new(songs);
            for s in cases.iter().filter(|s| given(s) == Some(kind)) {
                tally.add(s.song, s.as_kind(kind).is_some_and(|(_, wrong)| wrong));
            }
            let raised = match raw {
                Some(raw) if (t - raw).abs() > 1e-9 => format!(", raised from {raw:.2}"),
                _ => String::new(),
            };
            if tally.n == 0.0 {
                return format!("from {t:.2}{raised}: none given");
            }
            format!(
                "from {t:.2}{raised}: {:.1}% given, {:.1}% wrong (up to {:.1}%)",
                100.0 * tally.n / cases.len() as f64,
                100.0 * tally.errors / tally.n,
                100.0 * metrics::error_upper(&tally.per_song)
            )
        };
        knot_lines.push(format!(
            "{name} ({} cases): one key {}; key or its relative {}; home note {}.",
            cases.len(),
            say(Kind::One, knot(&one, j), one_raw.get(j).copied().flatten()),
            say(Kind::Pair, knot(&pair, j), pair_raw.get(j).copied().flatten()),
            say(Kind::Home, knot(&home, j), home_raw.get(j).copied().flatten()),
        ));
    }
    Fitted { thresholds: Thresholds { one, pair, home }, not_tonal_below, scale_line, knot_lines }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mo_harmony::PitchClass;
    use mo_harmony::keyfind::{self, Grading, KeyAnswer, Span};

    use crate::metrics::Rng;

    const C_MAJOR: Key = Key::major(PitchClass::C);

    /// A case at a knot whose first key has chance `p`, the rest shared evenly.
    fn case(song: usize, knot: usize, truth: Key, p: f64) -> Graded {
        let mut chances = [(1.0 - p) / 23.0; 24];
        chances[C_MAJOR.index()] = p;
        Graded { song, truth, knot, notes: 20, diatonic_fit: 0.95, chances }
    }

    #[test]
    fn lengths_are_fitted_at_the_knot_at_or_above_them() {
        let knots = [1, 2, 3, 4, 5, 6, 8, 9, 16, 17, 32, 33, 64].map(|b| knot_of(b, false));
        assert_eq!(knots, [0, 1, 2, 2, 3, 3, 3, 4, 4, 5, 5, 5, 5]);
        assert_eq!((knot_of(0, false), knot_of(1, true)), (0, THRESHOLD_KNOTS - 1));
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
        // Every answer right, but from fewer songs than a threshold may be judged on: nothing to judge by, so the knot
        // borrows the next shorter knot's threshold rather than shutting that kind off at every shorter length.
        let mut cases: Vec<Graded> = (0..MIN_SONGS - 1).map(|s| case(s, 5, C_MAJOR, 0.7)).collect();
        cases.extend((100..400).map(|s| case(s, 3, C_MAJOR, 0.9)));
        let one = fit(&cases, 0.75).thresholds.one;
        assert_eq!((one[3], one[5]), (LEAST, LEAST));
        // With enough songs and the target out of reach, that kind is never given there, nor at shorter lengths.
        let wrong = Key::major(PitchClass::D);
        cases.extend((1000..1100).map(|s| case(s, 5, wrong, 0.95)));
        let one = fit(&cases, 0.75).thresholds.one;
        assert_eq!((one[3], one[5]), (NEVER, NEVER));
    }

    #[test]
    fn nothing_is_given_below_an_even_chance() {
        // Every answer right, at 0.6 or at 0.3: any threshold would do, but none is below 0.5.
        let mut cases: Vec<Graded> = (0..400).map(|s| case(s, 3, C_MAJOR, 0.6)).collect();
        cases.extend((400..800).map(|s| case(s, 3, C_MAJOR, 0.3)));
        let fitted = fit(&cases, 0.75);
        assert_eq!(fitted.thresholds.one, [LEAST; THRESHOLD_KNOTS]);
        assert!(
            fitted.knot_lines.iter().any(|l| l.contains("one key from 0.50: 50.0% given")),
            "{:?}",
            fitted.knot_lines
        );
        // With nothing as likely as that, the kind is never given.
        let unsure: Vec<Graded> = (0..400).map(|s| case(s, 3, C_MAJOR, 0.3)).collect();
        assert_eq!(fit(&unsure, 0.75).thresholds.one, [NEVER; THRESHOLD_KNOTS]);
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

    #[test]
    fn the_fit_gives_each_kind_of_answer_as_the_app_does() {
        // Chances of every shape, for every key (a clear first key; the relative or the parallel tied with it or close
        // behind; every key tied), graded both ways.
        let mut rng = Rng::new(65);
        let mut unit = || (rng.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
        let mut kinds = [0; 4];
        for i in 0..24_000 {
            let mut chances: [f64; 24] = std::array::from_fn(|_| unit().powi(4));
            let first = Key::ALL[(i / 6) % 24];
            chances[first.index()] += 2.0 * unit();
            match i % 6 {
                0 => chances[first.relative().index()] = chances[first.index()],
                1 => chances[first.parallel().index()] = chances[first.index()],
                2 => chances[first.relative().index()] = chances[first.index()] * unit(),
                3 => chances[first.parallel().index()] = chances[first.index()] * unit(),
                4 => chances = [1.0; 24],
                _ => {}
            }
            let total: f64 = chances.iter().sum();
            let chances = chances.map(|p| p / total);
            let (one, pair, home) = (unit(), unit(), unit());
            let thresholds = Thresholds {
                one: [one; THRESHOLD_KNOTS],
                pair: [pair; THRESHOLD_KNOTS],
                home: [home; THRESHOLD_KNOTS],
            };
            let model = keyfind::Model { thresholds: &thresholds, ..keyfind::SHIPPED };
            let grading = Grading { bars_heard: 4, notes: 20, diatonic_fit: 0.9, span: Span::Window };
            let app = match keyfind::grade(&chances, grading, &model).0 {
                KeyAnswer::Key(_) => Some(Kind::One),
                KeyAnswer::OrRelative(_) => Some(Kind::Pair),
                KeyAnswer::HomeNote { .. } => Some(Kind::Home),
                KeyAnswer::Unclear { .. } => None,
            };
            let fit = seen(&Graded { song: 0, truth: first, knot: 2, notes: 20, diatonic_fit: 0.9, chances });
            assert_eq!(fit.given(one, pair, home), app, "{chances:?} at {one}, {pair}, {home}");
            kinds[app.map_or(3, |k| k as usize)] += 1;
        }
        assert!(kinds.iter().all(|n| *n > 500), "every kind is tried: {kinds:?}");
    }
}
