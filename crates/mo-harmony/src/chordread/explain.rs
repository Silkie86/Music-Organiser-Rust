//! Why a stretch reads as it does ([`ChordReader::explain`](super::ChordReader::explain)): what each note sounding
//! there does, where the bass comes from, and the evidence that tells the first reading from the runner-up
//! (harmony/design/chord-reader-design.md, "Explanations").
//!
//! A reading keeps no reasons. An explanation hears the notes again when it's asked for (a fraction of a read), and
//! works out the evidence on the stretch alone, as the decode weighs it ([`SpanFacts::features`]).
//!
//! What each note does ([`NoteDoes`]) is told from the first reading as written and from the reader's own tests, in
//! this order:
//!
//! - **Held over.** A note whose only sound in the stretch is the end it holds over a bar line or the middle of a bar
//!   ([`HeardNote::held_over`]) rings on from the chord before: the reader keeps that apart from what the chord plays.
//! - **The bass** (a bass part's note, or a chord note heard as the bass where no bass part plays). The held bass, and
//!   any of the chord's own notes, is its bass. Another note passes by when its shape on the bass line passes under
//!   the chord ([`Shaped::passes_under`]); else it walks when it's played no longer than a felt beat and sounds only
//!   off the bar line and the middle of the bar, and it clashes when it doesn't (what a moving bass is marked down
//!   for).
//! - **The chords and the tune.** A note on one of the written chord's notes is one of its own notes (its root, third,
//!   fifth, seventh or sixth, or a sus chord's 4th or 2nd) or a tension it names; one on a note of the reading that
//!   the name leaves out (heard too little) is one of its own notes too. Any other note passes by when its shape on
//!   its line passes under the chord; else it's doubtful when the name lists it as doubtful; else, on a tension the
//!   chord can carry, it's colour too little to name (in the chords), or a tune note over the chord (in the tune: one
//!   on a note the chord can't carry there clashes); and on a note that doesn't belong, it clashes.
//!
//! The evidence ([`TermWhy`]) is each piece the scoring weighs ([`Feature`]), and the pair term for how the root moves
//! from the chord before and on to the chord after, for the first reading and the runner-up: each one's value and what
//! it adds to the reading's score, the pieces that tell them apart most first.

use super::candidates::Label;
use super::hear::{BassTexture, HeardNote, Hearing, Level, Line, Shape, Shaped, Stream};
use super::name::{Reading, SpanFlags, push_to};
use super::reader::{ChordReading, Model};
use super::score::{Evidence, Feature, PerFeature, SpanFacts};
use super::weights::Priors;
use crate::chord::{Chord, Family, ToneRole, role};
use crate::notes::Grid;
use crate::pitch::PitchClass;
use crate::spell::Degree;

/// How many pieces of evidence an explanation shows: the six that tell the first reading from the runner-up most.
pub const TERMS_SHOWN: usize = 6;

/// What a note does in a stretch, under its first reading as written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NoteDoes {
    /// The bass the chord stands on: the held bass, or a bass note on one of the chord's notes. By its degree above
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
    /// runner-up's is 0 when there's none.
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

/// Why a stretch reads as it does ([`ChordReader::explain`](super::ChordReader::explain)).
#[derive(Clone, Debug, PartialEq)]
pub struct Explanation {
    /// The stretch: its place in the reading's spans.
    pub span: usize,
    /// The reading explained, the first; and the runner-up, the second, when the reading kept one.
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

/// Works out explanations: the evidence's working space, kept between them.
#[derive(Debug, Default)]
pub(super) struct Explainer {
    evidence: Evidence,
    facts: SpanFacts,
    /// The notes explained so far, by part and place, in order.
    seen: Vec<(usize, usize)>,
}

impl Explainer {
    /// Explains a stretch of a reading of what `hearing` heard on `grid`. None when the reading isn't of these notes
    /// (as far as can be told from where it starts and ends, its unit, its grid and how many notes it heard), when
    /// there's no such stretch, or when it's a rest.
    pub(super) fn explain(
        &mut self,
        hearing: &Hearing,
        grid: Grid,
        reading: &ChordReading,
        span: usize,
        model: &Model,
    ) -> Option<Explanation> {
        if !is_reading_of(hearing, grid, reading) {
            return None;
        }
        let here = reading.spans.get(span)?;
        let first = *here.readings.first()?;
        let second = here.readings.get(1).copied();
        // The stretch's units heard, and the pushes to its start and to its end, as the decode scored it.
        let reported = hearing.reported().start;
        let units = reported + here.units.start..reported + here.units.end;
        let pushed = |s: &super::ChordSpan| s.flags.contains(SpanFlags::PUSHED);
        let start = if pushed(here) { push_to(hearing, units.start) } else { None };
        let end = if reading.spans.get(span + 1).is_some_and(pushed) { push_to(hearing, units.end) } else { None };
        self.evidence.prepare(hearing);
        self.evidence.facts(hearing, units, start, end, &mut self.facts);

        let (notes, from_part) = self.notes(hearing, &first);
        let bass_from = match hearing.bass(self.facts.sound()) {
            BassTexture::None => BassFrom::NoBass,
            _ if from_part => BassFrom::BassPart,
            _ => BassFrom::LowestKeys,
        };
        let (terms, totals) = self.terms(reading, span, [Some(first.label), second.map(|r| r.label)], model);
        Some(Explanation { span, first, second, notes, bass_from, terms, totals })
    }

