//! Why a stretch reads as it does ([`ChordExplainer::explain`]): what each note sounding there does, where the bass
//! comes from, and the evidence that tells the first reading from the runner-up (harmony/design/chord-reader-design.md,
//! "Explanations").
//!
//! A reading keeps no reasons. An explanation hears the notes again when it's asked for, with no decode or naming (64
//! dense bars: under a millisecond, where the read takes 50), or once for every stretch explained together
//! ([`ChordExplainer::explain_all`]), and works out the evidence on the stretch alone, as the decode weighs it
//! ([`SpanFacts::features`]).
//!
//! What each note does ([`NoteDoes`]) is told from the first reading as written and from the reader's own tests, in
//! this order:
//!
//! - **Held over.** A note whose only sound in the stretch is the end it holds over a bar line or the middle of a bar
//!   ([`HeardNote::held_over`]) rings on from the chord before: the reader keeps that apart from what the chord plays.
//! - **The bass** (a bass part's note, or a chord note heard as the bass where no bass part plays). The held bass, and
//!   any note the written chord sounds (its own, or the bass written under it), is its bass. Another note passes by
//!   when its shape on the bass line passes under the chord ([`Shaped::passes_under`]); else it walks when it's played
//!   no longer than a felt beat and sounds only off the bar line and the middle of the bar, and it clashes when it
//!   doesn't (what a moving bass is marked down for).
//! - **The chords and the tune.** A note on one of the written chord's notes is one of its own notes (its root, third,
//!   fifth, seventh or sixth, or a sus chord's 4th or 2nd) or a tension it names; one on the bass written under it
//!   that's only a bass (the C of D/C) doubles the bass; one on a note of the reading that the name leaves out (heard
//!   too little) is one of its own notes too. Any other note passes by when its shape on its line passes under the
//!   chord; else it's doubtful when the name lists it as doubtful; else, on a tension the chord can carry, it's colour
//!   too little to name (in the chords), or a tune note over the chord (in the tune: one on a note the chord can't
//!   carry there clashes); and on a note that doesn't belong, it clashes.
//!
//! The evidence ([`TermWhy`]) is each piece the scoring weighs ([`Feature`]), and the pair term for how the root moves
//! from the chord before and on to the chord after, for the first reading and the runner-up: each one's value and what
//! it adds to the reading's score, the pieces that tell them apart most first. A reading that's the chord before (for
//! a loop's first stretch, the chord the decode reads before it as the loop goes round: [`ChordReading::before`]) is
//! that chord held on, with no chord starting where the stretch starts: the second half of a chord cut in two where
//! its bass moves, a loop's first chord carried round from its end, or a runner-up that would be the chord before
//! going on. The decode weighs none of the evidence about where a chord starts for it ([`SpanFacts::held_score`]), and
//! nor does the explanation. The decode holds a chord on only from where the chord before ends, never from a chord
//! pushed in early, so where the stretch's chord is pushed, a reading held on is weighed on the stretch's own units.
//! Where a pin starts or ends, every reading starts a chord, as the decode reads it.

use std::collections::HashSet;
use std::ops::Range;

use super::candidates::Label;
use super::decode::{PinnedSpan, kept_pins};
use super::hear::{BassTexture, HeardNote, Hearing, Level, Line, Shape, Shaped, Stream};
use super::name::{Reading, SpanFlags, push_to};
use super::reader::{ChordContext, ChordReading, ChordSpan, Model, ReadFrom, SHIPPED, pinned};
use super::score::{Evidence, Feature, PerFeature, SpanFacts};
use super::weights::Priors;
use crate::chord::{Chord, Family, ToneRole, role};
use crate::notes::{Grid, Part};
use crate::pitch::PitchClass;
use crate::spell::Degree;

/// How many pieces of evidence an explanation shows: the six that tell the first reading from the runner-up most.
pub const TERMS_SHOWN: usize = 6;

