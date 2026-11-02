//! An explanation in plain words ([`Explanation::words`]; harmony/design/chord-reader-design.md, "Explanations"):
//!
//! - **The chord:** its name, then how its notes make it: the bass and what it is to the chord, and the notes above
//!   it by their degrees ("Dm9: D in the bass is its root, and the chords play F, A, C and E, its b3, 5, b7 and 9").
//! - **The notes:** how it's played (given, pushed, written in two where its bass moves, the chord before going on,
//!   over a pedal), then what the notes that aren't its own do, a line for each kind: notes ringing on from the chord
//!   before, notes coming early, suspensions, appoggiaturas, passing notes, neighbours, approaches, a walking bass, the
//!   tune moving over the chord, colour, doubtful tensions and clashes.
//! - **Why:** the evidence that tells it from the runner-up ([`Explanation::shown_terms`]), a line for each kind of
//!   evidence (the bass, the root, the chords' notes, notes missing, the tune, a moving bass, where a chord starts,
//!   how common the chord is, how the root moves), the kind that leads by most first. Each line tells the facts behind
//!   the evidence, with their counts ("75% of what the chords play"). With no runner-up, it tells the first reading's
//!   evidence, the biggest first.
//!
//! The words say nothing the reader didn't weigh. The evidence is about the chord the decode reads (its root and its
//! kind of chord), told by the name as written; but a note the name leaves out, as it isn't heard, is told by the
//! chord the decode reads ("C is read as Cmaj7, whose seventh, B, isn't heard"). Each note is spelled by its degree
//! above the chord's root, as the chart spells the chord (C7(#9)'s D#, Db7's Cb), with no double sharps or flats, and
//! with no octave (each DAW numbers octaves its own way). The text is plain ASCII, with "b" and "#" as in chord names.

use super::answer::ChordAnswer;
use super::candidates::Label;
use super::cores::Core;
use super::explain::{BassFrom, Explanation, NoteDoes, NoteWhy, Term, TermWhy};
use super::hear::Stream;
use super::name::{BassKind, Reading, SpanFlags};
use super::reader::{ChordReading, ChordSpan};
use super::score::Feature;
use crate::chord::Chord;
use crate::key::Key;
use crate::notes::Grid;
use crate::pitch::{PcSet, PitchClass};
use crate::spell::{Degree, Spelled};

/// An explanation in plain words ([`Explanation::words`]): each line a sentence or two, ending with a full stop.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Words {
    /// What the chord is and how its notes make it: "Dm9: D in the bass is its root, and the chords play F, A, C and
    /// E, its b3, 5, b7 and 9."
    pub chord: String,
    /// How it's played, and what the notes that aren't its own do, a line for each kind: "Pushed: struck half a beat
    /// early, on the and of 4.", "E in the bass passes between D and F."
    pub notes: Vec<String>,
    /// Why it reads as this chord and not the runner-up, a line for each kind of evidence, the one that leads by most
    /// first: "D in the bass is the root of Dm9, and would be the sixth of F6/D." With no runner-up, the first
    /// reading's evidence, the biggest first.
    pub why: Vec<String>,
}

impl Explanation {
    /// The explanation in plain words ([`Words`]). `reading` is the reading it explains a stretch of, and its chords
    /// are spelled as the reading's chart spells them in `key`: `chart` is the reading's chart in that key
    /// ([`ChordReading::names`]), worked out once for every explanation told.
    pub fn words(&self, reading: &ChordReading, chart: &[Option<Spelled>], key: Option<Key>) -> Words {
        let teller = Teller::new(self, reading, chart, key);
        Words { chord: teller.chord(), notes: teller.notes(), why: teller.why() }
    }
}

/// A chord beside the stretch: what the decode reads there, and its name as written.
#[derive(Clone, Debug)]
struct Beside {
    label: Label,
    name: String,
}

/// The other half of a chord written in two where its bass moves ([`SpanFlags::SPLIT`]).
#[derive(Clone, Copy, Debug)]
struct Split {
    /// Whether the stretch is the first half.
    first: bool,
    /// The bass the second half stands on: the chord's third or seventh.
    bass: PitchClass,
}

/// What the words are told from: the explanation, its stretch, and the chords as written.
struct Teller<'a> {
    e: &'a Explanation,
    span: Option<&'a ChordSpan>,
    grid: Grid,
    /// The first reading and the runner-up as written, spelled as the chart spells chords there.
    first: Spelled,
    second: Option<Spelled>,
    /// The chords the decode reads for each, written plainly (Cmaj7 for a C cut from it) and spelled the same way.
    plain: [Option<Spelled>; 2],
    /// Whether each is written as the chord the decode reads: on its root, as the same kind of chord (C for Cmaj7 cut
    /// in two, Dm9 for Dm7; not D/C for D7, nor C#dim7 for Edim7). The evidence tells each as written where it is,
    /// else as the chord the decode reads.
    same: [bool; 2],
    /// The same notes written other ways: the other way the namer writes them, and a chord that repeats round the
    /// octave on its other roots.
    others: Vec<String>,
    /// The chords either side, as the decode reads them: the one before (round a loop, the one before its first) and
    /// the one after.
    before: Option<Beside>,
    after: Option<Beside>,
    split: Option<Split>,
}

