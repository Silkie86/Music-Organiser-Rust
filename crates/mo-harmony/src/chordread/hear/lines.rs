//! The lines in the music and how their notes move: which notes of the bass, the tunes, the top of the chords and each
//! broken chord are shaped like passing notes, neighbours, approaches, anticipations, suspensions or appoggiaturas.
//!
//! This is the half of the test that needs no chord. The reader then tries each shaped note against each chord: the
//! note only passes by under a chord when it isn't one of the chord's own notes and the notes it hangs on (its
//! anchors) are the chord's. Only lines are shaped: the inner notes of chords played together have no voice of their
//! own to move in.

use super::{HeardNote, Hearing, Level, Stream, uncount};
use crate::pitch::{PcSet, PitchClass};

/// How a line note moves between the notes around it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Shape {
    /// A step (one or two semitones) in and a step out the same way, on a weaker beat than one of the notes either
    /// side and no longer than either: the D of C D E.
    Passing,
    /// A step out and back to the same note, on a weaker beat than one of them and no longer than either: the E of
    /// D E D.
    Neighbour,
    /// In the bass, a step into the next note, which is on a stronger beat: the Ab of A Ab | G, the B of G B | C. A
    /// step up or down a scale (one of its two steps a whole tone) is a passing note instead: the E of D E F.
    Approach,
    /// Struck before a stronger beat (at most a felt beat before) and held into it, where its pitch class is struck,
    /// when that pitch class wasn't sounding just before it: the bass's G on beat 4 before the bar of G, the tune's A
    /// an eighth before the bar of A minor. A chord note played again on the next beat comes early to nothing.
    Anticipation,
    /// Not in the bass: held at least a unit into a felt beat at least as strong as where it's struck (the strongest
    /// it's held into), then a step down to a note no shorter than the part held: the F of F | E over a C chord,
    /// struck in the bar before. A note whose pitch class is struck where it's held into sounds with it there, so it's
    /// an anticipation instead.
    Suspension,
    /// Not in the bass: a leap in and a step out the other way, on a felt beat stronger than where it resolves.
    Appoggiatura,
}

// Passing notes, neighbours, approaches and appoggiaturas are at most a felt beat long: a longer note is the music's
// own, whichever way the notes around it move.

/// The line a shaped note is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Line {
    /// The bass: the lowest note of each strike of the bass parts, and in bars where no bass part plays, the chord
    /// notes that are the bass ([`Hearing::gap_bass`]).
    Bass,
    /// The top line of a tune part (the part's place in the list of parts).
    Tune(usize),
    /// The top line of the chords played together.
    Chords,
    /// A chord part's broken chords, strike by strike (the part's place in the list of parts).
    Broken(usize),
}

/// A line note with a shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Shaped {
    /// The note, by its place in [`Hearing::notes`].
    pub note: usize,
    pub line: Line,
    pub shape: Shape,
    pub pc: PitchClass,
    /// The pitch classes it hangs on: the notes either side of a passing note, the note a neighbour leaves and comes
    /// back to, the note an approach leads to, the note a suspension or an appoggiatura resolves to. An anticipation
    /// hangs on the chord it comes early to, so its anchor is its own pitch class.
    pub anchors: PcSet,
    /// Where it's shaped, in ticks from the first unit's start: the whole note as played, but an anticipation only
    /// until the beat it comes early to, and a suspension only from the beat it's held into.
    pub start: i64,
    pub end: i64,
}

/// A note of a line, as the shapes see it: as played (a broken chord's pedal left out).
#[derive(Clone, Copy, Debug)]
struct Step {
    note: usize,
    key: u8,
    start: i64,
    end: i64,
    length: i64,
    /// How strong the place it's struck is: its unit's weight ([`Level::weight`]), or 0 between unit starts.
    weight: u64,
    /// Whether it may be shaped. A strike of a chord part that isn't broken is on the part's broken line only as a note
    /// the broken notes either side move from or to: its top note is shaped on the top line of the chords.
    shaped: bool,
}