/// What a note does in a stretch, under its first reading as written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NoteDoes {
    /// The bass the chord stands on: the held bass, or a bass note on a note the chord sounds; or, in the chords or
    /// the tune, a note doubling a bass written under the chord that's only a bass (the C of D/C). By its degree above
    /// the chord's root (its own when it's one of the chord's notes, the ♭7 of D/C).
    Bass(Degree),
    /// One of the chord's own notes, by its degree: its root, third, fifth, seventh or sixth, or a sus chord's 4th or
    /// 2nd (the ♭3 of Dm9).
    ChordTone(Degree),
    /// A tension the chord names, by its degree: the 9 of Dm9.
    Extension(Degree),
    /// A tension heard too little to name: the name lists it beside the chord as doubtful.
    Doubtful(Degree),
    /// A tension in the chords heard too little to name or to list: passing colour.
    Colour(Degree),
    /// A tune note on a tension the chord can carry, too short or struck too weakly to count: the tune moving over the
    /// chord.
    TuneOver(Degree),
    /// Passes by under the chord, between two of its notes: the D of C D E.
    Passing,
    /// Steps away from one of the chord's notes and back: the E of D E D.
    Neighbour,
    /// In the bass, a step into the next note, on a stronger beat: the A♭ of A A♭ | G.
    Approach,
    /// Held over from before and stepping down to one of the chord's notes: the F of F | E over a C chord.
    Suspension,
    /// Struck just before the chord it belongs to, and held into it: the tune's A an eighth before the bar of A minor.
    Anticipation,
    /// Leaps in on a beat and steps out to one of the chord's notes.
    Appoggiatura,
    /// A bass note that isn't one of the chord's, moving between them off the bar line and the middle of the bar, no
    /// longer than a felt beat: a walking bass's note.
    Walking,
    /// Rings on from the chord before, over the bar line or the middle of the bar: legato or pedal.
    HeldOver,
    /// A note that doesn't belong and doesn't pass by: it clashes with the chord.
    Clash,
}

impl NoteDoes {
    /// What a note with a shape does when it passes by under a chord.
    pub const fn of_shape(shape: Shape) -> NoteDoes {
        match shape {
            Shape::Passing => NoteDoes::Passing,
            Shape::Neighbour => NoteDoes::Neighbour,
            Shape::Approach => NoteDoes::Approach,
            Shape::Suspension => NoteDoes::Suspension,
            Shape::Anticipation => NoteDoes::Anticipation,
            Shape::Appoggiatura => NoteDoes::Appoggiatura,
        }
    }

    /// Whether the note belongs to the chord: its bass, one of its own notes or a tension it names.
    pub const fn belongs(self) -> bool {
        matches!(self, NoteDoes::Bass(_) | NoteDoes::ChordTone(_) | NoteDoes::Extension(_))
    }
}

/// A note of the music and what it does in the stretch explained.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NoteWhy {
    /// Where it came from: its part's place in the parts read, and its place in the part's notes.
    pub part: usize,
    pub note: usize,
    pub key: u8,
    /// What it's heard as: the bass (a bass part's note, or a chord note heard as the bass where no bass part plays),
    /// the tune, or the chords.
    pub heard_as: Stream,
    pub does: NoteDoes,
}

/// Where the bass under a stretch comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BassFrom {
    /// A bass part plays it.
    BassPart,
    /// No bass part plays there: it's the lowest chord note, a fifth or more under the next one up
    /// ([`super::BASS_GAP`]).
    LowestKeys,
    /// No bass sounds.
    NoBass,
}

/// A piece of evidence an explanation weighs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Term {
    /// One the scoring weighs on the stretch.
    Feature(Feature),
    /// How the root moves from the chord before and on to the chord after ([`super::Tables::pair_prior`], times its
    /// trust): none into or out of no chord, or for the same chord again.
    Pair,
}