impl<'a> Teller<'a> {
    fn new(e: &'a Explanation, reading: &'a ChordReading, chart: &[Option<Spelled>], key: Option<Key>) -> Teller<'a> {
        let spell = |span: usize, chord: Chord| match chart.get(span).copied().flatten() {
            Some(shown) if shown.chord() == chord => shown,
            _ => reading.spell(chart, span, chord, key),
        };
        // A stretch's chord: what the decode reads there, written as the chart shows it, else as its first reading.
        let beside = |span: usize| -> Option<Beside> {
            let s = reading.spans.get(span)?;
            let label = s.label?;
            let name = match chart.get(span).copied().flatten() {
                Some(shown) => shown.name(),
                None => spell(span, s.readings.first()?.chord).name(),
            };
            Some(Beside { label, name })
        };
        let before = match e.span.checked_sub(1) {
            Some(b) => beside(b),
            // Round a loop: the chord the decode reads before its first, written as the loop's last when it's that.
            None => reading.before.map(|label| {
                let last = reading.spans.len().saturating_sub(1);
                beside(last)
                    .filter(|b| b.label == label)
                    .unwrap_or_else(|| Beside { label, name: spell(last, plain_chord(label)).name() })
            }),
        };
        let label = e.first.label;
        // Half of a chord written in two: the stretch next to it is the other half, the same chord.
        let half = |span: Option<usize>| {
            let s = span.and_then(|s| reading.spans.get(s));
            s.filter(|s| s.flags.contains(SpanFlags::SPLIT) && s.label == Some(label))
        };
        let split = match half(Some(e.span)) {
            None => None,
            Some(_) => match half(Some(e.span + 1)).and_then(|s| s.readings.first()) {
                Some(next) => Some(Split { first: true, bass: next.chord.bass() }),
                None => half(e.span.checked_sub(1)).map(|_| Split { first: false, bass: e.first.chord.bass() }),
            },
        };
        let mut others: Vec<String> = e.first.also.map(|also| spell(e.span, also).name()).into_iter().collect();
        // A chord that repeats round the octave (a diminished seventh, an augmented triad) on each of its other roots,
        // in root position.
        let root = e.first.chord.root();
        let mut roots: Vec<PitchClass> = e.first.roots.without(root).iter().collect();
        roots.sort_by_key(|r| r.above(root));
        others.extend(roots.into_iter().map(|r| spell(e.span, Chord::new(r, e.first.chord.notes(), r)).name()));
        let first = spell(e.span, e.first.chord);
        let second = e.second.map(|r| spell(e.span, r.chord));
        let plain = [Some(&e.first), e.second.as_ref()].map(|r| r.map(|r| spell(e.span, plain_chord(r.label))));
        let same = |written: Option<Spelled>, plain: Option<Spelled>| match (written, plain) {
            (Some(w), Some(p)) => w.chord().root() == p.chord().root() && w.chord().family() == p.chord().family(),
            _ => true,
        };
        let [p0, p1] = plain;
        Teller {
            e,
            span: reading.spans.get(e.span),
            grid: reading.grid,
            first,
            second,
            plain,
            same: [same(Some(first), p0), same(second, p1)],
            others,
            before,
            after: beside(e.span + 1),
            split,
        }
    }

    /// A reading: the first (0) or the runner-up (1).
    fn reading(&self, k: usize) -> Option<&Reading> {
        match k {
            0 => Some(&self.e.first),
            _ => self.e.second.as_ref(),
        }
    }

    /// A reading as written.
    fn spelled(&self, k: usize) -> Option<Spelled> {
        match k {
            0 => Some(self.first),
            _ => self.second,
        }
    }

    /// Whether a reading is written as the chord the decode reads.
    fn same(&self, k: usize) -> bool {
        self.same.get(k).copied().unwrap_or(true)
    }

    /// A reading as the evidence tells it: as written, where that's the chord the decode reads, else as that chord.
    fn told(&self, k: usize) -> Spelled {
        let plain = self.plain.get(k).copied().flatten();
        let told = if self.same(k) { self.spelled(k) } else { plain };
        told.unwrap_or(self.first)
    }

    /// A reading's name as the evidence tells it.
    fn name(&self, k: usize) -> String {
        self.told(k).name()
    }

    /// A reading's root as the evidence tells it.
    fn root(&self, k: usize) -> String {
        self.told(k).root().name()
    }

    /// A note, spelled by its degree above a reading's root as the evidence tells it.
    fn note_of(&self, k: usize, pc: PitchClass) -> String {
        let told = self.told(k);
        told.root().up(degree_in(told.chord(), pc)).without_doubles().name()
    }

    /// A note, spelled by its degree above the first reading's root: its own degree when it's one of the chord's
    /// notes or the bass written under it, else the degree it would have were it named.
    fn note(&self, pc: PitchClass) -> String {
        self.first.root().up(degree_in(self.e.first.chord, pc)).without_doubles().name()
    }

    /// The chord: its name and how its notes make it.
    fn chord(&self) -> String {
        let e = self.e;
        let name = self.first.name();
        let mut aside = Vec::new();
        let head = match self.span.map(|s| s.answer) {
            Some(ChordAnswer::SameNotes) => match (e.second, self.second) {
                (Some(r), Some(s)) if r.group == e.first.group => {
                    aside.push("the same notes".to_owned());
                    format!("{name} or {}", s.name())
                }
                _ => name,
            },
            Some(ChordAnswer::Implied) => {
                aside.push("implied: only one line plays".to_owned());
                name
            }
            Some(ChordAnswer::Unclear) => format!("Unclear, most likely {name}"),
            Some(ChordAnswer::NoChord(_)) => format!("Too few notes for a chord, most likely {name}"),
            _ => name,
        };
        if !self.others.is_empty() {
            aside.push(format!("also {}", list(self.others.clone(), "or")));
        }
        let head = if aside.is_empty() { head } else { format!("{head} ({})", aside.join("; ")) };
        let bass = self.bass();
        let root = self.first.root().name();
        match (self.above(), e.first.rootless) {
            (Some(above), true) => {
                format!("{head}: {bass}, and {above} (a rootless voicing: nothing plays its root, {root}).")
            }
            (Some(above), false) => format!("{head}: {bass}, and {above}."),
            (None, true) => format!("{head}: {bass}, and nothing plays its root, {root}."),
            (None, false) => format!("{head}: {bass}."),
        }
    }

    /// The bass and what it is to the chord.
    fn bass(&self) -> String {
        let e = self.e;
        let chord = e.first.chord;
        match e.first.bass {
            BassKind::None => "no bass plays".to_owned(),
            BassKind::Moving => {
                // The bass notes on the chord's notes, each pitch class once, in the order they come.
                let mut on: Vec<(PitchClass, Degree)> = Vec::new();
                for w in e.notes.iter().filter(|w| w.heard_as == Stream::Bass) {
                    let NoteDoes::Bass(degree) = w.does else { continue };
                    let pc = PitchClass::of_key(w.key);
                    if !on.iter().any(|&(p, _)| p == pc) {
                        on.push((pc, degree));
                    }
                }
                if on.is_empty() {
                    return "the bass moves under it (so no slash is written)".to_owned();
                }
                let notes = list(on.iter().map(|&(pc, _)| self.note(pc)), "and");
                let degrees = list(on.iter().map(|&(_, degree)| degree_name(degree)), "and");
                format!("the bass moves under it through {notes}, its {degrees} (so no slash is written)")
            }
            kind => {
                let what = match kind {
                    BassKind::Slash => format!("a {} under the chord", degree_name(chord.bass_degree())),
                    _ => format!("its {}", role_word(chord.bass_degree())),
                };
                let bass = self.note(chord.bass());
                match e.bass_from {
                    BassFrom::LowestKeys => format!("{bass}, the lowest note (no bass part plays), is {what}"),
                    _ => format!("{bass} in the bass is {what}"),
                }
            }
        }
    }

    /// The notes above the bass that are the chord's own or tensions it names, by their degrees; none when there are
    /// none.
    fn above(&self) -> Option<String> {
        let e = self.e;
        let chord = e.first.chord;
        // The notes the name has, and the chord's own notes it leaves out (heard too little), each once.
        let (mut named, mut left) = (Vec::<(Degree, PitchClass)>::new(), Vec::<(Degree, PitchClass)>::new());
        let (mut chords, mut tune) = (false, false);
        for w in e.notes.iter().filter(|w| w.heard_as != Stream::Bass) {
            let (NoteDoes::ChordTone(degree) | NoteDoes::Extension(degree)) = w.does else { continue };
            let pc = PitchClass::of_key(w.key);
            let into = if chord.notes().contains(pc) { &mut named } else { &mut left };
            if !into.iter().any(|&(_, p)| p == pc) {
                into.push((degree, pc));
            }
            chords |= w.heard_as == Stream::Harmony;
            tune |= w.heard_as == Stream::Melody;
        }
        if named.is_empty() && left.is_empty() {
            return None;
        }
        named.sort_by_key(|&(degree, _)| degree);
        left.sort_by_key(|&(degree, _)| degree);
        let who = match (chords, tune) {
            (true, true) => "the chords and the tune play",
            (false, true) => "the tune plays",
            _ => "the chords play",
        };
        let notes = |all: &[(Degree, PitchClass)]| list(all.iter().map(|&(_, pc)| self.note(pc)), "and");
        let degrees = |all: &[(Degree, PitchClass)]| list(all.iter().map(|&(degree, _)| degree_name(degree)), "and");
        let mut out = if named.is_empty() {
            format!("{who} only {}, its {}, heard too little to name", notes(&left), degrees(&left))
        } else {
            format!("{who} {}, its {}", notes(&named), degrees(&named))
        };
        if !named.is_empty() && !left.is_empty() {
            let is = verb(left.len(), "is", "are");
            out.push_str(&format!(" ({}, its {}, {is} heard too little to name)", notes(&left), degrees(&left)));
        }
        let root = chord.root();
        if e.first.bass == BassKind::Root && named.len() >= 4 && named.iter().all(|&(_, pc)| pc != root) {
            out.push_str(" (a rootless voicing over the root in the bass)");
        }
        Some(out)
    }

    /// How it's played, then what the notes that aren't its own do.
    fn notes(&self) -> Vec<String> {
        let e = self.e;
        let mut out = Vec::new();
        if let Some(s) = self.span {
            if s.flags.contains(SpanFlags::GIVEN) {
                out.push("You gave this chord (a fix or a Chord check answer), so it's written as given.".to_owned());
            }
            if s.flags.contains(SpanFlags::PUSHED) && s.heard_start < s.start {
                out.push(self.pushed(s));
            }
        }
        if let (false, Some(plain)) = (self.same(0), self.plain[0]) {
            out.push(format!(
                "The reader reads it as {}; it's written {} from the notes it plays.",
                plain.name(),
                self.first.name()
            ));
        }
        let held = e.held_on[0];
        if let (Some(split), Some(plain)) = (self.split, self.plain[0]) {
            let (with, starts) = match (split.first, held) {
                (true, _) => ("the next stretch", ""),
                (false, true) => ("the stretch before", ": no new chord starts"),
                (false, false) => ("the stretch before", ""),
            };
            let role = role_word(degree_in(plain.chord(), split.bass));
            out.push(format!(
                "With {with} it's one chord, {}, written in two where the bass moves to {}, its {role}{starts}.",
                plain.name(),
                self.note(split.bass)
            ));
        }
        if held && !self.split.is_some_and(|s| !s.first) {
            let before = self.before.as_ref().map_or("The chord before", |b| b.name.as_str());
            out.push(if e.span == 0 {
                format!("As the loop goes round, {before} goes on here: no new chord starts.")
            } else {
                format!("{before} goes on here: no new chord starts.")
            });
        }
        if self.span.is_some_and(|s| s.flags.contains(SpanFlags::PEDAL)) {
            out.push(format!("It stands over a pedal on {}.", self.note(e.first.chord.bass())));
        }
        for kind in KINDS {
            self.kind_lines(kind, &mut out);
        }
        out
    }

    /// A pushed chord: how early it's struck, and where.
    fn pushed(&self, s: &ChordSpan) -> String {
        let beat = self.grid.beat();
        let early = s.start - s.heard_start;
        let amount = match fraction(early, beat) {
            Some((1, 1)) => "a beat".to_owned(),
            Some((1, 2)) => "half a beat".to_owned(),
            Some((1, 3)) => "a third of a beat".to_owned(),
            Some((1, 4)) => "a quarter of a beat".to_owned(),
            Some((3, 4)) => "three quarters of a beat".to_owned(),
            Some((2, 3)) => "two thirds of a beat".to_owned(),
            Some((n, 1)) => format!("{n} beats"),
            _ if early < beat => "a little".to_owned(),
            _ => "more than a beat".to_owned(),
        };
        let at = (s.heard_start - self.grid.bar_line).rem_euclid(self.grid.bar());
        let (number, within) = (at / beat + 1, at % beat);
        let next =
            if number >= self.grid.bar_beats() { "the bar line".to_owned() } else { format!("beat {}", number + 1) };
        let place = match fraction(within, beat) {
            Some((0, _)) => format!("on beat {number}"),
            Some((1, 2)) => format!("on the and of {number}"),
            Some((1, 4)) => format!("on the e of {number}"),
            Some((3, 4)) => format!("on the a of {number}"),
            _ => format!("just before {next}"),
        };
        format!("Pushed: struck {amount} early, {place}.")
    }

    /// The lines for one kind of note that isn't the chord's own.
    fn kind_lines(&self, kind: Kind, out: &mut Vec<String>) {
        let e = self.e;
        let of_kind: Vec<&NoteWhy> = e.notes.iter().filter(|w| kind_of(w.does) == Some(kind)).collect();
        if of_kind.is_empty() {
            return;
        }
        // Each pitch class once, in order, with the notes' degrees where they have one.
        let pcs = |stream: Option<Stream>| -> Vec<(PitchClass, Option<Degree>)> {
            let mut all: Vec<(PitchClass, Option<Degree>)> = Vec::new();
            for w in of_kind.iter().filter(|w| stream.is_none_or(|s| w.heard_as == s)) {
                let pc = PitchClass::of_key(w.key);
                if !all.iter().any(|&(p, _)| p == pc) {
                    all.push((pc, degree_of(w.does)));
                }
            }
            all
        };
        let names = |all: &[(PitchClass, Option<Degree>)]| list(all.iter().map(|&(pc, _)| self.note(pc)), "and");
        let degrees =
            |all: &[(PitchClass, Option<Degree>)]| list(all.iter().filter_map(|&(_, d)| d).map(degree_name), "and");
        let streams = [Stream::Bass, Stream::Harmony, Stream::Melody];
        match kind {
            Kind::HeldOver => {
                let all = pcs(None);
                out.push(format!("{} {} on from the chord before.", names(&all), verb(all.len(), "rings", "ring")));
            }
            Kind::Anticipation | Kind::Walking | Kind::Clash => {
                for stream in streams {
                    let all = pcs(Some(stream));
                    if all.is_empty() {
                        continue;
                    }
                    let (one, many, rest) = match kind {
                        Kind::Anticipation => ("comes", "come", " early to the next chord".to_owned()),
                        Kind::Walking => ("walks", "walk", " between the chord's notes".to_owned()),
                        _ => ("clashes", "clash", format!(" with {}", self.first.name())),
                    };
                    let does = verb(all.len(), one, many);
                    out.push(format!("{} in the {} {does}{rest}.", names(&all), stream_word(stream)));
                }
            }
            Kind::TuneOver | Kind::Colour | Kind::Doubtful => {
                let all = pcs(None);
                let (n, d, one) = (names(&all), degrees(&all), all.len() == 1);
                out.push(match kind {
                    Kind::TuneOver => {
                        let moves = if one { "moves" } else { "move" };
                        format!("{n} in the tune, its {d}, {moves} over the chord, too short or too soft to count.")
                    }
                    Kind::Colour => {
                        let is = if one { "is" } else { "are" };
                        format!("{n} in the chords, its {d}, {is} passing colour, heard too little to name.")
                    }
                    _ => {
                        let (is, it) = if one { ("is", "it's") } else { ("are", "they're") };
                        format!(
                            "{n}, its {d}, {is} heard too little to name: {it} listed beside the chord as doubtful."
                        )
                    }
                });
            }
            Kind::Suspension | Kind::Appoggiatura | Kind::Passing | Kind::Neighbour | Kind::Approach => {
                // Each note with what it hangs on, once, with how many times it's played.
                let mut seen: Vec<(Stream, PitchClass, PcSet, usize)> = Vec::new();
                for w in &of_kind {
                    let pc = PitchClass::of_key(w.key);
                    match seen.iter_mut().find(|(s, p, a, _)| (*s, *p, *a) == (w.heard_as, pc, w.anchors)) {
                        Some(found) => found.3 += 1,
                        None => seen.push((w.heard_as, pc, w.anchors, 1)),
                    }
                }
                for (stream, pc, anchors, times) in seen {
                    let on: Vec<String> = anchors.iter().map(|a| self.note(a)).collect();
                    let does = match kind {
                        _ if on.is_empty() => "passes by".to_owned(),
                        Kind::Suspension => format!("is held over from before and steps down to {}", list(on, "or")),
                        Kind::Appoggiatura => format!("leaps in on the beat and steps to {}", list(on, "or")),
                        Kind::Passing if on.len() == 2 => format!("passes between {}", list(on, "and")),
                        Kind::Passing => "passes between the chord's notes".to_owned(),
                        Kind::Neighbour => format!("steps away from {} and back", list(on, "or")),
                        _ => format!("leads by a step into {}", list(on, "or")),
                    };
                    let times = match times {
                        1 => String::new(),
                        2 => " (twice)".to_owned(),
                        n => format!(" ({n} times)"),
                    };
                    out.push(format!("{} in the {} {does}{times}.", self.note(pc), stream_word(stream)));
                }
            }
        }
    }

    /// Why it reads as this chord: a line for each kind of evidence shown, the one that leads by most first.
    fn why(&self) -> Vec<String> {
        let shown = self.e.shown_terms();
        let mut topics: Vec<Topic> = Vec::new();
        for t in shown {
            let topic = topic_of(t.term);
            if !topics.contains(&topic) {
                topics.push(topic);
            }
        }
        // The runner-up written as another chord than the one the decode reads: the evidence tells it as that one.
        let runner_up = match (self.same(1), self.second, self.plain[1]) {
            (false, Some(second), Some(plain)) if !shown.is_empty() => {
                Some(format!("The runner-up, {}, is read as {}.", second.name(), plain.name()))
            }
            _ => None,
        };
        let lines = topics.into_iter().filter_map(|topic| {
            let terms: Vec<&TermWhy> = shown.iter().filter(|t| topic_of(t.term) == topic).collect();
            self.topic(topic, &terms).map(sentence)
        });
        runner_up.into_iter().chain(lines).collect()
    }

    /// The facts behind one kind of evidence, in a sentence (with no full stop yet); none when there are none to tell.
    fn topic(&self, topic: Topic, terms: &[&TermWhy]) -> Option<String> {
        match topic {
            Topic::Bass => self.bass_why(),
            Topic::Root => self.root_why(terms),
            Topic::Chords | Topic::Tune | Topic::Walking => self.shares_why(topic, terms),
            Topic::Missing => self.missing_why(terms),
            Topic::Start => self.start_why(terms),
            Topic::Common => self.common_why(terms),
            Topic::Move => self.move_why(terms.first()?),
        }
    }

    /// What the held bass is to each reading (B1 to B6), told for both, as the bass evidence weighs each.
    fn bass_why(&self) -> Option<String> {
        let mut facts: Vec<(PitchClass, String)> = Vec::new();
        for k in 0..2 {
            let (Some(_), Some(bass)) = (self.reading(k), self.e.held_bass.get(k).copied().flatten()) else { continue };
            // Its bass evidence: any of it, not only the pieces shown.
            let Some(f) = self
                .e
                .terms
                .iter()
                .find_map(|t| feature(t).filter(|_| topic_of(t.term) == Topic::Bass && value(t, k) != 0.0))
            else {
                continue;
            };
            let (verb, name, chord) = (if k == 0 { "is" } else { "would be" }, self.name(k), self.told(k).chord());
            facts.push((
                bass,
                match f {
                    Feature::BassTension => format!("{verb} a tension of {name} (its {})", named_degree(chord, bass)),
                    Feature::BassWrong => format!("{verb} a note outside {name}"),
                    _ => format!("{verb} the {} of {name}", role_word(degree_in(chord, bass))),
                },
            ));
        }
        let (bass, _) = *facts.first()?;
        if facts.iter().all(|&(b, _)| b == bass) {
            let told = list(facts.into_iter().map(|(_, fact)| fact), "and");
            Some(format!("{} in the bass {told}", self.note(bass)))
        } else {
            let told: Vec<String> =
                facts.into_iter().map(|(b, fact)| format!("{} in the bass {fact}", self.note(b))).collect();
            Some(told.join("; "))
        }
    }

    /// Where each reading's root is heard, and which root the notes lean to (R1 to R4, L1, T15).
    fn root_why(&self, terms: &[&TermWhy]) -> Option<String> {
        let mut facts = Vec::new();
        for t in terms {
            let Some(f) = feature(t) else { continue };
            if f == Feature::RootSupport {
                facts.push(self.leaning(t));
                continue;
            }
            for k in about(t) {
                let root = format!("{}'s root, {}", self.name(k), self.root(k));
                facts.push(match f {
                    Feature::RootPlayed => format!("{root}, is played in the chords or the tune"),
                    Feature::RootInBass => format!("{root}, is only in the bass"),
                    Feature::RootlessOverBass => format!("{root}, isn't played at all"),
                    Feature::RootlessNoBass => format!("{root}, isn't played at all, and no bass plays"),
                    _ => format!("{root}, is the lowest note {} of the time", share(value(t, k), "all")),
                });
            }
        }
        (!facts.is_empty()).then(|| facts.join("; "))
    }

    /// Which root the notes lean towards ([`Feature::RootSupport`]).
    fn leaning(&self, t: &TermWhy) -> String {
        let [a, b] = t.values;
        let (r0, r1) = (self.root(0), self.second.map(|s| s.root().name()));
        match r1 {
            Some(r1) if r1 != r0 && b > a => format!("the notes lean more towards {r1} as the root than towards {r0}"),
            Some(r1) if r1 != r0 => format!("the notes lean more towards {r0} as the root than towards {r1}"),
            _ => format!("the notes lean towards {r0} as the root"),
        }
    }

    /// How much of what the chords play, what the tune plays or what a moving bass plays is each reading's own notes,
    /// its tensions and other notes (T1 to T8, B7 to B9).
    fn shares_why(&self, topic: Topic, terms: &[&TermWhy]) -> Option<String> {
        let of = match topic {
            Topic::Chords => "Of what the chords play",
            Topic::Tune => "Of what the tune plays",
            _ => "Of what the moving bass plays (counting the strongest beats most)",
        };
        let mut parts: Vec<String> = Vec::new();
        for k in 0..2 {
            let items: Vec<(f64, Feature)> =
                terms.iter().filter_map(|t| Some((value(t, k), feature(t)?))).filter(|&(v, _)| v != 0.0).collect();
            if items.is_empty() {
                continue;
            }
            let name = self.name(k);
            // The reading is named in its first item, when it isn't named before them.
            let named_first = parts.is_empty();
            let told: Vec<String> = items
                .iter()
                .enumerate()
                .map(|(i, &(v, f))| {
                    let who = if i == 0 && named_first { name.as_str() } else { "it" };
                    let whose = if who == "it" { "its".to_owned() } else { format!("{who}'s") };
                    let what = match f {
                        Feature::ChordTones | Feature::TuneTones => format!("{whose} own notes"),
                        Feature::Tensions | Feature::TuneTensions => format!("tensions {who} can carry"),
                        Feature::WrongStruck => format!("notes struck that don't belong to {who}"),
                        Feature::WrongHeld => format!("notes carried in or held over that don't fit {who}"),
                        Feature::Passing => format!("notes that only pass by under {who}"),
                        Feature::TuneWrong => format!("notes that don't belong to {who}"),
                        Feature::WalkingRoot => format!("on {whose} root"),
                        Feature::WalkingTones => format!("on {whose} other notes"),
                        _ => format!("on notes not {whose} own, on a strong beat or held long"),
                    };
                    let is = if i == 0 { " is" } else { "" };
                    format!("{}{is} {what}", share(v, "all of it"))
                })
                .collect();
            let told = list(told, "and");
            parts.push(if named_first { told } else { format!("for {name}, {told}") });
        }
        (!parts.is_empty()).then(|| format!("{of}, {}", parts.join("; ")))
    }

    /// The notes each reading would need that aren't heard (T9 to T12), told by the reading as the evidence tells it;
    /// but where that's the name as written and it leaves the note out, by the chord the decode reads.
    fn missing_why(&self, terms: &[&TermWhy]) -> Option<String> {
        let mut facts = Vec::new();
        for t in terms {
            let Some(f) = feature(t) else { continue };
            for k in about(t) {
                let Some(r) = self.reading(k) else { continue };
                let (root, core) = (r.label.root, r.label.core);
                let (what, intervals) = match f {
                    Feature::NoThird => (third_word(core), core.third()),
                    Feature::NoSeventh if matches!(core, Core::Major6 | Core::Minor6) => ("sixth", core.seventh()),
                    Feature::NoSeventh => ("seventh", core.seventh()),
                    Feature::NoCoreFifth => (core_fifth_word(core), core.fifth()),
                    _ => ("fifth", const { PcSet::of(&[7]) }),
                };
                let Some(pc) = intervals.on(root).iter().next() else { continue };
                let far = value(t, k);
                let how = if far >= 1.0 {
                    "isn't heard"
                } else if far > 0.5 {
                    "is barely heard"
                } else {
                    "is only partly heard"
                };
                // A name leaves out a third, seventh or sixth it doesn't sound (C for Cmaj7, C5 for C), never a fifth.
                let (told, plain) = (self.told(k), self.plain.get(k).copied().flatten());
                let left_out = f != Feature::NoPlainFifth && !told.chord().notes().contains(pc);
                facts.push(match plain.filter(|_| left_out) {
                    Some(plain) => {
                        let read = if k == 0 { "is read as" } else { "would be read as" };
                        let note = plain.root().up(degree_in(plain.chord(), pc)).without_doubles().name();
                        format!("{} {read} {}, whose {what}, {note}, {how}", told.name(), plain.name())
                    }
                    None => format!("{}'s {what}, {}, {how}", told.name(), self.note_of(k, pc)),
                });
            }
        }
        (!facts.is_empty()).then(|| facts.join("; "))
    }

    /// Where each reading would start a chord (E1 to E8), or that it's the chord before going on.
    fn start_why(&self, terms: &[&TermWhy]) -> Option<String> {
        let mut facts = Vec::new();
        for k in 0..2 {
            if self.reading(k).is_none() {
                continue;
            }
            let name = self.name(k);
            if self.e.held_on.get(k).copied().unwrap_or(false) {
                let is = if k == 0 { "is" } else { "would be" };
                let before = match &self.before {
                    Some(b) if b.name != name => format!("the chord before, {},", b.name),
                    _ => "the chord before".to_owned(),
                };
                let counts = if k == 0 { "doesn't" } else { "wouldn't" };
                facts.push(format!("{name} {is} {before} going on, so where a chord starts {counts} count for it"));
                continue;
            }
            let (starts, at) =
                if k == 0 { ("starts", "as it starts") } else { ("would start", "where it would start") };
            for t in terms.iter().filter(|t| about(t).any(|j| j == k)) {
                let Some(f) = feature(t) else { continue };
                facts.push(match f {
                    Feature::BassStrikesRoot => format!("the bass moves to {name}'s root, {}, {at}", self.root(k)),
                    Feature::StruckAtStart => {
                        let place = if k == 0 { "as" } else { "where" };
                        format!("{place} {name} {starts}, {} of its notes are struck", share(value(t, k), "all"))
                    }
                    Feature::NothingStruck => format!("nothing new is struck where {name} {starts}"),
                    Feature::OnBar => format!("{name} {starts} on a bar line"),
                    Feature::MidBar => format!("{name} {starts} in the middle of a bar"),
                    Feature::OnBeat => format!("{name} {starts} on a beat"),
                    Feature::OffBeat => format!("{name} {starts} between beats"),
                    _ => format!("{name} {starts} pushed, struck before the beat"),
                });
            }
        }
        (!facts.is_empty()).then(|| facts.join("; "))
    }

    /// How common each reading's kind of chord is, and how usual its tensions are for that kind (P1, P2).
    fn common_why(&self, terms: &[&TermWhy]) -> Option<String> {
        let mut facts = Vec::new();
        for t in terms {
            match feature(t) {
                Some(Feature::CorePrior) => facts.extend(self.commonness(t)),
                Some(_) => facts.extend(self.tensions_usual(t)),
                None => {}
            }
        }
        (!facts.is_empty()).then(|| facts.join("; "))
    }

    /// How common each reading's kind of chord is in the songs counted ([`Feature::CorePrior`]: its value is the
    /// logarithm of the kind's share).
    fn commonness(&self, t: &TermWhy) -> Option<String> {
        let kinds: Vec<(Core, f64)> = (0..2).filter_map(|k| Some((self.reading(k)?.label.core, value(t, k)))).collect();
        let counted = "of the chords in the songs counted";
        match kinds[..] {
            [(a, x), (b, y)] if a != b => Some(format!(
                "{counted}, {} are {} chords and {} {} chords",
                percent(x.exp()),
                kind_word(a),
                percent(y.exp()),
                kind_word(b)
            )),
            [(a, x), ..] => Some(format!("{counted}, {} are {} chords", percent(x.exp()), kind_word(a))),
            [] => None,
        }
    }

    /// How usual each reading's tensions, as heard, are for its kind of chord ([`Feature::TensionPrior`]: its value is
    /// the logarithm of the chance that a chord of that kind names just those tensions, each counted on its own).
    fn tensions_usual(&self, t: &TermWhy) -> Option<String> {
        let kind = |k: usize| self.reading(k).map(|r| kind_word(r.label.core));
        let [a, b] = t.values;
        let counted = "as the songs counted name tensions";
        match (kind(0), kind(1)) {
            (Some(k0), Some(k1)) => {
                let (more, less, km, kl) = if a >= b { (0, 1, k0, k1) } else { (1, 0, k1, k0) };
                let is = if more == 0 { "is" } else { "would be" };
                let way = if km == kl { String::new() } else { format!(" is to write a {kl} chord") };
                Some(format!(
                    "{counted}, {} {is} a more usual way to write a {km} chord than {}{way}",
                    self.name(more),
                    self.name(less)
                ))
            }
            (Some(k0), None) => {
                Some(format!("{counted}, about {} of {k0} chords are written as {} is", percent(a.exp()), self.name(0)))
            }
            _ => None,
        }
    }

    /// How the root moves from the chord before and on to the chord after (the pair term).
    fn move_why(&self, t: &TermWhy) -> Option<String> {
        // Each reading's moves, as chord names: none into or out of the same chord.
        let path = |k: usize| -> Option<String> {
            let label = self.reading(k)?.label;
            let mut names = Vec::new();
            if let Some(b) = self.before.as_ref().filter(|b| b.label != label) {
                names.push(b.name.clone());
            }
            names.push(self.name(k));
            if let Some(a) = self.after.as_ref().filter(|a| a.label != label) {
                names.push(a.name.clone());
            }
            (names.len() > 1).then(|| names.join(" to "))
        };
        let [a, b] = t.values;
        let common = |v: f64| if v > 0.0 { "a common" } else { "an uncommon" };
        match (path(0), path(1)) {
            (Some(p0), Some(p1)) if a >= b => Some(format!("{p0} is a more common way for the root to move than {p1}")),
            (Some(p0), Some(p1)) => Some(format!("{p1} would be a more common way for the root to move than {p0}")),
            (Some(p0), None) => Some(format!("{p0} is {} way for the root to move", common(a))),
            (None, Some(p1)) => Some(format!("{p1} would be {} way for the root to move", common(b))),
            (None, None) => None,
        }
    }
}

