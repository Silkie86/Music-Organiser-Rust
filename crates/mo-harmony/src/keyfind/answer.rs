//! The answer as a person would give it ([`KeyAnswer`]): one key when the finder is sure enough; a key or its relative
//! when the notes are clear but home isn't; a home note when the tonic is clear but major or minor isn't; or no clear
//! key, and why.
//!
//! How sure is sure enough depends on how much music was heard ([`Thresholds`]): a one-bar loop must be surer than a
//! song before its key is given alone. Every rule here moves with the notes, ties included: notes moved up a semitone
//! give the same answer moved up a semitone.

use crate::key::{Key, Mode};
use crate::pitch::PitchClass;

use super::{Model, Span};

/// Notes needed for an answer: fewer give [`Why::TooFewNotes`].
pub const MIN_NOTES: u32 = 4;

/// The answer to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KeyAnswer {
    /// One key.
    Key(Key),
    /// A key or its relative ("C minor or Eb major"), the likelier first: the notes fit both, and which note is home
    /// isn't clear.
    OrRelative(Key),
    /// The home note is clear, major or minor isn't (a vamp that never plays the third, a blues). `leaning` is the
    /// likelier mode, when one is likelier.
    HomeNote { tonic: PitchClass, leaning: Option<Mode> },
    /// No clear key: why, and the likeliest key when one is likeliest.
    Unclear { why: Why, leaning: Option<Key> },
}

/// Why there's no clear key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Why {
    /// Fewer than [`MIN_NOTES`] notes.
    TooFewNotes,
    /// The notes keep to no one scale (whole-tone, octatonic, all twelve notes), so there's no key to find.
    NotTonal,
    /// The chances are spread over keys that aren't a relative or parallel pair.
    Spread,
}

impl KeyAnswer {
    /// The key the answer gives or leans to: the key, the likelier of a pair, the home note in its likelier mode.
    /// `None` when it leans nowhere (a home note with both modes as likely, or no clear key with no likeliest key).
    pub fn leaning_key(self) -> Option<Key> {
        match self {
            KeyAnswer::Key(k) | KeyAnswer::OrRelative(k) => Some(k),
            KeyAnswer::HomeNote { tonic, leaning } => leaning.map(|mode| Key { tonic, mode }),
            KeyAnswer::Unclear { leaning, .. } => leaning,
        }
    }

    /// The home note the answer gives: a key's tonic, the likelier tonic of a pair, or the home note. `None` when the
    /// key isn't clear.
    pub fn tonic(self) -> Option<PitchClass> {
        match self {
            KeyAnswer::Key(k) | KeyAnswer::OrRelative(k) => Some(k.tonic),
            KeyAnswer::HomeNote { tonic, .. } => Some(tonic),
            KeyAnswer::Unclear { .. } => None,
        }
    }

    /// Whether the answer names `key` as the key, or as one of the two it gives (a pair, or a home note's two modes).
    pub fn names(self, key: Key) -> bool {
        match self {
            KeyAnswer::Key(k) => k == key,
            KeyAnswer::OrRelative(k) => k == key || k.relative() == key,
            KeyAnswer::HomeNote { tonic, .. } => tonic == key.tonic,
            KeyAnswer::Unclear { .. } => false,
        }
    }

    /// Every key the answer gives or leans to, the likelier first: the key; a pair's two keys; a home note in the mode
    /// it leans to, then the other (major first when it leans to neither); the key no clear key leans to.
    pub fn keys(self) -> [Option<Key>; 2] {
        match self {
            KeyAnswer::Key(k) => [Some(k), None],
            KeyAnswer::OrRelative(k) => [Some(k), Some(k.relative())],
            KeyAnswer::HomeNote { tonic, leaning } => {
                let first = Key { tonic, mode: leaning.unwrap_or(Mode::Major) };
                [Some(first), Some(first.parallel())]
            }
            KeyAnswer::Unclear { leaning, .. } => [leaning, None],
        }
    }

    /// The answer for the same notes moved this many semitones up (down when negative).
    pub fn up(self, semitones: i32) -> KeyAnswer {
        match self {
            KeyAnswer::Key(k) => KeyAnswer::Key(k.up(semitones)),
            KeyAnswer::OrRelative(k) => KeyAnswer::OrRelative(k.up(semitones)),
            KeyAnswer::HomeNote { tonic, leaning } => KeyAnswer::HomeNote { tonic: tonic.up(semitones), leaning },
            KeyAnswer::Unclear { why, leaning } => {
                KeyAnswer::Unclear { why, leaning: leaning.map(|k| k.up(semitones)) }
            }
        }
    }
}

/// Knots of the thresholds: 1, 2, 4, 8, 16 and 32 bars heard (flat past 32). A song takes the last knot's values.
pub const THRESHOLD_KNOTS: usize = 6;

/// How likely each kind of answer must be before it's given, at each knot of bars heard, and straight on a log scale
/// of bars in between, so one bar more never makes the answer jump.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Thresholds {
    /// One key.
    pub one: [f64; THRESHOLD_KNOTS],
    /// A key and its relative together.
    pub pair: [f64; THRESHOLD_KNOTS],
    /// A tonic in either mode.
    pub home: [f64; THRESHOLD_KNOTS],
}