    /// What each note sounding in the stretch does, and whether a bass part plays there.
    fn notes(&mut self, hearing: &Hearing, first: &Reading) -> (Vec<NoteWhy>, bool) {
        let sound = self.facts.sound();
        let (from, to) = (hearing.offset(sound.start), hearing.offset(sound.end));
        let held = hearing.bass(sound).held();
        // The shapes whose shaped part is in the stretch: a note has at most one.
        let shaped: Vec<&Shaped> = hearing.shaped().iter().filter(|s| s.start < to && s.end > from).collect();
        let together = hearing.together();
        self.seen.clear();
        let (mut out, mut from_part) = (Vec::new(), false);
        for (i, n) in hearing.notes().iter().enumerate().take_while(|(_, n)| n.start < to) {
            let inside = n.end.min(to) - n.start.max(from);
            let struck_here = n.struck && n.start >= from;
            if inside <= 0 || (!struck_here && inside < together) {
                continue;
            }
            let Err(at) = self.seen.binary_search(&(n.part, n.note)) else { continue };
            self.seen.insert(at, (n.part, n.note));
            let shape = shaped.iter().find(|s| s.note == i).copied();
            let bass = in_bass(hearing, n, from);
            let does = does(hearing, n, shape, Place { from, to, held, bass }, first);
            from_part |= n.stream == Stream::Bass;
            let heard_as = if bass { Stream::Bass } else { n.stream };
            out.push(NoteWhy { part: n.part, note: n.note, key: n.key, heard_as, does });
        }
        (out, from_part)
    }

    /// The evidence for each label (the runner-up's none when there isn't one), the piece that leads by most first,
    /// and each one's total.
    fn terms(
        &self,
        reading: &ChordReading,
        span: usize,
        labels: [Option<Label>; 2],
        model: &Model,
    ) -> (Vec<TermWhy>, [f64; 2]) {
        let priors = Priors::of(model.tables);
        let mut values = [PerFeature::default(); 2];
        for (label, out) in labels.iter().zip(&mut values) {
            if let Some(label) = label {
                self.facts.features(*label, &priors, out);
            }
        }
        let w = &model.weights.features;
        let beats = self.facts.beats();
        let mut terms: Vec<TermWhy> = Feature::ALL
            .iter()
            .map(|&f| {
                let counted = if f.once() { 1.0 } else { beats };
                let [a, b] = values;
                TermWhy {
                    term: Term::Feature(f),
                    values: [a[f], b[f]],
                    scores: [w[f] * a[f] * counted, w[f] * b[f] * counted],
                }
            })
            .collect();
        // The pair term: how the root moves from the chord before (round a loop, its last) and to the chord after (the
        // decode reads no move from a loop's last chord round to its first).
        let before = match span.checked_sub(1) {
            Some(b) => reading.spans.get(b),
            None if reading.looped => reading.spans.last(),
            None => None,
        };
        let (before, after) = (before.and_then(|s| s.label), reading.spans.get(span + 1).and_then(|s| s.label));
        let pair = |label: Option<Label>| {
            let Some(label) = label else { return 0.0 };
            let into = before.map_or(0.0, |b| model.tables.pair_prior(b, label));
            into + after.map_or(0.0, |a| model.tables.pair_prior(label, a))
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

/// Whether a reading is of what was heard on a grid, as far as can be told: the same grid and unit, the same start and
/// end, as many notes, read as a loop or not alike, and its stretches running to the end.
fn is_reading_of(hearing: &Hearing, grid: Grid, reading: &ChordReading) -> bool {
    let reported = hearing.reported();
    let notes = u32::try_from(hearing.notes_heard()).unwrap_or(u32::MAX);
    reading.grid == grid
        && reading.unit == hearing.unit()
        && reading.looped == (reported.start > 0)
        && reading.start == hearing.start(reported.start)
        && reading.end == hearing.start(hearing.units())
        && reading.notes == notes
        && reading.spans.last().is_some_and(|s| s.units.end == reported.len())
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
        if held == Some(pc) || tones.contains(pc) || chord.notes().contains(pc) {
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
    match n.stream {
        // The tune counts only on a tension the chord can carry there, as the name's gate has it.
        Stream::Melody => {
            let intervals = label.core.intervals();
            let carried = role(intervals, Family::of(intervals), i32::from(pc.above(label.root))) != ToneRole::Avoid;
            if carried { NoteDoes::TuneOver(degree) } else { NoteDoes::Clash }
        }
        Stream::Harmony | Stream::Bass => NoteDoes::Colour(degree),
    }
}

impl Explanation {
    /// The pieces of evidence to show: the [`TERMS_SHOWN`] that tell the readings apart most.
    pub fn shown_terms(&self) -> &[TermWhy] {
        self.terms.get(..TERMS_SHOWN.min(self.terms.len())).unwrap_or_default()
    }
}