/// A piece of evidence for the first reading of a stretch and the runner-up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TermWhy {
    pub term: Term,
    /// The evidence itself, for each: a share of the stretch's sound (0 to 1), 1 or 0 for yes or no, or a prior (a
    /// logarithm: 0 or less for P1 and P2, above 0 for a root moving as chords move more often than at random). The
    /// runner-up's is 0 when there's none, and a reading held on from the chord before has none about where a chord
    /// starts (the module's last paragraph).
    pub values: [f64; 2],
    /// What it adds to each one's score: its value weighed, for each felt beat the stretch sounds, or once for the
    /// evidence about where it starts.
    pub scores: [f64; 2],
}

impl TermWhy {
    /// How much more it adds to the first reading's score than to the runner-up's.
    pub fn lead(&self) -> f64 {
        let [first, second] = self.scores;
        first - second
    }
}

/// Why a stretch reads as it does ([`ChordExplainer::explain`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Explanation {
    /// The stretch: its place in the reading's spans.
    pub span: usize,
    /// The reading explained, the first (where the decode reads no chord, the likeliest chord all the same); and the
    /// runner-up, the second, when the reading kept one.
    pub first: Reading,
    pub second: Option<Reading>,
    /// What each note sounding in the stretch does under the first reading, in order of where they start (a note
    /// that only rings on into it for less than a quarter of a unit is left out, as are drums and ghost notes).
    pub notes: Vec<NoteWhy>,
    pub bass_from: BassFrom,
    /// The evidence that tells the first reading from the runner-up, the piece that leads by most first (with no
    /// runner-up, the first reading's evidence, the biggest first); evidence that's the same for both is left out.
    /// The first [`TERMS_SHOWN`] are the ones to show.
    pub terms: Vec<TermWhy>,
    /// Each one's score on the stretch read on its own: every piece of its evidence added up (0 for no runner-up). The
    /// decode reads the stretches together, so these needn't be the scores that ranked them.
    pub totals: [f64; 2],
}

/// Explains readings ([`ChordExplainer::explain`]). Keep one while explaining: it keeps its working space between
/// explanations (the notes heard again among it), apart from any [`super::ChordReader`], so a reader kept for reading
/// many parts holds none of it.
#[derive(Debug, Default)]
pub struct ChordExplainer {
    hearing: Hearing,
    evidence: Evidence,
    facts: SpanFacts,
    /// The evidence on a pushed stretch's own units, for a reading held on into it.
    plain: SpanFacts,
    /// Each note's shape, by the note's place in the notes heard (a note has at most one).
    shape_of: Vec<Option<usize>>,
    /// How long the longest note heard sounds, in ticks.
    longest: i64,
    /// The units where every reading starts a chord, as the decode reads it: where each pin it keeps starts and ends.
    edges: Vec<usize>,
    pinned: Vec<PinnedSpan>,
    /// The notes explained in the stretch so far, by part and place (a loop's copied bar copies some).
    seen: HashSet<(usize, usize)>,
}

impl ChordExplainer {
    pub fn new() -> ChordExplainer {
        ChordExplainer::default()
    }

    /// Why a stretch of a reading reads as it does ([`Explanation`]): what each note sounding there does, where the
    /// bass comes from, and the evidence that tells its first reading from the runner-up (the second reading kept, so
    /// the reading must keep two for there to be one). The notes are heard again (no decode or naming: 64 dense bars
    /// take under a millisecond, where the read takes 50), so give the parts, the grid and the context the reading
    /// was read from, its pins too. None when the reading isn't of those notes and pins (as far as can be told), when
    /// there's no such stretch, or when it's a rest. To explain every stretch, [`ChordExplainer::explain_all`] hears
    /// the notes once.
    pub fn explain(
        &mut self,
        parts: &[Part],
        grid: Grid,
        cx: &ChordContext,
        reading: &ChordReading,
        span: usize,
    ) -> Option<Explanation> {
        self.explain_with(parts, grid, cx, reading, span, &SHIPPED)
    }

    /// Explains a stretch with a model ([`ChordExplainer::explain`]): the one the reading was read with.
    pub fn explain_with(
        &mut self,
        parts: &[Part],
        grid: Grid,
        cx: &ChordContext,
        reading: &ChordReading,
        span: usize,
        model: &Model,
    ) -> Option<Explanation> {
        if !self.hear(parts, grid, cx, reading) {
            return None;
        }
        self.stretch(reading, span, model)
    }