impl Thresholds {
    /// The thresholds for one key, a pair and a home note, for this much music.
    pub fn at(&self, bars: u32, span: Span) -> (f64, f64, f64) {
        let last = (THRESHOLD_KNOTS - 1) as f64;
        let at = if span == Span::Song { last } else { f64::from(bars.max(1)).log2().clamp(0.0, last) };
        let below = at.floor() as usize;
        let above = at - below as f64;
        let curve = |knots: &[f64; THRESHOLD_KNOTS]| {
            let a = knots.get(below).copied().unwrap_or(1.0);
            let b = knots.get(below + 1).copied().unwrap_or(a);
            a + (b - a) * above
        };
        (curve(&self.one), curve(&self.pair), curve(&self.home))
    }
}

/// The 24 keys with their chances, likeliest first, and whether the first is clear.
///
/// Ties are put in an order that moves with the notes. After the likeliest key come the others by chance, and those
/// with the same chance by how far their tonic is above the likeliest key's, major first. When the likeliest chance is
/// shared by two keys that are a relative or a parallel pair, the major one is first. Any other tie for the likeliest
/// can't be broken in a way that moves with the notes: the first key is then simply the first of them in [`Key::ALL`],
/// and the first isn't clear.
pub fn ranked(chances: &[f64; 24]) -> ([(Key, f64); 24], bool) {
    // A chance that isn't a number (never one the finder gives) counts as 0.
    let all = Key::ALL.map(|k| (k, chances.get(k.index()).copied().filter(|p| !p.is_nan()).unwrap_or(0.0)));
    let top = all.iter().map(|(_, p)| *p).fold(f64::NEG_INFINITY, f64::max);
    let mut tied = all.iter().filter(|(_, p)| *p == top).map(|(k, _)| *k);
    let (first, clear) = match (tied.next(), tied.next(), tied.next()) {
        (Some(only), None, _) => (only, true),
        // Key::ALL has the major keys first, so the first of a relative or parallel pair is its major key.
        (Some(a), Some(b), None) if a.relative() == b || a.parallel() == b => (a, true),
        (first, _, _) => (first.unwrap_or(Key::major(PitchClass::C)), false),
    };
    let order = |k: Key| (k != first, k.tonic.above(first.tonic), k.is_minor());
    let mut ranked = all;
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| order(a.0).cmp(&order(b.0))));
    (ranked, clear)
}

/// What an answer is graded on besides the chances.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grading {
    /// How many bars differ in what was heard.
    pub bars_heard: u32,
    /// Notes heard.
    pub notes: u32,
    /// The largest share of the notes inside one scale ([`super::Heard::diatonic_fit`]).
    pub diatonic_fit: f64,
    pub span: Span,
}

/// The answer to give for these chances, and the chance that it's right: the key's chance, the pair's or the home
/// note's (both modes together); for no clear key, the likeliest key's chance (0 when it leans nowhere).
///
/// In order: too few notes; notes in no one scale; one key, if it's likely enough; the key and its relative, if the
/// relative is the runner-up and the two are likely enough together; the home note, if its two modes are likely
/// enough together; otherwise no clear key, leaning to the likeliest.
pub fn grade(chances: &[f64; 24], grading: Grading, model: &Model) -> (KeyAnswer, f64) {
    if grading.notes < MIN_NOTES {
        return (KeyAnswer::Unclear { why: Why::TooFewNotes, leaning: None }, 0.0);
    }
    // A share that isn't a number isn't in a scale either.
    let in_a_scale = grading.diatonic_fit >= model.not_tonal_below;
    if !in_a_scale {
        return (KeyAnswer::Unclear { why: Why::NotTonal, leaning: None }, 0.0);
    }
    let (ranked, clear) = ranked(chances);
    let [(k1, p1), (k2, p2), ..] = ranked;
    let chance = |k: Key| ranked.iter().find(|(x, _)| *x == k).map_or(0.0, |(_, p)| *p);
    let (one, pair, home) = model.thresholds.at(grading.bars_heard, grading.span);
    if !clear {
        return (KeyAnswer::Unclear { why: Why::Spread, leaning: None }, 0.0);
    }
    if p1 > p2 && p1 >= one {
        return (KeyAnswer::Key(k1), p1);
    }
    // A pair needs the relative to be the runner-up, so its sum is never below the home note's (the first key and
    // its parallel): when both are likely enough, the pair is given.
    let pair_sum = if k2 == k1.relative() { p1 + p2 } else { 0.0 };
    let home_sum = p1 + chance(k1.parallel());
    if pair_sum >= pair {
        (KeyAnswer::OrRelative(k1), pair_sum)
    } else if home_sum >= home {
        let leaning = (p1 > chance(k1.parallel())).then_some(k1.mode);
        (KeyAnswer::HomeNote { tonic: k1.tonic, leaning }, home_sum)
    } else if p1 > p2 {
        (KeyAnswer::Unclear { why: Why::Spread, leaning: Some(k1) }, p1)
    } else {
        (KeyAnswer::Unclear { why: Why::Spread, leaning: None }, 0.0)
    }
}
