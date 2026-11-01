//! Tests of the namer ([`super::name`]): the design's hand cases read whole and named as a player writes them, and the
//! rules that hold for any music (a name never claims a note that isn't played, the bass kind fits the name, the
//! readings' chances add up to at most 1, the same notes moved are named the same, moved, and the same music at another
//! resolution or in another order is named the same).

use std::ops::Range;

use super::decode_tests::{dense, random_pins, small_music, song, the_music, whole_cases};
use super::hear_tests::{BAR, BEAT, Dice, Music, VELOCITY, line, notes, random_music};
use super::score_tests::{bars, hand_cases};
use super::{
    BassKind, COUNTED, Decoder, Decoding, HAND, Hearing, Label, NamedSpan, Namer, Naming, PinnedSpan, Reading,
    SpanFlags,
};
use crate::chord::Chord;
use crate::keyfind::Span as Stretch;
use crate::notes::{Grid, Note, Part, Role};
use crate::pitch::{PcSet, PitchClass};

/// A reading as the tests see it.
#[derive(Clone, Debug, PartialEq)]
struct Written {
    name: String,
    bass: BassKind,
    /// The held bass, when there is one.
    bass_note: Option<String>,
    also: Option<String>,
    roots: Vec<String>,
    intervals: Vec<i32>,
    chance: f64,
    group: u8,
    rootless: bool,
    doubtful: Vec<u8>,
}

/// A span named, with its units counted from the first unit reported.
#[derive(Clone, Debug, PartialEq)]
struct Span {
    units: Range<usize>,
    flags: SpanFlags,
    rest: bool,
    start: i64,
    heard_start: i64,
    readings: Vec<Written>,
}

impl Span {
    /// What it's written as: its first reading's name, or N.
    fn name(&self) -> &str {
        self.readings.first().map_or("N", |r| r.name.as_str())
    }
}

/// Some music read whole and named.
struct Named {
    hearing: Hearing,
    decoding: Decoding,
    naming: Naming,
}

impl Named {
    fn read(music: &Music, pins: &[PinnedSpan], most: usize) -> Named {
        let hearing = music.hear();
        let (mut decoder, mut decoding, mut naming) = (Decoder::new(), Decoding::new(), Naming::new());
        assert!(decoder.decode(&hearing, &HAND, &COUNTED, pins, &mut decoding), "nothing read");
        Namer::new().name(&hearing, &decoder, &decoding, most, &mut naming);
        Named { hearing, decoding, naming }
    }