/// The chord the decode reads, written plainly: the label's notes in root position.
const fn plain_chord(label: Label) -> Chord {
    Chord::of(label.root, label.core.intervals())
}

/// The piece of evidence a term is, when it's one the scoring weighs.
const fn feature(t: &TermWhy) -> Option<Feature> {
    match t.term {
        Term::Feature(f) => Some(f),
        Term::Pair => None,
    }
}

/// A term's value for a reading (0 the first, 1 the runner-up).
fn value(t: &TermWhy, k: usize) -> f64 {
    t.values.get(k).copied().unwrap_or(0.0)
}

/// The readings a piece of evidence is about: those with a value.
fn about(t: &TermWhy) -> impl Iterator<Item = usize> {
    let values = t.values;
    (0..2).filter(move |&k| values.get(k).is_some_and(|&v| v != 0.0))
}

/// The kinds of note that aren't the chord's own, in the order their lines come.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    HeldOver,
    Anticipation,
    Suspension,
    Appoggiatura,
    Passing,
    Neighbour,
    Approach,
    Walking,
    TuneOver,
    Colour,
    Doubtful,
    Clash,
}

const KINDS: [Kind; 12] = [
    Kind::HeldOver,
    Kind::Anticipation,
    Kind::Suspension,
    Kind::Appoggiatura,
    Kind::Passing,
    Kind::Neighbour,
    Kind::Approach,
    Kind::Walking,
    Kind::TuneOver,
    Kind::Colour,
    Kind::Doubtful,
    Kind::Clash,
];