impl Step {
    fn pc(self) -> PitchClass {
        PitchClass::of_key(self.key)
    }
}

/// Working space for the lines, kept between hearings.
#[derive(Debug, Default)]
pub(super) struct LineSpace {
    steps: Vec<Step>,
    /// The notes a line is drawn from, by their place in the notes.
    picked: Vec<usize>,
    /// Notes struck before, still sounding: where each stops, and its key.
    ringing: Vec<(i64, u8)>,
    parts: Vec<usize>,
}

impl Hearing {
    /// The line notes that have a shape: the bass line's first, then each tune's, the top of the chords', and each
    /// chord part's broken chords', each in order of where they start (a loop's copied bar included). A note has at
    /// most one shape.
    pub fn shaped(&self) -> &[Shaped] {
        &self.shaped
    }

    /// Finds the lines and the shapes of their notes. It needs the units worked out first: the bass of the bars where
    /// no bass part plays, and what's struck where.
    pub(super) fn find_shapes(&mut self) {
        let mut out = std::mem::take(&mut self.shaped);
        let mut space = std::mem::take(&mut self.line_space);
        out.clear();

        self.bass_line(&mut space);
        self.shape_line(&space.steps, Line::Bass, &mut out);

        let mut parts = std::mem::take(&mut space.parts);
        self.parts_of(&mut parts, |n| n.stream == Stream::Melody);
        for &part in &parts {
            self.top_line(&mut space, |n| n.stream == Stream::Melody && n.part == part, false);
            self.shape_line(&space.steps, Line::Tune(part), &mut out);
        }

        self.top_line(&mut space, |n| n.stream == Stream::Harmony && !n.broken, true);
        self.shape_line(&space.steps, Line::Chords, &mut out);

        self.parts_of(&mut parts, |n| n.broken);
        for &part in &parts {
            self.broken_line(&mut space, part);
            self.shape_line(&space.steps, Line::Broken(part), &mut out);
        }

        space.parts = parts;
        self.shaped = out;
        self.line_space = space;
    }

    /// The parts with notes that `of` picks, in order.
    fn parts_of(&self, parts: &mut Vec<usize>, of: impl Fn(&HeardNote) -> bool) {
        parts.clear();
        parts.extend(self.notes.iter().filter(|n| of(n)).map(|n| n.part));
        parts.sort_unstable();
        parts.dedup();
    }

    /// Whether a chord note is the bass of the unit it's struck in.
    fn is_gap_bass(&self, n: &HeardNote) -> bool {
        self.gap_bass(self.layout.unit_at(n.start)) == Some(n.key)
    }

    /// Whether a chord note struck in a strike is the bass there, as the hearing has it: the bass of its unit, or its
    /// octave with no chord note of another pitch class struck below it (a left hand's octave, not a chord's top).
    fn in_bass(&self, n: &HeardNote, strike: &[usize]) -> bool {
        let Some(bass) = self.gap_bass(self.layout.unit_at(n.start)) else { return false };
        let other_below = |m: &HeardNote| m.key < n.key && m.key % 12 != bass % 12;
        n.key % 12 == bass % 12 && !self.strike_notes(strike).any(|(_, m)| other_below(&m))
    }

    /// Notes struck within a quarter of a unit of each other are one strike.
    fn together(&self) -> i64 {
        (self.layout.unit_ticks() / 4).max(1)
    }

    fn step(&self, note: usize, n: &HeardNote) -> Step {
        let unit = self.layout.unit_at(n.start);
        // Struck a little late (by less than a quarter of a unit) is on the unit's start: that's a player's timing.
        let on_start = n.start - self.layout.start(unit) < self.together();
        let weight = if on_start { self.layout.level(unit).weight() } else { 0 };
        let length = n.length.max(1);
        Step { note, key: n.key, start: n.start, end: n.start.saturating_add(length), length, weight, shaped: true }
    }