    /// Explains every stretch of a reading that has a reading ([`ChordExplainer::explain`]), in order, hearing the
    /// notes once: rests and stretches with no reading are left out (each explanation says which stretch it's of).
    /// None when the reading isn't of those notes and pins.
    pub fn explain_all(
        &mut self,
        parts: &[Part],
        grid: Grid,
        cx: &ChordContext,
        reading: &ChordReading,
    ) -> Option<Vec<Explanation>> {
        self.explain_all_with(parts, grid, cx, reading, &SHIPPED)
    }

    /// Explains every stretch with a model ([`ChordExplainer::explain_all`]): the one the reading was read with.
    pub fn explain_all_with(
        &mut self,
        parts: &[Part],
        grid: Grid,
        cx: &ChordContext,
        reading: &ChordReading,
        model: &Model,
    ) -> Option<Vec<Explanation>> {
        if !self.hear(parts, grid, cx, reading) {
            return None;
        }
        Some((0..reading.spans.len()).filter_map(|span| self.stretch(reading, span, model)).collect())
    }

    /// Hears the notes a reading is said to be of and gets the evidence ready: false when nothing is heard or the
    /// reading isn't of them and the pins given (as far as can be told from where it starts and ends, its unit, its
    /// grid, how many notes it heard, its stretches running to the end, and its stretches fitting the pins:
    /// `pinned_as_read`).
    fn hear(&mut self, parts: &[Part], grid: Grid, cx: &ChordContext, reading: &ChordReading) -> bool {
        let hearing = &mut self.hearing;
        if !hearing.hear(parts, grid, cx.span)
            || ReadFrom::heard(hearing, grid) != ReadFrom::of(reading)
            || !reading.spans.last().is_some_and(|s| s.units.end == hearing.reported().len())
        {
            return false;
        }
        let hearing = &self.hearing;
        self.pinned.clear();
        self.pinned.extend(cx.pins.iter().map(|pin| pinned(hearing, pin)));
        let kept = kept_pins(hearing, &self.pinned);
        self.edges.clear();
        self.edges.extend(kept.iter().flat_map(|(units, _)| [units.start, units.end]));
        self.edges.sort_unstable();
        self.edges.dedup();
        if !pinned_as_read(hearing, &kept, &self.edges, reading) {
            return false;
        }
        self.evidence.prepare(hearing);
        let notes = hearing.notes();
        self.longest = notes.iter().map(|n| n.end - n.start).max().unwrap_or(0);
        self.shape_of.clear();
        self.shape_of.resize(notes.len(), None);
        for (k, shaped) in hearing.shaped().iter().enumerate() {
            if let Some(slot @ None) = self.shape_of.get_mut(shaped.note) {
                *slot = Some(k);
            }
        }
        true
    }

