//! The chord reader as the app uses it ([`ChordReader`], [`read_chords`]): some notes in, the chords a player would
//! write over them out ([`ChordReading`]), each stretch with its readings and the answer a person would give.
//!
//! It hears the notes, decodes where the chords change and which chord each stretch is, names each chord from the
//! notes really played, and grades each answer ([`super::grade`]). The answers never depend on how many readings the
//! caller keeps: every reading is named and graded, then the first few are kept.
//!
//! What it gives, besides the readings:
//!
//! - the chord chart, spelled in a key ([`ChordReading::names`]); the key is never evidence;
//! - the choices for Chord check ([`ChordSpan::choices`]);
//! - chord names found elsewhere (a file's name) checked against the reading ([`ChordReading::check_names`]): never
//!   evidence either.

use std::ops::Range;

use super::answer::{ChordAnswer, Grading, Texture, Thresholds, grade};
use super::cores::Core;
use super::decode::{Decoder, Decoding, PinnedSpan};
use super::hear::{Hearing, Unit};
use super::name::{Namer, Naming, Reading, SpanFlags};
use super::tables::COUNTED;
use super::weights::{HAND, THRESHOLDS, Tables, Weights};
use crate::chord::Chord;
use crate::key::Key;
use crate::keyfind::Span;
use crate::notes::{Grid, Part};
use crate::pitch::PitchClass;
use crate::spell::{Spelled, root_in_key, spell_in_key};

/// The most readings a span keeps ([`ChordContext::readings`]).
pub const MOST_READINGS: u8 = 6;

/// The least chance a reading needs to be one of Chord check's choices, besides the first ([`ChordSpan::choices`]):
/// below it are mostly the same notes written as no player would (Gsus4(add6)/C for C). The choices are worked out
/// from the readings each time they're shown (a Chord check answer keeps those it showed), so the floor isn't part of
/// what the reader's version covers.
pub const CHOICE_FLOOR: f64 = 0.01;

/// A reader: the weights the evidence is weighed by, the counted tables the priors read, and the thresholds the
/// answers are graded by. [`SHIPPED`] is the one built in; the bench passes the ones it fits.
#[derive(Clone, Copy, Debug)]
pub struct Model<'a> {
    pub weights: &'a Weights,
    pub tables: &'a Tables,
    pub thresholds: &'a Thresholds,
}

/// The reader built into the app: the hand-set weights and thresholds, on the counted tables.
pub const SHIPPED: Model<'static> = Model { weights: &HAND, tables: &COUNTED, thresholds: &THRESHOLDS };

/// What a read is told besides the notes.
#[derive(Clone, Copy, Debug)]
pub struct ChordContext<'a> {
    /// What the notes are, as cut from the music: a loop goes round (its first chord comes after its last, and a chord
    /// pushed across its end is struck there); a song, or a stretch cut from one, doesn't.
    pub span: Span,
    /// Stretches whose chord is given, or only where they start and end: Chord check answers and the user's fixes.
    /// A control, never evidence.
    pub pins: &'a [Pin],
    /// How many readings each span keeps, best first: 1 to [`MOST_READINGS`] (the MIDI page's index keeps 2, Chord
    /// check 6); 0 keeps 1, and more than the most keeps the most. The answers don't depend on it.
    pub readings: u8,
}

impl ChordContext<'static> {
    /// No pins, and two readings a span, as the MIDI page's index keeps.
    pub const fn new(span: Span) -> ChordContext<'static> {
        ChordContext { span, pins: &[], readings: 2 }
    }
}

/// A stretch whose chord is given (a Chord check answer or the user's fix), or only where it starts and ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Pin {
    /// Where it starts and ends, in the music's own ticks: each is moved to the nearest place within the music where
    /// a chord may change (a unit's start). One that's empty there, or overlaps one before it, is left out. In a loop,
    /// one over its last bar is the chord before its first too, as the loop goes round.
    pub start: i64,
    pub end: i64,
    /// Its chord, written as given (in root position, over the bass held under it). None pins only where it starts
    /// and ends: it's read as one chord, whichever reads best. A chord with no third, 4th, 2nd or fifth pins only
    /// where it starts and ends too.
    pub chord: Option<Chord>,
}