    /// Splits `picked` (in order of where the notes start) into strikes, calling `each` with each one.
    fn strikes(&self, picked: &[usize], mut each: impl FnMut(&[usize])) {
        let together = self.together();
        let mut rest = picked;
        while let Some(first) = rest.first().and_then(|&i| self.notes.get(i)) {
            let same = |i: &&usize| self.notes.get(**i).is_some_and(|n| n.start - first.start < together);
            let size = rest.iter().take_while(same).count().max(1);
            let Some((strike, later)) = rest.split_at_checked(size) else { break };
            rest = later;
            each(strike);
        }
    }

    /// A strike's notes, with their places in the notes.
    fn strike_notes<'a>(&'a self, strike: &'a [usize]) -> impl Iterator<Item = (usize, HeardNote)> + 'a {
        strike.iter().filter_map(|&i| self.notes.get(i).map(|n| (i, *n)))
    }

    /// The highest of some notes (the first of them, when two are as high).
    fn top(notes: impl Iterator<Item = (usize, HeardNote)>) -> Option<(usize, HeardNote)> {
        notes.reduce(|top, n| if n.1.key > top.1.key { n } else { top })
    }

    /// The bass line: the lowest note of each strike of the bass parts and of the chord notes that are the bass.
    fn bass_line(&self, space: &mut LineSpace) {
        let LineSpace { steps, picked, .. } = space;
        steps.clear();
        picked.clear();
        let bass = |n: &HeardNote| match n.stream {
            Stream::Bass => n.struck,
            Stream::Harmony => n.struck && !n.broken && self.is_gap_bass(n),
            Stream::Melody => false,
        };
        picked.extend(self.notes.iter().enumerate().filter(|(_, n)| bass(n)).map(|(i, _)| i));
        self.strikes(picked, |strike| {
            let lowest = self.strike_notes(strike).reduce(|low, n| if n.1.key < low.1.key { n } else { low });
            if let Some((i, n)) = lowest {
                steps.push(self.step(i, &n));
            }
        });
    }

    /// The top line of some notes (those `take` picks): the highest note struck in each strike, unless a higher note
    /// struck before still sounds more than a quarter of a unit into it (a little legato overlap, or a held-over end,
    /// doesn't count): then it's an inner voice. A loop's copy of a note struck before its last bar is one struck
    /// before. On the chords' line (`chords`), the bass isn't a chord's top ([`Hearing::in_bass`]).
    fn top_line(&self, space: &mut LineSpace, take: impl Fn(&HeardNote) -> bool, chords: bool) {
        let LineSpace { steps, picked, ringing, .. } = space;
        steps.clear();
        picked.clear();
        ringing.clear();
        let together = self.together();
        // Where a note stops ringing: as played, before any held-over end.
        let until = |n: &HeardNote| {
            let played = n.start.saturating_add(n.length.max(1)).min(n.end);
            played.min(n.held_over.unwrap_or(i64::MAX))
        };
        picked.extend(self.notes.iter().enumerate().filter(|(_, n)| take(n)).map(|(i, _)| i));
        self.strikes(picked, |strike| {
            let notes = || self.strike_notes(strike);
            ringing.extend(notes().filter(|(_, n)| !n.struck).map(|(_, n)| (until(&n), n.key)));
            let bass = |n: &HeardNote| chords && self.in_bass(n, strike);
            if let Some((i, top)) = Self::top(notes().filter(|(_, n)| n.struck && !bass(n))) {
                ringing.retain(|&(until, _)| until - top.start > together);
                if !ringing.iter().any(|&(_, key)| key > top.key) {
                    steps.push(self.step(i, &top));
                }
            }
            ringing.extend(notes().filter(|(_, n)| n.struck).map(|(_, n)| (until(&n), n.key)));
        });
    }

