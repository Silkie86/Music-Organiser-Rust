//! The evidence for and against each reading of a stretch of music, and the stretch's readings ranked by it: what the
//! reader weighs before it decides where the chords change (harmony/design/chord-reader-design.md, "Candidates and
//! evidence").
//!
//! A reading ([`Label`]) is scored on a span (a stretch of units) by 37 pieces of evidence ([`Feature`]): how much of
//! the chords' and the tune's sound its own notes, its tensions and the notes that don't belong to it take; whether
//! its third, seventh and fifth sound; where its root is; what the bass does under it; and where the span starts.
//! Each is worked out from the hearing ([`Hearing`]) and weighed ([`Weights`]). Most count for every felt beat the
//! span sounds, so evidence adds up over time and cutting a held chord in two gains nothing; the ones about where the
//! span starts count once.
//!
//! What only passes by (a passing note, a neighbour, an approach, an anticipation, a suspension or an appoggiatura on
//! the chords' top line, in a broken chord or in the tune) is tested against each label: under a label, a shaped
//! note passes by when it isn't one of the label's own notes and the notes it hangs on are (an anticipation or an
//! approach needs only the first: it hangs on the chord after). It then costs the label a little, where a note that
//! doesn't belong costs much more. The bass's notes count by where they fall instead (B7 to B9).
//!
//! Everything is counted in whole ticks and divided once, so the same notes moved give every label on its new root
//! exactly the same evidence, to the last bit, and the same ranking.
//!
//! The decoder scores some fifty readings on each of thousands of spans, so it scores them the quick way
//! ([`SpanFacts::score_all`]): what depends only on the span is worked out once, what depends only on a root once for
//! each root, and the ticks of any set of notes are three lookups ([`SetSums`]). It adds the same evidence up in
//! another order, so it may differ from the evidence weighed one piece at a time ([`SpanFacts::features`], then
//! [`SpanFacts::score`]) in the last bits, no more; that slower way stays, as what the quick way is tested against
//! and what an explanation shows.

use std::ops::{Index, IndexMut, Range};

use super::candidates::{Candidate, Label, Route, candidates};
use super::cores::Core;
use super::hear::{BassTexture, Hearing, Level, Line, Mass, Push, Shaped, Stream, heard_in, played_in, presence_of};
use super::weights::{Priors, Weights};
use crate::pitch::{PcSet, PerPitch, PitchClass, SetSums, WrappingSum};

/// A piece of evidence about a reading of a span (the design's features, named by their codes there). Shares are of
/// the span's sound in ticks, so they lie between 0 and 1; the yes-or-no ones are 1 or 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Feature {
    /// T1: the share of the chords' sound in the label's own notes.
    ChordTones,
    /// T2: the share in its tensions struck in the span, not passing by.
    Tensions,
    /// T3: the share in notes that don't belong to it, struck in the span, not passing by.
    WrongStruck,
    /// T4: the share in notes that don't belong, carried in from before the span, and the same share of what the chords
    /// hold over a bar line or the middle of a bar; neither passing by.
    WrongHeld,
    /// T5: the share in notes that only pass by under it, and the same share of what the chords hold over (a
    /// suspension held over the bar line).
    Passing,
    /// T6: the share of the tune's sound in its own notes.
    TuneTones,
    /// T7: the tune's share in its tensions, not passing by.
    TuneTensions,
    /// T8: the tune's share in notes that don't belong, not passing by.
    TuneWrong,
    /// T9: how far its third (or a sus chord's 4th or 2nd) is from sounding: 1 when it doesn't, 0 when it's fully
    /// present. A 7sus4's 2 may stand in for its 4th.
    NoThird,
    /// T10: the same for a four-note chord's seventh (or sixth).
    NoSeventh,
    /// T11: the same for the fifth that says which chord it is ([`Core::fifth`]): the flat fifth of a diminished or
    /// half-diminished chord, the sharp fifth of an augmented one, a power chord's fifth.
    NoCoreFifth,
    /// T12: the same for a plain fifth, which shells leave out (not a power chord's: that's T11).
    NoPlainFifth,
    /// T15: how much the chords' notes lean towards its root, each note by its interval above it: the unison 1, the
    /// fifth ½, the major third .3, the minor seventh .2, the whole tone .1 (Parncutt's root weights, as HarmAn uses
    /// them).
    RootSupport,
    /// R1: its root is heard in the chords or the tune's held notes.
    RootPlayed,
    /// R2: its root is heard only with the bass.
    RootInBass,
    /// R3: its root isn't heard, while a bass sounds.
    RootlessOverBass,
    /// R4: its root isn't heard, and no bass sounds.
    RootlessNoBass,
    /// L1: no bass sounds, and the lowest chord note is its root: the share of the span's time it is (in the units
    /// where a chord note sounds).
    LowestIsRoot,
    /// B1: the held bass is its root.
    BassRoot,
    /// B2: the held bass is its third (or a sus chord's 4th or 2nd).
    BassThird,
    /// B3: the held bass is its fifth.
    BassFifth,
    /// B4: the held bass is its seventh or sixth.
    BassSeventh,
    /// B5: the held bass is one of its tensions.
    BassTension,
    /// B6: the held bass is a note that doesn't belong.
    BassWrong,
    /// B7: the share of a moving bass on its root, each unit weighted by how strong its start is.
    WalkingRoot,
    /// B8: the same on its other notes.
    WalkingTones,
    /// B9: the same on notes that aren't its own (its tensions too), where they sound on a bar line or the middle of a
    /// bar, or from a note played longer than a felt beat.
    WalkingWrong,
    /// E1, once: the first bass note struck in the span's first felt beat is its root, the bass moving to it, and it's
    /// no walking note (one no longer than a felt beat, under chords that strike nothing new).
    BassStrikesRoot,
    /// E2, once: the share of its own notes heard in the span that are struck where it starts.
    StruckAtStart,
    /// E3, once: nothing in the chords or the bass is struck where the span starts, or only walking bass notes.
    NothingStruck,
    /// E4, once: the span starts on a bar line.
    OnBar,
    /// E5, once: the span starts in the middle of a bar.
    MidBar,
    /// E6, once: the span starts on another felt beat.
    OnBeat,
    /// E7, once: the span starts between felt beats.
    OffBeat,
    /// E8, once: the span starts at a pushed chord, counted from the beat.
    Pushed,
    /// P1: how common its core is (ln f_core, the template prior).
    CorePrior,
    /// P2: for each tension its core can carry, ln f_ten when it's heard (in the chords, struck in the span, not
    /// passing by) and ln (1 − f_ten) when not.
    TensionPrior,
}

impl Feature {
    pub const COUNT: usize = 37;

    /// Every piece of evidence, in order.
    pub const ALL: [Feature; Feature::COUNT] = [
        Feature::ChordTones,
        Feature::Tensions,
        Feature::WrongStruck,
        Feature::WrongHeld,
        Feature::Passing,
        Feature::TuneTones,
        Feature::TuneTensions,
        Feature::TuneWrong,
        Feature::NoThird,
        Feature::NoSeventh,
        Feature::NoCoreFifth,
        Feature::NoPlainFifth,
        Feature::RootSupport,
        Feature::RootPlayed,
        Feature::RootInBass,
        Feature::RootlessOverBass,
        Feature::RootlessNoBass,
        Feature::LowestIsRoot,
        Feature::BassRoot,
        Feature::BassThird,
        Feature::BassFifth,
        Feature::BassSeventh,
        Feature::BassTension,
        Feature::BassWrong,
        Feature::WalkingRoot,
        Feature::WalkingTones,
        Feature::WalkingWrong,
        Feature::BassStrikesRoot,
        Feature::StruckAtStart,
        Feature::NothingStruck,
        Feature::OnBar,
        Feature::MidBar,
        Feature::OnBeat,
        Feature::OffBeat,
        Feature::Pushed,
        Feature::CorePrior,
        Feature::TensionPrior,
    ];

