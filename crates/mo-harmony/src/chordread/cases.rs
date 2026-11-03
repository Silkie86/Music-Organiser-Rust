//! The chord reader's hand cases (harmony/design/chord-reader-design.md, "Hand cases"): short pieces written out by
//! hand, each with what the reader must read (its gate: weights that break one aren't shipped) or should read once it's
//! good enough (its target). The tests read them with the weights built in ([`SHIPPED`](super::SHIPPED)); `theorybench
//! chordfit` reads them with the weights it fits, says which gates break, and learns from them too ([`fit_cases`]).
//!
//! They're judged three ways, each a [`Step`]:
//!
//! - **one stretch at a time** ([`hand_cases`]): a stretch's candidates ranked on their own, with no neighbours;
//! - **read whole** ([`whole_cases`], then every stretch case read whole): where the chords change, pushes, long chords;
//! - **named** ([`named_cases`], and the targets [`named_targets`]): read whole and named as a player writes them.
//!
//! Every case is a loop in 4/4 at 480 ticks a beat (unless it says otherwise), every note at half velocity. The cases
//! the design marks G\* (6, 15, 32, 34 and 35) are held out: the tests check them, the fit never learns from them, and
//! `chordfit` doesn't look at them ([`Verdict::held_out`]).

use std::cell::RefCell;
use std::ops::Range;

use super::fitter::FitLabel;
use super::{
    BassKind, Core, DecodedSpan, Decoder, Decoding, Evidence, Hearing, Label, Model, NamedSpan, Namer, Naming,
    PinnedSpan, Priors, Push, Route, Scored, Scorer, SpanFacts, SpanFlags, Tables, Weights,
};
use crate::chord::Chord;
use crate::keyfind::Span;
use crate::notes::{Grid, Note, Part, Role};
use crate::pitch::PitchClass;

/// Ticks in a beat, and in a bar of 4/4.
pub(crate) const BEAT: i64 = 480;
pub(crate) const BAR: i64 = 4 * BEAT;
/// Half of MIDI 2.0's velocity range.
pub(crate) const VELOCITY: u16 = 1 << 15;
/// A bar of 4/4, counted in units (half beats).
const B: usize = 8;

/// Some music: parts with their roles, on a grid, heard as a loop, a song or a stretch.
#[derive(Clone, Debug, PartialEq)]
pub struct Music {
    pub parts: Vec<(Role, Vec<Note>)>,
    pub grid: Grid,
    pub span: Span,
}

impl Music {
    /// 4/4 at 480 ticks a beat, heard as a song.
    pub(crate) fn new() -> Music {
        Music { parts: Vec::new(), grid: Grid::four_four(480), span: Span::Song }
    }

    pub(crate) fn grid(mut self, beats_per_bar: u32, ticks_per_beat: u32) -> Music {
        self.grid = Grid { ticks_per_beat, beats_per_bar, bar_line: 0 };
        self
    }

    pub(crate) fn looped(mut self) -> Music {
        self.span = Span::Loop;
        self
    }

    pub(crate) fn part(mut self, role: Role, notes: Vec<Note>) -> Music {
        self.parts.push((role, notes));
        self
    }

    pub(crate) fn bass(self, notes: Vec<Note>) -> Music {
        self.part(Role::Bass, notes)
    }

    pub(crate) fn keys(self, notes: Vec<Note>) -> Music {
        self.part(Role::Harmony, notes)
    }

    pub(crate) fn tune(self, notes: Vec<Note>) -> Music {
        self.part(Role::Melody, notes)
    }

    /// The parts, as the readers take them.
    pub fn parts(&self) -> Vec<Part<'_>> {
        self.parts.iter().map(|(role, notes)| Part { role: *role, notes }).collect()
    }

    /// The music heard; none when nothing is.
    pub fn heard(&self) -> Option<Hearing> {
        let mut hearing = Hearing::new();
        hearing.hear(&self.parts(), self.grid, self.span).then_some(hearing)
    }
}

/// The MIDI key of a note's name, C4 being 60: "C4", "Eb3", "F#2".
pub(crate) fn key(name: &str) -> Result<u8, String> {
    let mut chars = name.chars();
    let mut semitones: i32 = match chars.next() {
        Some('C') => 0,
        Some('D') => 2,
        Some('E') => 4,
        Some('F') => 5,
        Some('G') => 7,
        Some('A') => 9,
        Some('B') => 11,
        _ => return Err(format!("not a note: {name}")),
    };
    let mut rest = chars.as_str();
    while let Some(r) = rest.strip_prefix('#') {
        semitones += 1;
        rest = r;
    }
    while let Some(r) = rest.strip_prefix('b') {
        semitones -= 1;
        rest = r;
    }
    let octave: i32 = rest.parse().map_err(|_| format!("not a note: {name}"))?;
    u8::try_from((octave + 1) * 12 + semitones).map_err(|_| format!("not a MIDI key: {name}"))
}

/// Writes the cases' notes from text, keeping any slip in the text to say at the end ([`Writer::done`]), so the case
/// lists read as the notes are written.
#[derive(Debug, Default)]
pub(crate) struct Writer {
    slips: RefCell<Vec<String>>,
}

impl Writer {
    pub(crate) fn new() -> Writer {
        Writer::default()
    }

    fn slip(&self, why: String) {
        self.slips.borrow_mut().push(why);
    }

    /// A note's key, or none (and a slip) when its name isn't a note's.
    fn key(&self, name: &str) -> Option<u8> {
        key(name).map_err(|why| self.slip(why)).ok()
    }

    /// The notes named, all struck at `start` and held `length` ticks.
    pub(crate) fn notes(&self, names: &str, start: i64, length: i64) -> Vec<Note> {
        let keys = names.split_whitespace().filter_map(|n| self.key(n));
        keys.map(|key| Note { start, length, key, velocity: VELOCITY }).collect()
    }

    /// The notes named, one after another from `start`, each `step` ticks long.
    pub(crate) fn line(&self, names: &str, start: i64, step: i64) -> Vec<Note> {
        let named = names.split_whitespace().zip(0i64..);
        let keyed = named.filter_map(|(n, i)| Some((self.key(n)?, i)));
        keyed.map(|(key, i)| Note { start: start + i * step, length: step, key, velocity: VELOCITY }).collect()
    }

    /// Notes bar by bar: bars split by "|", each a run of notes held the whole bar, or groups of notes each followed
    /// by the beats they sound in brackets (from 1, in 4/4): "C2 (1-2) B1 (3-4) | A1".
    pub(crate) fn bars(&self, text: &str) -> Vec<Note> {
        let mut out = Vec::new();
        for (bar, b) in text.split('|').zip(0i64..) {
            let start = b * BAR;
            let mut names: Vec<&str> = Vec::new();
            for word in bar.split_whitespace() {
                if let Some(beats) = word.strip_prefix('(').and_then(|w| w.strip_suffix(')')) {
                    let (from, to) = beats.split_once('-').unwrap_or((beats, beats));
                    let (Ok(from), Ok(to)) = (from.parse::<i64>(), to.parse::<i64>()) else {
                        self.slip(format!("not beats: {word}"));
                        names.clear();
                        continue;
                    };
                    out.extend(self.notes(&names.join(" "), start + (from - 1) * BEAT, (to - from + 1) * BEAT));
                    names.clear();
                } else {
                    names.push(word);
                }
            }
            if !names.is_empty() {
                out.extend(self.notes(&names.join(" "), start, BAR));
            }
        }
        out
    }

    /// Nothing, or every slip in the text.
    pub(crate) fn done(self) -> Result<(), String> {
        let slips = self.slips.into_inner();
        if slips.is_empty() { Ok(()) } else { Err(slips.join("; ")) }
    }
}

/// Every label: 12 roots, 16 cores.
pub(crate) fn all_labels() -> impl Iterator<Item = Label> {
    PitchClass::ALL.into_iter().flat_map(|root| Core::ALL.map(|core| Label::new(root, core)))
}

/// A label by its name, as it's written: "C", "F#m7b5", "Bb7sus4".
pub(crate) fn label_named(name: &str) -> Option<Label> {
    all_labels().find(|l| l.to_string() == name)
}

/// What a span holds: `units` counted from the first unit reported (a loop's copied bar left out).
pub(crate) fn facts_on(hearing: &Hearing, units: Range<usize>, push: Option<&Push>) -> SpanFacts {
    let mut evidence = Evidence::new();
    evidence.prepare(hearing);
    let first = hearing.reported().start;
    let mut facts = SpanFacts::new();
    evidence.facts(hearing, first + units.start..first + units.end, push, None, &mut facts);
    facts
}

/// The candidates ranked by some weights, on some tables.
pub(crate) fn ranked(facts: &SpanFacts, weights: &Weights, tables: &Tables) -> Vec<Scored> {
    let mut candidates = Vec::new();
    facts.candidates(&mut candidates);
    let mut out = Vec::new();
    facts.rank(&candidates, &Scorer::new(weights, &Priors::of(tables)), &mut out);
    out
}

/// Whether a case's number (the digits it starts with, after a "w" for a stretch case read whole) is one the design
/// holds out (G\*): the tests check it, and the fit never sees it.
pub fn held_out(name: &str) -> bool {
    let name = name.strip_prefix('w').unwrap_or(name);
    let number: String = name.chars().take_while(char::is_ascii_digit).collect();
    matches!(number.as_str(), "6" | "15" | "32" | "34" | "35")
}

// ---- One stretch at a time.

/// What a hand case's stretch must read on its own.
#[derive(Clone, Copy, Debug)]
pub enum Want {
    /// The first reading is one of these (the design's gate allows any, or the namer turns it into the name wanted).
    First(&'static [&'static str]),
    /// These are the first two readings, or the first two hold the one given.
    TopTwo(&'static [&'static str]),
    /// These are kept for deciding where the chords change: among the best eight, or the best rootless reading. Their
    /// gates wait for what decides between neighbouring stretches (the pair term): at no trusts in the counted tables
    /// (up to 0.5 each) do they reach the top two or three.
    Kept(&'static [&'static str]),
}

/// Whether a stretch's ranking reads what a hand case wants.
pub(crate) fn reads(ranked: &[Scored], want: Want) -> bool {
    let among =
        |these: &[&str], most: usize| these.iter().all(|t| ranked.iter().take(most).any(|s| s.label.to_string() == *t));
    match want {
        Want::First(any) => ranked.first().is_some_and(|s| any.contains(&s.label.to_string().as_str())),
        Want::TopTwo(these) => among(these, 2),
        Want::Kept(these) => kept(ranked, these),
    }
}

