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

use std::ops::{Index, IndexMut, Range};

use super::candidates::{Candidate, Label, Route, candidates};
use super::cores::Core;
use super::hear::{BassTexture, Hearing, Level, Line, Mass, Push, Shape, Shaped, Stream, heard_in, presence_of};
use super::weights::{Tables, Weights};
use crate::pitch::{PcSet, PerPitch, PitchClass};

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
    /// E1, once: the first bass note struck in the span is its root, the bass moving to it, and it's no walking note
    /// (one no longer than a felt beat, under chords that strike nothing new).
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

/// A shaped note's part in a span.
#[derive(Clone, Copy, Debug)]
struct ShapedPart {
    voice: Voice,
    /// Whether it's the note's held-over end: it then counts with what the chords hold over.
    held_over: bool,
    pc: PitchClass,
    anchors: PcSet,
    shape: Shape,
    /// Its shaped ticks inside the span, from the first unit's start.
    start: i64,
    end: i64,
}

impl ShapedPart {
    fn ticks(&self) -> u64 {
        u64::try_from(self.end - self.start).unwrap_or(0)
    }
}

/// Whether a shaped note only passes by under a label whose own notes are `tones`: it isn't one of them, and the notes
/// it hangs on are (an anticipation or an approach hangs on the chord after it, so needs only the first).
fn passes_by(shape: Shape, pc: PitchClass, anchors: PcSet, tones: PcSet) -> bool {
    !tones.contains(pc)
        && match shape {
            Shape::Anticipation | Shape::Approach => true,
            Shape::Passing | Shape::Neighbour | Shape::Suspension | Shape::Appoggiatura => tones.contains_all(anchors),
        }
}

/// A bass note's part in a unit where it would count against a label it doesn't belong to (B9): on a bar line or the
/// middle of a bar, or from a note played longer than a felt beat.
#[derive(Clone, Copy, Debug)]
struct StrongBass {
    pc: PitchClass,
    /// Its ticks in the unit times the unit's weight ([`Level::weight`]).
    weighted: u64,
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
        out.played = heard_in(&presence_of(&out.chords, &PerPitch::default(), &held_tune), s);
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
        let mut onset = (u..sound.end).find_map(|v| hearing.bass_onset(v..v + 1).map(|pc| (pc, v, false)));
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
        out.shaped.clear();
        let (a, b) = (hearing.offset(sound.start), hearing.offset(sound.end));
        let from = self.shaped.partition_point(|s| s.shaped.start < a.saturating_sub(self.longest));
        for s in self.shaped.get(from..).unwrap_or_default().iter().take_while(|s| s.shaped.start < b) {
            let Shaped { line, shape, pc, anchors, start: first, end: last, .. } = s.shaped;
            let voice = match line {
                Line::Bass => continue,
                Line::Tune(_) => Voice::Tune,
                Line::Chords | Line::Broken(_) => Voice::Chords,
            };
            // A held-over end counts with what the chords hold over; the tune's counts nowhere, as the masses have it.
            let split = s.held_from.clamp(first, last);
            for (from, to, held_over) in [(first, split, false), (split, last, true)] {
                let (start, end) = (from.max(a), to.min(b));
                if start < end && !(held_over && voice == Voice::Tune) {
                    out.shaped.push(ShapedPart { voice, held_over, pc, anchors, shape, start, end });
                }
            }
        }