    /// Its code in the design: T1, R3, B9, E8, P2.
    pub const fn code(self) -> &'static str {
        use Feature::*;
        match self {
            ChordTones => "T1",
            Tensions => "T2",
            WrongStruck => "T3",
            WrongHeld => "T4",
            Passing => "T5",
            TuneTones => "T6",
            TuneTensions => "T7",
            TuneWrong => "T8",
            NoThird => "T9",
            NoSeventh => "T10",
            NoCoreFifth => "T11",
            NoPlainFifth => "T12",
            RootSupport => "T15",
            RootPlayed => "R1",
            RootInBass => "R2",
            RootlessOverBass => "R3",
            RootlessNoBass => "R4",
            LowestIsRoot => "L1",
            BassRoot => "B1",
            BassThird => "B2",
            BassFifth => "B3",
            BassSeventh => "B4",
            BassTension => "B5",
            BassWrong => "B6",
            WalkingRoot => "B7",
            WalkingTones => "B8",
            WalkingWrong => "B9",
            BassStrikesRoot => "E1",
            StruckAtStart => "E2",
            NothingStruck => "E3",
            OnBar => "E4",
            MidBar => "E5",
            OnBeat => "E6",
            OffBeat => "E7",
            Pushed => "E8",
            CorePrior => "P1",
            TensionPrior => "P2",
        }
    }

    /// Whether it counts once for the span (where the span starts), rather than for every felt beat it sounds.
    pub const fn once(self) -> bool {
        use Feature::*;
        matches!(self, BassStrikesRoot | StruckAtStart | NothingStruck | OnBar | MidBar | OnBeat | OffBeat | Pushed)
    }
}

/// How many pieces of evidence count once ([`Feature::once`]).
const ONCE_COUNT: usize = 8;

/// The pieces of evidence that count for each felt beat, in order.
const PER_BEAT: [Feature; Feature::COUNT - ONCE_COUNT] = features_that_count(false);

/// The pieces of evidence that count once, in order.
const ONCE: [Feature; ONCE_COUNT] = features_that_count(true);

/// The pieces of evidence that count once (or for each felt beat), in order; worked out when the app is built, which
/// fails if `N` isn't how many there are.
// The loop stays inside the list of features, and a place past `N` stops the build.
#[allow(clippy::indexing_slicing)]
const fn features_that_count<const N: usize>(once: bool) -> [Feature; N] {
    let mut out = [Feature::ChordTones; N];
    let (mut i, mut k) = (0, 0);
    while i < Feature::COUNT {
        if Feature::ALL[i].once() == once {
            out[k] = Feature::ALL[i];
            k += 1;
        }
        i += 1;
    }
    assert!(k == N, "the wrong number of features");
    out
}

/// A value for each piece of evidence, picked by [`Feature`] (so a pick can never miss).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PerFeature<T>(pub [T; Feature::COUNT]);

impl<T: Copy + Default> Default for PerFeature<T> {
    fn default() -> PerFeature<T> {
        PerFeature([T::default(); Feature::COUNT])
    }
}

impl<T> Index<Feature> for PerFeature<T> {
    type Output = T;
    // A feature's place is always inside the array.
    #[allow(clippy::indexing_slicing)]
    fn index(&self, feature: Feature) -> &T {
        &self.0[feature as usize]
    }
}

impl<T> IndexMut<Feature> for PerFeature<T> {
    #[allow(clippy::indexing_slicing)]
    fn index_mut(&mut self, feature: Feature) -> &mut T {
        &mut self.0[feature as usize]
    }
}

/// Root weights in tenths, by interval above the root (T15): unison 10, major third 3, fifth 5, minor seventh 2,
/// whole tone 1.
const ROOT_SUPPORT: [u64; 12] = [10, 0, 1, 0, 3, 0, 0, 5, 0, 0, 2, 0];

/// Where a shaped note is heard: the bass's shapes count through the bass's own evidence (B7 to B9), by where its
/// notes fall, so only these pass by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Voice {
    /// The chords' top line or a broken chord.
    Chords,
    Tune,
}

/// A shaped note, and where its held-over end starts (its end when it has none).
#[derive(Clone, Copy, Debug)]
struct ShapedNote {
    shaped: Shaped,
    held_from: i64,
}

/// What may pass by under a label in a span, from its shaped notes, with the notes alike added up so each label tests
/// each kind once: a pitch class, the notes it hangs on ([`Shaped::hangs_on`]), and its shaped ticks inside the span
/// in the chords, in what they hold over (a note's held-over end) and in the tune.
#[derive(Clone, Copy, Debug)]
struct Passer {
    pc: PitchClass,
    anchors: PcSet,
    chords: u64,
    over: u64,
    tune: u64,
}

impl Passer {
    /// Whether it only passes by under a label whose own notes are `tones`: it isn't one of them, and the notes it hangs
    /// on are.
    fn passes_under(&self, tones: PcSet) -> bool {
        !tones.contains(self.pc) && tones.contains_all(self.anchors)
    }
}

/// Some pitch classes' ticks in a span: in the chords (struck in the span, and carried in), in what the chords hold
/// over, and in the tune. Added up in whole ticks, round the top as the release build adds (they never get near it).
#[derive(Clone, Copy, Debug, Default)]
struct Sums {
    struck: u64,
    carried: u64,
    held: u64,
    tune: u64,
}

impl Sums {
    /// Those of all the pitch classes less those of some.
    fn less(self, other: Sums) -> Sums {
        Sums {
            struck: self.struck.wrapping_sub(other.struck),
            carried: self.carried.wrapping_sub(other.carried),
            held: self.held.wrapping_sub(other.held),
            tune: self.tune.wrapping_sub(other.tune),
        }
    }
}

/// Works out the evidence on the spans of one hearing. Keep one and reuse it: [`Evidence::prepare`] once for each
/// hearing, then [`Evidence::facts`] for each span.
#[derive(Debug, Default)]
pub struct Evidence {
    /// The shaped notes in order of where their shaped part starts, and the longest shaped part.
    shaped: Vec<ShapedNote>,
    longest: i64,
    /// For each unit, the bass pitch classes sounding in it from a note played longer than a felt beat.
    long_bass: Vec<PcSet>,
    /// For each unit, the bass pitch classes of the last unit before it, at most a bar before, in which the bass
    /// sounds (as the masses count it): the bass a note struck there moves from.
    bass_before: Vec<PcSet>,
    /// For each unit, the chords' notes sounding in the last unit before it, at most a bar before, in which they sound:
    /// what the chords struck there are new against.
    chords_before: Vec<PcSet>,
}

impl Evidence {
    pub fn new() -> Evidence {
        Evidence::default()
    }