/// What the reader read in some notes: the chords, stretch by stretch, from where the music starts to where it ends.
#[derive(Clone, Debug, PartialEq)]
pub struct ChordReading {
    /// The stretches, in order: the next starts where one ends.
    pub spans: Vec<ChordSpan>,
    /// What kinds of line were heard.
    pub texture: Texture,
    /// The unit chords change on: half a beat, or a beat in compound metre.
    pub unit: Unit,
    /// Where the reading starts and ends, in the music's own ticks, on units' starts: from the bar line at or before
    /// the first note to the end of the unit the last note ends in (a loop: from the bar line at or before its first
    /// note to the bar line it ends on).
    pub start: i64,
    pub end: i64,
    /// Notes heard (drums and ghost notes left out).
    pub notes: u32,
    /// Whether some notes were left out, lying further apart than the reader hears at once (a damaged file).
    pub truncated: bool,
    /// Whether it was read as a loop: its first chord comes after its last.
    pub looped: bool,
}

/// A stretch read as one chord, or as no chord.
#[derive(Clone, Debug, PartialEq)]
pub struct ChordSpan {
    /// Its units, counted from the reading's first.
    pub units: Range<usize>,
    /// Where it starts and ends, in the music's own ticks (on units' starts), and where its chord is first struck:
    /// before `start` when it's pushed.
    pub start: i64,
    pub end: i64,
    pub heard_start: i64,
    /// The chance a chord starts where it starts.
    pub change: f64,
    pub answer: ChordAnswer,
    /// Its readings, the one the best way reads first, then the others by their chances, each written differently.
    /// None for a rest.
    pub readings: Vec<Reading>,
    /// The chance it's no chord, and the chance it's none of the readings kept (no chord among them).
    pub no_chord: f64,
    pub other: f64,
    pub flags: SpanFlags,
}

/// Where one of Chord check's choices comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChoiceFrom {
    /// The first reading.
    First,
    /// The same notes on another root or bass (C6 for Am7).
    SameNotes,
    /// The first reading with its doubtful tensions named, or with its tensions left out (Cmaj9 for Cmaj7, Cmaj7 for
    /// Cmaj9).
    Tensions,
    /// Another reading.
    Other,
}

/// One of Chord check's choices: a chord to play (with its own root in the bass) and to pick.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Choice {
    pub chord: Chord,
    pub from: ChoiceFrom,
}

/// How a chord name found elsewhere (a file's name) stands to the reading ([`ChordReading::check_names`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NameCheck {
    /// It's a chord the reading shows.
    Agrees,
    /// It's one of the other readings of a chord the reading shows.
    Listed,
    /// The reading doesn't have it.
    Disagrees,
}

impl ChordSpan {
    /// The reading the chart shows for it: the first, unless the answer is no chord.
    pub fn shown(&self) -> Option<&Reading> {
        self.readings.first().filter(|_| self.answer.shows_chord())
    }

    /// Chord check's choices for it, at most `n`, all different: the first reading; the best reading of its same-notes
    /// group on another root or bass (Am7 for C6), or when there's none, the first reading with its doubtful tensions
    /// named or its tensions left out (Cmaj9 for Cmaj7); the best reading outside its group; then the other readings,
    /// in order. Besides the first, a reading is a choice only with a chance of [`CHOICE_FLOOR`] or more.
    pub fn choices(&self, n: usize) -> Vec<Choice> {
        let Some(first) = self.readings.first() else { return Vec::new() };
        let rest = || self.readings.iter().skip(1).filter(|r| r.chance >= CHOICE_FLOOR);
        let other_way = |r: &&Reading| {
            r.group == first.group && (r.chord.root() != first.chord.root() || r.chord.bass() != first.chord.bass())
        };
        let second = match rest().find(other_way) {
            Some(same) => Some(Choice { chord: same.chord, from: ChoiceFrom::SameNotes }),
            None => tensions_other_way(first).map(|chord| Choice { chord, from: ChoiceFrom::Tensions }),
        };
        let outside = rest().find(|r| r.group != first.group);
        let others = outside.into_iter().chain(rest()).map(|r| Choice { chord: r.chord, from: ChoiceFrom::Other });
        let mut out: Vec<Choice> = Vec::new();
        for choice in [Choice { chord: first.chord, from: ChoiceFrom::First }].into_iter().chain(second).chain(others) {
            if out.len() < n && !out.iter().any(|c| c.chord == choice.chord) {
                out.push(choice);
            }
        }
        out
    }
}

/// A reading's chord with its doubtful tensions named, or when it has none, with its tensions left out: its core's
/// own notes on its root (Cmaj7 for Cmaj9, G7sus4 for G9sus4). None when it has neither.
fn tensions_other_way(r: &Reading) -> Option<Chord> {
    let c = r.chord;
    if !r.doubtful.is_empty() {
        return Some(Chord::new(c.root(), c.notes().union(r.doubtful), c.bass()));
    }
    let core = Core::of(c.intervals().with(PitchClass::C))?;
    let own = core.intervals().on(c.root());
    let plain = c.notes().intersection(own);
    (plain != c.notes()).then(|| Chord::new(c.root(), plain, c.bass()))
}