/// Whether each of these readings is kept for deciding where the chords change: among the best eight, or the best
/// rootless reading.
pub(crate) fn kept(ranked: &[Scored], these: &[&str]) -> bool {
    let best_rootless = ranked.iter().find(|s| s.route == Route::Rootless).map(|s| s.label.to_string());
    these
        .iter()
        .all(|t| ranked.iter().take(8).any(|s| s.label.to_string() == *t) || best_rootless.as_deref() == Some(*t))
}

// ---- Read whole.

/// A reading's name: its label, or N for no chord.
pub(crate) fn name_of(label: Option<Label>) -> String {
    label.map_or_else(|| "N".to_owned(), |l| l.to_string())
}

/// A span read, with its units counted from the first unit reported.
#[derive(Clone, Debug, PartialEq)]
pub struct Read {
    pub units: Range<usize>,
    pub sound: Range<usize>,
    pub label: String,
    pub pushed: bool,
    pub rest: bool,
    pub change: f64,
    pub chances: Vec<(String, f64)>,
}

/// Some music read whole: what was heard, the decoder that read it (which gives the chances on any units), and what
/// it read.
pub struct Whole {
    pub hearing: Hearing,
    pub decoder: Decoder,
    pub out: Decoding,
}

impl Whole {
    /// Some music heard and read whole; none when nothing is heard or read.
    pub fn of(music: &Music, weights: &Weights, tables: &Tables, pins: &[PinnedSpan]) -> Option<Whole> {
        let hearing = music.heard()?;
        let (mut decoder, mut out) = (Decoder::new(), Decoding::new());
        decoder.decode(&hearing, weights, tables, pins, &mut out).then_some(Whole { hearing, decoder, out })
    }

    /// The first unit reported: units in the checks count from it.
    pub fn first(&self) -> usize {
        self.hearing.reported().start
    }

    pub fn spans(&self) -> Vec<Read> {
        let first = self.first();
        let from_first = |r: &Range<usize>| r.start - first..r.end - first;
        let read = |s: &DecodedSpan| Read {
            units: from_first(&s.units),
            sound: from_first(&s.sound),
            label: name_of(s.label),
            pushed: s.pushed,
            rest: s.rest,
            change: s.change,
            chances: self.out.chances(s).iter().map(|c| (name_of(c.label), c.chance)).collect(),
        };
        self.out.spans().iter().map(read).collect()
    }

    /// Each reading's chance over some units (counted from the first unit reported), averaged, best first.
    pub fn chances(&self, units: Range<usize>) -> Vec<(String, f64)> {
        let first = self.first();
        let mut out = Vec::new();
        self.decoder.chances(&self.hearing, first + units.start..first + units.end, &mut out);
        out.iter().map(|c| (name_of(c.label), c.chance)).collect()
    }
}

/// The spans read, as text: each one's first unit, its reading (^ when it's pushed, "rest" for a rest) and the chance
/// a chord changes there.
pub fn render_whole(spans: &[Read]) -> String {
    let span = |r: &Read| {
        let pushed = if r.pushed { "^" } else { "" };
        let rest = if r.rest { " rest" } else { "" };
        format!("{}:{}{pushed}{rest} ({:.2})", r.units.start, r.label, r.change)
    };
    spans.iter().map(span).collect::<Vec<_>>().join("  ")
}

/// What a case read whole must read, with units counted from the first unit reported.
#[derive(Clone, Debug)]
pub enum Check {
    /// Every span over these units reads one of these (N for no chord).
    Reads(Range<usize>, &'static [&'static str]),
    /// These units are one span.
    One(Range<usize>),
    /// A span starts at this unit, pushed or not.
    Starts(usize, bool),
    /// No span is pushed.
    NonePushed,
    /// The best readings over these units by their chances, the first so many, hold these.
    Top(Range<usize>, usize, &'static [&'static str]),
    /// These are kept for deciding between them: each has a chance over these units.
    Kept(Range<usize>, &'static [&'static str]),
}

/// Whether a check holds on what was read, and if not, why.
pub fn holds(check: &Check, whole: &Whole, spans: &[Read]) -> Result<(), String> {
    let over = |units: &Range<usize>| -> Vec<&Read> {
        spans.iter().filter(|r| r.units.start < units.end && units.start < r.units.end).collect()
    };
    match check {
        Check::Reads(units, these) => match over(units).into_iter().find(|r| !these.contains(&r.label.as_str())) {
            None => Ok(()),
            Some(r) => Err(format!("{} over {:?}", r.label, r.units)),
        },
        Check::One(units) => match over(units).len() {
            1 => Ok(()),
            n => Err(format!("{n} spans over {units:?}")),
        },
        Check::Starts(unit, pushed) => match spans.iter().find(|r| r.units.start == *unit) {
            Some(r) if r.pushed == *pushed => Ok(()),
            Some(_) => Err(format!("pushed at {unit}: not {pushed}")),
            None => Err(format!("no change at {unit}")),
        },
        Check::NonePushed => match spans.iter().find(|r| r.pushed) {
            None => Ok(()),
            Some(r) => Err(format!("pushed at {}", r.units.start)),
        },
        Check::Top(units, most, these) => {
            let chances = whole.chances(units.clone());
            let best: Vec<&str> = chances.iter().take(*most).map(|(name, _)| name.as_str()).collect();
            match these.iter().all(|t| best.contains(t)) {
                true => Ok(()),
                false => Err(format!("best {most} over {units:?}: {best:?}")),
            }
        }
        Check::Kept(units, these) => {
            let chances = whole.chances(units.clone());
            match these.iter().find(|t| !chances.iter().any(|(name, _)| name == *t)) {
                None => Ok(()),
                Some(t) => Err(format!("{t} not kept over {units:?}")),
            }
        }
    }
}

/// The check a stretch case is held to when it's read whole: what's read over the stretch must be a first reading the
/// case allows, its best two readings by their chances the two it gives, and readings it keeps must have a chance.
fn check_of(units: Range<usize>, want: Want) -> Check {
    match want {
        Want::First(these) => Check::Reads(units, these),
        Want::TopTwo(these) => Check::Top(units, 2, these),
        Want::Kept(these) => Check::Kept(units, these),
    }
}

/// Every case read whole, as loops: those only a whole read decides, then the stretch cases, each named "w" and its
/// own name. (The decode's fingerprint reads them in this order.)
pub fn read_whole_cases() -> Result<Vec<(String, Music, Vec<Check>)>, String> {
    let whole = whole_cases()?.into_iter().map(|(name, music, checks)| (name.to_owned(), music, checks));
    let stretches = hand_cases()?
        .into_iter()
        .map(|(name, music, units, want)| (format!("w{name}"), music, vec![check_of(units, want)]));
    Ok(whole.chain(stretches).collect())
}

// ---- Named.

/// A reading as the cases see it.
#[derive(Clone, Debug, PartialEq)]
pub struct Written {
    pub name: String,
    pub bass: BassKind,
    /// The held bass, when there is one.
    pub bass_note: Option<String>,
    pub also: Option<String>,
    pub roots: Vec<String>,
    pub intervals: Vec<i32>,
    pub chance: f64,
    pub group: u8,
    pub rootless: bool,
    pub doubtful: Vec<u8>,
}

/// A span named, with its units counted from the first unit reported.
#[derive(Clone, Debug, PartialEq)]
pub struct WrittenSpan {
    pub units: Range<usize>,
    pub flags: SpanFlags,
    pub rest: bool,
    pub start: i64,
    pub heard_start: i64,
    pub readings: Vec<Written>,
}

impl WrittenSpan {
    /// What it's written as: its first reading's name, or N.
    pub fn name(&self) -> &str {
        self.readings.first().map_or("N", |r| r.name.as_str())
    }
}

/// Some music read whole and named.
pub struct Named {
    pub hearing: Hearing,
    pub decoding: Decoding,
    pub naming: Naming,
}

impl Named {
    /// Some music heard, read whole and named, each span keeping at most `most` readings; none when nothing is heard
    /// or read.
    pub fn of(music: &Music, weights: &Weights, tables: &Tables, pins: &[PinnedSpan], most: usize) -> Option<Named> {
        let hearing = music.heard()?;
        let (mut decoder, mut decoding, mut naming) = (Decoder::new(), Decoding::new(), Naming::new());
        if !decoder.decode(&hearing, weights, tables, pins, &mut decoding) {
            return None;
        }
        Namer::new().name(&hearing, &decoder, &decoding, most, &mut naming);
        Some(Named { hearing, decoding, naming })
    }

    pub fn spans(&self) -> Vec<WrittenSpan> {
        let first = self.hearing.reported().start;
        let written = |c: Chord| c.name();
        let span = |s: &NamedSpan| WrittenSpan {
            units: s.units.start - first..s.units.end - first,
            flags: s.flags,
            rest: s.rest,
            start: s.start,
            heard_start: s.heard_start,
            readings: self
                .naming
                .readings(s)
                .iter()
                .map(|r| Written {
                    name: written(r.chord),
                    bass: r.bass,
                    bass_note: matches!(r.bass, BassKind::Root | BassKind::Inversion | BassKind::Slash)
                        .then(|| r.chord.bass().to_string()),
                    also: r.also.map(written),
                    roots: r.roots.iter().map(|pc| pc.to_string()).collect(),
                    intervals: r.chord.intervals().iter().map(|pc| i32::from(pc.number())).collect(),
                    chance: r.chance,
                    group: r.group,
                    rootless: r.rootless,
                    doubtful: r.doubtful.iter().map(|pc| pc.number()).collect(),
                })
                .collect(),
        };
        self.naming.spans().iter().map(span).collect()
    }
}

/// The spans named, as text.
pub fn render_named(spans: &[WrittenSpan]) -> String {
    let flags = |f: SpanFlags| {
        let all = [
            (SpanFlags::PUSHED, "pushed"),
            (SpanFlags::PEDAL, "pedal"),
            (SpanFlags::WALKING, "walking"),
            (SpanFlags::NO_BASS, "no bass"),
            (SpanFlags::SPLIT, "split"),
        ];
        let on: Vec<&str> = all.iter().filter(|(flag, _)| f.contains(*flag)).map(|(_, name)| *name).collect();
        if on.is_empty() { String::new() } else { format!(" [{}]", on.join(", ")) }
    };
    let reading = |r: &Written| {
        let also = r.also.as_ref().map_or(String::new(), |a| format!(" also {a}"));
        let rootless = if r.rootless { " rootless" } else { "" };
        let doubtful = if r.doubtful.is_empty() { String::new() } else { format!(" doubtful {:?}", r.doubtful) };
        format!("{} {:?}{also}{rootless}{doubtful} {:.2} g{}", r.name, r.bass, r.chance, r.group)
    };
    let span = |s: &WrittenSpan| {
        let readings: Vec<String> = s.readings.iter().map(reading).collect();
        let rest = if s.rest { " rest" } else { "" };
        format!("  {:?}{rest}{}: {}", s.units, flags(s.flags), readings.join(" | "))
    };
    spans.iter().map(span).collect::<Vec<_>>().join("\n")
}