    /// A part's broken chords, strike by strike: the highest broken note of each (a bass held under the arpeggio and
    /// struck with it isn't broken). The part's other strikes are on the line too, their highest notes, but aren't
    /// shaped there: an arpeggio's last note may lead to the top of the chord played together after it.
    fn broken_line(&self, space: &mut LineSpace, part: usize) {
        let LineSpace { steps, picked, .. } = space;
        steps.clear();
        picked.clear();
        let of_part = |n: &HeardNote| n.part == part && n.stream == Stream::Harmony && n.struck;
        picked.extend(self.notes.iter().enumerate().filter(|(_, n)| of_part(n)).map(|(i, _)| i));
        self.strikes(picked, |strike| {
            let broken = Self::top(self.strike_notes(strike).filter(|(_, n)| n.broken));
            let (top, shaped) = match broken {
                Some(top) => (Some(top), true),
                None => (Self::top(self.strike_notes(strike)), false),
            };
            if let Some((i, n)) = top {
                steps.push(Step { shaped, ..self.step(i, &n) });
            }
        });
    }

    /// Shapes the notes of one line. Two notes of a line are next to each other only when the later starts at most a
    /// felt beat after the earlier stops; a loop's last note goes on to its first, as it goes round.
    fn shape_line(&self, steps: &[Step], line: Line, out: &mut Vec<Shaped>) {
        let felt = self.layout.felt_ticks();
        // A loop's own first note, a loop further on.
        let wrapped = self.loop_round().and_then(|(loop_start, round)| {
            let first = steps.iter().find(|s| s.start >= loop_start)?;
            Some(Step { start: first.start.saturating_add(round), end: first.end.saturating_add(round), ..*first })
        });
        for (i, &s) in steps.iter().enumerate().filter(|(_, s)| s.shaped) {
            let prev = i.checked_sub(1).and_then(|p| steps.get(p)).copied().filter(|p| s.start - p.end <= felt);
            let next = steps.get(i + 1).copied().or(wrapped).filter(|n| n.start > s.start && n.start - s.end <= felt);
            if let Some((shape, anchors, start, end)) = self.shape(s, prev, next, line == Line::Bass) {
                out.push(Shaped { note: s.note, line, shape, pc: s.pc(), anchors, start, end });
            }
        }
    }

    /// In a loop: where its own first unit starts, and how long it is.
    fn loop_round(&self) -> Option<(i64, i64)> {
        let loop_start = self.layout.start(self.layout.first);
        (self.layout.first > 0).then_some((loop_start, self.end - loop_start))
    }