impl ChordReading {
    /// The chord chart: each span's shown reading ([`ChordSpan::shown`]) spelled in `key`, each root by what the next
    /// chord on the chart does (round a loop, the first comes after the last), or as charts spell chords when no key
    /// is given; none for a span with no chord. The key is only for spelling, never evidence.
    pub fn names(&self, key: Option<Key>) -> Vec<Option<Spelled>> {
        let chart: Vec<(usize, Chord)> =
            self.spans.iter().enumerate().filter_map(|(i, s)| s.shown().map(|r| (i, r.chord))).collect();
        let mut run: Vec<Chord> = chart.iter().map(|&(_, c)| c).collect();
        // Round a loop: spelled twice over, so the last chord knows the first as it's spelled.
        if self.looped {
            run.extend_from_within(..);
        }
        let spelled = spell_in_key(&run, key);
        let mut out = vec![None; self.spans.len()];
        for (&(i, _), s) in chart.iter().zip(spelled) {
            if let Some(slot) = out.get_mut(i) {
                *slot = Some(s);
            }
        }
        out
    }

    /// A chord written at a span (one of its readings, or the other way to write one), spelled as the chart spells
    /// chords there: in `key`, by what the next chord on the chart does ([`ChordReading::names`]).
    pub fn spell(&self, span: usize, chord: Chord, key: Option<Key>) -> Spelled {
        let Some(key) = key else { return Spelled::plain(chord) };
        let names = self.names(Some(key));
        let after = names.get(span + 1..).unwrap_or_default().iter().flatten().next();
        let round = if self.looped { names.iter().flatten().next() } else { None };
        Spelled::new(chord, root_in_key(chord, key, after.or(round).copied()))
    }

    /// Checks chord names found elsewhere (a file's name: "Fm9 Bb13 Ebmaj9.mid") against the reading, in order: each
    /// agrees when a chord the chart shows after the last one matched is it, is listed when it's one of the readings
    /// of one, and disagrees otherwise. A name is a chord when its root is one of the reading's roots (any of a
    /// diminished seventh's), its core is the same (its tensions may differ), and, when it has a bass of its own (a
    /// slash), its bass is the same. The names are never evidence.
    pub fn check_names(&self, names: &[Chord]) -> Vec<NameCheck> {
        let chart: Vec<&ChordSpan> = self.spans.iter().filter(|s| s.shown().is_some()).collect();
        let mut from = 0;
        let find = |from: &mut usize, matches: &dyn Fn(&ChordSpan) -> bool| {
            let k = chart.get(*from..).unwrap_or_default().iter().position(|s| matches(s))?;
            *from += k + 1;
            Some(())
        };
        names
            .iter()
            .map(|&name| {
                if find(&mut from, &|s| s.readings.first().is_some_and(|r| names_it(r, name))).is_some() {
                    NameCheck::Agrees
                } else if find(&mut from, &|s| s.readings.iter().any(|r| names_it(r, name))).is_some() {
                    NameCheck::Listed
                } else {
                    NameCheck::Disagrees
                }
            })
            .collect()
    }
}

/// Whether a chord name is a reading: its chord or the other way to write it ([`ChordReading::check_names`]).
fn names_it(r: &Reading, name: Chord) -> bool {
    let core = |c: Chord| Core::of(c.intervals().with(PitchClass::C));
    let bass = |c: Chord| name.bass() == name.root() || name.bass() == c.bass();
    let chord = r.roots.contains(name.root()) && core(r.chord) == core(name) && bass(r.chord);
    chord || r.also.is_some_and(|c| c.root() == name.root() && core(c) == core(name) && bass(c))
}

/// Reads the chords in some notes ([`ChordReader::read`]). Keep one and reuse it: it keeps its working space between
/// reads, so reading many parts doesn't ask for memory each time (only the reading it gives back does).
#[derive(Debug, Default)]
pub struct ChordReader {
    hearing: Hearing,
    decoder: Decoder,
    decoding: Decoding,
    namer: Namer,
    naming: Naming,
    pins: Vec<PinnedSpan>,
}

impl ChordReader {
    pub fn new() -> ChordReader {
        ChordReader::default()
    }

    /// Reads the chords in the parts on the grid. None when no pitched note is heard (drums and ghost notes are left
    /// out).
    pub fn read(&mut self, parts: &[Part], grid: Grid, cx: &ChordContext) -> Option<ChordReading> {
        self.read_with(parts, grid, cx, &SHIPPED)
    }