/// What a case named must be, with units counted from the first unit reported.
#[derive(Clone, Debug)]
pub enum NameWant {
    /// The span starting at this unit is written so, over this kind of bass.
    Writes(usize, &'static str, BassKind),
    /// The span starting at this tick (in the music's own ticks) is written so.
    WritesAt(i64, &'static str),
    /// The span starting at this unit is written the other way so, or isn't written another way.
    Also(usize, Option<&'static str>),
    /// Spans start at these units, and nowhere else.
    Cuts(&'static [usize]),
    /// The span starting at this unit has these flags, and none of those.
    Flags(usize, SpanFlags, SpanFlags),
    /// Among the first so many readings of the span starting at this unit are these.
    Among(usize, usize, &'static [&'static str]),
    /// No span is written with any of these in its name.
    Never(&'static [&'static str]),
    /// The span starting at this unit is written with these notes above its root (the notes as played).
    Intervals(usize, &'static [i32]),
    /// The span starting at this unit may be written on these roots.
    Roots(usize, &'static [&'static str]),
    /// These two readings of the span starting at this unit are one same-notes group.
    Together(usize, &'static str, &'static str),
    /// The span starting at this unit is first struck at this tick.
    HeardFrom(usize, i64),
    /// No reading of any span is over this held bass.
    NeverOver(&'static str),
}

/// Whether what's wanted holds on what was named, and if not, why.
pub fn named_holds(want: &NameWant, spans: &[WrittenSpan]) -> Result<(), String> {
    let at = |unit: &usize| spans.iter().find(|s| s.units.start == *unit).ok_or(format!("no span at {unit}"));
    let first = |unit: &usize| at(unit).and_then(|s| s.readings.first().ok_or(format!("no reading at {unit}")));
    let check = |yes: bool, why: String| if yes { Ok(()) } else { Err(why) };
    match want {
        NameWant::Writes(unit, name, bass) => {
            let r = first(unit)?;
            check(r.name == *name && r.bass == *bass, format!("at {unit}: {} {:?}", r.name, r.bass))
        }
        NameWant::WritesAt(tick, name) => match spans.iter().find(|s| s.start == *tick) {
            Some(s) => check(s.name() == *name, format!("at tick {tick}: {}", s.name())),
            None => Err(format!("no span at tick {tick}")),
        },
        NameWant::Also(unit, also) => {
            let r = first(unit)?;
            check(r.also.as_deref() == *also, format!("at {unit}: {} also {:?}", r.name, r.also))
        }
        NameWant::Cuts(units) => {
            let starts: Vec<usize> = spans.iter().map(|s| s.units.start).collect();
            check(starts == *units, format!("cut at {starts:?}"))
        }
        NameWant::Flags(unit, on, off) => {
            let s = at(unit)?;
            check(s.flags.contains(*on) && s.flags.bits() & off.bits() == 0, format!("at {unit}: {:?}", s.flags))
        }
        NameWant::Among(unit, most, these) => {
            let s = at(unit)?;
            let best: Vec<&str> = s.readings.iter().take(*most).map(|r| r.name.as_str()).collect();
            check(these.iter().all(|t| best.contains(t)), format!("best {most} at {unit}: {best:?}"))
        }
        NameWant::Never(these) => match spans.iter().find(|s| these.iter().any(|t| s.name().contains(t))) {
            Some(s) => Err(format!("{} at {}", s.name(), s.units.start)),
            None => Ok(()),
        },
        NameWant::Intervals(unit, intervals) => {
            let r = first(unit)?;
            check(r.intervals == *intervals, format!("at {unit}: {} {:?}", r.name, r.intervals))
        }
        NameWant::Roots(unit, roots) => {
            let r = first(unit)?;
            check(r.roots == *roots, format!("at {unit}: {} on {:?}", r.name, r.roots))
        }
        NameWant::Together(unit, a, b) => {
            let s = at(unit)?;
            let group = |name: &str| s.readings.iter().find(|r| r.name == name).map(|r| r.group);
            check(
                group(a).is_some() && group(a) == group(b),
                format!("at {unit}: {a} {:?}, {b} {:?}", group(a), group(b)),
            )
        }
        NameWant::HeardFrom(unit, tick) => {
            let s = at(unit)?;
            check(s.heard_start == *tick, format!("at {unit}: heard from {}", s.heard_start))
        }
        NameWant::NeverOver(bass) => {
            let over = |r: &&Written| r.bass_note.as_deref() == Some(*bass);
            match spans.iter().flat_map(|s| s.readings.iter()).find(over) {
                Some(r) => Err(format!("{} over {bass}", r.name)),
                None => Ok(()),
            }
        }
    }
}

// ---- The verdicts.

/// How a case is read for its verdict.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Step {
    /// A stretch's candidates ranked on their own.
    Stretch,
    /// Read whole: where the chords change, and which chord each stretch is.
    Whole,
    /// Read whole and named.
    Named,
}

impl Step {
    pub const ALL: [Step; 3] = [Step::Stretch, Step::Whole, Step::Named];

    /// In a few words.
    pub fn about(self) -> &'static str {
        match self {
            Step::Stretch => "one stretch at a time",
            Step::Whole => "read whole",
            Step::Named => "named",
        }
    }
}

/// What a model read on one hand case, against what the case wants.
#[derive(Clone, Debug, PartialEq)]
pub struct Verdict {
    pub step: Step,
    pub name: String,
    /// A gate (weights that break it aren't shipped), or a target (tracked until it's met).
    pub gate: bool,
    /// A case the design holds out (G\*), a gate or a target: the tests check it, the fit never sees it.
    pub held_out: bool,
    /// What isn't as the case wants: nothing when it holds.
    pub wrong: Vec<String>,
    /// What was read, as text.
    pub read: String,
    /// The music read (as a loop): the fit finds the case it learns from for the same music by it.
    pub music: Music,
}

impl Verdict {
    /// Whether the case reads as it wants.
    pub fn holds(&self) -> bool {
        self.wrong.is_empty()
    }
}

/// The verdicts of the cases one stretch read on its own decides.
pub fn stretch_verdicts(model: &Model) -> Result<Vec<Verdict>, String> {
    let mut out = Vec::new();
    for (name, music, units, want) in hand_cases()? {
        let music = music.looped();
        let (wrong, read) = match music.heard() {
            Some(hearing) => {
                let ranked = ranked(&facts_on(&hearing, units, None), model.weights, model.tables);
                let read: Vec<String> = ranked.iter().take(8).map(|s| format!("{} {:.2}", s.label, s.score)).collect();
                let wrong = if reads(&ranked, want) { Vec::new() } else { vec![format!("wanted {want:?}")] };
                (wrong, read.join(", "))
            }
            None => (vec!["nothing heard".to_owned()], String::new()),
        };
        out.push(Verdict {
            step: Step::Stretch,
            name: name.to_owned(),
            gate: true,
            held_out: held_out(name),
            wrong,
            read,
            music,
        });
    }
    Ok(out)
}

/// The verdicts of every case read whole: those only a whole read decides, then the stretch cases.
pub fn whole_verdicts(model: &Model) -> Result<Vec<Verdict>, String> {
    let mut out = Vec::new();
    for (name, music, checks) in read_whole_cases()? {
        let music = music.looped();
        let (wrong, read) = match Whole::of(&music, model.weights, model.tables, &[]) {
            Some(whole) => {
                let spans = whole.spans();
                (checks.iter().filter_map(|c| holds(c, &whole, &spans).err()).collect(), render_whole(&spans))
            }
            None => (vec!["nothing read".to_owned()], String::new()),
        };
        let held_out = held_out(&name);
        out.push(Verdict { step: Step::Whole, name, gate: true, held_out, wrong, read, music });
    }
    Ok(out)
}

/// The verdicts of the cases named: their gates, or with `targets` the design's targets (and the gates the decode
/// doesn't meet yet).
pub fn named_verdicts(model: &Model, targets: bool) -> Result<Vec<Verdict>, String> {
    let cases = if targets { named_targets()? } else { named_cases()? };
    let mut out = Vec::new();
    for (name, music, wants) in cases {
        let music = music.looped();
        let (wrong, read) = match Named::of(&music, model.weights, model.tables, &[], 6) {
            Some(named) => {
                let spans = named.spans();
                (wants.iter().filter_map(|w| named_holds(w, &spans).err()).collect(), render_named(&spans))
            }
            None => (vec!["nothing read".to_owned()], String::new()),
        };
        let held_out = held_out(name);
        let (gate, name) = (!targets, name.to_owned());
        out.push(Verdict { step: Step::Named, name, gate, held_out, wrong, read, music });
    }
    Ok(out)
}

/// Every hand case read with one model, and judged.
#[derive(Clone, Debug)]
pub struct Book {
    /// One stretch at a time, read whole, named (the gates), then the named targets.
    pub verdicts: Vec<Verdict>,
}

impl Book {
    /// Reads every hand case with `model`.
    pub fn read(model: &Model) -> Result<Book, String> {
        let mut verdicts = stretch_verdicts(model)?;
        verdicts.extend(whole_verdicts(model)?);
        verdicts.extend(named_verdicts(model, false)?);
        verdicts.extend(named_verdicts(model, true)?);
        Ok(Book { verdicts })
    }

    /// The gates that break.
    pub fn broken(&self) -> Vec<&Verdict> {
        self.verdicts.iter().filter(|v| v.gate && !v.holds()).collect()
    }