    /// The shape of a line note, if it has one: the first that fits of an anticipation, a suspension (not in the
    /// bass), an approach (in the bass), a passing note, a neighbour and an appoggiatura (not in the bass). With its
    /// anchors, and where it's shaped.
    fn shape(&self, s: Step, prev: Option<Step>, next: Option<Step>, bass: bool) -> Option<(Shape, PcSet, i64, i64)> {
        let felt = self.layout.felt_ticks();
        let short = s.length <= felt;
        let into = prev.map(|p| i32::from(s.key) - i32::from(p.key));
        let out = next.map(|n| i32::from(n.key) - i32::from(s.key));
        let step = |d: Option<i32>| d.is_some_and(|d| (1..=2).contains(&d.abs()));
        let same_way = into.map(i32::signum) == out.map(i32::signum);
        let one = |pc: PitchClass| PcSet::EMPTY.with(pc);

        // Struck before a stronger beat and held into it, where its pitch class is struck, and new.
        if let Some(at) = self.next_stronger(s)
            && s.end >= at
            && at - s.start <= felt
            && self.struck_at(at).contains(s.pc())
            && !self.sounding_before(s).contains(s.pc())
        {
            return Some((Shape::Anticipation, one(s.pc()), s.start, at));
        }
        // Held into a felt beat at least as strong as where it's struck, then a step down to a note no shorter than
        // the part held.
        if let Some(n) = next
            && !bass
            && let Some(at) = self.held_into(s)
            && n.start >= at
            && out.is_some_and(|d| (-2..=-1).contains(&d))
            && s.end - at <= n.length
        {
            return Some((Shape::Suspension, one(n.pc()), at, s.end));
        }
        // In the bass, a step into a stronger beat; but not a step up or down a scale.
        if let Some(n) = next
            && bass
            && short
            && step(out)
            && n.weight > s.weight
            && s.length <= n.length
        {
            let scale = step(into) && same_way && (into.map(i32::abs) == Some(2) || out.map(i32::abs) == Some(2));
            if !scale {
                return Some((Shape::Approach, one(n.pc()), s.start, s.end));
            }
        }
        // Steps in and out, on a weaker beat than one of the notes either side and no longer than either.
        if let (Some(p), Some(n)) = (prev, next)
            && short
            && step(into)
            && step(out)
            && s.weight < p.weight.max(n.weight)
            && s.length <= p.length.min(n.length)
        {
            if same_way {
                return Some((Shape::Passing, one(p.pc()).with(n.pc()), s.start, s.end));
            }
            if n.key == p.key {
                return Some((Shape::Neighbour, one(p.pc()), s.start, s.end));
            }
        }
        // A leap in and a step out the other way, on a felt beat stronger than where it resolves.
        if let Some(n) = next
            && !bass
            && short
            && into.is_some_and(|d| d.abs() >= 3)
            && step(out)
            && !same_way
            && s.weight >= Level::Beat.weight()
            && s.weight > n.weight
            && s.length <= n.length
        {
            return Some((Shape::Appoggiatura, one(n.pc()), s.start, s.end));
        }
        None
    }

    /// The units after the one a note is struck in, for a bar: a bar line comes within them, and nothing is stronger.
    fn bar_after(&self, s: Step) -> impl Iterator<Item = usize> + use<> {
        let from = self.layout.unit_at(s.start);
        (1..=uncount(self.layout.per_bar).max(1)).map(move |k| from.saturating_add(k))
    }

    /// The first unit start after a note is struck that is stronger than where it's struck (none for a note struck on
    /// a bar line, as nothing is stronger).
    fn next_stronger(&self, s: Step) -> Option<i64> {
        let layout = self.layout;
        self.bar_after(s).find(|&u| layout.level(u).weight() > s.weight).map(|u| layout.start(u))
    }

    /// Where a suspension starts: the strongest felt beat a note is held at least a unit into (the first, when two are
    /// as strong), when it's at least as strong as where the note is struck.
    fn held_into(&self, s: Step) -> Option<i64> {
        let layout = self.layout;
        // Held at least a unit into it: a little legato isn't held.
        let latest = s.end - layout.unit_ticks();
        let best = self
            .bar_after(s)
            .take_while(|&u| layout.start(u) <= latest)
            .reduce(|best, u| if layout.level(u) > layout.level(best) { u } else { best })?;
        let level = layout.level(best);
        (level >= Level::Beat && level.weight() >= s.weight).then(|| layout.start(best))
    }

    /// The pitch classes sounding (in any stream) in the unit before the one a note is struck in.
    fn sounding_before(&self, s: Step) -> PcSet {
        let Some(unit) = self.layout.unit_at(s.start).checked_sub(1) else { return PcSet::EMPTY };
        Stream::ALL.into_iter().fold(PcSet::EMPTY, |set, stream| set.union(self.sounding(unit, stream)))
    }

    /// The pitch classes struck (in any stream) in the unit that starts at a tick. A loop's end is its start.
    fn struck_at(&self, tick: i64) -> PcSet {
        let tick = match self.loop_round() {
            Some((_, round)) if tick >= self.end => tick - round,
            _ => tick,
        };
        if !(0..self.end).contains(&tick) {
            return PcSet::EMPTY;
        }
        let unit = self.layout.unit_at(tick);
        Stream::ALL.into_iter().fold(PcSet::EMPTY, |set, stream| set.union(self.struck(unit, stream)))
    }
}