/// The kind of a note that isn't the chord's own: none for the bass, the chord's own notes and the tensions it names.
pub(super) const fn kind_of(does: NoteDoes) -> Option<Kind> {
    match does {
        NoteDoes::Bass(_) | NoteDoes::ChordTone(_) | NoteDoes::Extension(_) => None,
        NoteDoes::HeldOver => Some(Kind::HeldOver),
        NoteDoes::Anticipation => Some(Kind::Anticipation),
        NoteDoes::Suspension => Some(Kind::Suspension),
        NoteDoes::Appoggiatura => Some(Kind::Appoggiatura),
        NoteDoes::Passing => Some(Kind::Passing),
        NoteDoes::Neighbour => Some(Kind::Neighbour),
        NoteDoes::Approach => Some(Kind::Approach),
        NoteDoes::Walking => Some(Kind::Walking),
        NoteDoes::TuneOver(_) => Some(Kind::TuneOver),
        NoteDoes::Colour(_) => Some(Kind::Colour),
        NoteDoes::Doubtful(_) => Some(Kind::Doubtful),
        NoteDoes::Clash => Some(Kind::Clash),
    }
}

/// The degree a note's line names, when it names one.
const fn degree_of(does: NoteDoes) -> Option<Degree> {
    match does {
        NoteDoes::Bass(d)
        | NoteDoes::ChordTone(d)
        | NoteDoes::Extension(d)
        | NoteDoes::Doubtful(d)
        | NoteDoes::Colour(d)
        | NoteDoes::TuneOver(d) => Some(d),
        _ => None,
    }
}

