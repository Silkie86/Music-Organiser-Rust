//! Tests of the decode ([`super::decode`]): the design's hand cases read whole, rests, pins, pushes, the chances, and
//! the rules that hold for any music (moved, at another resolution, in another order, read again). Some tools, left
//! out of the run, print the cases read whole, count the cases other weights fail, explain what ways of reading a case
//! score, and time a long song.

use std::ops::Range;

use super::hear_tests::{BAR, BEAT, Music, line, notes};
use super::score_tests::{Want, bars, facts_on, hand_cases, label, ranking_by, reads, samples};
use super::{
    COUNTED, Chance, Decoder, Decoding, Evidence, Feature, HAND, Hearing, Label, PerFeature, PinnedSpan, SpanFacts,
    Weights,
};
use crate::notes::{Note, Role};

/// Bars of 4/4 counted in units: bar k is units 8k to 8k + 8, and its third beat is unit 8k + 4.
const B: usize = 8;

/// A reading's name: its label, or N for no chord.
fn name(label: Option<Label>) -> String {
    label.map_or_else(|| "N".to_owned(), |l| l.to_string())
}

/// A span read, with its units counted from the first unit reported.
#[derive(Clone, Debug, PartialEq)]
struct Read {
    units: Range<usize>,
    sound: Range<usize>,
    label: String,
    pushed: bool,
    rest: bool,
    change: f64,
    chances: Vec<(String, f64)>,
}

/// Some music read whole: what was heard, the decoder that read it (which gives the chances on any units), and what
/// it read.
struct Whole {
    hearing: Hearing,
    decoder: Decoder,
    out: Decoding,
}

impl Whole {
    fn read(music: &Music, weights: &Weights, pins: &[PinnedSpan]) -> Whole {
        let hearing = music.hear();
        let (mut decoder, mut out) = (Decoder::new(), Decoding::new());
        assert!(decoder.decode(&hearing, weights, &COUNTED, pins, &mut out), "nothing read");
        Whole { hearing, decoder, out }
    }

    /// The first unit reported: units in the checks count from it.
    fn first(&self) -> usize {
        self.hearing.reported().start
    }

    fn spans(&self) -> Vec<Read> {
        let first = self.first();
        let from_first = |r: &Range<usize>| r.start - first..r.end - first;
        let read = |s: &super::DecodedSpan| Read {
            units: from_first(&s.units),
            sound: from_first(&s.sound),
            label: name(s.label),
            pushed: s.pushed,
            rest: s.rest,
            change: s.change,
            chances: self.out.chances(s).iter().map(|c| (name(c.label), c.chance)).collect(),
        };
        self.out.spans().iter().map(read).collect()
    }

    /// Each reading's chance over some units (counted from the first unit reported), averaged, best first.
    fn chances(&self, units: Range<usize>) -> Vec<(String, f64)> {
        let first = self.first();
        let mut out = Vec::new();
        self.decoder.chances(&self.hearing, first + units.start..first + units.end, &mut out);
        out.iter().map(|c| (name(c.label), c.chance)).collect()
    }
}

/// The spans read, as text: each one's first unit, its reading (^ when it's pushed, "rest" for a rest) and the chance
/// a chord changes there.
fn render(spans: &[Read]) -> String {
    let span = |r: &Read| {
        let pushed = if r.pushed { "^" } else { "" };
        let rest = if r.rest { " rest" } else { "" };
        format!("{}:{}{pushed}{rest} ({:.2})", r.units.start, r.label, r.change)
    };
    spans.iter().map(span).collect::<Vec<_>>().join("  ")
}

/// The spans read, as their units and readings.
fn cuts(spans: &[Read]) -> Vec<(Range<usize>, &str)> {
    spans.iter().map(|r| (r.units.clone(), r.label.as_str())).collect()
}

