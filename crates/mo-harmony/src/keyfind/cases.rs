//! The hand cases (the design's "Test cases"): short pieces written out by hand, each with what the finder must say
//! about it (its gate: weights that break a gate aren't shipped) and what it should say once it's good enough (its
//! target, tracked until it's met). The tests read them with the weights built in; `theorybench keyfit` reads them
//! with the weights it fits, and won't write weights that break a gate.
//!
//! Unless a case says otherwise: 4/4 at 480 ticks a beat, read as a loop, every note at half velocity; chords last a
//! bar each, as whole notes in a harmony part with each chord note from middle C up to the B above it, and their bass
//! notes as whole notes in a bass part, from C2 up to the B above it.
//!
//! The design's cases 32 to 34 (every case moved, shuffled, split and doubled; a long song heard bar by bar; damaged
//! notes) are rules that hold for any weights, so they're in the tests.

use crate::chord::Chord;
use crate::key::{Key, Mode};
use crate::notes::{Grid, Note, Part, Role};
use crate::pitch::PitchClass;

use super::colour::of_key;
use super::{Colours, Heard, HintCheck, HintSource, KeyAnswer, KeyReader, KeyReading, Model, Span, Why, read_heard};

/// Ticks in a beat.
const TPB: u32 = 480;
/// Half of MIDI 2.0's velocity range.
const VELOCITY: u16 = 1 << 15;

const C_MAJOR: Key = Key::major(PitchClass::C);
const C_MINOR: Key = Key::minor(PitchClass::C);
const D_MINOR: Key = Key::minor(PitchClass::D);
const EB_MAJOR: Key = Key::major(PitchClass::new(3));
const F_MAJOR: Key = Key::major(PitchClass::F);
const F_MINOR: Key = Key::minor(PitchClass::F);
const G_MAJOR: Key = Key::major(PitchClass::G);
const G_MINOR: Key = Key::minor(PitchClass::G);
const AB_MAJOR: Key = Key::major(PitchClass::new(8));
const A_MINOR: Key = Key::minor(PitchClass::A);
const BB_MAJOR: Key = Key::major(PitchClass::new(10));

/// One hand case's notes.
#[derive(Clone, Debug)]
pub struct HandCase {
    /// The design's number, with a letter for each piece of a case that has more than one ("3a"), or what was done to
    /// another case ("4x2").
    pub name: &'static str,
    /// What it is, in a few words.
    pub about: &'static str,
    pub parts: Vec<(Role, Vec<Note>)>,
    pub grid: Grid,
    pub span: Span,
}

impl HandCase {
    pub fn parts(&self) -> Vec<Part<'_>> {
        self.parts.iter().map(|(role, notes)| Part { role: *role, notes }).collect()
    }
}

type Parts = Vec<(Role, Vec<Note>)>;

/// A note `beats` long from `beat`.
fn note(beat: f64, beats: f64, key: u8) -> Note {
    let ticks = |b: f64| (b * f64::from(TPB)).round() as i64;
    Note { start: ticks(beat), length: ticks(beats), key, velocity: VELOCITY }
}