    fn spans(&self) -> Vec<Span> {
        let first = self.hearing.reported().start;
        let written = |c: Chord| c.name();
        let span = |s: &NamedSpan| Span {
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
fn render(spans: &[Span]) -> String {
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
    let span = |s: &Span| {
        let readings: Vec<String> = s.readings.iter().map(reading).collect();
        let rest = if s.rest { " rest" } else { "" };
        format!("  {:?}{rest}{}: {}", s.units, flags(s.flags), readings.join(" | "))
    };
    spans.iter().map(span).collect::<Vec<_>>().join("\n")
}

/// What a case named must be, with units counted from the first unit reported.
#[derive(Clone, Debug)]
enum Want {
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
fn holds(want: &Want, spans: &[Span]) -> Result<(), String> {
    let at = |unit: &usize| spans.iter().find(|s| s.units.start == *unit).ok_or(format!("no span at {unit}"));
    let first = |unit: &usize| at(unit).and_then(|s| s.readings.first().ok_or(format!("no reading at {unit}")));
    let check = |yes: bool, why: String| if yes { Ok(()) } else { Err(why) };
    match want {
        Want::Writes(unit, name, bass) => {
            let r = first(unit)?;
            check(r.name == *name && r.bass == *bass, format!("at {unit}: {} {:?}", r.name, r.bass))
        }
        Want::WritesAt(tick, name) => match spans.iter().find(|s| s.start == *tick) {
            Some(s) => check(s.name() == *name, format!("at tick {tick}: {}", s.name())),
            None => Err(format!("no span at tick {tick}")),
        },
        Want::Also(unit, also) => {
            let r = first(unit)?;
            check(r.also.as_deref() == *also, format!("at {unit}: {} also {:?}", r.name, r.also))
        }
        Want::Cuts(units) => {
            let starts: Vec<usize> = spans.iter().map(|s| s.units.start).collect();
            check(starts == *units, format!("cut at {starts:?}"))
        }
        Want::Flags(unit, on, off) => {
            let s = at(unit)?;
            check(s.flags.contains(*on) && s.flags.bits() & off.bits() == 0, format!("at {unit}: {:?}", s.flags))
        }
        Want::Among(unit, most, these) => {
            let s = at(unit)?;
            let best: Vec<&str> = s.readings.iter().take(*most).map(|r| r.name.as_str()).collect();
            check(these.iter().all(|t| best.contains(t)), format!("best {most} at {unit}: {best:?}"))
        }
        Want::Never(these) => match spans.iter().find(|s| these.iter().any(|t| s.name().contains(t))) {
            Some(s) => Err(format!("{} at {}", s.name(), s.units.start)),
            None => Ok(()),
        },
        Want::Intervals(unit, intervals) => {
            let r = first(unit)?;
            check(r.intervals == *intervals, format!("at {unit}: {} {:?}", r.name, r.intervals))
        }
        Want::Roots(unit, roots) => {
            let r = first(unit)?;
            check(r.roots == *roots, format!("at {unit}: {} on {:?}", r.name, r.roots))
        }
        Want::Together(unit, a, b) => {
            let s = at(unit)?;
            let group = |name: &str| s.readings.iter().find(|r| r.name == name).map(|r| r.group);
            check(
                group(a).is_some() && group(a) == group(b),
                format!("at {unit}: {a} {:?}, {b} {:?}", group(a), group(b)),
            )
        }
        Want::HeardFrom(unit, tick) => {
            let s = at(unit)?;
            check(s.heard_start == *tick, format!("at {unit}: heard from {}", s.heard_start))
        }
        Want::NeverOver(bass) => {
            let over = |r: &&Written| r.bass_note.as_deref() == Some(*bass);
            match spans.iter().flat_map(|s| s.readings.iter()).find(over) {
                Some(r) => Err(format!("{} over {bass}", r.name)),
                None => Ok(()),
            }
        }
    }
}

/// A bar of 4/4, counted in units.
const B: usize = 8;

/// The design's hand cases, named: a name (from the design's table), the music (read as a loop), and what it must be
/// named.
fn named_cases() -> Vec<(&'static str, Music, Vec<Want>)> {
    use BassKind::{Inversion, Moving, Root, Slash};
    use Want::*;
    let keys_bass = |bass: &str, keys: &str| Music::new().bass(bars(bass)).keys(bars(keys));
    let keys = |keys: &str| Music::new().keys(bars(keys));
    let none = SpanFlags::NONE;
    let ii_v_i = |keys: &str| Music::new().bass(bars("D2 | G2 | C2 | C2")).keys(bars(keys));
    let cmaj7 = |t: Vec<Note>| Music::new().bass(bars("C2")).keys(bars("E3 G3 B3")).tune(t);
    let dominant = |upper: &str| keys_bass("C2", &format!("E3 Bb3 {upper}"));
    vec![
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
            dense(4),
            vec![
                Cuts(&[0, B, 2 * B, 3 * B]),
                Writes(0, "Cmaj9", Moving),
                Writes(B, "Am11", Moving),
                Writes(2 * B, "Dm9", Moving),
                Writes(3 * B, "G13", Moving),
            ],
        ),
    ]
}

/// The design's targets, and its gates the decode doesn't meet yet (work for the chord fit), named: a name, the music
/// (read as a loop), and what it would be named. `show_the_targets` prints which are met.
fn named_targets() -> Vec<(&'static str, Music, Vec<Want>)> {
    use BassKind::{Inversion, Moving, Root, Slash};
    use Want::*;
    let keys_bass = |bass: &str, keys: &str| Music::new().bass(bars(bass)).keys(bars(keys));
    let keys = |keys: &str| Music::new().keys(bars(keys));
    let no_bass = BassKind::None;
    let rootless = || keys("F4 A4 C5 E5 | F4 A4 B4 E5 | E4 G4 B4 D5 | E4 G4 B4 D5");
    vec![
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
    ]
}

#[test]
fn the_hand_cases_are_named_as_a_player_writes_them() {
    let mut failed = Vec::new();
    for (name, music, wants) in named_cases() {
        let spans = Named::read(&music.looped(), &[], 6).spans();
        let wrong: Vec<String> = wants.iter().filter_map(|w| holds(w, &spans).err()).collect();
        if !wrong.is_empty() {
            failed.push(format!("{name}: {}\n{}", wrong.join(", "), render(&spans)));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// The pitch classes of the notes that sound in some units.
fn sounding_in(hearing: &Hearing, units: Range<usize>) -> PcSet {
    let (from, to) = (hearing.offset(units.start), hearing.offset(units.end));
    hearing.notes().iter().filter(|n| n.start < to && n.end > from).map(|n| n.pitch_class()).collect()
}

/// Whether a naming keeps the rules that hold for any music, and if not, why: the spans run from the first unit
/// reported to the last, each from where the one before ends, and a span is cut in two only where the decode read one
/// chord; a rest has no readings, and a chord at most `most`, the one the decode read first; the chances are chances,
/// sorted after the first, adding up to at most 1 with the chance of no chord; no two readings are written the same;
/// a name holds only notes that sound, and its root when it isn't rootless (over silence, it's written in full, as the
/// decode read it); the bass kind fits the name, and is the
/// same kind of bass on every reading of a span; and readings share a same-notes group when they hold the same notes.
fn keeps_the_rules(hearing: &Hearing, decoding: &Decoding, naming: &Naming, most: usize) -> Result<(), String> {
    let reported = hearing.reported();
    let spans = naming.spans();
    let check = |yes: bool, why: &dyn Fn() -> String| if yes { Ok(()) } else { Err(why()) };
    check(spans.first().map(|s| s.units.start) == Some(reported.start), &|| format!("starts {spans:?}"))?;
    check(spans.last().map(|s| s.units.end) == Some(reported.end), &|| format!("ends {spans:?}"))?;
    check(spans.windows(2).all(|w| w[0].units.end == w[1].units.start), &|| format!("gaps {spans:?}"))?;
    for s in spans {
        let readings = naming.readings(s);
        let why = || format!("{s:?}: {readings:?}");
        let decoded = decoding.spans().iter().find(|d| d.units.start <= s.units.start && s.units.end <= d.units.end);
        let Some(decoded) = decoded else { return Err(format!("not within a decoded span: {}", why())) };
        let split = s.flags.contains(SpanFlags::SPLIT);
        check(split || s.units == decoded.units, &why)?;
        check(!s.units.is_empty() && !s.sound.is_empty() && s.sound.end <= reported.end, &why)?;
        check(s.start == hearing.start(s.units.start) && s.end == hearing.start(s.units.end), &why)?;
        check(s.heard_start <= s.start && (s.heard_start == s.start || s.flags.contains(SpanFlags::PUSHED)), &why)?;
        check(s.rest == decoded.rest && (!s.rest || readings.is_empty()), &why)?;
        check(readings.len() <= most.max(1), &why)?;
        check(decoded.rest || decoded.label.is_none() || readings.first().map(|r| r.label) == decoded.label, &why)?;
        let chance = |c: f64| (0.0..=1.0).contains(&c);
        let sum = readings.iter().map(|r| r.chance).sum::<f64>() + s.none;
        check(chance(s.none) && readings.iter().all(|r| chance(r.chance)) && sum <= 1.0 + 1e-9, &why)?;
        check(readings.get(1..).unwrap_or_default().windows(2).all(|w| w[0].chance >= w[1].chance), &why)?;
        let sounding = sounding_in(hearing, s.sound.clone());
        let silent = hearing.heard(s.sound.clone()).is_empty();
        let mut groups: Vec<PcSet> = Vec::new();
        for (i, r) in readings.iter().enumerate() {
            let why = || format!("{} ({r:?}) in {}", r.chord.name(), why());
            let c = r.chord;
            check(!readings.get(..i).unwrap_or_default().iter().any(|o| o.chord == c), &why)?;
            if silent {
                // Over silence, written in full.
                check(c.notes() == r.label.tones() && !r.rootless && r.also.is_none(), &why)?;
            } else {
                check(sounding.contains_all(c.notes().union(r.doubtful)), &why)?;
                check(r.rootless || sounding.contains(c.root()), &why)?;
            }
            check(r.doubtful.intersection(c.notes()).is_empty() && r.roots.contains(c.root()), &why)?;
            let held = match r.bass {
                BassKind::Root => c.bass() == c.root(),
                BassKind::Inversion => c.bass() != c.root() && c.notes().contains(c.bass()),
                BassKind::Slash => !c.notes().contains(c.bass()),
                BassKind::Moving | BassKind::None => {
                    check(c.bass() == c.root(), &why)?;
                    false
                }
            };
            check(!held || sounding.contains(c.bass()), &why)?;
            let kind = |f: SpanFlags| match r.bass {
                BassKind::Moving => f.contains(SpanFlags::WALKING),
                BassKind::None => f.contains(SpanFlags::NO_BASS),
                BassKind::Root | BassKind::Inversion | BassKind::Slash => {
                    !f.contains(SpanFlags::WALKING) && !f.contains(SpanFlags::NO_BASS)
                }
            };
            let first_bass = readings.first().map(|f| (f.bass == BassKind::Moving, f.bass == BassKind::None, f.chord));
            let same_bass = first_bass.is_some_and(|(moving, none, f)| {
                (moving || none || f.bass() == c.bass())
                    && moving == (r.bass == BassKind::Moving)
                    && none == (r.bass == BassKind::None)
            });
            check(kind(s.flags) && same_bass, &why)?;
            let notes = if held { c.notes().with(c.bass()) } else { c.notes() };
            let group = groups.iter().position(|&g| g == notes).unwrap_or(groups.len());
            check(usize::from(r.group) == group, &why)?;
            if group == groups.len() {
                groups.push(notes);
            }
        }
    }
    Ok(())
}

#[test]
fn any_music_with_any_pins_is_named_by_the_rules() {
    let mut dice = Dice(0x51_7cc1_b727_220a);
    let (mut decoder, mut decoding, mut hearing) = (Decoder::new(), Decoding::new(), Hearing::new());
    let (mut namer, mut naming) = (Namer::new(), Naming::new());
    let mut named = 0;
    for round in 0..300 {
        let music = if round % 2 == 0 { random_music(&mut dice) } else { small_music(&mut dice) };
        if !music.hear_into(&mut hearing) {
            continue;
        }
        let pins = random_pins(&mut dice, &hearing);
        let most = dice.pick(&[0, 1, 3, 6]);
        let what = format!("round {round}: {music:?}, pins {pins:?}, most {most}");
        assert!(decoder.decode(&hearing, &HAND, &COUNTED, &pins, &mut decoding), "nothing read: {what}");
        namer.name(&hearing, &decoder, &decoding, most, &mut naming);
        if let Err(why) = keeps_the_rules(&hearing, &decoding, &naming, most) {
            panic!("{why}\n{what}");
        }
        named += 1;
    }
    assert!(named > 250, "{named} pieces named");
}

#[test]
fn the_music_and_the_hand_cases_are_named_by_the_rules() {
    let music = the_music().into_iter().chain(named_cases().into_iter().map(|(_, m, _)| (m.looped(), Vec::new())));
    for (music, pins) in music.chain([(song(), Vec::new())]) {
        for most in [1, 6, 40] {
            let named = Named::read(&music, &pins, most);
            if let Err(why) = keeps_the_rules(&named.hearing, &named.decoding, &named.naming, most) {
                panic!("{why}\n{music:?}");
            }
        }
    }
}

/// Everything a naming holds, as bits, with each chord moved back by `shift` semitones and each tick put back to 480
/// a beat from `ppq`.
fn everything(naming: &Naming, shift: i32, ppq: i64) -> Vec<String> {
    let tick = |t: i64| t * 480 / ppq;
    let back = |c: Chord| c.transposed(-shift);
    let reading = |r: &Reading| {
        let label = Label::new(r.label.root.up(-shift), r.label.core);
        let (roots, doubtful) = (r.roots.transposed(-shift).bits(), r.doubtful.transposed(-shift).bits());
        let (chance, also) = (r.chance.to_bits(), r.also.map(back));
        let (bass, rootless, group) = (r.bass, r.rootless, r.group);
        format!(
            "  {label:?} {:?} {also:?} {roots:x} {bass:?} {rootless} {doubtful:x} {chance:x} {group}",
            back(r.chord)
        )
    };
    let mut all = Vec::new();
    for s in naming.spans() {
        let (units, sound, flags) = (&s.units, &s.sound, s.flags.bits());
        let (start, end, heard) = (tick(s.start), tick(s.end), tick(s.heard_start));
        let (change, none, rest) = (s.change.to_bits(), s.none.to_bits(), s.rest);
        all.push(format!("{units:?} {sound:?} {start} {end} {heard} {change:x} {rest} {none:x} {flags}"));
        all.extend(naming.readings(s).iter().map(reading));
    }
    all
}

#[test]
fn the_same_notes_moved_are_named_the_same_moved() {
    for (music, pins) in the_music() {
        let want = everything(&Named::read(&music, &pins, 6).naming, 0, 480);
        for shift in [-6, -5, -1, 1, 2, 5] {
            let moved = music.map(|n| Note { key: (i32::from(n.key) + shift) as u8, ..n });
            let up = |pin: &PinnedSpan| PinnedSpan {
                units: pin.units.clone(),
                label: pin.label.map(|l| Label::new(l.root.up(shift), l.core)),
            };
            let pins: Vec<PinnedSpan> = pins.iter().map(up).collect();
            assert_eq!(everything(&Named::read(&moved, &pins, 6).naming, shift, 480), want, "moved {shift}");
        }
    }
    // Named as a player names them, moved: C#dim7 before Dm is Ddim7 before Ebm, Bb13sus4 is B13sus4.
    for (case, want) in [("24 C#dim7", "Ddim7"), ("8 Bb13sus4", "B13sus4"), ("3 D/C", "Eb/Db"), ("2 G7/F", "Ab7/Gb")] {
        let (_, music, _) = named_cases().into_iter().find(|(name, _, _)| name.starts_with(case)).unwrap();
        let moved = music.map(|n| Note { key: n.key + 1, ..n }).looped();
        let spans = Named::read(&moved, &[], 6).spans();
        assert_eq!(spans.first().map(Span::name), Some(want), "{case} moved up a semitone:\n{}", render(&spans));
    }
}

#[test]
fn the_same_music_at_another_resolution_or_in_another_order_is_named_the_same() {
    for (music, pins) in the_music() {
        let want = everything(&Named::read(&music, &pins, 6).naming, 0, 480);
        for ppq in [96, 960] {
            let scale = |t: i64| t * ppq / 480;
            let mut other = music.map(|n| Note { start: scale(n.start), length: scale(n.length), ..n });
            other.grid.ticks_per_beat = ppq as u32;
            assert_eq!(everything(&Named::read(&other, &pins, 6).naming, 0, ppq), want, "at {ppq} ticks a beat");
        }
        let mut shuffled = music.clone();
        shuffled.parts.reverse();
        for (_, notes) in &mut shuffled.parts {
            notes.reverse();
            let third = notes.len() / 3;
            notes.rotate_left(third);
        }
        let drums = music.clone().part(Role::Drums, line("C2 D2 C2 D2 F#2 F#2", 0, BEAT / 2));
        let quiet = music.map(|n| Note { velocity: n.velocity / 2, ..n });
        for (what, other) in [("shuffled", shuffled), ("drums", drums), ("quiet", quiet)] {
            assert_eq!(everything(&Named::read(&other, &pins, 6).naming, 0, 480), want, "{what}");
        }
    }
}

#[test]
fn a_namer_kept_and_used_again_names_the_same() {
    let (mut decoder, mut decoding, mut namer, mut naming) =
        (Decoder::new(), Decoding::new(), Namer::new(), Naming::new());
    let music = the_music();
    for (music, pins) in music.iter().chain(music.iter().rev()) {
        let hearing = music.hear();
        assert!(decoder.decode(&hearing, &HAND, &COUNTED, pins, &mut decoding));
        namer.name(&hearing, &decoder, &decoding, 6, &mut naming);
        assert_eq!(everything(&naming, 0, 480), everything(&Named::read(music, pins, 6).naming, 0, 480));
    }
}

#[test]
fn a_reading_is_always_named_and_most_counts_readings() {
    // Am6 and its same-notes readings: the first is the reading the decode read, however few are asked for.
    let music = Music::new().bass(bars("A1")).keys(bars("C4 E4 F#4 A4")).looped();
    for most in [0, 1, 2, 3, 100] {
        let named = Named::read(&music, &[], most);
        let spans = named.spans();
        let names: Vec<&str> = spans.iter().flat_map(|s| s.readings.iter().map(|r| r.name.as_str())).collect();
        assert!(names.len() <= most.max(1) && names.first() == Some(&"Am6"), "most {most}: {names:?}");
    }
    // A pinned chord is named from the pin, however unlikely: Db7 pinned over C E G is written with what's heard of it,
    // rootless.
    let music = Music::new().bass(bars("C2")).keys(bars("C4 E4 G4"));
    let pin = PinnedSpan { units: 0..B, label: Some(Label::new(PitchClass::new(1), super::Core::Dominant7)) };
    let named = Named::read(&music, std::slice::from_ref(&pin), 4);
    let first = named.naming.spans().first().and_then(|s| named.naming.readings(s).first());
    assert!(first.is_some_and(|r| Some(r.label) == pin.label && r.rootless), "{}", render(&named.spans()));
    // Pinned over silence, it's written in full: C | Db7 | F, with nothing played in the second bar.
    let music = Music::new().bass(bars("C2 | | F2")).keys(bars("C4 E4 G4 | | F3 A3 C4"));
    let pin = PinnedSpan { units: B..2 * B, label: Some(Label::new(PitchClass::new(1), super::Core::Dominant7)) };
    let spans = Named::read(&music, std::slice::from_ref(&pin), 4).spans();
    let names: Vec<(&str, bool)> =
        spans.iter().map(|s| (s.name(), s.readings.first().is_some_and(|r| r.rootless))).collect();
    assert_eq!(names, [("C", false), ("Db7", false), ("F", false)], "{}", render(&spans));
}

#[test]
fn damaged_notes_are_named_without_a_panic() {
    let wild = [
        Note { start: i64::MAX - 5, length: i64::MAX, key: 255, velocity: 0 },
        Note { start: i64::MIN, length: -7, key: 127, velocity: u16::MAX },
        Note { start: 0, length: 0, key: 0, velocity: 1 },
        Note { start: 0, length: 480, key: 60, velocity: VELOCITY },
        Note { start: 240, length: 960, key: 64, velocity: VELOCITY },
        Note { start: 10_000_000 * 480, length: 480, key: 60, velocity: VELOCITY },
        Note { start: 10_000_000 * 480 + 240, length: 480, key: 131, velocity: VELOCITY },
    ];
    let (mut hearing, mut decoder, mut decoding) = (Hearing::new(), Decoder::new(), Decoding::new());
    let (mut namer, mut naming) = (Namer::new(), Naming::new());
    for (beats_per_bar, ticks_per_beat) in [(4, 480), (0, 0), (u32::MAX, u32::MAX), (1, u32::MAX)] {
        for bar_line in [0, i64::MIN, i64::MAX] {
            let grid = Grid { ticks_per_beat, beats_per_bar, bar_line };
            for span in [Stretch::Song, Stretch::Loop] {
                let parts = [Part { role: Role::Harmony, notes: &wild }, Part { role: Role::Bass, notes: &wild[3..] }];
                assert!(hearing.hear(&parts, grid, span));
                assert!(decoder.decode(&hearing, &HAND, &COUNTED, &[], &mut decoding));
                namer.name(&hearing, &decoder, &decoding, 4, &mut naming);
                if let Err(why) = keeps_the_rules(&hearing, &decoding, &naming, 4) {
                    panic!("{why}\n{grid:?} {span:?}");
                }
            }
        }
    }
    // Nothing read: nothing named.
    let nothing = Hearing::new();
    assert!(!decoder.decode(&nothing, &HAND, &COUNTED, &[], &mut decoding));
    namer.name(&nothing, &decoder, &decoding, 4, &mut naming);
    assert!(naming.spans().is_empty());
}

#[test]
#[ignore = "prints which of the design's targets, and of its gates the decode doesn't meet yet, are named as wanted"]
fn show_the_targets() {
    let mut met = 0;
    let targets = named_targets();
    for (name, music, wants) in &targets {
        let spans = Named::read(&music.clone().looped(), &[], 6).spans();
        let wrong: Vec<String> = wants.iter().filter_map(|w| holds(w, &spans).err()).collect();
        met += usize::from(wrong.is_empty());
        let verdict = if wrong.is_empty() { "met".to_string() } else { format!("not met: {}", wrong.join(", ")) };
        println!("{name}: {verdict}\n{}", render(&spans));
    }
    println!("{met} of {} met", targets.len());
}

#[test]
#[ignore = "prints the design's hand cases read whole and named"]
fn show_the_cases_named() {
    for (name, music, _) in named_cases() {
        println!("{name}\n{}", render(&Named::read(&music.looped(), &[], 4).spans()));
    }
    for (name, music, _) in whole_cases() {
        println!("{name}\n{}", render(&Named::read(&music.looped(), &[], 4).spans()));
    }
    for (name, music, units, _) in hand_cases() {
        let spans = Named::read(&music.looped(), &[], 4).spans();
        let over: Vec<Span> =
            spans.into_iter().filter(|s| s.units.start < units.end && units.start < s.units.end).collect();
        println!("{name} over {units:?}\n{}", render(&over));
    }
}

#[test]
#[ignore = "times naming a long song and a dense part, once they're read (run with --release)"]
fn show_the_naming_speed() {
    for (what, music) in [("a 64-bar song", song()), ("16 dense bars", dense(16))] {
        let hearing = music.hear();
        let (mut decoder, mut decoding) = (Decoder::new(), Decoding::new());
        assert!(decoder.decode(&hearing, &HAND, &COUNTED, &[], &mut decoding));
        let (mut namer, mut naming) = (Namer::new(), Naming::new());
        let mut times = Vec::new();
        for _ in 0..50 {
            let started = std::time::Instant::now();
            namer.name(&hearing, &decoder, &decoding, 6, &mut naming);
            times.push(started.elapsed());
        }
        times.sort();
        let readings: usize = naming.spans().iter().map(|s| naming.readings(s).len()).sum();
        let chances: usize = decoding.spans().iter().map(|s| decoding.chances(s).len()).sum();
        println!(
            "{what}: {} spans, {chances} chances named into {readings} readings, in {:?} (the middle of 50; {:?} at most)",
            naming.spans().len(),
            times[25],
            times[49]
        );
    }
}