    /// Explains a stretch of a reading of the notes last heard ([`ChordExplainer::hear`]). None when there's no such
    /// stretch, or when it's a rest.
    fn stretch(&mut self, reading: &ChordReading, span: usize, model: &Model) -> Option<Explanation> {
        let here = reading.spans.get(span)?;
        let first = *here.readings.first()?;
        let second = here.readings.get(1).copied();
        // The stretch's units heard, and the pushes to its start and to its end, as the decode scored it.
        let reported = self.hearing.reported().start;
        let units = reported + here.units.start..reported + here.units.end;
        let pushed = |s: &ChordSpan| s.flags.contains(SpanFlags::PUSHED);
        let start = if pushed(here) { push_to(&self.hearing, units.start) } else { None };
        let end =
            if reading.spans.get(span + 1).is_some_and(pushed) { push_to(&self.hearing, units.end) } else { None };
        self.evidence.facts(&self.hearing, units.clone(), start, end, &mut self.facts);
        // The chords either side, as the decode read them: the one before (in a loop, the one before its first as it
        // goes round), and the one after (the decode reads no move from a loop's last chord round to its first).
        let around = Around {
            before: match span.checked_sub(1) {
                Some(b) => reading.spans.get(b).and_then(|s| s.label),
                None => reading.before,
            },
            after: reading.spans.get(span + 1).and_then(|s| s.label),
            edge: self.edges.binary_search(&units.start).is_ok(),
        };
        let labels = [Some(first.label), second.map(|r| r.label)];
        // A reading held on into a stretch with a chord pushed to it is held on from the place it's pushed from (the
        // decode never holds a chord on from a place a chord is pushed to): it sounds over the stretch's own units.
        let plain = start.is_some() && labels.iter().flatten().any(|&label| around.holds_on(label));
        if plain {
            self.evidence.facts(&self.hearing, units, None, end, &mut self.plain);
        }

        let (notes, from_part) = self.notes(&first);
        let bass_from = match self.hearing.bass(self.facts.sound()) {
            BassTexture::None => BassFrom::NoBass,
            _ if from_part => BassFrom::BassPart,
            _ => BassFrom::LowestKeys,
        };
        let (terms, totals) = self.terms(labels, around, plain, model);
        Some(Explanation { span, first, second, notes, bass_from, terms, totals })
    }

    /// What each note sounding in the stretch does, and whether a bass part plays there (a note only held over into
    /// it doesn't).
    fn notes(&mut self, first: &Reading) -> (Vec<NoteWhy>, bool) {
        let hearing = &self.hearing;
        let sound = self.facts.sound();
        let (from, to) = (hearing.offset(sound.start), hearing.offset(sound.end));
        let held = hearing.bass(sound).held();
        let together = hearing.together();
        self.seen.clear();
        let (mut out, mut from_part) = (Vec::new(), false);
        // A note that starts before the longest note ends before the stretch.
        let notes = hearing.notes();
        let skip = notes.partition_point(|n| n.start < from - self.longest);
        for (i, n) in notes.iter().enumerate().skip(skip).take_while(|(_, n)| n.start < to) {
            let inside = n.end.min(to) - n.start.max(from);
            let struck_here = n.struck && n.start >= from;
            if inside <= 0 || (!struck_here && inside < together) || !self.seen.insert((n.part, n.note)) {
                continue;
            }
            // Its shape, when its shaped part is in the stretch.
            let shape = self.shape_of.get(i).copied().flatten().and_then(|k| hearing.shaped().get(k));
            let shape = shape.filter(|s| s.start < to && s.end > from);
            let bass = in_bass(hearing, n, from);
            let does = does(hearing, n, shape, Place { from, to, held, bass }, first);
            from_part |= n.stream == Stream::Bass && does != NoteDoes::HeldOver;
            let heard_as = if bass { Stream::Bass } else { n.stream };
            out.push(NoteWhy { part: n.part, note: n.note, key: n.key, heard_as, does });
        }
        (out, from_part)
    }