    /// Gets ready for the spans of a hearing: what passes by, where the bass holds a note longer than a felt beat, and
    /// what the bass and the chords sound last before each unit.
    pub fn prepare(&mut self, hearing: &Hearing) {
        self.shaped.clear();
        self.shaped.extend(hearing.shaped().iter().map(|&shaped| {
            let held_from = hearing.notes().get(shaped.note).and_then(|n| n.held_over).unwrap_or(shaped.end);
            ShapedNote { shaped, held_from }
        }));
        self.shaped.sort_unstable_by_key(|s| (s.shaped.start, s.shaped.end, s.shaped.note));
        self.longest = self.shaped.iter().map(|s| s.shaped.end - s.shaped.start).max().unwrap_or(0).max(0);
        let units = hearing.units();
        self.long_bass.clear();
        self.long_bass.resize(units, PcSet::EMPTY);
        let felt = hearing.felt_ticks();
        for n in hearing.notes().iter().filter(|n| n.length > felt) {
            // Its main part: a held-over end is never the bass.
            let end = n.held_over.unwrap_or(n.end).clamp(n.start, n.end);
            if end <= n.start {
                continue;
            }
            let (from, to) = (hearing.unit_of(n.start), hearing.unit_of(end - 1).min(units.saturating_sub(1)));
            for u in from..=to {
                if let (true, Some(set)) = (hearing.is_bass_in(u, n), self.long_bass.get_mut(u)) {
                    *set = set.with(n.pitch_class());
                }
            }
        }
        self.bass_before.clear();
        self.chords_before.clear();
        let bar = hearing.bar_units().max(1);
        // The last unit in which the bass sounds, and in which the chords do, with what sounds there.
        type Last = Option<(usize, PcSet)>;
        let before = |last: Last, u: usize| last.filter(|&(w, _)| u - w <= bar).map_or(PcSet::EMPTY, |l| l.1);
        let (mut last_bass, mut last_chords): (Last, Last) = (None, None);
        for u in 0..units {
            self.bass_before.push(before(last_bass, u));
            self.chords_before.push(before(last_chords, u));
            let bass = hearing.mass(u..u + 1, Mass::Bass);
            let set = PitchClass::ALL.into_iter().filter(|&pc| bass[pc] > 0).fold(PcSet::EMPTY, PcSet::with);
            if !set.is_empty() {
                last_bass = Some((u, set));
            }
            let chords = hearing.sounding(u, Stream::Harmony);
            if !chords.is_empty() {
                last_chords = Some((u, chords));
            }
        }
    }

    /// Whether the chords strike a note in a unit that didn't sound in them just before (in the last unit, at most a
    /// bar before, in which they sound). The same chord struck again is no new chord.
    fn struck_anew(&self, hearing: &Hearing, unit: usize) -> bool {
        let before = self.chords_before.get(unit).copied().unwrap_or(PcSet::EMPTY);
        !before.contains_all(hearing.struck(unit, Stream::Harmony))
    }

    /// Whether a bass note struck in a unit is a walking note: played no longer than a felt beat, under chords that
    /// sound there and strike nothing new (held through the unit, or struck again on the same notes). A walking bass's
    /// notes move under one chord; they aren't where a chord starts.
    fn walking(&self, hearing: &Hearing, unit: usize, pc: PitchClass) -> bool {
        let long = self.long_bass.get(unit).is_some_and(|set| set.contains(pc));
        !long && !hearing.sounding(unit, Stream::Harmony).is_empty() && !self.struck_anew(hearing, unit)
    }

    /// What a span holds, for scoring every label on it: `units` are its units; `start` a push to its first unit, when
    /// the chord read there is pushed (struck in the unit before and held over); and `end` a push to the unit after its
    /// last, when the next chord is.
    ///
    /// A pushed chord's sound is its own from the unit it's struck in: the span's sound starts a unit early with
    /// `start`, and ends a unit early with `end`, so each unit's sound counts in exactly one of the spans a reading cuts
    /// the music into. Where the span starts (how strong that is, what's struck there, the first bass note) is read at
    /// its first unit, a pushed chord's notes counting as struck there, and its bass (as heard where it's struck) as the
    /// first bass note.
    pub fn facts(
        &self,
        hearing: &Hearing,
        units: Range<usize>,
        start: Option<&Push>,
        end: Option<&Push>,
        out: &mut SpanFacts,
    ) {
        let last = units.end.min(hearing.units());
        let units = units.start.min(last)..last;
        let start = start.filter(|p| p.unit == units.start && units.start > 0 && units.start < units.end);
        let end = end.filter(|p| p.unit == units.end && units.end > units.start);
        let sound = units.start - usize::from(start.is_some())..units.end - usize::from(end.is_some());
        let s = hearing.sounding_ticks(sound.clone());
        out.units = units.clone();
        out.sound = sound.clone();
        out.sounding = s;
        out.beats = s as f64 / hearing.felt_ticks().max(1) as f64;
        out.chords = hearing.mass(sound.clone(), Mass::Harmony);
        out.chords_total = out.chords.total();
        out.held_over = hearing.mass(sound.clone(), Mass::HeldOver);
        out.held_over_total = out.held_over.total();
        out.tune = hearing.mass(sound.clone(), Mass::Melody);
        out.tune_total = out.tune.total();
        out.weighted_bass = hearing.mass(sound.clone(), Mass::WeightedBass);
        out.weighted_bass_total = out.weighted_bass.total();
        let bass = hearing.mass(sound.clone(), Mass::Bass);
        let held_tune = hearing.mass(sound.clone(), Mass::HeldMelody);
        out.presence = presence_of(&out.chords, &bass, &held_tune);
        out.heard = heard_in(&out.presence, s);
        out.played = played_in(&out.chords, &held_tune, s);
        out.bass = BassTexture::of(&bass);
        out.level = hearing.level(units.start);
        out.pushed = start.is_some();
        out.lowest = hearing.lowest(sound.clone()).map(PitchClass::of_key);
        // With no bass, how long each pitch class is the lowest chord note (for L1).
        out.lowest_chord = PerPitch::default();
        out.lowest_chord_total = 0;
        if out.bass == BassTexture::None {
            for u in sound.clone() {
                if let Some(key) = hearing.lowest_sounding(u) {
                    let ticks = u64::try_from(hearing.offset(u + 1) - hearing.offset(u)).unwrap_or(0);
                    let pc = PitchClass::of_key(key);
                    out.lowest_chord[pc] = out.lowest_chord[pc].saturating_add(ticks);
                    out.lowest_chord_total = out.lowest_chord_total.saturating_add(ticks);
                }
            }
        }

        // The chord notes struck in the span (a pushed chord's among them, struck in its first unit of sound), and
        // where it starts: there, a pushed chord's notes count as struck, and its bass (as heard where it's struck,
        // if it has one) is the first bass note struck. A bass note struck in the unit before that isn't the pushed
        // chord's is a pickup, not where the span starts.
        let u = units.start;
        let struck = hearing.struck_in(sound.clone(), Stream::Harmony);
        let mut first = hearing.struck(u, Stream::Harmony).union(hearing.struck(u, Stream::Bass));
        let mut chords_struck = self.struck_anew(hearing, u);
        // The first bass note struck where the span starts: in its first felt beat (a bass entering later doesn't say
        // where the chord starts).
        let first_beat = u..sound.end.min(u + hearing.felt_units().max(1));
        let mut onset = first_beat.clone().find_map(|v| hearing.bass_onset(v..v + 1).map(|pc| (pc, v, false)));
        if let Some(push) = start {
            let before = push.unit - 1;
            let mut pushed_bass: Option<u8> = None;
            for (_, n) in hearing.push_notes(push) {
                if hearing.is_bass_in(before, n) {
                    pushed_bass = Some(pushed_bass.map_or(n.key, |k| k.min(n.key)));
                } else {
                    chords_struck = true;
                }
                first = first.with(n.pitch_class());
            }
            onset = pushed_bass.map(|k| (PitchClass::of_key(k), before, true)).or(onset);
        }
        out.struck = struck;
        out.first_struck = first;
        out.bass_onset = onset.map(|(pc, _, _)| pc);
        // The first bass note struck is the root at a change (E1) only when the bass moves to it, and it isn't a
        // walking note: the chords strike something new with it, or it's longer than a felt beat (a pushed chord's
        // bass, or one struck under a pushed chord, always counts). The span starts where nothing is struck (E3) when
        // only walking notes are.
        out.onset_counts = onset.is_some_and(|(pc, v, pushed)| {
            let moves = !self.bass_before.get(v).is_some_and(|set| set.contains(pc));
            let with_chords = pushed || (v == u && chords_struck);
            moves && (with_chords || !self.walking(hearing, v, pc))
        });
        let bass_first = hearing.struck(u, Stream::Bass);
        let walks =
            !chords_struck && !bass_first.is_empty() && bass_first.iter().all(|pc| self.walking(hearing, u, pc));
        out.nothing_struck = first.is_empty() || walks;
        // What passes by in the span.
        out.passers.clear();
        let (a, b) = (hearing.offset(sound.start), hearing.offset(sound.end));
        let from = self.shaped.partition_point(|s| s.shaped.start < a.saturating_sub(self.longest));
        for s in self.shaped.get(from..).unwrap_or_default().iter().take_while(|s| s.shaped.start < b) {
            let Shaped { line, pc, start: first, end: last, .. } = s.shaped;
            let voice = match line {
                Line::Bass => continue,
                Line::Tune(_) => Voice::Tune,
                Line::Chords | Line::Broken(_) => Voice::Chords,
            };
            // A held-over end counts with what the chords hold over; the tune's counts nowhere, as the masses have it.
            let split = s.held_from.clamp(first, last);
            let anchors = s.shaped.hangs_on();
            for (from, to, held_over) in [(first, split, false), (split, last, true)] {
                let (start, end) = (from.max(a), to.min(b));
                if start >= end || (held_over && voice == Voice::Tune) {
                    continue;
                }
                let ticks = u64::try_from(end - start).unwrap_or(0);
                let at = match out.passers.iter().position(|p| p.pc == pc && p.anchors == anchors) {
                    Some(at) => at,
                    None => {
                        out.passers.push(Passer { pc, anchors, chords: 0, over: 0, tune: 0 });
                        out.passers.len() - 1
                    }
                };
                if let Some(passer) = out.passers.get_mut(at) {
                    let sum = match (voice, held_over) {
                        (Voice::Chords, false) => &mut passer.chords,
                        (Voice::Chords, true) => &mut passer.over,
                        (Voice::Tune, _) => &mut passer.tune,
                    };
                    *sum = sum.saturating_add(ticks);
                }
            }
        }

        // The bass where a note that doesn't belong would count against a label (B9).
        out.strong_bass = PerPitch::default();
        if out.bass == BassTexture::Moving {
            for u in sound {
                let level = hearing.level(u);
                let long = self.long_bass.get(u).copied().unwrap_or(PcSet::EMPTY);
                let ticks = hearing.mass(u..u + 1, Mass::Bass);
                for pc in PitchClass::ALL {
                    if ticks[pc] == 0 || (level < Level::HalfBar && !long.contains(pc)) {
                        continue;
                    }
                    out.strong_bass[pc] = out.strong_bass[pc].saturating_add(ticks[pc].saturating_mul(level.weight()));
                }
            }
        }

        // All the pitch classes' ticks, and how much the chords lean towards each root: each note's ticks times its
        // root weight (in tenths) by its interval above the root.
        out.all = out.sums(PcSet::ALL);
        out.fill_ticks();
        out.leaning = PerPitch::default();
        for (pc, ticks) in out.chords.iter().filter(|&(_, ticks)| ticks > 0) {
            for (k, &weight) in (0..).zip(&ROOT_SUPPORT).filter(|&(_, &weight)| weight > 0) {
                let root = pc.up(-k);
                out.leaning[root] = out.leaning[root].wrapping_add(ticks.saturating_mul(weight));
            }
        }
    }
}