    /// The targets the fit looks at (not those held out): how many are met, and how many there are.
    pub fn targets(&self) -> (usize, usize) {
        let targets = self.verdicts.iter().filter(|v| !v.gate && !v.held_out);
        targets.fold((0, 0), |(met, all), v| (met + usize::from(v.holds()), all + 1))
    }
}

// ---- What the fit learns from.

/// Some music the fit learns from, as a loop, with the stretches the gates on it say how to read, and the names of
/// the cases read on it (several cases may read the same music, each over its own stretch).
#[derive(Clone, Debug)]
pub struct FitCase {
    /// The first case's name: the one the report goes by.
    pub name: String,
    pub names: Vec<String>,
    pub music: Music,
    pub labels: Vec<FitLabel>,
}

/// The hand cases the fit learns from: every case read whole that isn't held out, with a label for each stretch its
/// gates say what to read: what it must read ([`Check::Reads`]: the cases only a whole read decides, and each stretch
/// case's first reading), or what must come among its best two ([`Check::Top`]: the label accepts either, so the fit
/// makes the two likelier between them; that's less than the gate asks, as one of them may still be starved, and
/// `chordfit --hold` can't hold such a gate once its label is as likely as it asks). A gate that only asks for readings
/// to be kept ([`Check::Kept`]) says nothing about what's read, so it isn't learned from (only checked), and nor is a
/// label that allows no chord. The labels are in the music's ticks, from the start of the units they cover. Cases on
/// the same music (a whole case and the stretch cases cut from it) are one piece with all their labels, so each music
/// counts once. A case with no label is left out.
pub fn fit_cases() -> Result<Vec<FitCase>, String> {
    let mut out: Vec<FitCase> = Vec::new();
    for (name, music, checks) in read_whole_cases()? {
        if held_out(&name) {
            continue;
        }
        let music = music.looped();
        let hearing = music.heard().ok_or_else(|| format!("case {name}: nothing heard"))?;
        let first = hearing.reported().start;
        let mut labels = Vec::new();
        for check in &checks {
            let (Check::Reads(units, names) | Check::Top(units, _, names)) = check else {
                continue;
            };
            if names.contains(&"N") {
                continue;
            }
            let accepts = names
                .iter()
                .map(|n| label_named(n).ok_or_else(|| format!("case {name}: {n} is no reading")))
                .collect::<Result<Vec<Label>, String>>()?;
            let (start, end) = (hearing.start(first + units.start), hearing.start(first + units.end));
            labels.push(FitLabel { start, end, accepts });
        }
        if labels.is_empty() {
            continue;
        }
        match out.iter_mut().find(|case| case.music == music) {
            Some(case) => {
                case.names.push(name);
                for label in labels {
                    if !case.labels.contains(&label) {
                        case.labels.push(label);
                    }
                }
            }
            None => out.push(FitCase { name: name.clone(), names: vec![name], music, labels }),
        }
    }
    Ok(out)
}

// ---- The cases.

/// A case one stretch read on its own decides: its name, the music, the stretch's units and what it must read.
pub type StretchCase = (&'static str, Music, Range<usize>, Want);
/// A case only a whole read decides: its name, the music, and what it must read.
pub type WholeCase = (&'static str, Music, Vec<Check>);
/// A case named: its name, the music, and what it must be named.
pub type NamedCase = (&'static str, Music, Vec<NameWant>);

/// The units of bar `k` (a bar of 4/4 is 8 units).
pub(crate) fn bar(k: usize) -> Range<usize> {
    B * k..B * k + B
}

/// The design's hand cases that one stretch read on its own decides, as far as it decides them
/// (harmony/design/chord-reader-design.md, "Hand cases"): a name, the music, the units, and what it must read.
pub fn hand_cases() -> Result<Vec<StretchCase>, String> {
    use Want::*;
    let w = Writer::new();
    let notes = |n: &str, s: i64, l: i64| w.notes(n, s, l);
    let line = |n: &str, s: i64, l: i64| w.line(n, s, l);
    let bars = |t: &str| w.bars(t);
    let keys_bass = |bass: &str, keys: &str| Music::new().bass(bars(bass)).keys(bars(keys));
    let keys = |keys: &str| Music::new().keys(bars(keys));
    let pedal = || Music::new().bass(notes("C2", 0, 4 * BAR)).keys(bars("C4 E4 G4 | C4 F4 A4 | B3 D4 G4 | C4 E4 G4"));
    let inversion = || keys_bass("C2 (1-2) B1 (3-4) | A1", "C4 E4 G4 | A3 C4 E4");
    let two_feel = || keys_bass("C2 (1-2) G2 (3-4) | F2", "C4 E4 G4 | F3 A3 C4");
    let ii_v_i = || keys_bass("D2 | G2 | C2 | C2", "F3 A3 C4 E4 | F3 A3 B3 E4 | E3 G3 B3 D4 | E3 G3 B3 D4");
    let low = || keys_bass("D2 | G2 | C2 | C2", "C3 E3 F3 A3 | B2 E3 F3 A3 | B2 D3 E3 G3 | B2 D3 E3 G3");
    let rootless = || keys("F4 A4 C5 E5 | F4 A4 B4 E5 | E4 G4 B4 D5 | E4 G4 B4 D5");
    let shells = || keys("D3 C4 F4 | G2 B3 F4 | C3 B3 E4");
    let dim7 = || keys_bass("C#2 | D2", "E3 G3 Bb3 | F3 A3 C4");
    let chromatic = || keys_bass("D2 | Db2 | C2", "F3 A3 C4 | F3 B3 Eb4 | E3 G3 B3");
    let sus = || keys_bass("G1", "G3 C4 D4 F4 (1-2) G3 B3 D4 F4 (3-4)");
    let tune = |t: Vec<Note>| Music::new().bass(bars("G2")).keys(bars("G3 B3 D4")).tune(t);
    let cmaj7 = |t: Vec<Note>| Music::new().bass(bars("C2")).keys(bars("E3 G3 B3")).tune(t);
    let turn = [notes("D5", 0, 3 * BEAT), line("E5 D5", 3 * BEAT, BEAT / 2)].concat();
    let cases = vec![
        ("1", keys_bass("C2", "C4 E4 G4"), bar(0), First(&["C"])),
        ("2 C/E", keys_bass("E2", "C4 E4 G4"), bar(0), First(&["C"])),
        ("2 C/E in octaves", keys_bass("E2 E3", "C4 E4 G4"), bar(0), First(&["C"])),
        ("2 C/G", keys_bass("G1", "C4 E4 G4"), bar(0), First(&["C"])),
        ("2 G7/F", keys_bass("F2", "G3 B3 D4 F4"), bar(0), First(&["G7"])),
        ("3 D/C", keys_bass("C2", "D4 F#4 A4"), bar(0), First(&["D7", "D"])),
        ("4 C C/B", inversion(), bar(0), First(&["C", "Cmaj7"])),
        ("4 Am", inversion(), bar(1), First(&["Am"])),
        ("5 C", two_feel(), bar(0), First(&["C"])),
        ("5 F", two_feel(), bar(1), First(&["F"])),
        ("7 G9sus4", keys_bass("G1", "F3 A3 C4"), bar(0), First(&["G7sus4", "F"])),
        ("7 G9sus4 with D", keys_bass("G1", "F3 A3 C4 D4"), bar(0), First(&["G7sus4"])),
        ("7 C9sus4", keys_bass("C2", "Bb3 D4 F4"), bar(0), First(&["C7sus4", "Bb"])),
        ("7 Bb9sus4", keys_bass("Bb1", "Ab3 C4 Eb4"), bar(0), First(&["Bb7sus4", "Ab"])),
        ("8 Bb13sus4", keys_bass("Bb1", "F3 Ab3 C4 Eb4 G4"), bar(0), First(&["Bb7sus4"])),
        ("9 C", pedal(), bar(0), First(&["C"])),
        ("9 F/C", pedal(), bar(1), First(&["F"])),
        ("9 G/C", pedal(), bar(2), First(&["G"])),
        ("10 Dm9", ii_v_i(), bar(0), First(&["Dm7"])),
        ("10 G13", ii_v_i(), bar(1), First(&["G7"])),
        ("10 Cmaj9", ii_v_i(), 16..32, First(&["Cmaj7"])),
        ("11 Dm9", low(), bar(0), First(&["Dm7"])),
        ("11 G13", low(), bar(1), First(&["G7"])),
        ("11 Cmaj9", low(), 16..32, First(&["Cmaj7"])),
        ("12 Fmaj7 or Dm9", rootless(), bar(0), Kept(&["Fmaj7", "Dm7"])),
        ("12 G13 or Fmaj7(#11)", rootless(), bar(1), Kept(&["G7", "Fmaj7"])),
        ("12 Em7 or Cmaj9", rootless(), 16..32, Kept(&["Em7", "Cmaj7"])),
        ("13 G13 rootless", keys("B2 E3 F3 A3"), bar(0), Kept(&["G7"])),
        ("14 Dm7", shells(), bar(0), First(&["Dm7"])),
        ("14 G7", shells(), bar(1), First(&["G7"])),
        ("14 Cmaj7", shells(), bar(2), First(&["Cmaj7"])),
        ("16 C7(#9)", keys_bass("C2", "E3 Bb3 Eb4"), bar(0), First(&["C7"])),
        ("16 E7(#9)", keys_bass("E2", "G#3 D4 G4"), bar(0), First(&["E7"])),
        ("16 E7(#9) keys alone", keys("E3 G#3 D4 G4"), bar(0), First(&["E7"])),
        ("17 C7alt", keys_bass("C2", "E3 Bb3 D#4 G#4"), bar(0), First(&["C7"])),
        ("18 G7alt", keys_bass("G2", "B3 F4 Ab4 Bb4 Eb5"), bar(0), First(&["G7"])),
        ("19 Db13", keys_bass("Db2", "F3 B3 Eb4 Bb4"), bar(0), First(&["Db7"])),
        ("19 G7alt", keys_bass("G1", "F3 B3 Eb4 Bb4"), bar(0), First(&["G7"])),
        ("20 no bass", keys("F3 B3 Eb4 Bb4"), bar(0), Kept(&["Db7", "G7"])),
        ("21 Am6", keys_bass("A1", "C4 E4 F#4 A4"), bar(0), First(&["Am6"])),
        ("21 F#m7b5", keys_bass("F#2", "C4 E4 F#4 A4"), bar(0), First(&["F#m7b5"])),
        ("21 D9", keys_bass("D2", "C4 E4 F#4 A4"), bar(0), First(&["D7"])),
        ("21 no bass", keys("C4 E4 F#4 A4"), bar(0), TopTwo(&["Am6", "F#m7b5"])),
        ("22 C6", keys_bass("C2", "C4 E4 G4 A4"), bar(0), First(&["C6"])),
        ("22 Am7", keys_bass("A1", "C4 E4 G4 A4"), bar(0), First(&["Am7"])),
        ("22 Fmaj9", keys_bass("F1", "C4 E4 G4 A4"), bar(0), First(&["Fmaj7"])),
        ("22 no bass", keys("C4 E4 G4 A4"), bar(0), TopTwo(&["C6", "Am7"])),
        ("23 Cmaj9/E", keys_bass("E2", "G3 B3 C4 D4"), bar(0), TopTwo(&["Cmaj7", "Em7"])),
        ("24 C#dim7", dim7(), bar(0), First(&["Dbdim7", "Edim7", "Gdim7", "Bbdim7"])),
        ("24 Dm7", dim7(), bar(1), First(&["Dm7"])),
        ("26 C7(b9)", keys_bass("C2", "E3 G3 Bb3 Db4"), bar(0), First(&["C7"])),
        ("30 C+", keys_bass("C2", "E3 G#3 C4"), bar(0), First(&["C+", "E+", "Ab+"])),
        ("31 G7sus4", sus(), 0..4, First(&["G7sus4"])),
        ("31 G7", sus(), 4..8, First(&["G7"])),
        ("33 Em11", keys_bass("E2", "E3 A3 D4 G4 B4"), bar(0), First(&["Em7"])),
        ("33 Dm11", keys_bass("D2", "G3 C4 F4"), bar(0), First(&["Dm7"])),
        ("33 no bass", keys("E3 A3 D4 G4"), bar(0), TopTwo(&["A7sus4", "Em7"])),
        ("34 Cmaj9", keys_bass("C2", "D4 E4 G4 B4"), bar(0), First(&["Cmaj7"])),
        ("47 G, E5 a neighbour", tune(turn), bar(0), First(&["G"])),
        ("48a Cmaj9, D5 held", cmaj7(bars("D5")), bar(0), First(&["Cmaj7"])),
        ("48b Cmaj7, F5 held", cmaj7(bars("F5")), bar(0), First(&["Cmaj7"])),
        ("48c Cmaj7, C5 D5 E5", cmaj7(line("C5 D5 E5", 0, BEAT / 2)), bar(0), First(&["Cmaj7"])),
        ("49 Cmaj9 keys alone", keys("C2 G2 C3 E4 G4 B4 D5"), bar(0), First(&["Cmaj7"])),
        ("50 Cmaj9/E keys alone", keys("E2 E3 C4 G4 B4 D5"), bar(0), TopTwo(&["Cmaj7"])),
        ("55 Dm7", chromatic(), bar(0), First(&["Dm7"])),
        ("55 Db9", chromatic(), bar(1), First(&["Db7"])),
        ("55 Cmaj7", chromatic(), bar(2), First(&["Cmaj7"])),
    ];
    w.done()?;
    Ok(cases)
}

/// The design's hand cases that only a whole read decides (where the chords change, pushes, long chords), as loops: a
/// name, the music, and what it must read. 12, 13 and 20 have no bass: the design wants their rootless readings among
/// the best two or three, which no weights reach yet (harmony/design/chord-reader-design.md, "As built"); for now
/// they must be kept.
pub fn whole_cases() -> Result<Vec<WholeCase>, String> {
    use Check::*;
    let w = Writer::new();
    let notes = |n: &str, s: i64, l: i64| w.notes(n, s, l);
    let line = |n: &str, s: i64, l: i64| w.line(n, s, l);
    let bars = |t: &str| w.bars(t);
    let keys_bass = |bass: &str, keys: &str| Music::new().bass(bars(bass)).keys(bars(keys));
    let keys = |keys: &str| Music::new().keys(bars(keys));
    let struck = |bass: &str, keys: &str, n: usize| keys_bass(&vec![bass; n].join(" | "), &vec![keys; n].join(" | "));
    let cases = vec![
        (
            "4 C C/B | Am",
            keys_bass("C2 (1-2) B1 (3-4) | A1", "C4 E4 G4 | A3 C4 E4"),
            vec![Reads(0..B, &["C", "Cmaj7"]), Reads(B..2 * B, &["Am"]), Starts(B, false)],
        ),
        (
            "5 C | F, never C/G",
            keys_bass("C2 (1-2) G2 (3-4) | F2", "C4 E4 G4 | F3 A3 C4"),
            vec![Reads(0..B, &["C"]), One(0..B), Reads(B..2 * B, &["F"])],
        ),
        (
            "6 C C/E | F F#dim7 | C/G",
            keys_bass(
                "C2 (1-2) E2 (3-4) | F2 (1-2) F#2 (3-4) | G2",
                "C4 E4 G4 | C4 F4 A4 (1-2) C4 Eb4 A4 (3-4) | C4 E4 G4",
            ),
            vec![
                Reads(0..B, &["C"]),
                Reads(B..B + 4, &["F"]),
                Reads(B + 4..2 * B, &["F#dim7", "Adim7", "Cdim7", "Ebdim7"]),
                Reads(2 * B..3 * B, &["C"]),
            ],
        ),
        (
            "27 C C#dim7 | Dm7",
            keys_bass("C2 (1-2) C#2 (3-4) | D2", "C4 E4 G4 (1-2) E4 G4 Bb4 (3-4) | F4 A4 C5"),
            vec![
                Reads(0..4, &["C"]),
                Reads(4..B, &["Dbdim7", "Edim7", "Gdim7", "Bbdim7"]),
                Starts(4, false),
                Reads(B..2 * B, &["Dm7"]),
            ],
        ),
        (
            "28 C Cdim7 | C over a held C",
            Music::new().bass(notes("C2", 0, 2 * BAR)).keys(bars("C4 E4 G4 (1-2) C4 Eb4 F#4 A4 (3-4) | C4 E4 G4")),
            vec![Reads(0..4, &["C"]), Reads(4..B, &["Cdim7", "Ebdim7", "F#dim7", "Adim7"]), Reads(B..2 * B, &["C"])],
        ),
        (
            "29 Em7 | Ebdim7 | Dm7",
            keys_bass("E2 | Eb2 | D2", "G3 B3 D4 | Gb3 A3 C4 | F3 A3 C4"),
            vec![
                Reads(0..B, &["Em7"]),
                Reads(B..2 * B, &["Ebdim7", "F#dim7", "Adim7", "Cdim7"]),
                Reads(2 * B..3 * B, &["Dm7"]),
            ],
        ),
        (
            "35 Cm | Cm(maj7) | Cm7 | Cm6 over a held C",
            Music::new()
                .bass(notes("C2", 0, 4 * BAR))
                .keys([notes("Eb4 G4", 0, 4 * BAR), bars("C5 | B4 | Bb4 | A4")].concat()),
            vec![
                Reads(0..B, &["Cm"]),
                Reads(B..2 * B, &["Cm(maj7)"]),
                Reads(2 * B..3 * B, &["Cm7"]),
                Reads(3 * B..4 * B, &["Cm6"]),
            ],
        ),
        (
            "37 Dm7 | G13 over a walk",
            keys_bass("D2 (1) E2 (2) F2 (3) G2 (4) | G2", "F3 A3 C4 | F3 A3 B3 E4"),
            vec![Reads(0..B, &["Dm7"]), One(0..B), Reads(B..2 * B, &["G7"])],
        ),
        (
            "38 Dm7 | G7 | Cmaj7 over a walk",
            Music::new()
                .bass(line("D2 F2 A2 Ab2 G2 B2 D3 Db3 C3 E3 G3 B2 C3 D3 E3 G2", 0, BEAT))
                .keys(bars("F3 C4 | F3 B3 | E3 B3 | E3 B3")),
            vec![Reads(0..B, &["Dm7"]), Reads(B..2 * B, &["G7"]), Reads(2 * B..4 * B, &["Cmaj7"]), One(2 * B..4 * B)],
        ),
        (
            "39 bar 2 Am7",
            Music::new().bass(line("C2 E2 G2 Ab2 A2", 0, BEAT)).keys(bars("E4 G4 B4 | C4 E4 G4")),
            vec![Reads(B..2 * B, &["Am7"])],
        ),
        (
            "40 C | F in eighths",
            Music::new().part(Role::Harmony, line("C3 E3 G3 C4 E4 C4 G3 E3 F3 A3 C4 F4 A4 F4 C4 A3", 0, BEAT / 2)),
            vec![Reads(0..B, &["C"]), Reads(B..2 * B, &["F"])],
        ),
        (
            "41 C, D4 passing",
            Music::new().keys(line("C4 D4 E4 G4 E4 D4 C4 G3", 0, BEAT / 2)),
            vec![Reads(0..B, &["C"])],
        ),
        ("42 Alberti C", Music::new().keys(line("C3 G3 E3 G3 C3 G3 E3 G3", 0, BEAT / 2)), vec![Reads(0..B, &["C"])]),
        (
            "43 Cmaj7 | Am7 pushed",
            Music::new().bass(bars("C2 | A1")).keys(
                [notes("C4 E4 G4 B4", 0, BAR - BEAT / 2), notes("A3 C4 E4 G4", BAR - BEAT / 2, BAR + BEAT / 2)]
                    .concat(),
            ),
            vec![Reads(0..B, &["Cmaj7"]), Reads(B..2 * B, &["Am7"]), Starts(B, true)],
        ),
        (
            "44 C | F pushed",
            Music::new()
                .bass([notes("C2", 0, BAR - BEAT / 2), notes("F2", BAR - BEAT / 2, BAR + BEAT / 2)].concat())
                .keys(
                    [notes("C4 E4 G4", 0, BAR - BEAT / 2), notes("F3 A3 C4", BAR - BEAT / 2, BAR + BEAT / 2)].concat(),
                ),
            vec![Reads(0..B, &["C"]), Reads(B..2 * B, &["F"]), Starts(B, true)],
        ),
        (
            "45 C | Am, A5 anticipated",
            keys_bass("C2 | A1", "C4 E4 G4 | A3 C4 E4").tune(notes("A5", BAR - BEAT / 2, BAR / 2)),
            vec![Reads(0..B, &["C"]), Reads(B..2 * B, &["Am"]), Starts(B, false), NonePushed],
        ),
        (
            "51 Cmaj7 | Fmaj7, B4 held over",
            Music::new()
                .bass(bars("C2 | F2"))
                .keys([notes("C4 E4 G4 B4", 0, BAR + BEAT / 2), notes("F4 A4 C5 E5", BAR, BAR)].concat()),
            vec![Reads(0..B, &["Cmaj7"]), Reads(B..2 * B, &["Fmaj7"]), Starts(B, false)],
        ),
        (
            "52 C | F, C E G rung on",
            Music::new()
                .bass(bars("C2 | F2"))
                .keys([notes("C4 E4 G4", 0, BAR + BEAT), notes("F3 A3 C4", BAR, BAR)].concat()),
            vec![Reads(0..B, &["C"]), Reads(B..2 * B, &["F"]), Starts(B, false)],
        ),
        (
            "54 C | G | Am | F with a tune",
            keys_bass("C2 | G1 | A1 | F1", "C4 E4 G4 | B3 D4 G4 | C4 E4 A4 | C4 F4 A4").tune(line(
                "E5 D5 C5 D5 D5 C5 B4 C5 C5 B4 A4 B4 A4 G4 F4 G4",
                0,
                BEAT,
            )),
            vec![
                Reads(0..B, &["C"]),
                Reads(B..2 * B, &["G"]),
                Reads(2 * B..3 * B, &["Am"]),
                Reads(3 * B..4 * B, &["F"]),
            ],
        ),
        (
            "12 rootless, no bass",
            keys("F4 A4 C5 E5 | F4 A4 B4 E5 | E4 G4 B4 D5 | E4 G4 B4 D5"),
            vec![
                Kept(0..B, &["Fmaj7", "Dm7"]),
                Kept(B..2 * B, &["G7", "Fmaj7"]),
                Kept(2 * B..4 * B, &["Em7", "Cmaj7"]),
            ],
        ),
        ("13 G13 rootless, no bass", keys("B2 E3 F3 A3"), vec![Kept(0..B, &["G7"])]),
        ("20 G7alt or Db13, no bass", keys("F3 B3 Eb4 Bb4"), vec![Kept(0..B, &["G7", "Db7"])]),
        // Long chords: one chord, read as one, however long it's held or however often it's struck again; and C E G A
        // over C stays C6 though labels name Am7 more often.
        (
            "h1 C6 held 8 bars",
            Music::new().bass(notes("C2", 0, 8 * BAR)).keys(notes("C4 E4 G4 A4", 0, 8 * BAR)),
            vec![Reads(0..8 * B, &["C6"]), One(0..8 * B)],
        ),
        ("h2 C6 struck each bar", struck("C2", "C4 E4 G4 A4", 8), vec![Reads(0..8 * B, &["C6"]), One(0..8 * B)]),
        ("h3 Am7 struck each bar", struck("A1", "C4 E4 G4 A4", 6), vec![Reads(0..6 * B, &["Am7"]), One(0..6 * B)]),
        ("h4 Cdim7 struck each bar", struck("C2", "Eb3 Gb3 A3", 6), vec![Reads(0..6 * B, &["Cdim7"]), One(0..6 * B)]),
        (
            "h5 Dm7 held 6 bars, no bass",
            Music::new().keys(notes("D3 F3 A3 C4", 0, 6 * BAR)),
            vec![Reads(0..6 * B, &["Dm7"]), One(0..6 * B)],
        ),
        // The same chord struck again is nothing new: the bass under it walks, however quick the chords are.
        (
            "h6 Cmaj9 | Am11 | Dm9 | G13 in sixteenths over eighths",
            dense_on(&w, 4),
            vec![
                Reads(0..B, &["Cmaj7"]),
                One(0..B),
                Reads(B..2 * B, &["Am7"]),
                One(B..2 * B),
                Reads(2 * B..3 * B, &["Dm7"]),
                One(2 * B..3 * B),
                Reads(3 * B..4 * B, &["G7"]),
                One(3 * B..4 * B),
            ],
        ),
    ];
    w.done()?;
    Ok(cases)
}

/// A dense gospel part, so many bars long: Cmaj9, Am11, Dm9 and G13 by turns, a bar each, the chords struck in
/// sixteenths over a bass moving in eighths.
pub(crate) fn dense_on(w: &Writer, bars: i64) -> Music {
    let (mut bass, mut keys) = (Vec::new(), Vec::new());
    let each = [
        ("C2 G2 C3 G2 E2 G2 C3 B2", "E3 G3 B3 D4"),
        ("A1 E2 A2 E2 G2 E2 A2 G2", "G3 C4 D4 E4"),
        ("D2 A2 D3 A2 F2 A2 C3 A2", "F3 A3 C4 E4"),
        ("G1 D2 G2 D2 F2 D2 B2 D2", "F3 A3 B3 E4"),
    ];
    for (bar, (walk, chord)) in (0..bars).zip(each.iter().cycle()) {
        bass.extend(w.line(walk, bar * BAR, BEAT / 2));
        keys.extend((0..16).flat_map(|k| w.notes(chord, bar * BAR + k * BEAT / 4, BEAT / 4)));
    }
    Music::new().bass(bass).keys(keys)
}

/// The design's hand cases, named: a name (from the design's table), the music (read as a loop), and what it must be
/// named.
pub fn named_cases() -> Result<Vec<NamedCase>, String> {
    use BassKind::{Inversion, Moving, Root, Slash};
    use NameWant::*;
    let w = Writer::new();
    let notes = |n: &str, s: i64, l: i64| w.notes(n, s, l);
    let line = |n: &str, s: i64, l: i64| w.line(n, s, l);
    let bars = |t: &str| w.bars(t);
    let keys_bass = |bass: &str, keys: &str| Music::new().bass(bars(bass)).keys(bars(keys));
    let keys = |keys: &str| Music::new().keys(bars(keys));
    let none = SpanFlags::NONE;
    let ii_v_i = |keys: &str| Music::new().bass(bars("D2 | G2 | C2 | C2")).keys(bars(keys));
    let cmaj7 = |t: Vec<Note>| Music::new().bass(bars("C2")).keys(bars("E3 G3 B3")).tune(t);
    let dominant = |upper: &str| keys_bass("C2", &format!("E3 Bb3 {upper}"));
    let cases = vec![
        ("1 C", keys_bass("C2", "C4 E4 G4"), vec![Writes(0, "C", Root), Also(0, None)]),
        ("2 C/E", keys_bass("E2", "C4 E4 G4"), vec![Writes(0, "C/E", Inversion), Never(&["Em"])]),
        ("2 C/E in octaves", keys_bass("E2 E3", "C4 E4 G4"), vec![Writes(0, "C/E", Inversion)]),
        ("2 C/G", keys_bass("G1", "C4 E4 G4"), vec![Writes(0, "C/G", Inversion)]),
        ("2 G7/F", keys_bass("F2", "G3 B3 D4 F4"), vec![Writes(0, "G7/F", Inversion), Also(0, None)]),
        ("3 D/C", keys_bass("C2", "D4 F#4 A4"), vec![Writes(0, "D/C", Slash), Also(0, Some("D7/C"))]),
        (
            "4 C C/B | Am",
            keys_bass("C2 (1-2) B1 (3-4) | A1", "C4 E4 G4 | A3 C4 E4"),
            vec![
                Cuts(&[0, 4, B]),
                Writes(0, "C", Root),
                Writes(4, "C/B", Slash),
                Also(4, Some("Cmaj7/B")),
                Flags(0, SpanFlags::SPLIT, none),
                Flags(4, SpanFlags::SPLIT, none),
                Writes(B, "Am", Root),
                Flags(B, none, SpanFlags::SPLIT),
            ],
        ),
        (
            "4 C7 C7/Bb | F/A, its own seventh",
            keys_bass("C2 (1-2) Bb1 (3-4) | A1", "C4 E4 G4 Bb4 | A3 C4 F4"),
            vec![Cuts(&[0, 4, B]), Writes(0, "C7", Root), Writes(4, "C7/Bb", Inversion), Writes(B, "F/A", Inversion)],
        ),
        (
            "4 C7, never cut where the bass plays B",
            keys_bass("C2 (1-2) B1 (3-4) | A1", "C4 E4 G4 Bb4 | A3 C4 E4"),
            vec![Cuts(&[0, B]), Writes(0, "C7", Moving), Never(&["/B"])],
        ),
        (
            "4 Cmaj7, never cut where the bass plays Bb",
            keys_bass("C2 (1-2) Bb1 (3-4) | A1", "C4 E4 G4 B4 | A3 C4 F4"),
            vec![Cuts(&[0, B]), Writes(0, "Cmaj7", Moving)],
        ),
        (
            "5 C | F, never C/G",
            keys_bass("C2 (1-2) G2 (3-4) | F2", "C4 E4 G4 | F3 A3 C4"),
            vec![Cuts(&[0, B]), Writes(0, "C", Moving), Writes(B, "F", Root)],
        ),
        (
            "6 C C/E | F F#dim7 | C/G",
            keys_bass(
                "C2 (1-2) E2 (3-4) | F2 (1-2) F#2 (3-4) | G2",
                "C4 E4 G4 | C4 F4 A4 (1-2) C4 Eb4 A4 (3-4) | C4 E4 G4",
            ),
            vec![
                Cuts(&[0, 4, B, B + 4, 2 * B]),
                Writes(0, "C", Root),
                Writes(4, "C/E", Inversion),
                Writes(B, "F", Root),
                Writes(B + 4, "F#dim7", Root),
                Writes(2 * B, "C/G", Inversion),
            ],
        ),
        ("7 G9sus4", keys_bass("G1", "F3 A3 C4"), vec![Writes(0, "G9sus4", Root), Also(0, Some("F/G"))]),
        ("7 G9sus4 with D4", keys_bass("G1", "F3 A3 C4 D4"), vec![Writes(0, "G9sus4", Root)]),
        ("7 C9sus4", keys_bass("C2", "Bb3 D4 F4"), vec![Writes(0, "C9sus4", Root), Also(0, Some("Bb/C"))]),
        ("7 Bb9sus4", keys_bass("Bb1", "Ab3 C4 Eb4"), vec![Writes(0, "Bb9sus4", Root), Also(0, Some("Ab/Bb"))]),
        (
            "8 Bb13sus4",
            keys_bass("Bb1", "F3 Ab3 C4 Eb4 G4"),
            vec![Writes(0, "Bb13sus4", Root), Also(0, Some("Fm9/Bb"))],
        ),
        (
            "9 C | F/C | G/C | C over a pedal",
            Music::new().bass(notes("C2", 0, 4 * BAR)).keys(bars("C4 E4 G4 | C4 F4 A4 | B3 D4 G4 | C4 E4 G4")),
            vec![
                Cuts(&[0, B, 2 * B, 3 * B]),
                Writes(0, "C", Root),
                Writes(B, "F/C", Inversion),
                Writes(2 * B, "G/C", Slash),
                Writes(3 * B, "C", Root),
                Flags(0, none, SpanFlags::PEDAL),
                Flags(B, SpanFlags::PEDAL, none),
                Flags(2 * B, SpanFlags::PEDAL, none),
                Flags(3 * B, none, SpanFlags::PEDAL),
                Never(&["sus", "add"]),
            ],
        ),
        (
            "10 Dm9 | G13 | Cmaj9",
            ii_v_i("F3 A3 C4 E4 | F3 A3 B3 E4 | E3 G3 B3 D4 | E3 G3 B3 D4"),
            vec![Cuts(&[0, B, 2 * B]), Writes(0, "Dm9", Root), Writes(B, "G13", Root), Writes(2 * B, "Cmaj9", Root)],
        ),
        (
            "11 the same, voiced low",
            ii_v_i("C3 E3 F3 A3 | B2 E3 F3 A3 | B2 D3 E3 G3 | B2 D3 E3 G3"),
            vec![Cuts(&[0, B, 2 * B]), Writes(0, "Dm9", Root), Writes(B, "G13", Root), Writes(2 * B, "Cmaj9", Root)],
        ),
        (
            "14 shells, the bass by the gap",
            keys("D3 C4 F4 | G2 B3 F4 | C3 B3 E4"),
            vec![Writes(0, "Dm7", Root), Writes(B, "G7", Root), Writes(2 * B, "Cmaj7", Root)],
        ),
        ("15a C13(#11)", dominant("D4 F#4 A4"), vec![Intervals(0, &[0, 2, 4, 6, 9, 10])]),
        ("15b C7(#9)", dominant("Eb4 G4 Bb4"), vec![Writes(0, "C7(#9)", Root)]),
        ("15c C13(b9)", dominant("A4 C#5 E5"), vec![Writes(0, "C13(b9)", Root)]),
        ("15d C7alt", dominant("Ab4 C5 Eb5"), vec![Writes(0, "C7alt", Root), Intervals(0, &[0, 3, 4, 8, 10])]),
        ("16 C7(#9)", keys_bass("C2", "E3 Bb3 Eb4"), vec![Writes(0, "C7(#9)", Root)]),
        ("16 E7(#9)", keys_bass("E2", "G#3 D4 G4"), vec![Writes(0, "E7(#9)", Root)]),
        ("16 E7(#9) keys alone", keys("E3 G#3 D4 G4"), vec![Writes(0, "E7(#9)", BassKind::None)]),
        (
            "17 C7alt",
            keys_bass("C2", "E3 Bb3 D#4 G#4"),
            vec![Writes(0, "C7alt", Root), Intervals(0, &[0, 3, 4, 8, 10])],
        ),
        ("18 G7alt", keys_bass("G2", "B3 F4 Ab4 Bb4 Eb5"), vec![Writes(0, "G7alt", Root)]),
        ("19 Db13", keys_bass("Db2", "F3 B3 Eb4 Bb4"), vec![Writes(0, "Db13", Root)]),
        ("19 G7alt", keys_bass("G1", "F3 B3 Eb4 Bb4"), vec![Writes(0, "G7alt", Root)]),
        ("21 Am6", keys_bass("A1", "C4 E4 F#4 A4"), vec![Writes(0, "Am6", Root), Together(0, "Am6", "F#m7b5/A")]),
        ("21 F#m7b5", keys_bass("F#2", "C4 E4 F#4 A4"), vec![Writes(0, "F#m7b5", Root)]),
        ("21 D9", keys_bass("D2", "C4 E4 F#4 A4"), vec![Writes(0, "D9", Root)]),
        ("21 no bass", keys("C4 E4 F#4 A4"), vec![Among(0, 2, &["Am6", "F#m7b5"]), Together(0, "Am6", "F#m7b5")]),
        ("22 C6", keys_bass("C2", "C4 E4 G4 A4"), vec![Writes(0, "C6", Root), Together(0, "C6", "Am7/C")]),
        ("22 Am7", keys_bass("A1", "C4 E4 G4 A4"), vec![Writes(0, "Am7", Root), Together(0, "Am7", "C6/A")]),
        ("22 Fmaj9", keys_bass("F1", "C4 E4 G4 A4"), vec![Writes(0, "Fmaj9", Root)]),
        ("22 no bass", keys("C4 E4 G4 A4"), vec![Among(0, 2, &["C6", "Am7"]), Together(0, "C6", "Am7")]),
        ("23 Cmaj9/E or Em7(b13)", keys_bass("E2", "G3 B3 C4 D4"), vec![Among(0, 2, &["Cmaj9/E", "Em7(b13)"])]),
        (
            "24 C#dim7 | Dm7",
            keys_bass("C#2 | D2", "E3 G3 Bb3 | F3 A3 C4"),
            vec![Writes(0, "C#dim7", Root), Roots(0, &["Db", "E", "G", "Bb"]), Writes(B, "Dm7", Root)],
        ),
        (
            "25 C#dim7, no bass, before Dm",
            keys("E4 G4 Bb4 C#5 | D4 F4 A4"),
            vec![
                Writes(0, "C#dim7", BassKind::None),
                Roots(0, &["Db", "E", "G", "Bb"]),
                Together(0, "C#dim7", "A7(b9)"),
            ],
        ),
        ("26 C7(b9)", keys_bass("C2", "E3 G3 Bb3 Db4"), vec![Writes(0, "C7(b9)", Root)]),
        (
            "27 C C#dim7 | Dm7",
            keys_bass("C2 (1-2) C#2 (3-4) | D2", "C4 E4 G4 (1-2) E4 G4 Bb4 (3-4) | F4 A4 C5"),
            vec![Cuts(&[0, 4, B]), Writes(0, "C", Root), Writes(4, "C#dim7", Root), Writes(B, "Dm7", Root)],
        ),
        (
            "28 C Cdim7 | C over a held C",
            Music::new().bass(notes("C2", 0, 2 * BAR)).keys(bars("C4 E4 G4 (1-2) C4 Eb4 F#4 A4 (3-4) | C4 E4 G4")),
            vec![
                Writes(0, "C", Root),
                Writes(4, "Cdim7", Root),
                Writes(B, "C", Root),
                Flags(4, none, SpanFlags::PEDAL),
                Never(&["/"]),
            ],
        ),
        (
            "29 Em7 | Ebdim7 | Dm7",
            keys_bass("E2 | Eb2 | D2", "G3 B3 D4 | Gb3 A3 C4 | F3 A3 C4"),
            vec![Writes(0, "Em7", Root), Writes(B, "D#dim7", Root), Writes(2 * B, "Dm7", Root)],
        ),
        ("30 C+", keys_bass("C2", "E3 G#3 C4"), vec![Writes(0, "C+", Root), Roots(0, &["C", "E", "Ab"])]),
        (
            "30 C+ with no bass, before F",
            Music::new().keys(bars("E4 G#4 C5 | F4 A4 C5")).bass(bars("| F2")),
            vec![Writes(0, "C+", BassKind::None), Roots(0, &["C", "E", "Ab"]), Writes(B, "F", Root)],
        ),
        (
            "31 G7sus4 G7",
            keys_bass("G1", "G3 C4 D4 F4 (1-2) G3 B3 D4 F4 (3-4)"),
            vec![Cuts(&[0, 4]), Writes(0, "G7sus4", Root), Writes(4, "G7", Root), Never(&["11"])],
        ),
        ("32 C9sus4", keys_bass("C2", "Bb3 D4 F4 G4"), vec![Writes(0, "C9sus4", Root)]),
        ("32 Dm11", keys_bass("D2", "F3 A3 C4 E4 G4"), vec![Writes(0, "Dm11", Root)]),
        ("32 C7sus4(b9)", keys_bass("C2", "Bb3 Db4 F4 G4"), vec![Writes(0, "C7sus4(b9)", Root)]),
        ("33 Em11", keys_bass("E2", "E3 A3 D4 G4 B4"), vec![Writes(0, "Em11", Root)]),
        ("33 Dm11", keys_bass("D2", "G3 C4 F4"), vec![Writes(0, "Dm11", Root)]),
        ("33 no bass", keys("E3 A3 D4 G4"), vec![Among(0, 2, &["A7sus4", "Em11"])]),
        ("34 Cm11, C3 the bass", keys("C3 G3 D4 Eb4 Bb4 F5"), vec![Writes(0, "Cm11", Root)]),
        ("34 Cmaj9", keys_bass("C2", "D4 E4 G4 B4"), vec![Writes(0, "Cmaj9", Root)]),
        (
            "35 Cm | Cm(maj7) | Cm7 | Cm6 over a held C",
            Music::new()
                .bass(notes("C2", 0, 4 * BAR))
                .keys([notes("Eb4 G4", 0, 4 * BAR), bars("C5 | B4 | Bb4 | A4")].concat()),
            vec![
                Writes(0, "Cm", Root),
                Writes(B, "Cm(maj7)", Root),
                Writes(2 * B, "Cm7", Root),
                Writes(3 * B, "Cm6", Root),
                Flags(B, none, SpanFlags::PEDAL),
                Flags(2 * B, none, SpanFlags::PEDAL),
                Flags(3 * B, none, SpanFlags::PEDAL),
            ],
        ),
        (
            "36 a walk under F A C",
            Music::new().bass(line("D2 E2 F2 G2", 0, BEAT)).keys(bars("F3 A3 C4")),
            vec![Never(&["9", "11", "13", "/"]), Flags(0, SpanFlags::WALKING, none)],
        ),
        (
            "37 Dm7 | G13 over a walk",
            keys_bass("D2 (1) E2 (2) F2 (3) G2 (4) | G2", "F3 A3 C4 | F3 A3 B3 E4"),
            vec![Writes(0, "Dm7", Moving), Writes(B, "G13", Root), Never(&["11"])],
        ),
        (
            "38 Dm7 | G7 | Cmaj7 over a walk",
            Music::new()
                .bass(line("D2 F2 A2 Ab2 G2 B2 D3 Db3 C3 E3 G3 B2 C3 D3 E3 G2", 0, BEAT))
                .keys(bars("F3 C4 | F3 B3 | E3 B3 | E3 B3")),
            vec![Writes(0, "Dm7", Moving), Writes(B, "G7", Moving), Writes(2 * B, "Cmaj7", Moving), Never(&["/"])],
        ),
        (
            "39 Cmaj7 | Am7 over a walk",
            Music::new().bass(line("C2 E2 G2 Ab2 A2", 0, BEAT)).keys(bars("E4 G4 B4 | C4 E4 G4")),
            vec![Writes(B, "Am7", Root), Never(&["/"])],
        ),
        (
            "40 C | F in eighths",
            Music::new().part(Role::Harmony, line("C3 E3 G3 C4 E4 C4 G3 E3 F3 A3 C4 F4 A4 F4 C4 A3", 0, BEAT / 2)),
            vec![Writes(0, "C", BassKind::None), Writes(B, "F", BassKind::None)],
        ),
        (
            "41 C, D4 passing",
            Music::new().keys(line("C4 D4 E4 G4 E4 D4 C4 G3", 0, BEAT / 2)),
            vec![Writes(0, "C", BassKind::None)],
        ),
        (
            "42 Alberti C",
            Music::new().keys(line("C3 G3 E3 G3 C3 G3 E3 G3", 0, BEAT / 2)),
            vec![Cuts(&[0]), Among(0, 1, &["C"]), Never(&["/"])],
        ),
        (
            "43 Cmaj7 | Am7 pushed",
            Music::new().bass(bars("C2 | A1")).keys(
                [notes("C4 E4 G4 B4", 0, BAR - BEAT / 2), notes("A3 C4 E4 G4", BAR - BEAT / 2, BAR + BEAT / 2)]
                    .concat(),
            ),
            vec![
                Writes(0, "Cmaj7", Root),
                Writes(B, "Am7", Root),
                Flags(B, SpanFlags::PUSHED, none),
                HeardFrom(B, 1680),
            ],
        ),
        (
            "44 C | F pushed",
            Music::new()
                .bass([notes("C2", 0, BAR - BEAT / 2), notes("F2", BAR - BEAT / 2, BAR + BEAT / 2)].concat())
                .keys(
                    [notes("C4 E4 G4", 0, BAR - BEAT / 2), notes("F3 A3 C4", BAR - BEAT / 2, BAR + BEAT / 2)].concat(),
                ),
            vec![Writes(0, "C", Root), Writes(B, "F", Root), Flags(B, SpanFlags::PUSHED, none), HeardFrom(B, 1680)],
        ),
        (
            "45 C | Am, A5 anticipated",
            keys_bass("C2 | A1", "C4 E4 G4 | A3 C4 E4").tune(notes("A5", BAR - BEAT / 2, BAR / 2)),
            vec![Writes(0, "C", Root), Writes(B, "Am", Root), Flags(B, none, SpanFlags::PUSHED), HeardFrom(B, 1920)],
        ),
        (
            "47 G, E5 a neighbour",
            Music::new()
                .bass(bars("G2"))
                .keys(bars("G3 B3 D4"))
                .tune([notes("D5", 0, 3 * BEAT), line("E5 D5", 3 * BEAT, BEAT / 2)].concat()),
            vec![Writes(0, "G", Root), Never(&["6"])],
        ),
        ("48a Cmaj9, D5 held", cmaj7(bars("D5")), vec![Writes(0, "Cmaj9", Root)]),
        ("48b Cmaj7, F5 held", cmaj7(bars("F5")), vec![Writes(0, "Cmaj7", Root), Never(&["sus", "11", "F"])]),
        ("48c Cmaj7, C5 D5 E5", cmaj7(line("C5 D5 E5", 0, BEAT / 2)), vec![Writes(0, "Cmaj7", Root)]),
        ("49 Cmaj9 keys alone", keys("C2 G2 C3 E4 G4 B4 D5"), vec![Writes(0, "Cmaj9", Root)]),
        ("50 Cmaj9/E keys alone", keys("E2 E3 C4 G4 B4 D5"), vec![Among(0, 2, &["Cmaj9/E"])]),
        (
            "51 Cmaj7 | Fmaj7, B4 held over",
            Music::new()
                .bass(bars("C2 | F2"))
                .keys([notes("C4 E4 G4 B4", 0, BAR + BEAT / 2), notes("F4 A4 C5 E5", BAR, BAR)].concat()),
            vec![Writes(0, "Cmaj7", Root), Writes(B, "Fmaj7", Root), Never(&["#11"])],
        ),
        (
            "52 C | F, C E G rung on",
            Music::new()
                .bass(bars("C2 | F2"))
                .keys([notes("C4 E4 G4", 0, BAR + BEAT), notes("F3 A3 C4", BAR, BAR)].concat()),
            vec![Writes(0, "C", Root), Writes(B, "F", Root)],
        ),
        (
            "54 C | G | Am | F with a tune",
            keys_bass("C2 | G1 | A1 | F1", "C4 E4 G4 | B3 D4 G4 | C4 E4 A4 | C4 F4 A4").tune(line(
                "E5 D5 C5 D5 D5 C5 B4 C5 C5 B4 A4 B4 A4 G4 F4 G4",
                0,
                BEAT,
            )),
            vec![Writes(0, "C", Root), Writes(B, "G", Root), Writes(2 * B, "Am", Root), Writes(3 * B, "F", Root)],
        ),
        (
            "53 C | G7 in 3/4",
            Music::new()
                .grid(3, 480)
                .bass([notes("C2", 0, 1440), notes("G2", 1440, 1440)].concat())
                .keys([notes("C4 E4 G4", 0, 1440), notes("F4 B4 D5", 1440, 1440)].concat()),
            vec![WritesAt(0, "C"), WritesAt(1440, "G7")],
        ),
        (
            "53 Ab | Bbm7 in 12/8",
            Music::new()
                .grid(12, 240)
                .bass([notes("Ab2", 0, 1440), notes("Bb2", 1440, 1440)].concat())
                .keys([notes("Ab3 C4 Eb4", 0, 1440), notes("Ab3 Db4 F4", 1440, 1440)].concat()),
            vec![WritesAt(0, "Ab"), WritesAt(1440, "Bbm7")],
        ),
        (
            "55 Dm7 | Db9 | Cmaj7",
            keys_bass("D2 | Db2 | C2", "F3 A3 C4 | F3 B3 Eb4 | E3 G3 B3"),
            vec![Writes(0, "Dm7", Root), Writes(B, "Db9", Root), Writes(2 * B, "Cmaj7", Root)],
        ),
        (
            "h6 Cmaj9 | Am11 | Dm9 | G13 in sixteenths over eighths",
            dense_on(&w, 4),
            vec![
                Cuts(&[0, B, 2 * B, 3 * B]),
                Writes(0, "Cmaj9", Moving),
                Writes(B, "Am11", Moving),
                Writes(2 * B, "Dm9", Moving),
                Writes(3 * B, "G13", Moving),
            ],
        ),
    ];
    w.done()?;
    Ok(cases)
}

/// The design's targets, and its gates the decode doesn't meet yet (work for the chord fit), named: a name, the music
/// (read as a loop), and what it would be named. `show_the_targets` prints which are met.
pub fn named_targets() -> Result<Vec<NamedCase>, String> {
    use BassKind::{Inversion, Moving, Root, Slash};
    use NameWant::*;
    let w = Writer::new();
    let notes = |n: &str, s: i64, l: i64| w.notes(n, s, l);
    let line = |n: &str, s: i64, l: i64| w.line(n, s, l);
    let bars = |t: &str| w.bars(t);
    let keys_bass = |bass: &str, keys: &str| Music::new().bass(bars(bass)).keys(bars(keys));
    let keys = |keys: &str| Music::new().keys(bars(keys));
    let no_bass = BassKind::None;
    let rootless = || keys("F4 A4 C5 E5 | F4 A4 B4 E5 | E4 G4 B4 D5 | E4 G4 B4 D5");
    let cases = vec![
        (
            "12 G, not yet",
            rootless(),
            vec![
                Among(0, 2, &["Fmaj7", "Dm9"]),
                Among(B, 3, &["G13", "Fmaj7(#11)"]),
                Among(2 * B, 2, &["Em7", "Cmaj9"]),
            ],
        ),
        (
            "12 T",
            rootless(),
            vec![Writes(0, "Dm9", no_bass), Writes(B, "G13", no_bass), Writes(2 * B, "Cmaj9", no_bass)],
        ),
        ("13 G, not yet", keys("B2 E3 F3 A3"), vec![Among(0, 3, &["G13"]), NeverOver("B")]),
        ("15e G*, not yet", keys_bass("C2", "E3 Bb3 Gb4 Bb4 Db5"), vec![Intervals(0, &[0, 1, 4, 6, 10])]),
        ("20 G, not yet", keys("F3 B3 Eb4 Bb4"), vec![Among(0, 3, &["G7alt", "Db13"])]),
        ("20 T", keys("F3 B3 Eb4 Bb4"), vec![Among(0, 2, &["G7alt", "Db13"])]),
        (
            "21 T: F#m7b5 before B7",
            Music::new().keys(bars("C4 E4 F#4 A4 | D#4 F#4 A4")).bass(bars("| B1")),
            vec![Writes(0, "F#m7b5", no_bass)],
        ),
        (
            "22 T: Am7 before D7",
            Music::new().keys(bars("C4 E4 G4 A4 | C4 F#4 A4")).bass(bars("| D2")),
            vec![Writes(0, "Am7", no_bass)],
        ),
        ("23 T: Cmaj9/E first", keys_bass("E2", "G3 B3 C4 D4"), vec![Writes(0, "Cmaj9/E", Inversion)]),
        (
            "36 T: Dm7",
            Music::new().bass(line("D2 E2 F2 G2", 0, BEAT)).keys(bars("F3 A3 C4")),
            vec![Writes(0, "Dm7", Moving)],
        ),
        (
            "39 T: bar 1 Cmaj7",
            Music::new().bass(line("C2 E2 G2 Ab2 A2", 0, BEAT)).keys(bars("E4 G4 B4 | C4 E4 G4")),
            vec![Writes(0, "Cmaj7", Moving)],
        ),
        (
            "41 T: Cadd9 arpeggiated",
            Music::new().keys(line("C4 E4 G4 D5 C4 E4 G4 D5", 0, BEAT / 2)),
            vec![Cuts(&[0]), Writes(0, "Cadd9", no_bass)],
        ),
        (
            "46 T: Fmaj7 pushed from the loop's end | G7",
            Music::new()
                .bass([notes("G2", 960, 2640), notes("F2", 3600, 240)].concat())
                .keys([notes("F4 B4 D5", 960, 2640), notes("E4 A4 C5", 3600, 240)].concat()),
            vec![Writes(0, "Fmaj7", Root), Flags(0, SpanFlags::PUSHED, SpanFlags::NONE), Writes(4, "G7", Root)],
        ),
        ("50 T: Cmaj9/E first", keys("E2 E3 C4 G4 B4 D5"), vec![Writes(0, "Cmaj9/E", Inversion)]),
        ("56 T: Eb/E", keys_bass("E2", "Eb4 G4 Bb4"), vec![Writes(0, "Eb/E", Slash)]),
    ];
    w.done()?;
    Ok(cases)
}
