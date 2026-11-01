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
use super::hear::{BassTexture, Hearing, Level, Line, Mass, Push, Shape, Stream, heard_in};
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
    /// T4: the share in notes that don't belong, carried in from before the span, and what the chords hold over a bar
    /// line or the middle of a bar on notes that don't belong.
    WrongHeld,
    /// T5: the share in notes that only pass by under it.
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
    /// T11: the same for the flat or sharp fifth of a diminished, half-diminished or augmented chord.
    NoAlteredFifth,
    /// T12: the same for a plain fifth.
    NoFifth,
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
    /// L1: no bass sounds, and the lowest chord note sounding where the span starts is its root.
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
    /// B9: the same on notes that don't belong, where they sound on a bar line or the middle of a bar or from a note
    /// played longer than a felt beat.
    WalkingWrong,
    /// E1, once: the first bass note struck in the span is its root.
    BassStrikesRoot,
    /// E2, once: the share of its own notes heard in the span that are struck where it starts.
    StruckAtStart,
    /// E3, once: nothing in the chords or the bass is struck where the span starts.
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
        Feature::NoAlteredFifth,
        Feature::NoFifth,
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
            NoAlteredFifth => "T11",
            NoFifth => "T12",
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

/// A shaped note's part in a span.
#[derive(Clone, Copy, Debug)]
struct ShapedPart {
    voice: Voice,
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
    shaped: Vec<super::hear::Shaped>,
    longest: i64,
    /// For each unit, the bass pitch classes sounding in it from a note played longer than a felt beat.
    long_bass: Vec<PcSet>,
}

impl Evidence {
    pub fn new() -> Evidence {
        Evidence::default()
    }

    /// Gets ready for the spans of a hearing: what passes by, and where the bass holds a note longer than a felt beat.
    pub fn prepare(&mut self, hearing: &Hearing) {
        self.shaped.clear();
        self.shaped.extend_from_slice(hearing.shaped());
        self.shaped.sort_unstable_by_key(|s| (s.start, s.end, s.note));
        self.longest = self.shaped.iter().map(|s| s.end - s.start).max().unwrap_or(0).max(0);
        let units = hearing.units();
        self.long_bass.clear();
        self.long_bass.resize(units, PcSet::EMPTY);
        let felt = hearing.felt_ticks();
        for n in hearing.notes().iter().filter(|n| n.length > felt && n.end > n.start) {
            let (from, to) = (hearing.unit_of(n.start), hearing.unit_of(n.end - 1).min(units.saturating_sub(1)));
            let pc = n.pitch_class();
            for u in from..=to {
                // A chord note is the bass where it's the unit's gap bass, or its octave.
                let bass = match n.stream {
                    Stream::Bass => true,
                    Stream::Harmony => hearing.gap_bass(u).is_some_and(|k| k % 12 == n.key % 12),
                    Stream::Melody => false,
                };
                if let (true, Some(set)) = (bass, self.long_bass.get_mut(u)) {
                    *set = set.with(pc);
                }
            }
        }
    }