    /// Reads the chords with a model (the bench passes the ones it fits).
    pub fn read_with(&mut self, parts: &[Part], grid: Grid, cx: &ChordContext, model: &Model) -> Option<ChordReading> {
        if !self.hearing.hear(parts, grid, cx.span) {
            return None;
        }
        self.pin(cx.pins);
        if !self.decoder.decode(&self.hearing, model.weights, model.tables, &self.pins, &mut self.decoding) {
            return None;
        }
        // Every reading is named and graded, whatever the caller keeps.
        self.namer.name(&self.hearing, &self.decoder, &self.decoding, &self.pins, usize::MAX, &mut self.naming);
        Some(self.reading(cx, model))
    }

    /// What the last read heard.
    pub fn hearing(&self) -> &Hearing {
        &self.hearing
    }

    /// Puts the pins on the units heard: each end at the nearest unit start within the music (a loop's own units,
    /// never its copied bar), leaving out those empty there and those over one before them. A loop goes round: a pin
    /// over its last bar pins the copy of that bar heard in front too, so its first chord follows the chord given.
    fn pin(&mut self, pins: &[Pin]) {
        let reported = self.hearing.reported();
        let unit = |tick: i64| self.hearing.nearest_unit(tick).max(reported.start);
        self.pins.clear();
        self.pins.extend(pins.iter().map(|p| PinnedSpan { units: unit(p.start)..unit(p.end), chord: p.chord }));
        self.pins.retain(|p| !p.units.is_empty());
        self.pins.sort_by_key(|p| (p.units.start, p.units.end));
        let mut end = 0;
        self.pins.retain(|p| {
            let free = p.units.start >= end;
            if free {
                end = p.units.end;
            }
            free
        });
        // The copied bar's units are the last bar's, a loop's length earlier.
        let length = reported.len();
        if reported.start > 0 {
            for i in 0..self.pins.len() {
                let Some(p) = self.pins.get(i) else { break };
                let (copy, chord) = (p.units.start.saturating_sub(length)..p.units.end.saturating_sub(length), p.chord);
                if !copy.is_empty() {
                    self.pins.push(PinnedSpan { units: copy, chord });
                }
            }
        }
    }

    fn reading(&self, cx: &ChordContext, model: &Model) -> ChordReading {
        let hearing = &self.hearing;
        let texture = Texture::of(hearing);
        let reported = hearing.reported();
        let keep = usize::from(cx.readings.clamp(1, MOST_READINGS));
        let spans = self
            .naming
            .spans()
            .iter()
            .map(|s| {
                let all = self.naming.readings(s);
                let first = all.first();
                let group = |r: &&Reading| first.is_some_and(|f| f.group == r.group);
                let grading = Grading {
                    rest: s.rest,
                    given: s.flags.contains(SpanFlags::GIVEN),
                    heard: hearing.heard(s.sound.clone()).len(),
                    decoded: s.label.is_some(),
                    first: first.map_or(0.0, |r| r.chance),
                    rootless_with_no_bass: first.is_some_and(|r| r.rootless) && s.flags.contains(SpanFlags::NO_BASS),
                    group: all.iter().filter(group).count(),
                    group_chance: all.iter().filter(group).map(|r| r.chance).sum(),
                    texture,
                };
                let readings: Vec<Reading> = all.iter().take(keep).copied().collect();
                let kept: f64 = readings.iter().map(|r| r.chance).sum();
                ChordSpan {
                    units: s.units.start.saturating_sub(reported.start)..s.units.end.saturating_sub(reported.start),
                    start: s.start,
                    end: s.end,
                    heard_start: s.heard_start,
                    change: s.change,
                    answer: grade(grading, model.thresholds),
                    readings,
                    no_chord: s.none,
                    other: (1.0 - kept).clamp(0.0, 1.0),
                    flags: s.flags,
                }
            })
            .collect();
        ChordReading {
            spans,
            texture,
            unit: hearing.unit(),
            start: hearing.start(reported.start),
            end: hearing.start(hearing.units()),
            notes: u32::try_from(hearing.notes_heard()).unwrap_or(u32::MAX),
            truncated: hearing.truncated(),
            looped: reported.start > 0,
        }
    }
}

/// Reads the chords in the parts on the grid. None when no pitched note is heard. To read many parts, keep a
/// [`ChordReader`] and call [`ChordReader::read`], which reuses its working space.
pub fn read_chords(parts: &[Part], grid: Grid, cx: &ChordContext) -> Option<ChordReading> {
    ChordReader::new().read(parts, grid, cx)
}