/// The kinds of evidence, a line each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Topic {
    /// What the held bass is to each reading (B1 to B6).
    Bass,
    /// Where each reading's root is heard, and which root the notes lean to (R1 to R4, L1, T15).
    Root,
    /// How much of the chords' sound is each reading's own notes, its tensions, notes that don't belong and notes
    /// passing by (T1 to T5).
    Chords,
    /// The notes each reading would need that aren't heard (T9 to T12).
    Missing,
    /// The same for the tune (T6 to T8).
    Tune,
    /// What a moving bass plays of each reading (B7 to B9).
    Walking,
    /// Where each reading would start a chord (E1 to E8), and whether it's the chord before held on.
    Start,
    /// How common each reading's kind of chord and its tensions are (P1, P2).
    Common,
    /// How the root moves from the chord before and on to the chord after (the pair term).
    Move,
}

/// The kind of evidence a term is.
pub(super) const fn topic_of(term: Term) -> Topic {
    match term {
        Term::Pair => Topic::Move,
        Term::Feature(f) => match f {
            Feature::ChordTones | Feature::Tensions | Feature::WrongStruck | Feature::WrongHeld | Feature::Passing => {
                Topic::Chords
            }
            Feature::TuneTones | Feature::TuneTensions | Feature::TuneWrong => Topic::Tune,
            Feature::NoThird | Feature::NoSeventh | Feature::NoCoreFifth | Feature::NoPlainFifth => Topic::Missing,
            Feature::RootSupport
            | Feature::RootPlayed
            | Feature::RootInBass
            | Feature::RootlessOverBass
            | Feature::RootlessNoBass
            | Feature::LowestIsRoot => Topic::Root,
            Feature::BassRoot
            | Feature::BassThird
            | Feature::BassFifth
            | Feature::BassSeventh
            | Feature::BassTension
            | Feature::BassWrong => Topic::Bass,
            Feature::WalkingRoot | Feature::WalkingTones | Feature::WalkingWrong => Topic::Walking,
            Feature::BassStrikesRoot
            | Feature::StruckAtStart
            | Feature::NothingStruck
            | Feature::OnBar
            | Feature::MidBar
            | Feature::OnBeat
            | Feature::OffBeat
            | Feature::Pushed => Topic::Start,
            Feature::CorePrior | Feature::TensionPrior => Topic::Common,
        },
    }
}