/// What a case read whole must read, with units counted from the first unit reported.
#[derive(Clone, Debug)]
enum Check {
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
fn holds(check: &Check, whole: &Whole, spans: &[Read]) -> Result<(), String> {
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

/// The design's hand cases that only a whole read decides (where the chords change, pushes, long chords), as loops: a
/// name, the music, and what it must read. 12, 13 and 20 have no bass: the design wants their rootless readings among
/// the best two or three, which no weights reach yet (harmony/design/chord-reader-design.md, "As built"); for now
/// they must be kept.
fn whole_cases() -> Vec<(&'static str, Music, Vec<Check>)> {
    use Check::*;
    let keys_bass = |bass: &str, keys: &str| Music::new().bass(bars(bass)).keys(bars(keys));
    let keys = |keys: &str| Music::new().keys(bars(keys));
    let struck = |bass: &str, keys: &str, n: usize| keys_bass(&vec![bass; n].join(" | "), &vec![keys; n].join(" | "));
    vec![
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
            dense(4),
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
    ]
}

/// A dense gospel part, so many bars long: Cmaj9, Am11, Dm9 and G13 by turns, a bar each, the chords struck in
/// sixteenths over a bass moving in eighths.
fn dense(bars: i64) -> Music {
    let (mut bass, mut keys) = (Vec::new(), Vec::new());
    let each = [
        ("C2 G2 C3 G2 E2 G2 C3 B2", "E3 G3 B3 D4"),
        ("A1 E2 A2 E2 G2 E2 A2 G2", "G3 C4 D4 E4"),
        ("D2 A2 D3 A2 F2 A2 C3 A2", "F3 A3 C4 E4"),
        ("G1 D2 G2 D2 F2 D2 B2 D2", "F3 A3 B3 E4"),
    ];
    for bar in 0..bars {
        let (walk, chord) = each[bar as usize % 4];
        bass.extend(line(walk, bar * BAR, BEAT / 2));
        keys.extend((0..16).flat_map(|k| notes(chord, bar * BAR + k * BEAT / 4, BEAT / 4)));
    }
    Music::new().bass(bass).keys(keys)
}

/// The design's hand cases that one stretch read on its own decides, read whole as loops: what's read over the stretch
/// must be a first reading the case allows, its best two readings by their chances the two it gives, and readings it
/// keeps must have a chance.
fn stretch_cases() -> Vec<(String, Music, Vec<Check>)> {
    let check = |units: Range<usize>, want: Want| match want {
        Want::First(these) => Check::Reads(units, these),
        Want::TopTwo(these) => Check::Top(units, 2, these),
        Want::Kept(these) => Check::Kept(units, these),
    };
    let cases = hand_cases().into_iter();
    cases.map(|(name, music, units, want)| (format!("w{name}"), music, vec![check(units, want)])).collect()
}

/// Every case read whole: those only a whole read decides, and the hand cases read one stretch at a time.
fn all_cases() -> Vec<(String, Music, Vec<Check>)> {
    let whole = whole_cases().into_iter().map(|(name, music, checks)| (name.to_owned(), music, checks));
    whole.chain(stretch_cases()).collect()
}

/// The cases some weights read wrongly, as loops: each one's name, what fails, and what was read.
fn failures(cases: &[(String, Music, Vec<Check>)], weights: &Weights) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (name, music, checks) in cases {
        let whole = Whole::read(&music.clone().looped(), weights, &[]);
        let spans = whole.spans();
        let failed: Vec<String> = checks.iter().filter_map(|c| holds(c, &whole, &spans).err()).collect();
        if !failed.is_empty() {
            out.push((name.clone(), format!("{}; read {}", failed.join(", "), render(&spans))));
        }
    }
    out
}

#[test]
fn the_cases_only_a_whole_read_decides_read_as_a_player_reads_them() {
    let cases: Vec<_> =
        whole_cases().into_iter().map(|(name, music, checks)| (name.to_owned(), music, checks)).collect();
    let failed = failures(&cases, &HAND);
    assert!(failed.is_empty(), "{failed:#?}");
}

#[test]
fn the_hand_cases_read_whole_read_as_they_do_one_stretch_at_a_time() {
    let failed = failures(&stretch_cases(), &HAND);
    assert!(failed.is_empty(), "{failed:#?}");
}

#[test]
fn a_bar_or_more_of_silence_is_a_rest_and_less_belongs_to_the_chords_around_it() {
    // C, a bar or three of silence, F: the silence is one rest, read as no chord, and a chord changes for sure at its
    // edges.
    for gap in [1, 3] {
        let after = (1 + gap as i64) * BAR;
        let music = Music::new()
            .bass([notes("C2", 0, BAR), notes("F2", after, BAR)].concat())
            .keys([notes("C4 E4 G4", 0, BAR), notes("F3 A3 C4", after, BAR)].concat());
        let whole = Whole::read(&music, &HAND, &[]);
        let spans = whole.spans();
        let rest = B..(1 + gap) * B;
        assert_eq!(cuts(&spans), [(0..B, "C"), (rest.clone(), "N"), (rest.end..rest.end + B, "F")], "{gap} bars");
        let rests: Vec<bool> = spans.iter().map(|r| r.rest).collect();
        assert_eq!(rests, [false, true, false]);
        assert_eq!(spans[1].chances.len(), 1);
        assert!(spans[1].chances[0].0 == "N" && (spans[1].chances[0].1 - 1.0).abs() < 1e-9);
        assert!((spans[1].change - 1.0).abs() < 1e-9 && (spans[2].change - 1.0).abs() < 1e-9);
    }
    // Half a bar of silence belongs to the C before it.
    let music = Music::new()
        .bass([notes("C2", 0, 2 * BEAT), notes("F2", BAR, BAR)].concat())
        .keys([notes("C4 E4 G4", 0, 2 * BEAT), notes("F3 A3 C4", BAR, BAR)].concat());
    let spans = Whole::read(&music, &HAND, &[]).spans();
    assert_eq!(cuts(&spans), [(0..B, "C"), (B..2 * B, "F")]);
    assert!(spans.iter().all(|r| !r.rest));
}

#[test]
fn silence_before_the_music_starts_is_no_chord() {
    // A song is heard from the bar line before its first note: here half a bar, or three beats, before it. The
    // silence is too short to be a rest, but no chord sounds there.
    for (beats, unit) in [(2, 4), (3, 6)] {
        let start = beats * BEAT;
        let music = Music::new()
            .bass([notes("C2", start, 2 * BAR - start), notes("F2", 2 * BAR, BAR)].concat())
            .keys([notes("C4 E4 G4", start, 2 * BAR - start), notes("F3 A3 C4", 2 * BAR, BAR)].concat());
        let spans = Whole::read(&music, &HAND, &[]).spans();
        assert_eq!(cuts(&spans), [(0..unit, "N"), (unit..2 * B, "C"), (2 * B..3 * B, "F")]);
        assert!(!spans[0].rest);
    }
}

/// A ii V I in C as a song: Dm9, G13 and two bars of Cmaj9.
fn two_five_one() -> Music {
    Music::new().bass(bars("D2 | G2 | C2 | C2")).keys(bars("F3 A3 C4 E4 | F3 A3 B3 E4 | E3 G3 B3 D4 | E3 G3 B3 D4"))
}

/// A stretch pinned to a chord.
fn pin(units: Range<usize>, chord: &str) -> PinnedSpan {
    PinnedSpan { units, label: Some(label(chord)) }
}

#[test]
fn a_pinned_chord_is_read_as_given() {
    let music = two_five_one();
    let read = |pins: &[PinnedSpan]| Whole::read(&music, &HAND, pins).spans();
    let free = read(&[]);
    assert_eq!(cuts(&free), [(0..B, "Dm7"), (B..2 * B, "G7"), (2 * B..4 * B, "Cmaj7")]);
    // The tritone's substitute, given for the second bar: read as given, with no other reading, and a chord changes
    // for sure where it starts and where it ends.
    let given = read(&[pin(B..2 * B, "Db7")]);
    assert_eq!(cuts(&given), [(0..B, "Dm7"), (B..2 * B, "Db7"), (2 * B..4 * B, "Cmaj7")]);
    assert_eq!(given[1].chances.len(), 1);
    assert!(given[1].chances[0].0 == "Db7" && (given[1].chances[0].1 - 1.0).abs() < 1e-9);
    assert!((given[1].change - 1.0).abs() < 1e-9 && (given[2].change - 1.0).abs() < 1e-9);
    // A pin of what's read freely is still reported as given, though the same chord goes on after it.
    let same = read(&[pin(2 * B..3 * B, "Cmaj7")]);
    assert_eq!(cuts(&same), [(0..B, "Dm7"), (B..2 * B, "G7"), (2 * B..3 * B, "Cmaj7"), (3 * B..4 * B, "Cmaj7")]);
    // One that runs past the end is cut short there.
    let past = read(&[pin(3 * B..6 * B, "C6")]);
    assert_eq!(cuts(&past), [(0..B, "Dm7"), (B..2 * B, "G7"), (2 * B..3 * B, "Cmaj7"), (3 * B..4 * B, "C6")]);
    // One that overlaps an earlier one is left out, in whatever order they're given; and so is an empty one.
    for pins in [[pin(B..2 * B, "Db7"), pin(B + 4..3 * B, "Ab7")], [pin(B + 4..3 * B, "Ab7"), pin(B..2 * B, "Db7")]] {
        assert_eq!(cuts(&read(&pins)), cuts(&given));
    }
    assert_eq!(cuts(&read(&[pin(5..5, "E7"), pin(6 * B..7 * B, "E7")])), cuts(&free));
}

#[test]
fn a_stretch_pinned_at_its_ends_is_read_as_one_chord() {
    // C for two beats and Am for two, then F: read freely, the chord changes in the middle of the first bar.
    let music = Music::new()
        .bass(bars("C2 (1-2) A1 (3-4) | F2 | G2 | C2"))
        .keys(bars("C4 E4 G4 (1-2) C4 E4 A4 (3-4) | F3 A3 C4 | G3 B3 D4 | C4 E4 G4"));
    let free = Whole::read(&music, &HAND, &[]).spans();
    assert_eq!(cuts(&free), [(0..4, "C"), (4..B, "Am"), (B..2 * B, "F"), (2 * B..3 * B, "G"), (3 * B..4 * B, "C")]);
    // Pinned at the bar's ends, it's one chord.
    let ends = |units: Range<usize>| PinnedSpan { units, label: None };
    let one = Whole::read(&music, &HAND, &[ends(0..B)]).spans();
    assert_eq!(one[0].units, 0..B, "read {}", render(&one));
    assert!(["C", "C6", "Am", "Am7"].contains(&one[0].label.as_str()), "read {}", render(&one));
    assert_eq!(cuts(&one)[1..], cuts(&free)[2..]);
    // And so over three bars, longer than a span: one chord, held on.
    let long = Whole::read(&music, &HAND, &[ends(B..4 * B)]).spans();
    assert_eq!(long.len(), 3, "read {}", render(&long));
    assert_eq!(long[2].units, B..4 * B);
}

#[test]
fn a_pushed_chords_sound_starts_where_it_is_struck() {
    // C, then F struck half a beat before the bar line and held over it.
    let music = Music::new()
        .bass([notes("C2", 0, BAR - BEAT / 2), notes("F2", BAR - BEAT / 2, BAR + BEAT / 2)].concat())
        .keys([notes("C4 E4 G4", 0, BAR - BEAT / 2), notes("F3 A3 C4", BAR - BEAT / 2, BAR + BEAT / 2)].concat());
    let spans = Whole::read(&music, &HAND, &[]).spans();
    let read: Vec<_> = spans.iter().map(|r| (r.units.clone(), r.sound.clone(), r.label.as_str(), r.pushed)).collect();
    assert_eq!(read, [(0..B, 0..B - 1, "C", false), (B..2 * B, B - 1..2 * B, "F", true)]);
}

/// Music for the rules that hold for any music: the evidence's samples (pushes, loops, a walking bass, a broken bar),
/// some cases only a whole read decides (passing and symmetric chords, a walk, a push, chords struck again quickly), a
/// rest, and pins of both kinds.
fn the_music() -> Vec<(Music, Vec<PinnedSpan>)> {
    let mut out: Vec<(Music, Vec<PinnedSpan>)> = samples().into_iter().map(|m| (m, Vec::new())).collect();
    for (name, music, _) in whole_cases() {
        if ["6", "38", "44", "h4", "h6"].contains(&name.split(' ').next().unwrap_or("")) {
            out.push((music.looped(), Vec::new()));
        }
    }
    let rest = Music::new()
        .bass([notes("C2", BEAT, BAR), notes("F2", 3 * BAR, BAR)].concat())
        .keys([notes("C4 E4 G4", BEAT, BAR), notes("F3 A3 C4", 3 * BAR, BAR)].concat());
    out.push((rest, Vec::new()));
    out.push((two_five_one(), vec![pin(B..2 * B, "Db7"), PinnedSpan { units: 2 * B..4 * B, label: None }]));
    out
}

/// Everything a decode reads, as bits, with each label moved back by `shift` semitones: the spans with their chances,
/// and each unit's chances and the chance a chord changes there.
fn everything(hearing: &Hearing, decoder: &Decoder, out: &Decoding, shift: i32) -> Vec<String> {
    let back = |label: Option<Label>| name(label.map(|l| Label::new(l.root.up(-shift), l.core)));
    let chances = |list: &[Chance]| {
        let each = list.iter().map(|c| format!("{} {:x}", back(c.label), c.chance.to_bits()));
        each.collect::<Vec<_>>().join(", ")
    };
    let mut all: Vec<String> = out
        .spans()
        .iter()
        .map(|s| {
            let (units, sound, label) = (&s.units, &s.sound, back(s.label));
            let change = s.change.to_bits();
            format!("{units:?} {sound:?} {} {} {label} {change:x}: {}", s.pushed, s.rest, chances(out.chances(s)))
        })
        .collect();
    let mut list = Vec::new();
    for u in 0..hearing.units() {
        decoder.chances(hearing, u..u + 1, &mut list);
        all.push(format!("{u} {:x}: {}", decoder.change(u).to_bits(), chances(&list)));
    }
    all
}

/// Everything some music reads, with its pins.
fn read_all(music: &Music, pins: &[PinnedSpan]) -> Vec<String> {
    let whole = Whole::read(music, &HAND, pins);
    everything(&whole.hearing, &whole.decoder, &whole.out, 0)
}

#[test]
fn the_chances_on_each_unit_add_up_to_1_and_a_change_is_a_chance() {
    for (music, pins) in the_music() {
        let whole = Whole::read(&music, &HAND, &pins);
        let spans = whole.spans();
        let reported = whole.hearing.reported().len();
        // The spans run from the first unit reported to the last, each from where the one before ends.
        let ends: Vec<usize> = spans.iter().map(|r| r.units.end).collect();
        let starts: Vec<usize> = spans.iter().map(|r| r.units.start).collect();
        assert_eq!(starts.first(), Some(&0));
        assert_eq!(starts[1..], ends[..ends.len() - 1]);
        assert_eq!(ends.last(), Some(&reported));
        let fine = |chances: &[(String, f64)]| {
            let sum: f64 = chances.iter().map(|(_, c)| c).sum();
            let sorted = chances.windows(2).all(|w| w[0].1 >= w[1].1);
            (sum - 1.0).abs() < 1e-9 && sorted && chances.iter().all(|(_, c)| *c > 0.0 && *c <= 1.0 + 1e-9)
        };
        for u in 0..reported {
            let chances = whole.chances(u..u + 1);
            assert!(fine(&chances), "unit {u}: {chances:?}");
            let change = whole.decoder.change(whole.first() + u);
            assert!((-1e-9..=1.0 + 1e-9).contains(&change), "unit {u}: change {change}");
        }
        for r in &spans {
            assert!(fine(&r.chances), "{r:?}");
            assert!(r.chances.iter().any(|(name, _)| *name == r.label), "{r:?}");
            assert_eq!(r.change, whole.decoder.change(whole.first() + r.units.start));
        }
        // Where a song starts, a chord starts for sure.
        if whole.first() == 0 {
            assert!((whole.decoder.change(0) - 1.0).abs() < 1e-9);
        }
    }
    // C, and C/B on the third beat, then Am: the chord likely holds where only the bass moves, and likely changes at
    // the bar line.
    let (_, music, _) = whole_cases().into_iter().find(|(name, _, _)| name.starts_with("4 ")).unwrap();
    let whole = Whole::read(&music.looped(), &HAND, &[]);
    let change = |unit: usize| whole.decoder.change(whole.first() + unit);
    assert!(change(4) < 0.5 && change(B) > 0.5, "change at 4: {:.2}, at {B}: {:.2}", change(4), change(B));
}

#[test]
fn the_same_notes_moved_read_the_same_moved_to_the_last_bit() {
    for (music, pins) in the_music() {
        let want = read_all(&music, &pins);
        for shift in [-6, -5, -1, 1, 2, 5] {
            let moved = music.map(|n| Note { key: (i32::from(n.key) + shift) as u8, ..n });
            let up = |pin: &PinnedSpan| PinnedSpan {
                units: pin.units.clone(),
                label: pin.label.map(|l| Label::new(l.root.up(shift), l.core)),
            };
            let pins: Vec<PinnedSpan> = pins.iter().map(up).collect();
            let whole = Whole::read(&moved, &HAND, &pins);
            assert_eq!(everything(&whole.hearing, &whole.decoder, &whole.out, shift), want, "moved {shift}");
        }
    }
}

#[test]
fn the_same_music_at_another_resolution_or_in_another_order_reads_the_same() {
    for (music, pins) in the_music() {
        let want = read_all(&music, &pins);
        for ppq in [96, 960] {
            let scale = |t: i64| t * ppq / 480;
            let mut other = music.map(|n| Note { start: scale(n.start), length: scale(n.length), ..n });
            other.grid.ticks_per_beat = ppq as u32;
            assert_eq!(read_all(&other, &pins), want, "at {ppq} ticks a beat");
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
            assert_eq!(read_all(&other, &pins), want, "{what}");
        }
    }
}

#[test]
fn a_decoder_kept_and_used_again_reads_the_same() {
    let (mut decoder, mut out) = (Decoder::new(), Decoding::new());
    let music = the_music();
    for (music, pins) in music.iter().chain(music.iter().rev()) {
        let hearing = music.hear();
        assert!(decoder.decode(&hearing, &HAND, &COUNTED, pins, &mut out));
        assert_eq!(everything(&hearing, &decoder, &out, 0), read_all(music, pins));
    }
}

#[test]
fn nothing_heard_reads_nothing() {
    let (mut decoder, mut out) = (Decoder::new(), Decoding::new());
    let music = Music::new().bass(bars("C2")).keys(bars("C4 E4 G4"));
    assert!(decoder.decode(&music.hear(), &HAND, &COUNTED, &[], &mut out));
    assert!(!out.spans().is_empty());
    let nothing = Hearing::new();
    assert!(!decoder.decode(&nothing, &HAND, &COUNTED, &[pin(0..8, "C")], &mut out));
    assert!(out.spans().is_empty());
    assert_eq!(decoder.change(0), 0.0);
    let mut chances = vec![Chance { label: None, chance: 1.0 }];
    decoder.chances(&nothing, 0..8, &mut chances);
    assert!(chances.is_empty());
}

/// The hand-set weights, with any changed by SET: "E3=-2,pair=0.3", each a feature's code, "pair" for the pair term's
/// trust, "none" for what no chord scores a beat, or "shift" to lower the four costs of starting a chord (E4 to E7) by
/// as much.
fn weights_set() -> Weights {
    let mut weights = HAND;
    for (name, value) in std::env::var("SET").unwrap_or_default().split(',').filter_map(|s| s.split_once('=')) {
        set(&mut weights, name.trim(), value.trim().parse().unwrap());
    }
    weights
}

/// Sets one weight by its name, as SET names them.
fn set(weights: &mut Weights, name: &str, value: f64) {
    match name {
        "pair" => weights.pair = value,
        "none" => weights.none = value,
        "shift" => {
            for f in [Feature::OnBar, Feature::MidBar, Feature::OnBeat, Feature::OffBeat] {
                weights.features[f] -= value;
            }
        }
        code => {
            let feature = Feature::ALL.into_iter().find(|f| f.code() == code);
            weights.features[feature.unwrap_or_else(|| panic!("no weight {code}"))] = value;
        }
    }
}

#[test]
#[ignore = "prints every case read whole and what fails (SET=\"E3=-2,pair=0.3\" to change weights)"]
fn show_the_cases_read_whole() {
    let weights = weights_set();
    let failed = failures(&all_cases(), &weights);
    for (name, music, _) in all_cases() {
        let spans = Whole::read(&music.looped(), &weights, &[]).spans();
        let fails = failed.iter().find(|(n, _)| *n == name).map_or(String::new(), |(_, why)| format!("  FAILS {why}"));
        println!("{name:<40} {}{fails}", render(&spans));
    }
}

#[test]
#[ignore = "prints the cases that fail at each point of a grid of weights, read whole and one stretch at a time \
            (AXES=\"pair=0,0.25,0.5|E3=-1.5,-2\")"]
fn show_the_grid() {
    let cases = all_cases();
    let stretches: Vec<(&str, SpanFacts, Want)> = hand_cases()
        .into_iter()
        .map(|(name, music, units, want)| (name, facts_on(&music.looped().hear(), units, None), want))
        .collect();
    let axes: Vec<(String, Vec<f64>)> = std::env::var("AXES")
        .unwrap_or_else(|_| "pair=0,0.25".to_owned())
        .split('|')
        .filter_map(|axis| axis.split_once('='))
        .map(|(name, values)| (name.to_owned(), values.split(',').map(|v| v.parse().unwrap()).collect()))
        .collect();
    let points: usize = axes.iter().map(|(_, values)| values.len()).product();
    for point in 0..points {
        let (mut weights, mut rest, mut at) = (weights_set(), point, Vec::new());
        for (name, values) in &axes {
            let value = values[rest % values.len()];
            rest /= values.len();
            set(&mut weights, name, value);
            at.push(format!("{name} {value}"));
        }
        let mut failed: Vec<String> = failures(&cases, &weights).into_iter().map(|(name, _)| name).collect();
        for (name, facts, want) in &stretches {
            if !reads(&ranking_by(facts, &weights), *want) {
                failed.push(format!("s{name}"));
            }
        }
        let broken = weights.check();
        let broken = if broken.is_empty() { String::new() } else { format!(" BREAKS {broken:?}") };
        let names: Vec<&str> = failed.iter().map(|n| n.split(' ').next().unwrap_or("")).collect();
        println!("{}: {} fail {names:?}{broken}", at.join(", "), failed.len());
    }
}

#[test]
#[ignore = "prints what some ways of reading a case score, span by span, and each feature's part in the difference \
            between the first two (CASE=37 PATHS=\"0:Dm7 8:G7|0:Dm7 4:F 8:G7\", units from the first reported)"]
fn explain_the_paths() {
    let case = std::env::var("CASE").unwrap_or_default();
    let paths = std::env::var("PATHS").unwrap_or_default();
    let weights = weights_set();
    let Some((name, music, _)) = all_cases().into_iter().find(|(name, _, _)| name.split(' ').next() == Some(&case))
    else {
        println!("no case {case}");
        return;
    };
    let hearing = music.looped().hear();
    let (first, end) = (hearing.reported().start, hearing.units());
    let mut evidence = Evidence::new();
    evidence.prepare(&hearing);
    let (mut facts, mut features) = (SpanFacts::new(), PerFeature::default());
    println!("{name}: units {first}..{end}, a loop's copied bar read as its last chord, without pushes");
    let mut totals: Vec<(PerFeature<f64>, f64)> = Vec::new();
    for path in paths.split('|') {
        let mut cuts: Vec<(usize, String)> = path
            .split_whitespace()
            .map(|cut| {
                let (unit, chord) = cut.split_once(':').unwrap();
                (first + unit.parse::<usize>().unwrap(), chord.to_owned())
            })
            .collect();
        if let (true, Some((_, last))) = (first > 0, cuts.last().cloned()) {
            cuts.insert(0, (0, last));
        }
        let (mut parts, mut total) = (PerFeature::default(), 0.0);
        let mut before: Option<Label> = None;
        for (k, (from, chord)) in cuts.iter().enumerate() {
            let to = cuts.get(k + 1).map_or(end, |(unit, _)| *unit);
            evidence.facts(&hearing, *from..to, None, None, &mut facts);
            let (score, pair) = match chord.as_str() {
                "N" => (facts.none_score(&weights), 0.0),
                chord => {
                    let label = label(chord);
                    facts.features(label, &COUNTED, &mut features);
                    for f in Feature::ALL {
                        let times = if f.once() { 1.0 } else { facts.beats() };
                        parts[f] += weights.features[f] * features[f] * times;
                    }
                    let pair = before.map_or(0.0, |b| weights.pair * COUNTED.pair_prior(b, label));
                    before = Some(label);
                    (facts.score(&features, &weights), pair)
                }
            };
            println!("  {from}..{to} {chord}: {score:.3} and the pair term {pair:.3} ({:.2} beats)", facts.beats());
            total += score + pair;
        }
        println!("  {path}: {total:.3}");
        totals.push((parts, total));
    }
    if let [(a, total_a), (b, total_b), ..] = totals.as_slice() {
        println!("the first less the second: {:.3}, of which", total_a - total_b);
        for f in Feature::ALL.into_iter().filter(|&f| (a[f] - b[f]).abs() > 1e-9) {
            println!("  {:>4}: {:+.3}", f.code(), a[f] - b[f]);
        }
    }
}

#[test]
#[ignore = "times reading a long song and a dense part whole, and prints what the song reads (run with --release)"]
fn show_the_speed() {
    let time = |what: &str, music: &Music| {
        let (mut decoder, mut out, mut hearing) = (Decoder::new(), Decoding::new(), Hearing::new());
        for round in 0..3 {
            let time = std::time::Instant::now();
            assert!(music.hear_into(&mut hearing));
            let heard = time.elapsed();
            assert!(decoder.decode(&hearing, &HAND, &COUNTED, &[], &mut out));
            let read = time.elapsed() - heard;
            let (units, spans) = (hearing.units(), out.spans().len());
            println!("{what}, round {round}: {units} units heard in {heard:?}, read in {read:?} into {spans} spans");
        }
    };
    // 64 bars of a ii V I in four keys, with a walking bass and a tune, the ii pushed into its bar every four bars,
    // and a bar of rest every sixteen.
    let (mut bass, mut keys, mut tune) = (Vec::new(), Vec::new(), Vec::new());
    for bar in 0..64i64 {
        let start = bar * BAR;
        let shift = [0, 5, 10, 3][(bar / 16) as usize % 4];
        let up = |notes: Vec<Note>| notes.into_iter().map(move |n| Note { key: n.key + shift, ..n });
        if bar % 16 == 15 {
            continue;
        }
        let (walk, chord) = match bar % 4 {
            0 => ("D2 F2 A2 Ab2", "F3 A3 C4 E4"),
            1 => ("G2 B2 D3 Db3", "F3 A3 B3 E4"),
            _ => ("C3 E3 G3 B2", "E3 G3 B3 D4"),
        };
        bass.extend(up(line(walk, start, BEAT)));
        if bar % 4 == 3 {
            keys.extend(up(notes(chord, start, BAR - BEAT / 2)));
            keys.extend(up(notes("F3 A3 C4 E4", start + BAR - BEAT / 2, BAR + BEAT / 2)));
        } else if bar % 4 != 0 || bar % 16 == 0 {
            keys.extend(up(notes(chord, start, BAR)));
        }
        tune.extend(up(line("E5 D5 C5 B4 A4 G4 A4 B4", start, BEAT / 2)));
    }
    let song = Music::new().bass(bass).keys(keys).tune(tune);
    time("64 bars", &song);
    // 16 bars of a dense gospel part.
    let dense = dense(16);
    time("16 dense bars", &dense);
    println!("{}", render(&Whole::read(&song, &HAND, &[]).spans()));
    println!("{}", render(&Whole::read(&dense, &HAND, &[]).spans()));
}