/// `part` over `whole`, 0 when the whole is nothing.
fn share(part: u64, whole: u64) -> f64 {
    if whole == 0 { 0.0 } else { part as f64 / whole as f64 }
}

fn flag(yes: bool) -> f64 {
    if yes { 1.0 } else { 0.0 }
}

/// What a span holds ([`Evidence::facts`]): the same for every label scored on it.
#[derive(Clone, Debug)]
pub struct SpanFacts {
    /// Its units, and the units whose sound is its own (a unit earlier at each end when a chord is pushed there).
    units: Range<usize>,
    sound: Range<usize>,
    /// S, the ticks of its sound where a chord or bass note sounds, and b = S over a felt beat's ticks.
    sounding: u64,
    beats: f64,
    /// The masses ([`Mass`]) of the chords, what they hold over, the tune and the weighted bass, and their totals.
    chords: PerPitch<u64>,
    chords_total: u64,
    held_over: PerPitch<u64>,
    held_over_total: u64,
    tune: PerPitch<u64>,
    tune_total: u64,
    weighted_bass: PerPitch<u64>,
    weighted_bass_total: u64,
    /// What presence counts ([`Hearing::presence_ticks`]), and the notes heard (P).
    presence: PerPitch<u64>,
    heard: PcSet,
    /// The notes heard counting only the chords and the tune's held notes (for R1).
    played: PcSet,
    /// The chord notes struck in the span, or pushed into it.
    struck: PcSet,
    /// The chord and bass notes struck in its first unit, or pushed into it.
    first_struck: PcSet,
    bass: BassTexture,
    /// The first bass note struck in its first felt beat (or the pushed chord's bass), and whether it may be the root
    /// at a change (E1): the bass moves to it, and it isn't a walking note ([`Evidence::facts`]).
    bass_onset: Option<PitchClass>,
    onset_counts: bool,
    /// Whether nothing is struck where the span starts (E3): no chord or bass note, or only walking notes.
    nothing_struck: bool,
    /// The lowest note sounding (for the candidates' order), and with no bass, the ticks in which each pitch class is
    /// the lowest chord note, and their total (for L1).
    lowest: Option<PitchClass>,
    lowest_chord: PerPitch<u64>,
    lowest_chord_total: u64,
    level: Level,
    pushed: bool,
    /// What may pass by under a label.
    passers: Vec<Passer>,
    /// For a moving bass, each pitch class's ticks where it would count against a label it doesn't belong to (B9): on a
    /// bar line or the middle of a bar, or from a note played longer than a felt beat; each unit's ticks times its
    /// weight ([`Level::weight`]).
    strong_bass: PerPitch<u64>,
    /// All the pitch classes' ticks (for the shares of those that aren't a label's own notes or tensions, found by
    /// subtraction), and how much the chords lean towards each root (T15, before it's divided).
    all: Sums,
    leaning: PerPitch<u64>,
    /// The ticks of any set of pitch classes, for the quick scoring.
    ticks: SetSums<Ticks>,
}

impl Default for SpanFacts {
    /// An empty span, before [`Evidence::facts`] fills it.
    fn default() -> SpanFacts {
        SpanFacts {
            units: 0..0,
            sound: 0..0,
            sounding: 0,
            beats: 0.0,
            chords: PerPitch::default(),
            chords_total: 0,
            held_over: PerPitch::default(),
            held_over_total: 0,
            tune: PerPitch::default(),
            tune_total: 0,
            weighted_bass: PerPitch::default(),
            weighted_bass_total: 0,
            presence: PerPitch::default(),
            heard: PcSet::EMPTY,
            played: PcSet::EMPTY,
            struck: PcSet::EMPTY,
            first_struck: PcSet::EMPTY,
            bass: BassTexture::None,
            bass_onset: None,
            onset_counts: false,
            nothing_struck: true,
            lowest: None,
            lowest_chord: PerPitch::default(),
            lowest_chord_total: 0,
            level: Level::Bar,
            pushed: false,
            passers: Vec::new(),
            strong_bass: PerPitch::default(),
            all: Sums::default(),
            leaning: PerPitch::default(),
            ticks: SetSums::default(),
        }
    }
}

/// A label and its score on a span.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scored {
    pub label: Label,
    /// The way it got among the candidates.
    pub route: Route,
    pub score: f64,
    /// Its score without what counts once, where the span starts ([`SpanFacts::held_score`]): what it scores when the
    /// chord before is the same, held on into the span.
    pub held: f64,
}