/// A note's degree in a chord: its own degree when it's one of the chord's notes, the bass's when it's the bass
/// written under it, else the degree it would have were it named.
fn degree_in(chord: Chord, pc: PitchClass) -> Degree {
    let interval = i32::from(pc.above(chord.root()));
    if chord.notes().contains(pc) {
        chord.degree(interval)
    } else if pc == chord.bass() {
        chord.bass_degree()
    } else {
        Chord::new(chord.root(), chord.notes().with(pc), chord.bass()).degree(interval)
    }
}

/// A note's degree in a chord were it named, the bass's too: the b9 of F7/Gb, where its bass is a b2.
fn named_degree(chord: Chord, pc: PitchClass) -> Degree {
    Chord::new(chord.root(), chord.notes().with(pc), chord.root()).degree(i32::from(pc.above(chord.root())))
}

/// A degree as a list of a chord's notes names it: "root" for 1, else as written ("b3", "5", "#9").
fn degree_name(degree: Degree) -> String {
    if degree == Degree::ROOT { "root".to_owned() } else { degree.to_string() }
}

/// What a degree is to a chord, in words: its root, third, fifth, sixth or seventh (any of them altered but the
/// fifth), a sus chord's 2nd or 4th, else the degree as written ("b5", "9", "#11").
fn role_word(degree: Degree) -> String {
    match (degree.step(), degree.alter()) {
        (1, 0) => "root",
        (3, _) => "third",
        (5, 0) => "fifth",
        (6, 0) => "sixth",
        (7, _) => "seventh",
        (2, 0) => "2nd",
        (4, 0) => "4th",
        _ => return degree.to_string(),
    }
    .to_owned()
}