    /// The evidence for each label (the runner-up's none when there isn't one), the piece that leads by most first,
    /// and each one's total; with `plain`, a reading held on is weighed on the stretch's own units.
    fn terms(
        &self,
        labels: [Option<Label>; 2],
        around: Around,
        plain: bool,
        model: &Model,
    ) -> (Vec<TermWhy>, [f64; 2]) {
        let priors = Priors::of(model.tables);
        let mut values = [PerFeature::default(); 2];
        let mut beats = [self.facts.beats(); 2];
        for ((label, out), beats) in labels.iter().zip(&mut values).zip(&mut beats) {
            let Some(label) = *label else { continue };
            let held = around.holds_on(label);
            let facts = if held && plain { &self.plain } else { &self.facts };
            facts.features(label, &priors, out);
            *beats = facts.beats();
            if held {
                for f in Feature::ALL.into_iter().filter(|f| f.once()) {
                    out[f] = 0.0;
                }
            }
        }
        let w = &model.weights.features;
        let mut terms: Vec<TermWhy> = Feature::ALL
            .iter()
            .map(|&f| {
                let counted = |k: usize| if f.once() { 1.0 } else { beats.get(k).copied().unwrap_or(0.0) };
                let [a, b] = values;
                TermWhy {
                    term: Term::Feature(f),
                    values: [a[f], b[f]],
                    scores: [w[f] * a[f] * counted(0), w[f] * b[f] * counted(1)],
                }
            })
            .collect();
        // The pair term: how the root moves from the chord before and on to the chord after (none for the same chord).
        let pair = |label: Option<Label>| {
            let Some(label) = label else { return 0.0 };
            let into = around.before.map_or(0.0, |b| model.tables.pair_prior(b, label));
            into + around.after.map_or(0.0, |a| model.tables.pair_prior(label, a))
        };
        let [a, b] = labels.map(pair);
        terms.push(TermWhy {
            term: Term::Pair,
            values: [a, b],
            scores: [model.weights.pair * a, model.weights.pair * b],
        });
        let totals = terms.iter().fold([0.0, 0.0], |[x, y], t| [x + t.scores[0], y + t.scores[1]]);
        terms.retain(|t| t.lead() != 0.0);
        // Equal leads keep the evidence's order.
        terms.sort_by(|a, b| b.lead().abs().total_cmp(&a.lead().abs()));
        (terms, totals)
    }
}

/// Whether a reading was read with the pins kept (whose `edges` are where they start and end), as far as can be told:
/// a chord starts where each starts and ends; the stretches given a chord are those of the pins with one (each as one
/// stretch, never cut in two), each chord written as given (in root position, over the bass held under it); and two
/// stretches side by side read as the same chord are one chord cut in two where its bass moves, or a pin's edge is
/// between them (the decode reads a chord held on as one stretch).
fn pinned_as_read(
    hearing: &Hearing,
    kept: &[(Range<usize>, &PinnedSpan)],
    edges: &[usize],
    reading: &ChordReading,
) -> bool {
    let reported = hearing.reported();
    // A stretch starts at a unit, or the music ends there.
    let starts = |unit: usize| {
        let unit = unit.saturating_sub(reported.start);
        unit == reported.len() || reading.spans.binary_search_by_key(&unit, |s| s.units.start).is_ok()
    };
    // The pins over the loop's own units (in a loop, those over the copy of its end in front are read there).
    let here = || kept.iter().filter(|(units, _)| units.start >= reported.start);
    let given = |s: &&ChordSpan| s.flags.contains(SpanFlags::GIVEN);
    let as_given = |s: &ChordSpan, chord: Chord| {
        s.readings.first().map(|r| r.chord).is_some_and(|written| {
            let over_the_bass = Chord::new(chord.root(), chord.notes(), written.bass());
            written == chord || (chord.bass() == chord.root() && written == over_the_bass)
        })
    };
    let pinned_given = |s: &ChordSpan| {
        let units = reported.start + s.units.start..reported.start + s.units.end;
        here().any(|(pin, p)| *pin == units && p.given().is_some_and(|chord| as_given(s, chord)))
    };
    let apart = |pair: &[ChordSpan]| match pair {
        [a, b] if b.label.is_some() && a.label == b.label && !b.flags.contains(SpanFlags::SPLIT) => {
            edges.binary_search(&(reported.start + b.units.start)).is_ok()
        }
        _ => true,
    };
    here().all(|(units, _)| starts(units.start) && starts(units.end))
        && here().filter(|(_, p)| p.given().is_some()).count() == reading.spans.iter().filter(given).count()
        && reading.spans.iter().filter(given).all(pinned_given)
        && reading.spans.windows(2).all(apart)
}

/// The chords either side of a stretch, as the decode read them, and whether a pin starts or ends where it starts.
#[derive(Clone, Copy, Debug)]
struct Around {
    before: Option<Label>,
    after: Option<Label>,
    edge: bool,
}

impl Around {
    /// Whether a reading of the stretch is the chord before it held on: the same chord, with no pin's edge between.
    fn holds_on(self, label: Label) -> bool {
        self.before == Some(label) && !self.edge
    }
}