/// The weights and the priors as the quick scoring reads them ([`SpanFacts::score_all`]), worked out once: the
/// weights themselves, and what depends only on a label's core and the held bass's interval above its root, weighed
/// (the priors among it).
#[derive(Clone, Debug)]
pub struct Scorer {
    weights: Weights,
    /// By core, and by the held bass's interval above the root: B1 to B6, weighed.
    held_bass: [[f64; 12]; Core::ALL.len()],
    /// By core: P1 weighed; and by core and tension interval, what P2 adds weighed, not heard and heard.
    core_prior: [f64; Core::ALL.len()],
    tension_prior: [[[f64; 2]; 12]; Core::ALL.len()],
}

impl Default for Scorer {
    /// Every weight 0.
    fn default() -> Scorer {
        let weights = Weights { features: PerFeature::default(), pair: 0.0, none: 0.0 };
        Scorer::new(&weights, &Priors::default())
    }
}

impl Scorer {
    /// What some weights and priors give.
    pub fn new(weights: &Weights, priors: &Priors) -> Scorer {
        use Feature::*;
        let w = &weights.features;
        let mut out = Scorer {
            weights: *weights,
            held_bass: [[0.0; 12]; Core::ALL.len()],
            core_prior: [0.0; Core::ALL.len()],
            tension_prior: [[[0.0; 2]; 12]; Core::ALL.len()],
        };
        let rows = out.held_bass.iter_mut().zip(&mut out.core_prior).zip(&mut out.tension_prior);
        for (core, ((held, core_prior), tension_prior)) in Core::ALL.into_iter().zip(rows) {
            let (third, seventh) = (core.third(), core.seventh());
            let fifth = core.intervals().difference(UNISON).difference(third).difference(seventh);
            // The six sets don't overlap, so at most one weight counts for each interval.
            let sets = [
                (BassRoot, UNISON),
                (BassThird, third),
                (BassFifth, fifth),
                (BassSeventh, seventh),
                (BassTension, core.tensions()),
                (BassWrong, core.others()),
            ];
            for (k, cell) in (0..).zip(held.iter_mut()) {
                *cell = sets.iter().filter(|(_, set)| set.has(k)).map(|&(f, _)| w[f]).sum();
            }
            *core_prior = w[CorePrior] * priors.core(core);
            for (interval, cell) in (0..).zip(tension_prior.iter_mut()) {
                let part = |heard| w[TensionPrior] * priors.tension(core, interval, heard);
                *cell = [part(false), part(true)];
            }
        }
        out
    }
}

/// The interval of a root above itself, as a set.
const UNISON: PcSet = PcSet::of(&[0]);

/// The notes of a label that the quick scoring weighs, worked out for every label when the app is built
/// ([`LabelNotes::of`]): its own notes, and those but its root; its tensions; and the notes that say which chord it is
/// (T9 to T12): its third (a 7sus4's 4th, or its 2 in place of it), its seventh or sixth, the fifth its core must have,
/// and a plain fifth where the core has one it may leave out.
#[derive(Clone, Copy, Debug)]
struct LabelNotes {
    tones: PcSet,
    upper: PcSet,
    tensions: PcSet,
    third: PcSet,
    seventh: PcSet,
    fifth: PcSet,
    plain_fifth: PcSet,
}

/// [`LabelNotes`] by core and root.
static LABEL_NOTES: [[LabelNotes; 12]; Core::ALL.len()] = label_notes();

// The loops stay inside the table (16 cores, 12 roots).
#[allow(clippy::indexing_slicing)]
const fn label_notes() -> [[LabelNotes; 12]; Core::ALL.len()] {
    let none = PcSet::EMPTY;
    let empty = LabelNotes {
        tones: none,
        upper: none,
        tensions: none,
        third: none,
        seventh: none,
        fifth: none,
        plain_fifth: none,
    };
    let mut out = [[empty; 12]; Core::ALL.len()];
    let mut k = 0;
    while k < Core::ALL.len() {
        let core = Core::ALL[k];
        // The notes that don't belong to a label are all less its own and its tensions, so none may be both.
        assert!(core.intervals().intersection(core.tensions()).is_empty(), "a core's tension is one of its own notes");
        let third = match core {
            Core::Dominant7Sus4 => core.third().union(PcSet::of(&[2])),
            _ => core.third(),
        };
        let plain_fifth = if core.intervals().has(7) && core.fifth().is_empty() { PcSet::of(&[7]) } else { none };
        let mut r = 0;
        while r < 12 {
            let root = PitchClass::new(r as i32);
            out[k][r] = LabelNotes {
                tones: core.intervals().on(root),
                upper: core.intervals().difference(UNISON).on(root),
                tensions: core.tensions().on(root),
                third: third.on(root),
                seventh: core.seventh().on(root),
                fifth: core.fifth().on(root),
                plain_fifth: plain_fifth.on(root),
            };
            r += 1;
        }
        k += 1;
    }
    out
}

impl LabelNotes {
    // A core's place is below 16 and a root's below 12, so always in the table.
    #[allow(clippy::indexing_slicing)]
    fn of(label: Label) -> &'static LabelNotes {
        &LABEL_NOTES[label.core.index()][label.root.index()]
    }
}

/// `part` over a whole already made a float (from whole ticks), as [`share`] divides them: the same to the last bit.
fn share_of(part: u64, whole: f64) -> f64 {
    if whole == 0.0 { 0.0 } else { part as f64 / whole }
}

/// Some pitch classes' ticks in a span, as the quick scoring adds them up: those of [`Sums`] (in the chords, struck in
/// the span and carried in; in what the chords hold over; in the tune), then in the moving bass, weighted (B7, B8),
/// and in the bass where it would count against a label it doesn't belong to (B9). Added up round the top, as
/// [`Sums`] are: the bass's two are only read where they add up to less than the top ([`Quick::exact`]).
#[derive(Clone, Copy, Debug, Default)]
struct Ticks([u64; 6]);

impl WrappingSum for Ticks {
    fn wrapping_plus(self, other: Ticks) -> Ticks {
        let mut out = self;
        for (sum, more) in out.0.iter_mut().zip(other.0) {
            *sum = sum.wrapping_add(more);
        }
        out
    }
}

impl Ticks {
    fn sums(self) -> Sums {
        let [struck, carried, held, tune, ..] = self.0;
        Sums { struck, carried, held, tune }
    }

    /// The moving bass's, weighted, and where it would count against a label.
    fn bass(self) -> (u64, u64) {
        let [.., walking, strong] = self.0;
        (walking, strong)
    }
}

/// What the quick scoring works out once for a span: how far each pitch class is from sounding (T9 to T12), the chord
/// notes struck long enough to be a tension heard where nothing passes by (P2), the evidence that counts once whatever
/// the label (E3 to E8), weighed; and the wholes each share is of, as floats. Every share is still its whole ticks
/// divided once, so the same music at another resolution scores the same to the last bit.
struct Quick {
    missing: PerPitch<f64>,
    heard_tensions: PcSet,
    once: f64,
    /// Whether the moving bass's ticks, weighted and where they'd count against a label, add up to less than the top:
    /// then any set's add up exactly, and those of the notes a set leaves out are all of them less the set's.
    exact: bool,
    strong_bass: u64,
    /// The chords' ticks, the larger of those and what they hold over, the tune's and the weighted bass's.
    chords: f64,
    held: f64,
    tune: f64,
    bass: f64,
}

impl Quick {
    /// How far the notes of a set are from sounding: the nearest of them (0 for none).
    fn missing(&self, set: PcSet) -> f64 {
        set.iter().map(|pc| self.missing[pc]).reduce(f64::min).unwrap_or(0.0)
    }
}

/// What only passes by under a label ([`SpanFacts::features`]): the pitch classes, and each one's ticks in the chords,
/// in what they hold over and in the tune.
#[derive(Default)]
struct PassingBy {
    set: PcSet,
    chords: PerPitch<u64>,
    over: PerPitch<u64>,
    tune: PerPitch<u64>,
}