    /// What a span holds, for scoring every label on it: `units` are its units, and `push` a push to its first unit
    /// when the chord struck just before that is counted from it (the chord's notes then count as struck there).
    pub fn facts(&self, hearing: &Hearing, units: Range<usize>, push: Option<&Push>, out: &mut SpanFacts) {
        let end = units.end.min(hearing.units());
        let units = units.start.min(end)..end;
        let push = push.filter(|p| p.unit == units.start && units.start < units.end);
        let s = hearing.sounding_ticks(units.clone());
        out.units = units.clone();
        out.sounding = s;
        out.beats = s as f64 / hearing.felt_ticks().max(1) as f64;
        out.chords = hearing.mass(units.clone(), Mass::Harmony);
        out.chords_total = total(&out.chords);
        out.held_over = hearing.mass(units.clone(), Mass::HeldOver);
        out.held_over_total = total(&out.held_over);
        out.tune = hearing.mass(units.clone(), Mass::Melody);
        out.tune_total = total(&out.tune);
        out.weighted_bass = hearing.mass(units.clone(), Mass::WeightedBass);
        out.weighted_bass_total = total(&out.weighted_bass);
        let bass = hearing.mass(units.clone(), Mass::Bass);
        let held_tune = hearing.mass(units.clone(), Mass::HeldMelody);
        let mut played = PerPitch::default();
        for pc in PitchClass::ALL {
            let h = out.chords[pc].saturating_mul(2);
            out.presence[pc] = h.saturating_add(bass[pc].saturating_mul(2)).saturating_add(held_tune[pc]);
            played[pc] = h.saturating_add(held_tune[pc]);
        }
        out.heard = heard_in(&out.presence, s);
        out.played = heard_in(&played, s);
        out.bass = hearing.bass(units.clone());
        out.level = hearing.level(units.start);
        out.pushed = push.is_some();
        out.lowest = hearing.lowest(units.clone()).map(PitchClass::of_key);
        out.lowest_chord = match out.bass {
            BassTexture::None => hearing.lowest_sounding(units.start).map(PitchClass::of_key),
            _ => None,
        };

        let mut struck = hearing.struck_in(units.clone(), Stream::Harmony);
        let mut first = hearing.struck(units.start, Stream::Harmony).union(hearing.struck(units.start, Stream::Bass));
        let mut onset = hearing.bass_onset(units.clone());
        if let Some(push) = push {
            // The pushed chord's notes count as struck where the span starts; its bass, if it has one, is the first
            // bass note struck.
            let before = push.unit.checked_sub(1).and_then(|u| hearing.gap_bass(u));
            let mut pushed_bass: Option<u8> = None;
            for (_, n) in hearing.push_notes(push) {
                let is_bass = match n.stream {
                    Stream::Bass => true,
                    Stream::Harmony => before.is_some_and(|k| k % 12 == n.key % 12),
                    Stream::Melody => false,
                };
                if is_bass {
                    pushed_bass = Some(pushed_bass.map_or(n.key, |k| k.min(n.key)));
                } else {
                    struck = struck.with(n.pitch_class());
                }
                first = first.with(n.pitch_class());
            }
            onset = pushed_bass.map(PitchClass::of_key).or(onset);
        }
        out.struck = struck;
        out.first_struck = first;
        out.bass_onset = onset;

        // What passes by in the span.
        out.shaped.clear();
        let (a, b) = (hearing.offset(units.start), hearing.offset(units.end));
        let from = self.shaped.partition_point(|s| s.start < a.saturating_sub(self.longest));
        for s in self.shaped.get(from..).unwrap_or_default().iter().take_while(|s| s.start < b) {
            let (start, end) = (s.start.max(a), s.end.min(b));
            if start >= end {
                continue;
            }
            let voice = match s.line {
                Line::Bass => continue,
                Line::Tune(_) => Voice::Tune,
                Line::Chords | Line::Broken(_) => Voice::Chords,
            };
            out.shaped.push(ShapedPart { voice, pc: s.pc, anchors: s.anchors, shape: s.shape, start, end });
        }

        // The bass where a note that doesn't belong would count against a label (B9).
        out.strong_bass.clear();
        if out.bass == BassTexture::Moving {
            for u in units.clone() {
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

/// The sum of twelve tick counts.
fn total(ticks: &PerPitch<u64>) -> u64 {
    ticks.0.iter().fold(0u64, |sum, &t| sum.saturating_add(t))
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
    units: Range<usize>,
    /// S, the ticks of its units where a chord or bass note sounds, and b = S over a felt beat's ticks.
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
    bass_onset: Option<PitchClass>,
    /// The lowest note sounding (for the candidates' order), and with no bass, the lowest chord note sounding where
    /// it starts (for L1).
    lowest: Option<PitchClass>,
    lowest_chord: Option<PitchClass>,
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
            lowest: None,
            lowest_chord: None,
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
}

impl SpanFacts {
    pub fn new() -> SpanFacts {
        SpanFacts::default()
    }

    /// Its units.
    pub fn units(&self) -> Range<usize> {
        self.units.clone()
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
        // The ticks of each pitch class that only pass by under the label, in the chords and in the tune.
        let (mut by_chords, mut by_tune) = (PerPitch::<u64>::default(), PerPitch::<u64>::default());
        for s in self.shaped.iter().filter(|s| passes_by(s.shape, s.pc, s.anchors, tones)) {
            match s.voice {
                Voice::Chords => by_chords[s.pc] = by_chords[s.pc].saturating_add(s.ticks()),
                Voice::Tune => by_tune[s.pc] = by_tune[s.pc].saturating_add(s.ticks()),
            }
        }

        // The chords and the tune, by what each note is to the label (whole ticks, divided once).
        let (mut own, mut tension, mut wrong, mut carried, mut passing, mut over, mut leaning) = (0, 0, 0, 0, 0, 0, 0);
        let (mut tune_own, mut tune_tension, mut tune_wrong) = (0, 0, 0);
        for (k, weight) in (0..12).zip(ROOT_SUPPORT) {
            let pc = root.up(k);
            let ticks = self.chords[pc];
            let passes = by_chords[pc].min(ticks);
            let (tune, tune_passes) = (self.tune[pc], by_tune[pc].min(self.tune[pc]));
            leaning += ticks.saturating_mul(weight);
            if tones.contains(pc) {
                own += ticks;
                tune_own += tune;
                continue;
            }
            passing += passes;
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
                over += self.held_over[pc];
                tune_wrong += tune - tune_passes;
            }
        }
        let d = self.chords_total;
        out[ChordTones] = share(own, d);
        out[Tensions] = share(tension, d);
        out[WrongStruck] = share(wrong, d);
        out[WrongHeld] = share(carried, d) + share(over, d.max(self.held_over_total));
        out[Passing] = share(passing, d);
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
        out[NoAlteredFifth] = match core {
            Core::HalfDiminished | Core::Diminished7 | Core::Diminished | Core::Augmented => missing(core.fifth()),
            _ => 0.0,
        };
        out[NoFifth] = if core.intervals().has(7) { missing(PcSet::of(&[7])) } else { 0.0 };

        // Where the root is.
        let (played, heard, bass) = (self.played.contains(root), self.heard.contains(root), self.bass);
        out[RootPlayed] = flag(played);
        out[RootInBass] = flag(heard && !played);
        out[RootlessOverBass] = flag(!heard && bass != BassTexture::None);
        out[RootlessNoBass] = flag(!heard && bass == BassTexture::None);
        out[LowestIsRoot] = flag(self.lowest_chord == Some(root));

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
        out[BassStrikesRoot] = flag(self.bass_onset == Some(root));
        let sounding_tones = tones.intersection(self.heard);
        out[StruckAtStart] =
            share(u64::from(sounding_tones.intersection(self.first_struck).len()), u64::from(sounding_tones.len()));
        out[NothingStruck] = flag(self.first_struck.is_empty());
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
            let f = tables.tension(core, interval);
            let unpassed = self.chords[pc] - by_chords[pc].min(self.chords[pc]);
            let heard = self.struck.contains(pc) && unpassed.saturating_mul(8) >= self.sounding.max(1);
            prior += if heard { f.ln() } else { (1.0 - f).ln() };
        }
        out[TensionPrior] = prior;
    }

    /// A label's score from its evidence: each piece weighed, those about the whole span counted for each felt beat it
    /// sounds (b), those about where it starts once.
    pub fn score(&self, features: &PerFeature<f64>, weights: &Weights) -> f64 {
        let (mut per_beat, mut once) = (0.0, 0.0);
        for f in Feature::ALL {
            let term = weights.features[f] * features[f];
            if f.once() {
                once += term;
            } else {
                per_beat += term;
            }
        }
        self.beats * per_beat + once
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
            out.push(Scored { label: c.label, route: c.route, score: self.score(&features, weights) });
        }
        out.sort_by(|a, b| b.score.total_cmp(&a.score));
    }
}