/// Where a note is looked at: the stretch's sound (in ticks from the first unit's start), the bass it holds, and
/// whether the note is heard as the bass.
#[derive(Clone, Copy, Debug)]
struct Place {
    from: i64,
    to: i64,
    held: Option<PitchClass>,
    bass: bool,
}

/// Whether a note is heard as the bass in a stretch starting at `from`: a bass part's note, or a chord note that is the
/// bass of the unit it first sounds in there.
fn in_bass(hearing: &Hearing, n: &HeardNote, from: i64) -> bool {
    match n.stream {
        Stream::Bass => true,
        Stream::Harmony => hearing.is_bass_in(hearing.unit_of(n.start.max(from)), n),
        Stream::Melody => false,
    }
}

/// What a note does in a stretch under its first reading (the module's rules, in order).
fn does(hearing: &Hearing, n: &HeardNote, shape: Option<&Shaped>, place: Place, first: &Reading) -> NoteDoes {
    let Place { from, to, held, bass } = place;
    if n.held_over.is_some_and(|h| h <= from) {
        return NoteDoes::HeldOver;
    }
    let (label, chord, pc) = (first.label, first.chord, n.pitch_class());
    let tones = label.tones();
    let passes = |line_is_bass: bool| {
        shape.filter(|s| (s.line == Line::Bass) == line_is_bass && s.passes_under(tones)).map(|s| s.shape)
    };
    if bass {
        if held == Some(pc) || tones.contains(pc) || chord.sounding().contains(pc) {
            return NoteDoes::Bass(Chord::new(chord.root(), chord.notes(), pc).bass_degree());
        }
        if let Some(shape) = passes(true) {
            return NoteDoes::of_shape(shape);
        }
        // Played no longer than a felt beat, and sounding only in units weaker than the middle of a bar.
        let (a, b) = (hearing.unit_of(n.start.max(from)), hearing.unit_of(n.end.min(to) - 1));
        let weak = (a..=b).all(|u| hearing.level(u) < Level::HalfBar);
        return if n.length <= hearing.felt_ticks() && weak { NoteDoes::Walking } else { NoteDoes::Clash };
    }
    let interval = i32::from(pc.above(chord.root()));
    if chord.notes().contains(pc) {
        let degree = chord.degree(interval);
        return match chord.role(interval) {
            ToneRole::Root | ToneRole::Third | ToneRole::Fifth | ToneRole::Seventh => NoteDoes::ChordTone(degree),
            ToneRole::Tension | ToneRole::Avoid => NoteDoes::Extension(degree),
        };
    }
    // The bass written under the chord, only a bass: doubled.
    if pc == chord.bass() {
        return NoteDoes::Bass(chord.bass_degree());
    }
    // Its degree were it named.
    let degree = Chord::new(chord.root(), chord.notes().with(pc), chord.bass()).degree(interval);
    if tones.contains(pc) {
        return NoteDoes::ChordTone(degree);
    }
    if let Some(shape) = passes(false) {
        return NoteDoes::of_shape(shape);
    }
    if first.doubtful.contains(pc) {
        return NoteDoes::Doubtful(degree);
    }
    if !label.tensions().contains(pc) {
        return NoteDoes::Clash;
    }
    // Only the chords and the tune get here: a bass part's notes are heard as the bass.
    if n.stream != Stream::Melody {
        return NoteDoes::Colour(degree);
    }
    // The tune counts only on a tension the chord can carry there, as the name's gate has it.
    let intervals = label.core.intervals();
    let carried = role(intervals, Family::of(intervals), i32::from(pc.above(label.root))) != ToneRole::Avoid;
    if carried { NoteDoes::TuneOver(degree) } else { NoteDoes::Clash }
}

impl Explanation {
    /// The pieces of evidence to show: the [`TERMS_SHOWN`] that tell the readings apart most.
    pub fn shown_terms(&self) -> &[TermWhy] {
        self.terms.get(..TERMS_SHOWN.min(self.terms.len())).unwrap_or_default()
    }
}