/// The quick scoring of one span's labels ([`SpanFacts::scoring`]), with what depends only on the span worked out once:
/// to score some labels one at a time, as [`SpanFacts::score_all`] scores them, without working that out again.
pub struct Scoring<'a> {
    facts: &'a SpanFacts,
    scorer: &'a Scorer,
    quick: Quick,
}

impl Scoring<'_> {
    /// A label's score and held-on score.
    pub fn score(&self, label: Label) -> (f64, f64) {
        self.with_root(label, self.facts.root_part(label.root, self.scorer))
    }

    /// A label's score and held-on score, given what depends only on its root ([`SpanFacts::root_part`]).
    fn with_root(&self, label: Label, (root_beat, root_once): (f64, f64)) -> (f64, f64) {
        let (per_beat, once) = self.facts.label_part(label, self.scorer, &self.quick);
        let held = self.facts.beats * (root_beat + per_beat);
        (held + (self.quick.once + root_once + once), held)
    }
}

/// The ticks of the chords and the tune by what each note is to a label, less what only passes by under it
/// ([`SpanFacts::by_kind`]): its own notes; its tensions struck in the span, and in the tune; the notes that don't
/// belong, struck in the span, carried in, held over and in the tune; and what passes by, in the chords and in what
/// they hold over. Whole ticks, divided once each where they're weighed.
#[derive(Clone, Copy, Debug)]
struct ByKind {
    own: Sums,
    tension: u64,
    tune_tension: u64,
    wrong: u64,
    carried: u64,
    over: u64,
    tune_wrong: u64,
    passing: u64,
    over_passing: u64,
}

impl SpanFacts {
    pub fn new() -> SpanFacts {
        SpanFacts::default()
    }

    /// Its units.
    pub fn units(&self) -> Range<usize> {
        self.units.clone()
    }

    /// The units whose sound is its own: its units, from a unit earlier when its chord is pushed, to a unit earlier
    /// when the next one is.
    pub fn sound(&self) -> Range<usize> {
        self.sound.clone()
    }

    /// S: the ticks of its sound in which a chord or bass note sounds.
    pub fn sounding(&self) -> u64 {
        self.sounding
    }

    /// Whether nothing sounds in it: no chord, bass or tune note, nor what the chords hold over.
    pub fn silent(&self) -> bool {
        self.chords_total == 0 && self.held_over_total == 0 && self.tune_total == 0 && self.bass == BassTexture::None
    }

    /// b: the felt beats it sounds (S over a felt beat's ticks).
    pub fn beats(&self) -> f64 {
        self.beats
    }

    /// The bass under its sound: held, moving or none (what B1 to B9, R3 and R4 weigh).
    pub fn bass(&self) -> BassTexture {
        self.bass
    }

    /// The notes heard in it (the design's P).
    pub fn heard(&self) -> PcSet {
        self.heard
    }

    /// The labels worth scoring on it ([`candidates()`]), in the order ties are broken.
    pub fn candidates(&self, out: &mut Vec<Candidate>) {
        candidates(self.heard, &self.presence, self.lowest.unwrap_or(PitchClass::C), out);
    }

    /// Some pitch classes' ticks in the span ([`Sums`]).
    fn sums(&self, set: PcSet) -> Sums {
        let mut out = Sums::default();
        for pc in set.iter() {
            let ticks = self.chords[pc];
            if self.struck.contains(pc) {
                out.struck = out.struck.wrapping_add(ticks);
            } else {
                out.carried = out.carried.wrapping_add(ticks);
            }
            out.held = out.held.wrapping_add(self.held_over[pc]);
            out.tune = out.tune.wrapping_add(self.tune[pc]);
        }
        out
    }

    /// A label's ticks by what each note is to it ([`ByKind`]), from those of its own notes and of its tensions
    /// (`tensions`, the notes `tension_set`), and what only passes by under it (`by`: none where nothing does). The
    /// notes that don't belong are all less the other two, so a core's tensions are never its own notes (checked when
    /// the app is built, in [`label_notes`]).
    // Inlined, so the quick scoring keeps it all in registers.
    #[inline(always)]
    fn by_kind(&self, own: Sums, tensions: Sums, tension_set: PcSet, by: Option<&PassingBy>) -> ByKind {
        let rest = self.all.less(own).less(tensions);
        let mut out = ByKind {
            own,
            tension: tensions.struck,
            tune_tension: tensions.tune,
            wrong: rest.struck,
            carried: rest.carried,
            over: rest.held,
            tune_wrong: rest.tune,
            passing: 0,
            over_passing: 0,
        };
        for (pc, by) in by.iter().flat_map(|by| by.set.iter().map(move |pc| (pc, by))) {
            let passes = by.chords[pc].min(self.chords[pc]);
            let held_passes = by.over[pc].min(self.held_over[pc]);
            let tune_passes = by.tune[pc].min(self.tune[pc]);
            out.passing = out.passing.wrapping_add(passes);
            out.over_passing = out.over_passing.wrapping_add(held_passes);
            let struck = self.struck.contains(pc);
            if tension_set.contains(pc) {
                if struck {
                    out.tension = out.tension.wrapping_sub(passes);
                }
                out.tune_tension = out.tune_tension.wrapping_sub(tune_passes);
            } else {
                if struck {
                    out.wrong = out.wrong.wrapping_sub(passes);
                } else {
                    out.carried = out.carried.wrapping_sub(passes);
                }
                out.over = out.over.wrapping_sub(held_passes);
                out.tune_wrong = out.tune_wrong.wrapping_sub(tune_passes);
            }
        }
        out
    }