/// A bar of chords: a chord by name (voiced as the module says), or voiced by hand (a bass note, or none, under
/// notes given as MIDI keys).
#[derive(Clone, Copy, Debug)]
enum Bar {
    Named(&'static str),
    Voiced(Option<u8>, &'static [u8]),
}

/// Chords a bar each: a harmony part, and a bass part when any bar has a bass.
fn bars(bars: &[Bar]) -> Result<Parts, String> {
    let (mut harmony, mut bass) = (Vec::new(), Vec::new());
    for (i, bar) in bars.iter().enumerate() {
        let at = 4.0 * i as f64;
        match *bar {
            Bar::Named(name) => {
                let chord = Chord::parse(name).ok_or_else(|| format!("{name} isn't a chord"))?;
                harmony.extend(chord.sounding().iter().map(|pc| note(at, 4.0, 60 + pc.number())));
                bass.push(note(at, 4.0, 36 + chord.bass().number()));
            }
            Bar::Voiced(low, keys) => {
                harmony.extend(keys.iter().map(|k| note(at, 4.0, *k)));
                bass.extend(low.map(|k| note(at, 4.0, k)));
            }
        }
    }
    let mut parts = vec![(Role::Harmony, harmony)];
    if !bass.is_empty() {
        parts.push((Role::Bass, bass));
    }
    Ok(parts)
}

/// Chords by name, a bar each.
fn chords(names: &[&'static str]) -> Result<Parts, String> {
    bars(&names.iter().copied().map(Bar::Named).collect::<Vec<_>>())
}

/// Notes one after another from the start, each (MIDI key, beats), in one part.
fn line(role: Role, notes: &[(u8, f64)]) -> Parts {
    let mut at = 0.0;
    let mut out = Vec::with_capacity(notes.len());
    for &(key, beats) in notes {
        out.push(note(at, beats, key));
        at += beats;
    }
    vec![(role, out)]
}

/// Eighth notes one after another from the start, `count` of them, going round `keys`.
fn eighths(role: Role, keys: &[u8], count: usize) -> Parts {
    line(role, &keys.iter().cycle().take(count).map(|k| (*k, 0.5)).collect::<Vec<_>>())
}

/// Notes held together for `beats`, in one part.
fn held(role: Role, keys: &[u8], beats: f64) -> Parts {
    vec![(role, keys.iter().map(|k| note(0.0, beats, *k)).collect())]
}

/// The parts played `times` in a row, `bars` bars each time.
fn repeated(parts: &Parts, bars: i64, times: i64) -> Parts {
    let length = bars * 4 * i64::from(TPB);
    parts
        .iter()
        .map(|(role, notes)| {
            let again = (0..times).flat_map(|t| notes.iter().map(move |n| Note { start: n.start + t * length, ..*n }));
            (*role, again.collect())
        })
        .collect()
}

/// Case 7's tune over the blues: three 4-bar phrases, each three bars of eighths up and down C Eb F Gb G Bb, then a
/// half-note C on the fourth bar's first beat.
fn blues_tune() -> Parts {
    const RUN: [u8; 10] = [60, 63, 65, 66, 67, 70, 67, 66, 65, 63];
    let mut tune = Vec::new();
    for phrase in 0..3 {
        let at = 16.0 * f64::from(phrase);
        tune.extend(RUN.iter().cycle().take(24).enumerate().map(|(i, k)| note(at + 0.5 * i as f64, 0.5, *k)));
        tune.push(note(at + 12.0, 2.0, 60));
    }
    vec![(Role::Melody, tune)]
}

/// Every hand case, in the design's order.
pub fn hand_cases() -> Result<Vec<HandCase>, String> {
    use Role::{Bass, Harmony, Melody};
    let grid = Grid::four_four(TPB);
    let case =
        |name: &'static str, about: &'static str, parts: Parts| HandCase { name, about, parts, grid, span: Span::Loop };
    let first = chords(&["Cm", "Ab", "Eb", "Bb"])?;
    let blues = chords(&["C7", "F7", "C7", "C7", "F7", "F7", "C7", "C7", "G7", "F7", "C7", "G7"])?;
    let alt_g7 = Bar::Voiced(Some(43), &[59, 65, 68, 70, 75]);
    let alt_a7 = Bar::Voiced(Some(45), &[61, 64, 67, 70, 72]);
    let mut with_tune = blues.clone();
    with_tune.extend(blues_tune());
    let melody13 = [
        (0.0, 1.0, 60),
        (1.0, 0.5, 63),
        (1.5, 0.5, 65),
        (2.0, 1.0, 67),
        (3.0, 1.0, 70),
        (4.0, 1.5, 67),
        (5.5, 0.5, 65),
        (6.0, 2.0, 60),
        (8.0, 1.0, 60),
        (9.0, 0.5, 63),
        (9.5, 0.5, 65),
        (10.0, 1.0, 67),
        (11.0, 0.5, 70),
        (11.5, 0.5, 72),
        (12.0, 1.0, 70),
        (13.0, 1.0, 67),
        (14.0, 2.0, 60),
    ];
    let mut melody8 = eighths(Melody, &[60, 63, 65, 66, 67, 70, 67, 65, 63, 60, 63, 65, 66, 65, 63], 15);
    if let Some((_, notes)) = melody8.first_mut() {
        notes.push(note(8.0, 2.0, 60));
    }
    let mut window = case("4w", "case 1 as a stretch of a song", first.clone());
    window.span = Span::Window;
    let mut autumn = case(
        "28",
        "Autumn Leaves as a song: Cm7 F7 Bbmaj7 Ebmaj7 Am7b5 D7 Gm Gm",
        chords(&["Cm7", "F7", "Bbmaj7", "Ebmaj7", "Am7b5", "D7", "Gm", "Gm"])?,
    );
    autumn.span = Span::Song;
    let drums = vec![(Role::Drums, vec![note(0.0, 0.25, 36), note(1.0, 0.25, 38), note(2.0, 0.25, 36)])];
    Ok(vec![
        case("1", "Cm Ab Eb Bb", first.clone()),
        case("2", "Eb Bb Cm Ab", chords(&["Eb", "Bb", "Cm", "Ab"])?),
        case("3a", "Ab Eb Bb Cm", chords(&["Ab", "Eb", "Bb", "Cm"])?),
        case("3b", "Bb Cm Ab Eb", chords(&["Bb", "Cm", "Ab", "Eb"])?),
        case("4x2", "case 1 written out twice", repeated(&first, 4, 2)),
        case("4x4", "case 1 written out four times", repeated(&first, 4, 4)),
        window,
        case("6", "a quick-change blues in C, root-position sevenths", blues),
        case("7", "the blues with a blues-scale tune", with_tune),
        case("8", "a blues-scale tune alone, ending on C", melody8),
        case("9", "Mixolydian vamp: C Bb F C", chords(&["C", "Bb", "F", "C"])?),
        case(
            "10",
            "Dorian: Cm9 for two bars, F9 for two",
            bars(&[
                Bar::Voiced(Some(36), &[63, 67, 70, 74]),
                Bar::Voiced(Some(36), &[63, 67, 70, 74]),
                Bar::Voiced(Some(41), &[57, 63, 67]),
                Bar::Voiced(Some(41), &[57, 63, 67]),
            ])?,
        ),
        case("11", "ii-V vamp: Dm7 G7", chords(&["Dm7", "G7"])?),
        case("12", "Dm7 G7 Cmaj7 Cmaj7", chords(&["Dm7", "G7", "Cmaj7", "Cmaj7"])?),
        case(
            "13",
            "a tune alone in C minor with no sixth",
            vec![(Melody, melody13.iter().map(|&(at, beats, key)| note(at, beats, key)).collect())],
        ),
        case("14", "Dm7 G7alt Cmaj7 A7(b9 #9)", bars(&[Bar::Named("Dm7"), alt_g7, Bar::Named("Cmaj7"), alt_a7])?),
        case("14p", "plain: Dm7 G7 Cmaj7 A7", chords(&["Dm7", "G7", "Cmaj7", "A7"])?),
        case(
            "14r",
            "rich: Dm11 G13 Cmaj9 A7(b9 #9)",
            bars(&[Bar::Named("Dm11"), Bar::Named("G13"), Bar::Named("Cmaj9"), alt_a7])?,
        ),
        case("15", "minor ii-V-i: Dm7b5 G7 Cm Cm", chords(&["Dm7b5", "G7", "Cm", "Cm"])?),
        case("16", "a bass alone: C Ab Eb Bb", line(Bass, &[(36, 4.0), (32, 4.0), (39, 4.0), (34, 4.0)])),
        case(
            "17",
            "a bass alone in G minor, in quarters",
            line(
                Bass,
                &[
                    (43, 1.0),
                    (46, 1.0),
                    (48, 1.0),
                    (50, 1.0),
                    (51, 1.0),
                    (50, 1.0),
                    (48, 1.0),
                    (46, 1.0),
                    (45, 1.0),
                    (48, 1.0),
                    (50, 1.0),
                    (42, 1.0),
                    (43, 4.0),
                ],
            ),
        ),
        case("18", "a bass alone: C C C G in eighths, two bars", eighths(Bass, &[36, 36, 36, 43], 16)),
        case("19", "Cm7 held for four bars, no bass part", held(Harmony, &[48, 63, 67, 70], 16.0)),
        case("20", "Cm7 inverted (Eb G Bb C), held, no bass part", held(Harmony, &[51, 55, 58, 60], 16.0)),
        case("21", "C E G over a C bass, held for four bars", {
            let mut parts = held(Harmony, &[60, 64, 67], 16.0);
            parts.extend(held(Bass, &[36], 16.0));
            parts
        }),
        case("22a", "a power chord, C G C, held", held(Harmony, &[36, 43, 48], 16.0)),
        case("22b", "a C drone on a bass part", held(Bass, &[36], 16.0)),
        case("23a", "whole-tone eighths", eighths(Melody, &[60, 62, 64, 66, 68, 70], 32)),
        case("23b", "octatonic eighths", eighths(Melody, &[60, 61, 63, 64, 66, 67, 69, 70], 32)),
        case("23c", "chromatic eighths", eighths(Melody, &[60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71], 32)),
        case("24a", "Am F C G", chords(&["Am", "F", "C", "G"])?),
        case("24b", "C G Am F", chords(&["C", "G", "Am", "F"])?),
        case("25", "Am7 Dm7 G7 Cmaj7", chords(&["Am7", "Dm7", "G7", "Cmaj7"])?),
        case("26", "Andalusian: Am G F E, twice", chords(&["Am", "G", "F", "E", "Am", "G", "F", "E"])?),
        case("27", "gospel: C C7 F Fm C C", chords(&["C", "C7", "F", "Fm", "C", "C"])?),
        autumn,
        case(
            "29a",
            "rootless, no bass: Dm9 G13 Cmaj9 Cmaj9",
            bars(&[
                Bar::Voiced(None, &[65, 69, 72, 76]),
                Bar::Voiced(None, &[65, 69, 71, 76]),
                Bar::Voiced(None, &[64, 67, 71, 74]),
                Bar::Voiced(None, &[64, 67, 71, 74]),
            ])?,
        ),
        case("29b", "a rootless C9 held, no bass", held(Harmony, &[64, 67, 70, 74], 16.0)),
        case("30", "C D E F G A B held together for a bar", held(Harmony, &[60, 62, 64, 65, 67, 69, 71], 4.0)),
        case("31a", "three notes", line(Melody, &[(60, 1.0), (64, 1.0), (67, 1.0)])),
        case("31b", "drums only", drums),
    ])
}

/// What the finder said about one of the design's cases, against what it must say (the gate) and should say (the
/// target).
#[derive(Clone, Debug)]
pub struct Verdict {
    /// The design's case number.
    pub case: u8,
    /// What its pieces are.
    pub about: String,
    /// What breaks the gate (nothing when it holds); `None` when the case has no gate.
    pub gate: Option<Vec<String>>,
    /// What's short of the target (nothing when it's met); `None` when the case has no target.
    pub target: Option<Vec<String>>,
}

impl Verdict {
    /// Whether the case keeps its gate (one with no gate keeps it).
    pub fn passes_gate(&self) -> bool {
        self.gate.as_ref().is_none_or(Vec::is_empty)
    }

    /// What broke the gate.
    pub fn broken(&self) -> &[String] {
        self.gate.as_deref().unwrap_or(&[])
    }

    /// Whether the case has a target and meets it.
    pub fn meets_target(&self) -> Option<bool> {
        self.target.as_ref().map(Vec::is_empty)
    }
}

/// An answer in words, for a verdict.
pub fn describe(answer: KeyAnswer) -> String {
    let mode = |m: Option<Mode>| match m {
        Some(Mode::Major) => ", leaning major",
        Some(Mode::Minor) => ", leaning minor",
        None => "",
    };
    match answer {
        KeyAnswer::Key(k) => k.name(),
        KeyAnswer::OrRelative(k) => format!("{k} or {}", k.relative()),
        KeyAnswer::HomeNote { tonic, leaning } => {
            // Spelled as the key it leans to would be: "F# minor", so "home note F#".
            format!("home note {}{}", Key { tonic, mode: leaning.unwrap_or(Mode::Major) }.tonic_name(), mode(leaning))
        }
        KeyAnswer::Unclear { why, leaning } => {
            let why = match why {
                Why::TooFewNotes => "too few notes",
                Why::NotTonal => "not in one scale",
                Why::Spread => "spread",
            };
            match leaning {
                Some(k) => format!("no clear key ({why}, leaning {k})"),
                None => format!("no clear key ({why})"),
            }
        }
    }
}

/// What a reading said, for a verdict: its answer and its top two keys.
fn said(r: &KeyReading) -> String {
    let [(k1, p1), (k2, p2), ..] = r.ranked();
    format!("it said {}; top {k1} {p1:.2}, {k2} {p2:.2}", describe(r.answer))
}

fn top(r: &KeyReading) -> Key {
    r.best().0
}

/// Whether the likeliest two keys are `a` and `b`, either way round.
fn top_two(r: &KeyReading, a: Key, b: Key) -> bool {
    let [(k1, _), (k2, _), ..] = r.ranked();
    (k1, k2) == (a, b) || (k1, k2) == (b, a)
}

fn one_key(r: &KeyReading) -> bool {
    matches!(r.answer, KeyAnswer::Key(_))
}

/// The marks one case gets.
#[derive(Default)]
struct Marks {
    gate: Option<Vec<String>>,
    target: Option<Vec<String>>,
}

impl Marks {
    fn gate(&mut self, ok: bool, what: impl FnOnce() -> String) {
        let marks = self.gate.get_or_insert_with(Vec::new);
        if !ok {
            marks.push(what());
        }
    }

    fn target(&mut self, ok: bool, what: impl FnOnce() -> String) {
        let marks = self.target.get_or_insert_with(Vec::new);
        if !ok {
            marks.push(what());
        }
    }
}

/// Every hand case read with one model, and what was heard in it (`None` for a case with nothing pitched).
pub struct Book {
    pub readings: Vec<(HandCase, Option<(Heard, KeyReading)>)>,
}

impl Book {
    /// Reads every hand case with `model`.
    pub fn read(model: &Model) -> Result<Book, String> {
        let mut reader = KeyReader::new();
        let readings = hand_cases()?
            .into_iter()
            .map(|case| {
                let read = reader.hear(&case.parts(), case.grid).map(|heard| {
                    let reading = read_heard(&heard, case.span, model);
                    (heard, reading)
                });
                (case, read)
            })
            .collect();
        Ok(Book { readings })
    }

    fn get(&self, name: &str) -> Option<&(Heard, KeyReading)> {
        self.readings.iter().find(|(c, _)| c.name == name).and_then(|(_, r)| r.as_ref())
    }

    /// The design's cases 1 to 31, each judged.
    pub fn verdicts(&self) -> Vec<Verdict> {
        (1..=31).filter_map(|case| self.verdict(case)).collect()
    }

    /// One case judged; `None` for a number with no case of its own (32 to 34 are in the tests).
    fn verdict(&self, case: u8) -> Option<Verdict> {
        let number = case.to_string();
        let pieces: Vec<&str> = self
            .readings
            .iter()
            .map(|(c, _)| c)
            .filter(|c| c.name.chars().take_while(char::is_ascii_digit).collect::<String>() == number)
            .map(|c| c.about)
            .collect();
        let about = match case {
            5 => "keys given beside cases 1 and 2: a C major key signature, \"loop_Cmaj\", \"Dark Keys Cm 90bpm\""
                .to_owned(),
            _ => pieces.join("; "),
        };
        let mut m = Marks::default();
        // A case whose notes give no reading (only 31b should be one) breaks its gate.
        let reading = |name: &str, m: &mut Marks| {
            let r = self.get(name);
            if r.is_none() {
                m.gate(false, || format!("{name}: no reading"));
            }
            r
        };
        match case {
            1 => {
                if let Some((_, r)) = reading("1", &mut m) {
                    m.gate(top_two(r, C_MINOR, EB_MAJOR), || format!("1: top two C minor and Eb major; {}", said(r)));
                    m.gate(r.answer != KeyAnswer::Key(EB_MAJOR), || format!("1: never Eb major alone; {}", said(r)));
                    for other in ["2", "3a", "3b"] {
                        if let Some((_, o)) = self.get(other) {
                            let (mine, theirs) = (r.chance(C_MINOR), o.chance(C_MINOR));
                            m.gate(mine > theirs, || {
                                format!("1: C minor likelier than in {other} ({mine:.3}, there {theirs:.3})")
                            });
                        }
                    }
                    m.target(top(r) == C_MINOR, || format!("1: C minor on top; {}", said(r)));
                }
            }
            2 => {
                if let Some((_, r)) = reading("2", &mut m) {
                    let [_, (second, _), ..] = r.ranked();
                    m.gate(top(r) == EB_MAJOR && second == C_MINOR, || {
                        format!("2: Eb major on top, C minor second; {}", said(r))
                    });
                    m.gate(matches!(r.answer, KeyAnswer::Key(EB_MAJOR) | KeyAnswer::OrRelative(EB_MAJOR)), || {
                        format!("2: Eb major, or Eb major or C minor; {}", said(r))
                    });
                }
            }
            3 => {
                for name in ["3a", "3b"] {
                    if let Some((_, r)) = reading(name, &mut m) {
                        m.gate(top_two(r, EB_MAJOR, C_MINOR), || {
                            format!("{name}: top two Eb major and C minor; {}", said(r))
                        });
                        m.gate(r.answer != KeyAnswer::Key(C_MINOR), || {
                            format!("{name}: never C minor alone; {}", said(r))
                        });
                        m.gate(![AB_MAJOR, BB_MAJOR].contains(&top(r)), || {
                            format!("{name}: never Ab or Bb major on top; {}", said(r))
                        });
                    }
                }
            }
            4 => {
                if let Some((_, one)) = reading("1", &mut m) {
                    for name in ["4x2", "4x4"] {
                        if let Some((_, r)) = reading(name, &mut m) {
                            let same = r.chances.map(f64::to_bits) == one.chances.map(f64::to_bits);
                            m.gate(same, || format!("{name}: the same chances as case 1, to the last bit"));
                        }
                    }
                    if let Some((_, r)) = reading("4w", &mut m) {
                        m.gate(top_two(r, C_MINOR, EB_MAJOR), || {
                            format!("4w: top two C minor and Eb major; {}", said(r))
                        });
                        let (window, as_loop) = (r.chance(C_MINOR), one.chance(C_MINOR));
                        m.gate(window <= as_loop, || {
                            format!("4w: C minor no likelier than as a loop ({window:.3}, as a loop {as_loop:.3})")
                        });
                    }
                }
            }
            5 => {
                if let (Some((_, two)), Some((_, one))) = (reading("2", &mut m), reading("1", &mut m)) {
                    let event = two.check_hint(C_MAJOR, HintSource::KeySignatureEvent);
                    m.gate(event == HintCheck::Ignored, || {
                        format!("5: a C major key signature is ignored, not {event:?}")
                    });
                    // A name is checked against every key the answer gives: C major is some other key than Eb
                    // major, and the parallel of C minor when the answer gives C minor too ("Eb major or C minor").
                    let named = Key::parse("Cmaj").map(|k| two.check_hint(k, HintSource::Name));
                    let pair = two.answer.keys().contains(&Some(C_MINOR));
                    let expected = if pair { HintCheck::Parallel } else { HintCheck::Disagrees };
                    m.gate(named == Some(expected), || {
                        format!("5: \"loop_Cmaj\" beside case 2 is {expected:?}, not {named:?}; {}", said(two))
                    });
                    let named = Key::parse("Cm").map(|k| one.check_hint(k, HintSource::Name));
                    m.gate(matches!(named, Some(HintCheck::Agrees | HintCheck::InAnswer)), || {
                        format!("5: \"Dark Keys Cm 90bpm\" agrees with case 1, not {named:?}; {}", said(one))
                    });
                }
            }
            6 | 7 => {
                let name = if case == 6 { "6" } else { "7" };
                if let Some((_, r)) = reading(name, &mut m) {
                    if case == 6 {
                        let [(k1, _), (k2, _), ..] = r.ranked();
                        m.gate(k1.tonic == PitchClass::C || k2.tonic == PitchClass::C, || {
                            format!("6: C in the top two; {}", said(r))
                        });
                    } else {
                        m.gate(r.answer != KeyAnswer::Unclear { why: Why::NotTonal, leaning: None }, || {
                            format!("7: in a scale; {}", said(r))
                        });
                    }
                    m.gate(!matches!(r.answer, KeyAnswer::Key(F_MAJOR | F_MINOR)), || {
                        format!("{name}: never F alone; {}", said(r))
                    });
                    let home = matches!(r.answer, KeyAnswer::Key(C_MAJOR))
                        || matches!(r.answer, KeyAnswer::HomeNote { tonic: PitchClass::C, .. });
                    m.target(home && r.colours.has(Colours::BLUES), || {
                        format!("{name}: C major or home note C, coloured blues ({:?}); {}", r.colours, said(r))
                    });
                }
            }
            8 => {
                if let Some((_, r)) = reading("8", &mut m) {
                    m.gate(!matches!(r.answer, KeyAnswer::Unclear { why: Why::NotTonal, .. }), || {
                        format!("8: in a scale (fit {:.2}); {}", r.diatonic_fit, said(r))
                    });
                    m.target(top(r).tonic == PitchClass::C, || format!("8: C on top; {}", said(r)));
                }
            }
            9 => {
                if let Some((_, r)) = reading("9", &mut m) {
                    m.target(top(r) == C_MAJOR && r.colours.has(Colours::MIXOLYDIAN), || {
                        format!("9: C major on top, coloured Mixolydian ({:?}); {}", r.colours, said(r))
                    });
                }
            }
            10 => {
                if let Some((heard, r)) = reading("10", &mut m) {
                    m.gate(top(r) == C_MINOR, || format!("10: C minor on top; {}", said(r)));
                    m.gate(of_key(heard, C_MINOR).has(Colours::DORIAN), || "10: C minor coloured Dorian".to_owned());
                }
            }
            11 => {
                if let Some((heard, r)) = reading("11", &mut m) {
                    m.gate(top(r) != G_MAJOR, || format!("11: never G major on top; {}", said(r)));
                    let dorian = top(r) == D_MINOR && of_key(heard, D_MINOR).has(Colours::DORIAN);
                    m.target(dorian || matches!(r.answer, KeyAnswer::OrRelative(_)), || {
                        format!("11: D minor on top coloured Dorian, or a pair; {}", said(r))
                    });
                    m.target(r.answer != KeyAnswer::Key(C_MAJOR), || format!("11: never C major alone; {}", said(r)));
                }
            }
            12 => {
                if let Some((_, r)) = reading("12", &mut m) {
                    m.gate(top(r) == C_MAJOR, || format!("12: C major on top; {}", said(r)));
                    m.target(r.answer == KeyAnswer::Key(C_MAJOR), || format!("12: C major alone; {}", said(r)));
                }
            }
            13 => {
                if let Some((heard, r)) = reading("13", &mut m) {
                    let [_, (second, _), ..] = r.ranked();
                    m.gate(top(r) == C_MINOR, || format!("13: C minor on top; {}", said(r)));
                    // Second, a key next to C minor whose scale holds all five notes: Eb major has the same notes,
                    // and F minor and G minor are a fifth either side. The design first left F minor out with no
                    // reason given, though C, the tune's long first and last note, is F minor's fifth (2026-11-01).
                    m.gate(second == EB_MAJOR || second == F_MINOR || second == G_MINOR, || {
                        format!("13: Eb major, F minor or G minor second; {}", said(r))
                    });
                    m.gate(of_key(heard, C_MINOR).has(Colours::NO_SIXTH), || "13: C minor with no sixth".to_owned());
                    m.target(second == EB_MAJOR, || format!("13: Eb major, the same five notes, second; {}", said(r)));
                    m.target(matches!(r.answer, KeyAnswer::Key(C_MINOR) | KeyAnswer::OrRelative(C_MINOR)), || {
                        format!("13: C minor, or C minor or Eb major; {}", said(r))
                    });
                }
            }
            14 => {
                for name in ["14", "14p", "14r"] {
                    if let Some((_, r)) = reading(name, &mut m) {
                        m.gate(top(r) == C_MAJOR || top(r) == D_MINOR, || {
                            format!("{name}: C major or D minor on top; {}", said(r))
                        });
                        m.target(top(r) == C_MAJOR, || format!("{name}: C major on top; {}", said(r)));
                    }
                }
                if let (Some((_, alt)), Some((_, plain))) = (self.get("14"), self.get("14p")) {
                    let (a, p) = (alt.chance(C_MAJOR), plain.chance(C_MAJOR));
                    m.gate(a >= p - 0.15, || {
                        format!("14: altered chords cost C major at most 0.15 ({a:.3}, plain {p:.3})")
                    });
                }
            }
            15 => {
                if let Some((heard, r)) = reading("15", &mut m) {
                    m.gate(top(r) == C_MINOR, || format!("15: C minor on top; {}", said(r)));
                    m.gate(of_key(heard, C_MINOR).has(Colours::HARMONIC_MINOR), || {
                        "15: C minor coloured harmonic minor".to_owned()
                    });
                }
            }
            16 => {
                if let Some((_, r)) = reading("16", &mut m) {
                    m.gate(top_two(r, C_MINOR, EB_MAJOR), || format!("16: top two C minor and Eb major; {}", said(r)));
                    m.gate(!one_key(r), || format!("16: no one key from a bass alone; {}", said(r)));
                    m.target(top(r) == C_MINOR, || format!("16: C minor on top; {}", said(r)));
                }
            }
            17 => {
                if let Some((_, r)) = reading("17", &mut m) {
                    m.target(top(r) == G_MINOR, || format!("17: G minor on top; {}", said(r)));
                }
            }
            18 => {
                if let Some((_, r)) = reading("18", &mut m) {
                    m.gate(top(r).tonic == PitchClass::C, || format!("18: C on top; {}", said(r)));
                    let ok = matches!(
                        r.answer,
                        KeyAnswer::HomeNote { tonic: PitchClass::C, .. } | KeyAnswer::Unclear { why: Why::Spread, .. }
                    );
                    m.gate(ok, || format!("18: home note C, or no clear key; {}", said(r)));
                }
            }
            19 => {
                if let Some((heard, r)) = reading("19", &mut m) {
                    m.gate(top(r) == C_MINOR, || format!("19: C minor on top; {}", said(r)));
                    m.gate(r.bars_heard == 1, || format!("19: one bar heard, not {}", r.bars_heard));
                    m.gate(r.answer != KeyAnswer::Key(EB_MAJOR), || format!("19: never Eb major alone; {}", said(r)));
                    m.gate(of_key(heard, C_MINOR).has(Colours::NO_SIXTH), || "19: C minor with no sixth".to_owned());
                }
            }
            20 => {
                if let Some((_, r)) = reading("20", &mut m) {
                    m.gate(top_two(r, C_MINOR, EB_MAJOR), || format!("20: top two C minor and Eb major; {}", said(r)));
                    m.gate(!one_key(r), || format!("20: no one key; {}", said(r)));
                }
            }
            21 => {
                if let Some((_, r)) = reading("21", &mut m) {
                    m.gate(top(r) == C_MAJOR, || format!("21: C major on top; {}", said(r)));
                    m.gate(!matches!(r.answer, KeyAnswer::Key(k) if k != C_MAJOR), || {
                        format!("21: no key but C major alone; {}", said(r))
                    });
                }
            }
            22 => {
                for name in ["22a", "22b"] {
                    if let Some((_, r)) = reading(name, &mut m) {
                        m.gate(top(r).tonic == PitchClass::C, || format!("{name}: C on top; {}", said(r)));
                        let ok = matches!(
                            r.answer,
                            KeyAnswer::HomeNote { tonic: PitchClass::C, .. } | KeyAnswer::Unclear { .. }
                        );
                        m.gate(ok, || format!("{name}: home note C, or no clear key; {}", said(r)));
                    }
                }
            }
            23 => {
                for name in ["23a", "23b", "23c"] {
                    if let Some((_, r)) = reading(name, &mut m) {
                        m.gate(r.answer == KeyAnswer::Unclear { why: Why::NotTonal, leaning: None }, || {
                            format!("{name}: no key, not in one scale (fit {:.2}); {}", r.diatonic_fit, said(r))
                        });
                    }
                }
            }
            24 => {
                if let Some((_, r)) = reading("24a", &mut m) {
                    m.gate(top_two(r, A_MINOR, C_MAJOR), || format!("24a: top two A minor and C major; {}", said(r)));
                    m.gate(r.answer != KeyAnswer::Key(C_MAJOR), || format!("24a: never C major alone; {}", said(r)));
                    m.target(top(r) == A_MINOR, || format!("24a: A minor on top; {}", said(r)));
                }
                if let Some((_, r)) = reading("24b", &mut m) {
                    m.gate(top(r) == C_MAJOR, || format!("24b: C major on top; {}", said(r)));
                }
            }
            25 => {
                if let Some((_, r)) = reading("25", &mut m) {
                    m.gate(top_two(r, C_MAJOR, A_MINOR), || format!("25: top two C major and A minor; {}", said(r)));
                    m.gate(r.best().1 < 0.9, || format!("25: under 0.9 sure; {}", said(r)));
                }
            }
            26 => {
                if let Some((heard, r)) = reading("26", &mut m) {
                    m.gate(top(r) == A_MINOR, || format!("26: A minor on top; {}", said(r)));
                    m.gate(of_key(heard, A_MINOR).has(Colours::HARMONIC_MINOR), || {
                        "26: A minor coloured harmonic minor".to_owned()
                    });
                }
            }
            27 => {
                if let Some((heard, r)) = reading("27", &mut m) {
                    m.gate(top(r) == C_MAJOR, || format!("27: C major on top; {}", said(r)));
                    let colours = of_key(heard, C_MAJOR);
                    m.gate(colours.has(Colours::MIXOLYDIAN) && colours.has(Colours::I7), || {
                        format!("27: C major coloured Mixolydian with C7 as home ({colours:?})")
                    });
                }
            }
            28 => {
                if let Some((_, r)) = reading("28", &mut m) {
                    m.gate(top(r) == G_MINOR, || format!("28: G minor on top; {}", said(r)));
                }
            }
            29 => {
                if let Some((_, r)) = reading("29a", &mut m) {
                    m.gate(top(r) == C_MAJOR || top(r) == A_MINOR, || {
                        format!("29a: C major or A minor on top; {}", said(r))
                    });
                    m.gate(!one_key(r), || format!("29a: no one key; {}", said(r)));
                }
                if let Some((_, r)) = reading("29b", &mut m) {
                    m.gate(!one_key(r), || format!("29b: no one key; {}", said(r)));
                }
            }
            30 => {
                if let Some((_, r)) = reading("30", &mut m) {
                    m.gate(top_two(r, C_MAJOR, A_MINOR), || format!("30: top two C major and A minor; {}", said(r)));
                    m.gate(!one_key(r), || format!("30: no one key; {}", said(r)));
                }
            }
            31 => {
                if let Some((_, r)) = reading("31a", &mut m) {
                    m.gate(r.answer == KeyAnswer::Unclear { why: Why::TooFewNotes, leaning: None }, || {
                        format!("31a: too few notes; {}", said(r))
                    });
                }
                m.gate(self.get("31b").is_none(), || "31b: drums alone give no reading".to_owned());
            }
            _ => return None,
        }
        Some(Verdict { case, about, gate: m.gate, target: m.target })
    }
}