        // The bass where a note that doesn't belong would count against a label (B9).
        out.strong_bass.clear();
        if out.bass == BassTexture::Moving {
            for u in sound {
                let level = hearing.level(u);
                let long = self.long_bass.get(u).copied().unwrap_or(PcSet::EMPTY);
                let ticks = hearing.mass(u..u + 1, Mass::Bass);
                for pc in PitchClass::ALL {
                    if ticks[pc] == 0 || (level < Level::HalfBar && !long.contains(pc)) {
                        continue;
                    }
                    let weighted = ticks[pc].saturating_mul(level.weight());
                    out.strong_bass.push(StrongBass { pc, weighted });
                }
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
    /// The first bass note struck, and whether it may be the root at a change (E1): the bass moves to it, and it isn't
    /// a walking note ([`Evidence::facts`]).
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
    shaped: Vec<ShapedPart>,
    strong_bass: Vec<StrongBass>,
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
            shaped: Vec::new(),
            strong_bass: Vec::new(),
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

    /// b: the felt beats it sounds (S over a felt beat's ticks).
    pub fn beats(&self) -> f64 {
        self.beats
    }

    /// The notes heard in it (the design's P).
    pub fn heard(&self) -> PcSet {
        self.heard
    }

    /// The labels worth scoring on it ([`candidates()`]), in the order ties are broken.
    pub fn candidates(&self, out: &mut Vec<Candidate>) {
        candidates(self.heard, &self.presence, self.lowest.unwrap_or(PitchClass::C), out);
    }

    /// The evidence about one label.
    pub fn features(&self, label: Label, tables: &Tables, out: &mut PerFeature<f64>) {
        use Feature::*;
        let (root, core) = (label.root, label.core);
        let (tones, tensions) = (label.tones(), label.tensions());
        // The ticks of each pitch class that only pass by under the label: in the chords, in what they hold over, and
        // in the tune.
        let (mut by_chords, mut by_over, mut by_tune) = (PerPitch::default(), PerPitch::default(), PerPitch::default());
        for s in self.shaped.iter().filter(|s| passes_by(s.shape, s.pc, s.anchors, tones)) {
            let by: &mut PerPitch<u64> = match (s.voice, s.held_over) {
                (Voice::Chords, false) => &mut by_chords,
                (Voice::Chords, true) => &mut by_over,
                (Voice::Tune, _) => &mut by_tune,
            };
            by[s.pc] = by[s.pc].saturating_add(s.ticks());
        }

        // The chords and the tune, by what each note is to the label (whole ticks, divided once).
        let (mut own, mut tension, mut wrong, mut carried, mut passing, mut leaning) = (0, 0, 0, 0, 0, 0);
        let (mut over, mut over_passing) = (0, 0);
        let (mut tune_own, mut tune_tension, mut tune_wrong) = (0, 0, 0);
        for (k, weight) in (0..12).zip(ROOT_SUPPORT) {
            let pc = root.up(k);
            let ticks = self.chords[pc];
            let passes = by_chords[pc].min(ticks);
            let (held, held_passes) = (self.held_over[pc], by_over[pc].min(self.held_over[pc]));
            let (tune, tune_passes) = (self.tune[pc], by_tune[pc].min(self.tune[pc]));
            leaning += ticks.saturating_mul(weight);
            if tones.contains(pc) {
                own += ticks;
                tune_own += tune;
                continue;
            }
            passing += passes;
            over_passing += held_passes;
            let struck = self.struck.contains(pc);
            if tensions.contains(pc) {
                tension += if struck { ticks - passes } else { 0 };
                tune_tension += tune - tune_passes;
            } else {
                if struck {
                    wrong += ticks - passes;
                } else {
                    carried += ticks - passes;
                }
                over += held - held_passes;
                tune_wrong += tune - tune_passes;
            }
        }
        // What the chords hold over counts as a share of the larger of their sound and all they hold over.
        let (d, held) = (self.chords_total, self.chords_total.max(self.held_over_total));
        out[ChordTones] = share(own, d);
        out[Tensions] = share(tension, d);
        out[WrongStruck] = share(wrong, d);
        out[WrongHeld] = share(carried, d) + share(over, held);
        out[Passing] = share(passing, d) + share(over_passing, held);
        out[TuneTones] = share(tune_own, self.tune_total);
        out[TuneTensions] = share(tune_tension, self.tune_total);
        out[TuneWrong] = share(tune_wrong, self.tune_total);
        out[RootSupport] = share(leaning, d.saturating_mul(10));

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
            Core::Dominant7Sus4 => core.third().union(PcSet::of(&[2])),
            _ => core.third(),
        };
        out[NoThird] = missing(third);
        out[NoSeventh] = missing(core.seventh());
        out[NoCoreFifth] = missing(core.fifth());
        out[NoPlainFifth] =
            if core.intervals().has(7) && core.fifth().is_empty() { missing(PcSet::of(&[7])) } else { 0.0 };

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
        let fifth = core.intervals().difference(PcSet::of(&[0])).difference(core.third()).difference(core.seventh());
        out[BassRoot] = held_is(PcSet::of(&[0]));
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
            for b in self.strong_bass.iter().filter(|b| !tones.contains(b.pc)) {
                on_wrong = on_wrong.saturating_add(b.weighted);
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
        out[CorePrior] = tables.core_prior(core);
        let mut prior = 0.0;
        for k in core.tensions().iter() {
            let interval = k.number();
            let pc = root.up(i32::from(interval));
            let unpassed = self.chords[pc] - by_chords[pc].min(self.chords[pc]);
            let heard = self.struck.contains(pc) && unpassed.saturating_mul(8) >= self.sounding.max(1);
            prior += tables.tension_prior(core, interval, heard);
        }
        out[TensionPrior] = prior;
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

    /// The weighed evidence that counts for each felt beat, and that which counts once.
    fn parts(&self, features: &PerFeature<f64>, weights: &Weights) -> (f64, f64) {
        let (mut per_beat, mut once) = (0.0, 0.0);
        for f in Feature::ALL {
            let term = weights.features[f] * features[f];
            if f.once() {
                once += term;
            } else {
                per_beat += term;
            }
        }
        (per_beat, once)
    }

    /// What the span scores read as no chord: w_N for each felt beat it sounds.
    pub fn none_score(&self, weights: &Weights) -> f64 {
        self.beats * weights.none
    }

    /// The candidates scored and ranked, best first; equal scores keep the candidates' order, which runs from the
    /// lowest note up, so the ranking moves with the music.
    pub fn rank(&self, candidates: &[Candidate], weights: &Weights, tables: &Tables, out: &mut Vec<Scored>) {
        out.clear();
        let mut features = PerFeature::default();
        for c in candidates {
            self.features(c.label, tables, &mut features);
            let (per_beat, once) = self.parts(&features, weights);
            let held = self.beats * per_beat;
            out.push(Scored { label: c.label, route: c.route, score: held + once, held });
        }
        out.sort_by(|a, b| b.score.total_cmp(&a.score));
    }
}