/// What stands in for a core's third: a sus chord's 4th or 2nd.
const fn third_word(core: Core) -> &'static str {
    match core {
        Core::Sus4 | Core::Dominant7Sus4 => "4th",
        Core::Sus2 => "2nd",
        _ => "third",
    }
}

/// The fifth that says which chord a core is ([`Core::fifth`]).
const fn core_fifth_word(core: Core) -> &'static str {
    match core {
        Core::HalfDiminished | Core::Diminished | Core::Diminished7 => "flat fifth",
        Core::Augmented => "sharp fifth",
        _ => "fifth",
    }
}

/// A core's kind of chord, in words.
const fn kind_word(core: Core) -> &'static str {
    match core {
        Core::Major => "major",
        Core::Minor => "minor",
        Core::Dominant7 => "dominant seventh",
        Core::Major7 => "major seventh",
        Core::Minor7 => "minor seventh",
        Core::Major6 => "major sixth",
        Core::Minor6 => "minor sixth",
        Core::MinorMajor7 => "minor-major seventh",
        Core::HalfDiminished => "half-diminished",
        Core::Diminished7 => "diminished seventh",
        Core::Diminished => "diminished",
        Core::Augmented => "augmented",
        Core::Sus4 => "sus4",
        Core::Sus2 => "sus2",
        Core::Dominant7Sus4 => "7sus4",
        Core::Power => "power",
    }
}