    /// The evidence about one label.
    pub fn features(&self, label: Label, priors: &Priors, out: &mut PerFeature<f64>) {
        use Feature::*;
        let (root, core) = (label.root, label.core);
        let (tones, tensions) = (label.tones(), label.tensions());
        // The chords and the tune, by what each note is to the label, each less what only passes by under it.
        let by = self.passing_by(tones);
        let kinds = self.by_kind(self.sums(tones), self.sums(tensions), tensions, Some(&by));
        // What the chords hold over counts as a share of the larger of their sound and all they hold over.
        let (d, held) = (self.chords_total, self.chords_total.max(self.held_over_total));
        out[ChordTones] = share(kinds.own.struck.wrapping_add(kinds.own.carried), d);
        out[Tensions] = share(kinds.tension, d);
        out[WrongStruck] = share(kinds.wrong, d);
        out[WrongHeld] = share(kinds.carried, d) + share(kinds.over, held);
        out[Passing] = share(kinds.passing, d) + share(kinds.over_passing, held);
        out[TuneTones] = share(kinds.own.tune, self.tune_total);
        out[TuneTensions] = share(kinds.tune_tension, self.tune_total);
        out[TuneWrong] = share(kinds.tune_wrong, self.tune_total);
        out[RootSupport] = share(self.leaning[root], d.saturating_mul(10));

        // The notes that say which chord it is: how far each is from sounding.
        let missing = |intervals: PcSet| -> f64 {
            let most = intervals.on(root).iter().map(|pc| self.presence[pc].saturating_mul(2)).max();
            most.map_or(0.0, |ticks| {
                let s = self.sounding.max(1);
                share(s - ticks.min(s), s)
            })
        };
        // A 7sus4 may sound its 2 in place of its 4th.
        let third = match core {
            Core::Dominant7Sus4 => core.third().union(const { PcSet::of(&[2]) }),
            _ => core.third(),
        };
        out[NoThird] = missing(third);
        out[NoSeventh] = missing(core.seventh());
        out[NoCoreFifth] = missing(core.fifth());
        out[NoPlainFifth] =
            if core.intervals().has(7) && core.fifth().is_empty() { missing(const { PcSet::of(&[7]) }) } else { 0.0 };

        // Where the root is.
        let (played, heard, bass) = (self.played.contains(root), self.heard.contains(root), self.bass);
        out[RootPlayed] = flag(played);
        out[RootInBass] = flag(heard && !played);
        out[RootlessOverBass] = flag(!heard && bass != BassTexture::None);
        out[RootlessNoBass] = flag(!heard && bass == BassTexture::None);
        out[LowestIsRoot] = share(self.lowest_chord[root], self.lowest_chord_total);

        // The bass under it.
        let held = match bass {
            BassTexture::Held(pc) => Some(i32::from(pc.above(root))),
            _ => None,
        };
        let held_is = |set: PcSet| flag(held.is_some_and(|k| set.has(k)));
        let fifth = core.intervals().difference(UNISON).difference(core.third()).difference(core.seventh());
        out[BassRoot] = held_is(UNISON);
        out[BassThird] = held_is(core.third());
        out[BassFifth] = held_is(fifth);
        out[BassSeventh] = held_is(core.seventh());
        out[BassTension] = held_is(core.tensions());
        out[BassWrong] = held_is(core.others());
        let (mut on_root, mut on_tones, mut on_wrong) = (0u64, 0u64, 0u64);
        if bass == BassTexture::Moving {
            on_root = self.weighted_bass[root];
            for pc in tones.without(root).iter() {
                on_tones = on_tones.saturating_add(self.weighted_bass[pc]);
            }
            for pc in PcSet::ALL.difference(tones).iter() {
                on_wrong = on_wrong.saturating_add(self.strong_bass[pc]);
            }
        }
        let v = self.weighted_bass_total;
        out[WalkingRoot] = share(on_root, v);
        out[WalkingTones] = share(on_tones, v);
        out[WalkingWrong] = share(on_wrong, v);

        // Where the span starts.
        out[BassStrikesRoot] = flag(self.onset_counts && self.bass_onset == Some(root));
        let sounding_tones = tones.intersection(self.heard);
        out[StruckAtStart] =
            share(u64::from(sounding_tones.intersection(self.first_struck).len()), u64::from(sounding_tones.len()));
        out[NothingStruck] = flag(self.nothing_struck);
        out[OnBar] = flag(self.level == Level::Bar);
        out[MidBar] = flag(self.level == Level::HalfBar);
        out[OnBeat] = flag(self.level == Level::Beat);
        out[OffBeat] = flag(self.level == Level::Off);
        out[Pushed] = flag(self.pushed);

        // The template priors.
        out[CorePrior] = priors.core(core);
        let mut prior = 0.0;
        for k in core.tensions().iter() {
            let interval = k.number();
            let pc = root.up(i32::from(interval));
            let unpassed = self.chords[pc] - by.chords[pc].min(self.chords[pc]);
            let heard = self.struck.contains(pc) && unpassed.saturating_mul(8) >= self.sounding.max(1);
            prior += priors.tension(core, interval, heard);
        }
        out[TensionPrior] = prior;
    }

    /// What the quick scoring works out once for the span.
    fn quick(&self, scorer: &Scorer) -> Quick {
        use Feature::*;
        let w = &scorer.weights.features;
        let s = self.sounding.max(1);
        let mut missing = PerPitch::default();
        for pc in PitchClass::ALL {
            missing[pc] = share(s - self.presence[pc].saturating_mul(2).min(s), s);
        }
        let heard_tensions = PitchClass::ALL
            .into_iter()
            .filter(|&pc| self.chords[pc].saturating_mul(8) >= s)
            .fold(PcSet::EMPTY, PcSet::with);
        let level = |at: Level| flag(self.level == at);
        let once = w[NothingStruck] * flag(self.nothing_struck)
            + w[OnBar] * level(Level::Bar)
            + w[MidBar] * level(Level::HalfBar)
            + w[OnBeat] * level(Level::Beat)
            + w[OffBeat] * level(Level::Off)
            + w[Pushed] * flag(self.pushed);
        let strong_bass = self.strong_bass.total();
        Quick {
            missing,
            heard_tensions: heard_tensions.intersection(self.struck),
            once,
            exact: self.weighted_bass_total < u64::MAX && strong_bass < u64::MAX,
            strong_bass,
            chords: self.chords_total as f64,
            held: self.chords_total.max(self.held_over_total) as f64,
            tune: self.tune_total as f64,
            bass: self.weighted_bass_total as f64,
        }
    }

    /// Works out the ticks of any set of pitch classes, for the quick scoring.
    fn fill_ticks(&mut self) {
        let mut ticks = PerPitch::<Ticks>::default();
        for pc in PitchClass::ALL {
            let Sums { struck, carried, held, tune } = self.sums(PcSet::EMPTY.with(pc));
            ticks[pc] = Ticks([struck, carried, held, tune, self.weighted_bass[pc], self.strong_bass[pc]]);
        }
        self.ticks.fill(&ticks);
    }

    /// What only passes by under a label whose own notes are `tones`.
    fn passing_by(&self, tones: PcSet) -> PassingBy {
        let mut out = PassingBy::default();
        for p in self.passers.iter().filter(|p| p.passes_under(tones)) {
            out.chords[p.pc] = out.chords[p.pc].saturating_add(p.chords);
            out.over[p.pc] = out.over[p.pc].saturating_add(p.over);
            out.tune[p.pc] = out.tune[p.pc].saturating_add(p.tune);
            out.set = out.set.with(p.pc);
        }
        out
    }

    /// The weighed evidence that depends only on a label's root: for each felt beat (T15, R1 to R4, L1, B7) and once
    /// (E1).
    fn root_part(&self, root: PitchClass, scorer: &Scorer) -> (f64, f64) {
        use Feature::*;
        let w = &scorer.weights.features;
        let (played, heard, bass) = (self.played.contains(root), self.heard.contains(root), self.bass);
        let mut per_beat = w[RootSupport] * share(self.leaning[root], self.chords_total.saturating_mul(10))
            + w[RootPlayed] * flag(played)
            + w[RootInBass] * flag(heard && !played)
            + w[RootlessOverBass] * flag(!heard && bass != BassTexture::None)
            + w[RootlessNoBass] * flag(!heard && bass == BassTexture::None)
            + w[LowestIsRoot] * share(self.lowest_chord[root], self.lowest_chord_total);
        if bass == BassTexture::Moving {
            per_beat += w[WalkingRoot] * share(self.weighted_bass[root], self.weighted_bass_total);
        }
        let once = w[BassStrikesRoot] * flag(self.onset_counts && self.bass_onset == Some(root));
        (per_beat, once)
    }