/// What a stream is called in a line: "bass", "chords", "tune".
const fn stream_word(stream: Stream) -> &'static str {
    match stream {
        Stream::Bass => "bass",
        Stream::Harmony => "chords",
        Stream::Melody => "tune",
    }
}

/// A share (0 to 1) as a whole percentage: `all` for all of it, at most 99% for less, and "under 1%" for a little.
fn share(value: f64, all: &str) -> String {
    if value >= 1.0 {
        return all.to_owned();
    }
    let p = (value * 100.0).round().min(99.0);
    if p < 1.0 { "under 1%".to_owned() } else { format!("{p:.0}%") }
}

/// A chance as a percentage, to a tenth of a percent under 10%, and "under 0.1%" for less.
fn percent(value: f64) -> String {
    let p = value * 100.0;
    if p >= 9.95 {
        format!("{p:.0}%")
    } else if p >= 0.05 {
        format!("{p:.1}%")
    } else {
        "under 0.1%".to_owned()
    }
}

/// The verb for one or for more.
const fn verb(count: usize, one: &'static str, many: &'static str) -> &'static str {
    if count == 1 { one } else { many }
}

/// Items in a list: "A", "A and B", "A, B and C" (or "or").
fn list(items: impl IntoIterator<Item = String>, and: &str) -> String {
    let items: Vec<String> = items.into_iter().collect();
    match items.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} {and} {last}", rest.join(", ")),
    }
}

/// Text as a sentence: its first letter a capital, a full stop at the end.
fn sentence(text: String) -> String {
    let mut chars = text.chars();
    let mut out: String = chars.next().map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect());
    out.push('.');
    out
}

/// How much of a whole a part is, in lowest terms, when it's a whole number of halves, thirds or quarters (or whole
/// ones): (1, 2) for half. None otherwise, or for a whole of 0.
fn fraction(part: i64, whole: i64) -> Option<(i64, i64)> {
    if whole <= 0 || part < 0 {
        return None;
    }
    if part == 0 {
        return Some((0, 1));
    }
    [1, 2, 3, 4].into_iter().find_map(|d| {
        let n = part.checked_mul(d)?;
        (n % whole == 0).then(|| {
            let n = n / whole;
            let g = gcd(n, d);
            (n / g, d / g)
        })
    })
}

fn gcd(a: i64, b: i64) -> i64 {
    if b == 0 { a.max(1) } else { gcd(b, a % b) }
}