    /// The rest of a label's weighed evidence: for each felt beat, and once.
    fn label_part(&self, label: Label, scorer: &Scorer, quick: &Quick) -> (f64, f64) {
        use Feature::*;
        let w = &scorer.weights.features;
        let (root, core, notes) = (label.root, label.core, LabelNotes::of(label));
        let (tones, tensions) = (notes.tones, notes.tensions);
        // The chords and the tune, by what each note is to the label, less what only passes by, as in `features`.
        let (tone_ticks, tension_ticks) = (self.ticks.sum(tones), self.ticks.sum(tensions));
        let by = (!self.passers.is_empty()).then(|| self.passing_by(tones));
        let kinds = self.by_kind(tone_ticks.sums(), tension_ticks.sums(), tensions, by.as_ref());
        let (d, held, tune) = (quick.chords, quick.held, quick.tune);
        let mut per_beat = w[ChordTones] * share_of(kinds.own.struck.wrapping_add(kinds.own.carried), d)
            + w[Tensions] * share_of(kinds.tension, d)
            + w[WrongStruck] * share_of(kinds.wrong, d)
            + w[WrongHeld] * (share_of(kinds.carried, d) + share_of(kinds.over, held))
            + w[Passing] * (share_of(kinds.passing, d) + share_of(kinds.over_passing, held))
            + w[TuneTones] * share_of(kinds.own.tune, tune)
            + w[TuneTensions] * share_of(kinds.tune_tension, tune)
            + w[TuneWrong] * share_of(kinds.tune_wrong, tune);

        // The notes that say which chord it is: how far each is from sounding.
        per_beat += w[NoThird] * quick.missing(notes.third)
            + w[NoSeventh] * quick.missing(notes.seventh)
            + w[NoCoreFifth] * quick.missing(notes.fifth);
        if !notes.plain_fifth.is_empty() {
            per_beat += w[NoPlainFifth] * quick.missing(notes.plain_fifth);
        }

        // The bass under it.
        match self.bass {
            BassTexture::Held(pc) => {
                let row = scorer.held_bass.get(core.index());
                per_beat += row.and_then(|row| row.get(usize::from(pc.above(root)))).copied().unwrap_or(0.0);
            }
            BassTexture::Moving => {
                // Its other notes' (the label's own but its root), and the notes' that aren't its own.
                let (on_tones, on_wrong) = if quick.exact {
                    let (walking, strong) = tone_ticks.bass();
                    (walking.wrapping_sub(self.weighted_bass[root]), quick.strong_bass.wrapping_sub(strong))
                } else {
                    let upper = notes.upper.iter().fold(0u64, |sum, pc| sum.saturating_add(self.weighted_bass[pc]));
                    let others = PcSet::ALL.difference(tones).iter();
                    (upper, others.fold(0u64, |sum, pc| sum.saturating_add(self.strong_bass[pc])))
                };
                per_beat +=
                    w[WalkingTones] * share_of(on_tones, quick.bass) + w[WalkingWrong] * share_of(on_wrong, quick.bass);
            }
            BassTexture::None => {}
        }

        // The template priors.
        per_beat += scorer.core_prior.get(core.index()).copied().unwrap_or(0.0);
        let row = scorer.tension_prior.get(core.index());
        for k in core.tensions().iter() {
            let interval = k.number();
            let pc = root.up(i32::from(interval));
            let heard = match &by {
                Some(by) if by.set.contains(pc) => {
                    let unpassed = self.chords[pc] - by.chords[pc].min(self.chords[pc]);
                    self.struck.contains(pc) && unpassed.saturating_mul(8) >= self.sounding.max(1)
                }
                _ => quick.heard_tensions.contains(pc),
            };
            let cell = row.and_then(|row| row.get(usize::from(interval)));
            per_beat += cell.and_then(|cell| cell.get(usize::from(heard))).copied().unwrap_or(0.0);
        }

        // Where the span starts.
        let sounding_tones = tones.intersection(self.heard);
        let struck_tones = sounding_tones.intersection(self.first_struck);
        let once = w[StruckAtStart] * share(u64::from(struck_tones.len()), u64::from(sounding_tones.len()));
        (per_beat, once)
    }

    /// The quick scoring of the span's labels, with what depends only on the span worked out once ([`Scoring`]).
    pub fn scoring<'a>(&'a self, scorer: &'a Scorer) -> Scoring<'a> {
        Scoring { facts: self, scorer, quick: self.quick(scorer) }
    }

    /// Every candidate's score and held-on score, in the candidates' order: the evidence ([`SpanFacts::features`])
    /// weighed ([`SpanFacts::score`], [`SpanFacts::held_score`]), the same up to rounding, worked out quicker. What
    /// depends only on the span is worked out once, and what depends only on the root once for each root.
    pub fn score_all(&self, candidates: &[Candidate], scorer: &Scorer, out: &mut Vec<Scored>) {
        out.clear();
        let scoring = self.scoring(scorer);
        let mut by_root: Option<(PitchClass, (f64, f64))> = None;
        for c in candidates {
            let root = c.label.root;
            let root_part = match by_root {
                Some((r, part)) if r == root => part,
                _ => {
                    let part = self.root_part(root, scorer);
                    by_root = Some((root, part));
                    part
                }
            };
            let (score, held) = scoring.with_root(c.label, root_part);
            out.push(Scored { label: c.label, route: c.route, score, held });
        }
    }

    /// One label's score and held-on score, as [`SpanFacts::score_all`] scores it.
    pub fn score_one(&self, label: Label, scorer: &Scorer) -> (f64, f64) {
        self.scoring(scorer).score(label)
    }

    /// A label's score from its evidence: each piece weighed, those about the whole span counted for each felt beat it
    /// sounds (b), those about where it starts once.
    pub fn score(&self, features: &PerFeature<f64>, weights: &Weights) -> f64 {
        let (per_beat, once) = self.parts(features, weights);
        self.beats * per_beat + once
    }

    /// A label's score without what counts once: what it scores when the same chord is held on into the span from
    /// before, so the span's start is no place a chord starts.
    pub fn held_score(&self, features: &PerFeature<f64>, weights: &Weights) -> f64 {
        self.beats * self.parts(features, weights).0
    }

    /// The weighed evidence that counts for each felt beat, and that which counts once, each added up in order.
    fn parts(&self, features: &PerFeature<f64>, weights: &Weights) -> (f64, f64) {
        let (mut per_beat, mut once) = (0.0, 0.0);
        for f in PER_BEAT {
            per_beat += weights.features[f] * features[f];
        }
        for f in ONCE {
            once += weights.features[f] * features[f];
        }
        (per_beat, once)
    }

    /// What the span scores read as no chord: w_N for each felt beat it sounds.
    pub fn none_score(&self, weights: &Weights) -> f64 {
        self.beats * weights.none
    }

    /// The candidates scored ([`SpanFacts::score_all`]) and ranked, best first; equal scores keep the candidates'
    /// order, which runs from the lowest note up, so the ranking moves with the music.
    pub fn rank(&self, candidates: &[Candidate], scorer: &Scorer, out: &mut Vec<Scored>) {
        self.score_all(candidates, scorer, out);
        out.sort_by(|a, b| b.score.total_cmp(&a.score));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chordread::{COUNTED, HAND};

    #[test]
    fn a_moving_bass_whose_ticks_reach_the_top_scores_as_the_evidence_says() {
        // Ticks no music has, so the bass's add up only as far as the top: the quick scoring adds them up one at a
        // time then, as the evidence does.
        let mut facts = SpanFacts::new();
        let huge = u64::MAX / 3;
        facts.sounding = 1920;
        facts.beats = 4.0;
        facts.bass = BassTexture::Moving;
        for (pc, ticks) in [(PitchClass::C, 960), (PitchClass::E, 480), (PitchClass::G, 480), (PitchClass::B, 240)] {
            facts.chords[pc] = ticks;
            facts.presence[pc] = ticks;
            facts.weighted_bass[pc] = huge;
            facts.strong_bass[pc] = huge;
            facts.heard = facts.heard.with(pc);
        }
        facts.chords_total = facts.chords.total();
        facts.weighted_bass_total = facts.weighted_bass.total();
        facts.struck = facts.heard;
        facts.played = facts.heard;
        facts.all = facts.sums(PcSet::ALL);
        facts.fill_ticks();
        let priors = Priors::of(&COUNTED);
        let scorer = Scorer::new(&HAND, &priors);
        assert!(!facts.quick(&scorer).exact);
        let mut f = PerFeature::default();
        for root in PitchClass::ALL {
            for core in Core::ALL {
                let label = Label::new(root, core);
                facts.features(label, &priors, &mut f);
                let (score, held) = facts.score_one(label, &scorer);
                let (want, want_held) = (facts.score(&f, &HAND), facts.held_score(&f, &HAND));
                assert!((score - want).abs() <= 1e-9 * want.abs().max(1.0), "{label}: {score}, not {want}");
                assert!((held - want_held).abs() <= 1e-9 * want_held.abs().max(1.0), "{label}");
            }
        }
    }
}
