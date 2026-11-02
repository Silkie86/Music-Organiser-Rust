//! Tests of the reader as the app uses it ([`super::reader`]): the answers the design's hand cases give and how each
//! is graded, the chart spelled in a key, Chord check's choices, names checked, pins given in ticks, the rules every
//! reading keeps for any music, and the checks that the reader's version is bumped when a reading can change.

use super::decode_tests::{small_music, song, the_music, whole_cases};
use super::hear_tests::{BAR, BEAT, Dice, Music, VELOCITY, line, notes, random_music};
use super::name_tests::named_cases;
use super::score_tests::bars;
use super::{
    BassKind, CHOICE_FLOOR, CHORD_READER_VERSION, ChoiceFrom, ChordAnswer, ChordContext, ChordReader, ChordReading,
    ChordSpan, Grading, Hearing, Label, MOST_READINGS, Model, NameCheck, NoChord, Pin, PinnedSpan, SHIPPED,
    SHIPPED_HASH, SpanFlags, Texture, Thresholds, grade, numbers_hash, read_chords,
};
use crate::chord::Chord;
use crate::fnv::Fnv;
use crate::key::Key;
use crate::keyfind::Span;
use crate::notes::{Grid, Note, Part, Role};
use crate::pitch::{PcSet, PitchClass};

/// What some music reads, keeping `readings` readings a span.
pub(super) fn read(music: &Music, pins: &[Pin], readings: u8) -> Option<ChordReading> {
    let parts: Vec<Part> = music.parts.iter().map(|(role, notes)| Part { role: *role, notes }).collect();
    let cx = ChordContext { span: music.span, pins, readings };
    ChordReader::new().read(&parts, music.grid, &cx)
}

/// The hand cases, each read as a loop: the decode's, then the namer's.
pub(super) fn hand_cases() -> Vec<(&'static str, Music)> {
    let whole = whole_cases().into_iter().map(|(name, music, _)| (name, music.looped()));
    whole.chain(named_cases().into_iter().map(|(name, music, _)| (name, music.looped()))).collect()
}

/// What the hand case named reads, every reading kept.
pub(super) fn case(name: &str) -> ChordReading {
    let (_, music) = hand_cases().into_iter().find(|(n, _)| *n == name).unwrap_or_else(|| panic!("no case {name}"));
    read(&music, &[], MOST_READINGS).unwrap_or_else(|| panic!("{name}: nothing read"))
}

/// The music the rules are checked on, with pins given in ticks: the decode's tests' music, the hand cases and a song.
pub(super) fn the_music_in_ticks() -> Vec<(Music, Vec<Pin>)> {
    let mut out: Vec<(Music, Vec<Pin>)> =
        the_music().into_iter().map(|(music, pins)| (music.clone(), in_ticks(&music.hear(), &pins))).collect();
    out.extend(hand_cases().into_iter().map(|(_, music)| (music, Vec::new())));
    out.push((song(), Vec::new()));
    out
}

/// Pins given by units, as the decode's tests give them, put in the music's own ticks.
fn in_ticks(hearing: &Hearing, pins: &[PinnedSpan]) -> Vec<Pin> {
    let tick = |unit: usize| hearing.start(unit);
    pins.iter().map(|p| Pin { start: tick(p.units.start), end: tick(p.units.end), chord: p.chord }).collect()
}

pub(super) fn chord(name: &str) -> Chord {
    Chord::parse(name).unwrap_or_else(|| panic!("{name} doesn't read"))
}

/// The chord each span shows, or N.
fn shown(reading: &ChordReading) -> Vec<String> {
    reading.spans.iter().map(|s| s.shown().map_or("N".to_owned(), |r| r.chord.name())).collect()
}

fn answers(reading: &ChordReading) -> Vec<ChordAnswer> {
    reading.spans.iter().map(|s| s.answer).collect()
}

/// A reading as one line a span: its answer, then each reading with its chance and group.
fn render(reading: &ChordReading) -> String {
    let mut out = format!("{:?}", reading.texture);
    for s in &reading.spans {
        let readings: Vec<String> =
            s.readings.iter().map(|r| format!("{} {:.3} g{}", r.chord.name(), r.chance, r.group)).collect();
        out.push_str(&format!(" | {:?} {:?}: {}", s.units, s.answer, readings.join(", ")));
    }
    out
}

#[test]
fn the_hand_cases_answer_as_a_player_would() {
    let one = ChordAnswer::Chord;
    // 1: C, one chord, on its root.
    let r = case("1 C");
    assert_eq!((shown(&r), answers(&r)), (vec!["C".to_owned()], vec![one]), "{}", render(&r));
    assert_eq!(r.spans[0].readings[0].bass, BassKind::Root);
    // 10 and 11: Dm9 | G13 | Cmaj9, each one chord on its root, however low the keys are voiced.
    for name in ["10 Dm9 | G13 | Cmaj9", "11 the same, voiced low"] {
        let r = case(name);
        assert_eq!(shown(&r), ["Dm9", "G13", "Cmaj9"], "{name}: {}", render(&r));
        assert_eq!(answers(&r), [one; 3], "{name}: {}", render(&r));
        assert!(r.spans.iter().all(|s| s.readings[0].bass == BassKind::Root), "{name}: {}", render(&r));
        assert_eq!(r.texture, Texture::Accompaniment);
    }
    // 12: with no bass, a rootless reading is never one chord.
    let r = case("12 rootless, no bass");
    for s in &r.spans {
        assert!(!s.readings[0].rootless || s.answer != one, "{}", render(&r));
    }
    // 13, 20, 21, 22 and 33 with no bass: keys alone a player could read more than one way are never one chord; 22's
    // C6 or Am7 is the same notes, or unclear.
    for name in ["13 G13 rootless, no bass", "20 G7alt or Db13, no bass", "21 no bass", "22 no bass", "33 no bass"] {
        let r = case(name);
        assert_eq!(r.texture, Texture::HarmonyOnly, "{name}");
        assert!(r.spans.iter().all(|s| s.answer != one), "{name}: {}", render(&r));
    }
    let r = case("22 no bass");
    assert!(matches!(answers(&r)[..], [ChordAnswer::SameNotes | ChordAnswer::Unclear]), "{}", render(&r));
    // 25: C#dim7 with no bass, its four roots and A7(b9) one group with 0.9 or more.
    let r = case("25 C#dim7, no bass, before Dm");
    let dim = &r.spans[0];
    let group: f64 = dim.readings.iter().filter(|x| x.group == dim.readings[0].group).map(|x| x.chance).sum();
    assert!(dim.readings[0].chord.name() == "C#dim7" && group >= 0.9, "{}", render(&r));
    // 54: C | G | Am | F under a tune, each one chord.
    let r = case("54 C | G | Am | F with a tune");
    assert_eq!(shown(&r), ["C", "G", "Am", "F"], "{}", render(&r));
    assert_eq!(answers(&r), [one; 4], "{}", render(&r));
    assert_eq!(r.texture, Texture::Full);
}

#[test]
fn a_tune_alone_implies_its_chord_and_one_note_or_none_is_no_chord() {
    // 58: a tune alone, C E G E, implies C.
    let tune = read(&Music::new().tune(line("C4 E4 G4 E4", 0, BEAT)), &[], 2).unwrap();
    assert_eq!(tune.texture, Texture::SingleLine);
    assert_eq!((shown(&tune), answers(&tune)), (vec!["C".to_owned()], vec![ChordAnswer::Implied]), "{}", render(&tune));
    // The chart shows a chord where the best way reads one, or where one line alone implies it: a tune alone is never
    // read as a chord for sure, as it's no chord more likely than not. Not sure of it otherwise, the chart shows none.
    let s = &tune.spans[0];
    assert!(s.label.is_none() && s.no_chord > s.readings[0].chance, "{s:?}");
    assert_eq!(ChordSpan { answer: ChordAnswer::Unclear, ..s.clone() }.shown(), None);
    // So does a bass line alone, or keys played one note at a time; and the answer is graded as implied however sure
    // the readings are.
    for music in [Music::new().bass(line("C2 E2 G2 E2", 0, BEAT)), Music::new().keys(line("C4 E4 G4 E4", 0, BEAT))] {
        let r = read(&music, &[], 2).unwrap();
        assert!(r.texture == Texture::SingleLine && answers(&r) == [ChordAnswer::Implied], "{}", render(&r));
    }
    // Nothing pitched: nothing read.
    assert_eq!(read(&Music::new(), &[], 2), None);
    assert_eq!(read(&Music::new().part(Role::Drums, notes("C2 D2", 0, BEAT)), &[], 2), None);
    // One note, whatever plays it: too few notes.
    for role in [Role::Harmony, Role::Bass, Role::Melody, Role::Unknown] {
        let r = read(&Music::new().part(role, notes("C4", 0, BAR)), &[], 2).unwrap();
        let few = ChordAnswer::NoChord(NoChord::TooFewNotes);
        assert!(!r.spans.is_empty() && answers(&r).iter().all(|&a| a == few), "{role:?}: {}", render(&r));
        assert!(r.spans.iter().all(|s| s.shown().is_none()) && r.names(None).iter().all(Option::is_none));
    }
    // All twelve held: unclear.
    let twelve = Music::new().keys(notes("C4 C#4 D4 D#4 E4 F4 F#4 G4 G#4 A4 A#4 B4", 0, BAR));
    let r = read(&twelve, &[], 2).unwrap();
    assert_eq!(answers(&r), [ChordAnswer::Unclear], "{}", render(&r));
    // A bar or more of silence is a rest: C, two bars of nothing, F.
    let rest = Music::new().bass(bars("C2 | | | F2")).keys(bars("C4 E4 G4 | | | F3 A3 C4"));
    let r = read(&rest, &[], 2).unwrap();
    assert_eq!(shown(&r), ["C", "N", "F"], "{}", render(&r));
    assert_eq!(r.spans[1].answer, ChordAnswer::NoChord(NoChord::Rest));
    assert!(r.spans[1].readings.is_empty() && r.spans[1].choices(3).is_empty());
}

#[test]
fn an_answer_is_graded_by_the_rules_in_order() {
    use ChordAnswer::{Implied, SameNotes, Unclear};
    let t = Thresholds { one: [0.8, 0.7, 0.3, 1.0], same: [0.9, 0.95, 0.2, 1.0] };
    let base = Grading {
        rest: false,
        given: false,
        heard: 3,
        decoded: true,
        first: 0.85,
        rootless_with_no_bass: false,
        group: 2,
        group_chance: 0.97,
        texture: Texture::Full,
    };
    let rest = ChordAnswer::NoChord(NoChord::Rest);
    let few = ChordAnswer::NoChord(NoChord::TooFewNotes);
    for (grading, want) in [
        (base, ChordAnswer::Chord),
        // A rest first, then a chord given, then too few notes, then one line.
        (Grading { rest: true, given: true, ..base }, rest),
        (Grading { given: true, heard: 0, first: 0.0, decoded: false, ..base }, ChordAnswer::Chord),
        (Grading { heard: 1, texture: Texture::SingleLine, ..base }, few),
        (Grading { first: 1.0, texture: Texture::SingleLine, ..base }, Implied),
        // One chord, unless rootless with no bass; then the same notes, two readings or more adding up enough.
        (Grading { first: 0.79, ..base }, SameNotes),
        (Grading { rootless_with_no_bass: true, ..base }, SameNotes),
        (Grading { first: 0.79, group: 1, ..base }, Unclear),
        (Grading { first: 0.79, group_chance: 0.89, ..base }, Unclear),
        (Grading { decoded: false, first: 0.99, ..base }, Unclear),
        // Each texture has its own; a threshold under a half counts as a half.
        (Grading { first: 0.75, texture: Texture::Accompaniment, ..base }, ChordAnswer::Chord),
        (Grading { first: 0.75, group_chance: 0.94, texture: Texture::Accompaniment, ..base }, ChordAnswer::Chord),
        (Grading { first: 0.65, group_chance: 0.94, texture: Texture::Accompaniment, ..base }, Unclear),
        (Grading { first: 0.45, group_chance: 0.45, texture: Texture::HarmonyOnly, ..base }, Unclear),
        (Grading { first: 0.5, texture: Texture::HarmonyOnly, ..base }, ChordAnswer::Chord),
        (Grading { first: 0.45, group_chance: 0.5, texture: Texture::HarmonyOnly, ..base }, SameNotes),
    ] {
        assert_eq!(grade(grading, &t), want, "{grading:?}");
    }
    // A threshold of 1 or more is never met, though a chance can round up to 1.
    let never = Thresholds { one: [1.0; 4], same: [1.5; 4] };
    assert_eq!(grade(Grading { first: 1.0, group_chance: 1.0, ..base }, &never), Unclear);
    // The answers in the order the facts number them, and which show a chord.
    let shows: Vec<bool> = ChordAnswer::ALL.iter().map(|a| a.shows_chord()).collect();
    assert_eq!(shows, [true, true, true, true, false, false]);
}

#[test]
fn the_texture_is_what_kinds_of_line_are_heard() {
    let hear = |music: Music| Texture::of(&music.hear());
    let (tune, bass, held) = (line("E5 D5 C5 D5", 0, BEAT), notes("C2", 0, BAR), notes("C4 E4 G4", 0, BAR));
    assert_eq!(hear(Music::new().tune(tune.clone()).keys(held.clone())), Texture::Full);
    assert_eq!(hear(Music::new().tune(tune.clone()).bass(bass.clone())), Texture::Full);
    assert_eq!(hear(Music::new().bass(bass.clone()).keys(held.clone())), Texture::Accompaniment);
    assert_eq!(hear(Music::new().keys(held.clone())), Texture::HarmonyOnly);
    assert_eq!(hear(Music::new().tune(tune)), Texture::SingleLine);
    assert_eq!(hear(Music::new().bass(bass)), Texture::SingleLine);
    // Chords are chords in any part: a triad in the tune, a fifth in the bass part.
    assert_eq!(hear(Music::new().tune(held.clone())), Texture::HarmonyOnly);
    assert_eq!(hear(Music::new().bass(notes("C2 G2", 0, BAR))), Texture::HarmonyOnly);
    // And however short: a stab on every beat, each shorter than a player's timing.
    let stabs: Vec<Note> = (0..8).flat_map(|beat| notes("C4 E4 G4", beat * BEAT, 20)).collect();
    assert_eq!(hear(Music::new().keys(stabs.clone())), Texture::HarmonyOnly);
    assert_eq!(hear(Music::new().tune(stabs)), Texture::HarmonyOnly);
    // Keys played one note at a time are one line, played legato too (each note held a little into the next); but
    // not two notes or more to a beat, which is an arpeggio (broken chords), and not with the pedal held.
    let lead = line("C4 E4 G4 C5 G4 E4 C4 G3", 0, BEAT);
    assert_eq!(hear(Music::new().keys(lead.clone())), Texture::SingleLine);
    let legato: Vec<Note> = lead.iter().map(|n| Note { length: n.length + 30, ..*n }).collect();
    assert_eq!(hear(Music::new().keys(legato)), Texture::SingleLine);
    assert_eq!(hear(Music::new().keys(line("C4 E4 G4 C5 G4 E4 C4 G3", 0, BEAT / 2))), Texture::HarmonyOnly);
    let pedalled: Vec<Note> = lead.iter().map(|n| Note { length: 2 * BAR - n.start, ..*n }).collect();
    assert_eq!(hear(Music::new().keys(pedalled)), Texture::HarmonyOnly);
    // An octave is one note; and a double stop here and there, one note in ten or fewer, doesn't make chords.
    let octaves: Vec<Note> = lead.iter().flat_map(|n| [*n, Note { key: n.key + 12, ..*n }]).collect();
    assert_eq!(hear(Music::new().tune(octaves)), Texture::SingleLine);
    let long = line(&"C4 E4 G4 C5 G4 E4 C4 G3 ".repeat(3), 0, BEAT);
    let double = |under: &str| [long.clone(), notes(under, 23 * BEAT, BEAT)].concat();
    assert_eq!(hear(Music::new().tune(double("E3"))), Texture::SingleLine);
    assert_eq!(hear(Music::new().tune(double("C3 E3"))), Texture::HarmonyOnly);
}

#[test]
fn the_chart_is_spelled_in_the_key() {
    let c_major = Some(Key::major(PitchClass::C));
    let names = |r: &ChordReading, key: Option<Key>| -> Vec<String> {
        r.names(key).iter().map(|s| s.map_or("N".to_owned(), |s| s.name())).collect()
    };
    // 29: Em7 | Ebdim7 | Dm7 in C major: the diminished chord falls a semitone to Dm7, so it's spelled flat. With no
    // key, as charts spell chords: D#dim7.
    let r = case("29 Em7 | Ebdim7 | Dm7");
    assert_eq!(names(&r, c_major), ["Em7", "Ebdim7", "Dm7"]);
    assert_eq!(names(&r, None), ["Em7", "D#dim7", "Dm7"]);
    // 55: Dm7 | Db9 | Cmaj7 in C major: Db9, never C#9.
    let r = case("55 Dm7 | Db9 | Cmaj7");
    assert_eq!(names(&r, c_major), ["Dm7", "Db9", "Cmaj7"]);
    // Round a loop, the last chord knows the first: Ebdim7 before Dm7, as the loop goes round from its last bar.
    let r = read(&Music::new().bass(bars("D2 | Eb2")).keys(bars("F3 A3 C4 | Gb3 A3 C4")).looped(), &[], 2).unwrap();
    assert_eq!(names(&r, c_major), ["Dm7", "Ebdim7"]);
    let song = Music::new().bass(bars("D2 | Eb2")).keys(bars("F3 A3 C4 | Gb3 A3 C4"));
    assert_eq!(names(&read(&song, &[], 2).unwrap(), c_major), ["Dm7", "D#dim7"]);
    // Any chord at a span is spelled as the chart spells chords there: the shown one as the chart has it.
    let keys = [None, c_major, Some(Key::major(PitchClass::new(6))), Some(Key::minor(PitchClass::new(6)))];
    for (music, pins) in the_music_in_ticks() {
        let r = read(&music, &pins, 2).unwrap();
        for key in keys {
            let chart = r.names(key);
            for (i, s) in r.spans.iter().enumerate() {
                let spelled = s.shown().map(|x| r.spell(&chart, i, x.chord, key));
                assert_eq!(spelled, chart[i], "span {i} in {key:?}: {}", render(&r));
                assert!(chart[i].is_none_or(|c| Some(c.chord()) == s.shown().map(|x| x.chord)));
            }
        }
    }
}

#[test]
fn chord_check_offers_the_same_notes_another_way_then_another_chord() {
    use ChoiceFrom::{First, Other, SameNotes, Tensions};
    let choices = |s: &ChordSpan, n: usize| -> Vec<(String, ChoiceFrom)> {
        s.choices(n).iter().map(|c| (c.chord.name(), c.from)).collect()
    };
    let own = |list: &[(&str, ChoiceFrom)]| -> Vec<(String, ChoiceFrom)> {
        list.iter().map(|&(name, from)| (name.to_owned(), from)).collect()
    };
    // C6 over C: Am7/C is the same notes on another root, and nothing else is likely enough to offer.
    let r = case("22 C6");
    assert_eq!(choices(&r.spans[0], 3), own(&[("C6", First), ("Am7/C", SameNotes)]), "{}", render(&r));
    // Dm9: Fmaj7/D is the same notes, but under 1%; so Dm9 with its tensions left out.
    let r = case("10 Dm9 | G13 | Cmaj9");
    assert_eq!(choices(&r.spans[0], 3), own(&[("Dm9", First), ("Dm7", Tensions)]), "{}", render(&r));
    // Dm7 over a walk, unclear: F6 is the same notes, F another chord.
    let r = case("38 Dm7 | G7 | Cmaj7 over a walk");
    assert_eq!(r.spans[0].answer, ChordAnswer::Unclear, "{}", render(&r));
    assert_eq!(choices(&r.spans[0], 3), own(&[("Dm7", First), ("F6", SameNotes), ("F", Other)]), "{}", render(&r));
    // A rest has none, and none are asked for: none.
    assert!(r.spans[0].choices(0).is_empty());
    // For every span: the first reading first, then each different, likely enough, and in the order given.
    for (music, pins) in the_music_in_ticks() {
        let r = read(&music, &pins, MOST_READINGS).unwrap();
        for s in &r.spans {
            let all = s.choices(MOST_READINGS.into());
            for n in 0..all.len() {
                assert_eq!(s.choices(n)[..], all[..n]);
            }
            let Some(first) = s.readings.first() else {
                assert!(all.is_empty());
                continue;
            };
            assert_eq!((all[0].chord, all[0].from), (first.chord, First), "{}", render(&r));
            for (i, c) in all.iter().enumerate().skip(1) {
                assert!(!all[..i].iter().any(|o| o.chord == c.chord), "{all:?}");
                let reading = s.readings.iter().skip(1).find(|x| x.chord == c.chord);
                let likely = reading.is_some_and(|x| x.chance >= CHOICE_FLOOR);
                match c.from {
                    First => panic!("{all:?}"),
                    SameNotes => assert!(likely && reading.is_some_and(|x| x.group == first.group), "{all:?}"),
                    Other => assert!(likely, "{all:?}"),
                    Tensions => {
                        let (a, b) = (c.chord, first.chord);
                        assert!(i == 1 && a.root() == b.root() && a.bass() == b.bass() && a.notes() != b.notes());
                    }
                }
            }
        }
    }
}

#[test]
fn names_found_elsewhere_are_checked_against_the_reading() {
    use NameCheck::{Agrees, Disagrees, Listed};
    let check = |r: &ChordReading, names: &[&str]| {
        let names: Vec<Chord> = names.iter().map(|n| chord(n)).collect();
        r.check_names(&names)
    };
    // 59: Fm9 | Bb13 | Ebmaj9, in a file named "Fm9 Bb13 Ebmaj9.mid": each agrees. The name is never evidence, so
    // what's read is the same with it or without.
    let music = Music::new().bass(bars("F2 | Bb1 | Eb2")).keys(bars("Ab3 C4 Eb4 G4 | Ab3 C4 D4 G4 | G3 Bb3 D4 F4"));
    let r = read(&music.clone().looped(), &[], 2).unwrap();
    assert_eq!(shown(&r), ["Fm9", "Bb13", "Ebmaj9"], "{}", render(&r));
    assert_eq!(check(&r, &["Fm9", "Bb13", "Ebmaj9"]), [Agrees; 3]);
    // Its tensions may differ, its core and bass may not; and the names go in order.
    assert_eq!(check(&r, &["Fm7", "Bb7(b9)", "Ebmaj7"]), [Agrees; 3]);
    assert_eq!(check(&r, &["F7", "Fm9/Ab", "Bb13sus4", "Ebm9"]), [Disagrees; 4]);
    assert_eq!(check(&r, &["Bb13", "Fm9", "Ebmaj9"]), [Agrees, Disagrees, Agrees]);
    // Round a loop, the names may start at any of its chords; a song's start at its first.
    assert_eq!(check(&r, &["Bb13", "Ebmaj9", "Fm9"]), [Agrees; 3]);
    let song = read(&music, &[], 2).unwrap();
    assert_eq!(shown(&song), ["Fm9", "Bb13", "Ebmaj9"], "{}", render(&song));
    assert_eq!(check(&song, &["Bb13", "Ebmaj9", "Fm9"]), [Agrees, Agrees, Disagrees]);
    // As many names are matched as can be, then as many agree as can: a name that matches only late on doesn't use
    // the chart up, and a name listed for a chord is matched there when the name after it needs the chord after.
    assert_eq!(check(&song, &["Ebmaj9", "Fm9", "Bb13"]), [Disagrees, Agrees, Agrees]);
    let sixth = Music::new().bass(bars("C2 | F2 | A1")).keys(bars("E3 G3 A3 C4 | F3 A3 C4 | E3 G3 A3 C4"));
    let r = read(&sixth, &[], 2).unwrap();
    assert_eq!(shown(&r), ["C6", "F", "Am7"], "{}", render(&r));
    assert_eq!(check(&r, &["Am7", "F"]), [Listed, Agrees], "{}", render(&r));
    assert_eq!(check(&r, &["Am7", "Am7"]), [Listed, Agrees], "{}", render(&r));
    // Another reading of a chord shown is listed: Am7 for C6 over C, slashed or not.
    let r = case("22 C6");
    let each: Vec<NameCheck> = ["C6", "Am7/C", "Am7", "Am7/E"].iter().flat_map(|&n| check(&r, &[n])).collect();
    assert_eq!(each, [Agrees, Listed, Listed, Disagrees]);
    // A name looks only after the chord the one before it matched: here, there's no chord after C6.
    assert_eq!(check(&r, &["C6", "Am7"]), [Agrees, Disagrees]);
    // A diminished seventh agrees on any of its four roots; the same notes written the other way (G9sus4 as F/G)
    // agree too.
    let r = case("24 C#dim7 | Dm7");
    assert_eq!(check(&r, &["Bbdim7", "Dm7"]), [Agrees, Agrees]);
    let r = case("7 G9sus4");
    assert_eq!(check(&r, &["F/G"]), [Agrees], "{}", render(&r));
    // The chords a reading shows always agree with it.
    for (music, pins) in the_music_in_ticks() {
        let r = read(&music, &pins, 2).unwrap();
        let chart: Vec<Chord> = r.spans.iter().filter_map(|s| s.shown()).map(|x| x.chord).collect();
        assert!(r.check_names(&chart).iter().all(|&c| c == Agrees), "{}", render(&r));
    }
}

/// A ii V I in C as a song: Dm9, G13 and two bars of Cmaj9.
fn two_five_one() -> Music {
    Music::new().bass(bars("D2 | G2 | C2 | C2")).keys(bars("F3 A3 C4 E4 | F3 A3 B3 E4 | E3 G3 B3 D4 | E3 G3 B3 D4"))
}

#[test]
fn pins_given_in_ticks_are_read_as_given() {
    let music = two_five_one();
    let free = read(&music, &[], 6).unwrap();
    assert_eq!(shown(&free), ["Dm9", "G13", "Cmaj9"], "{}", render(&free));
    let given = |start: i64, end: i64, name: &str| Pin { start, end, chord: Some(chord(name)) };
    // The tritone's substitute given for the second bar: read as given, over its bass, one chord for sure.
    let r = read(&music, &[given(BAR, 2 * BAR, "Db7")], 6).unwrap();
    assert_eq!(shown(&r), ["Dm9", "Db7/G", "Cmaj9"], "{}", render(&r));
    let sub = &r.spans[1];
    assert!(sub.flags.contains(SpanFlags::GIVEN) && sub.answer == ChordAnswer::Chord && sub.readings.len() == 1);
    assert_eq!((sub.start, sub.end, sub.units.clone()), (BAR, 2 * BAR, 8..16));
    // Ticks a little off the units are moved to the nearest unit start; so are those of a loop, in its own ticks.
    assert_eq!(read(&music, &[given(BAR + 50, 2 * BAR - 100, "Db7")], 6), Some(r.clone()));
    let looped = read(&music.clone().looped(), &[given(BAR, 2 * BAR, "Db7")], 6).unwrap();
    assert_eq!(shown(&looped), shown(&r), "{}", render(&looped));
    // Given only its ends, two bars are one chord, whichever reads best; and so they are pinned with a chord that
    // has nothing a decode can read (C alone).
    let bare = Chord::new(PitchClass::C, PcSet::EMPTY.with(PitchClass::C), PitchClass::C);
    for chord in [None, Some(bare)] {
        let r = read(&music, &[Pin { start: 0, end: 2 * BAR, chord }], 6).unwrap();
        assert_eq!(r.spans[0].units, 0..16, "{}", render(&r));
        assert!(!r.spans[0].flags.contains(SpanFlags::GIVEN), "{}", render(&r));
    }
    // Pins that are empty there, run past the end, or overlap one before them are left out, in any order.
    for pins in [
        [given(BAR, 2 * BAR, "Db7"), given(BAR + BEAT, 3 * BAR, "Ab7")],
        [given(BAR + BEAT, 3 * BAR, "Ab7"), given(BAR, 2 * BAR, "Db7")],
        [given(BAR + 5, BAR + 10, "E7"), given(BAR, 2 * BAR, "Db7")],
        [given(i64::MIN, -BAR, "E7"), given(BAR, 2 * BAR, "Db7")],
    ] {
        assert_eq!(shown(&read(&music, &pins, 6).unwrap()), ["Dm9", "Db7/G", "Cmaj9"], "{pins:?}");
    }
    let past = read(&music, &[given(3 * BAR, i64::MAX, "C6")], 6).unwrap();
    assert_eq!(shown(&past), ["Dm9", "G13", "Cmaj9", "C6"], "{}", render(&past));
    // Over silence, a chord given is written in full: C | Db7 | F, with nothing played in the second bar.
    let gap = Music::new().bass(bars("C2 | | F2")).keys(bars("C4 E4 G4 | | F3 A3 C4"));
    let r = read(&gap, &[given(BAR, 2 * BAR, "Db7")], 6).unwrap();
    assert_eq!(shown(&r), ["C", "Db7", "F"], "{}", render(&r));
    assert_eq!(answers(&r), [ChordAnswer::Chord; 3], "{}", render(&r));
    // A chord given with a bass of its own keeps it, and the answer is one chord though one line is heard.
    let tune = Music::new().tune(line("C4 E4 G4 E4", 0, BEAT));
    let r = read(&tune, &[given(0, BAR, "Am7/C")], 6).unwrap();
    assert_eq!((shown(&r), answers(&r)), (vec!["Am7/C".to_owned()], vec![ChordAnswer::Chord]), "{}", render(&r));
}

#[test]
fn a_chord_given_over_a_loops_last_bar_is_the_chord_before_its_first() {
    let music =
        Music::new().bass(bars("C2 | F2 | G2 | G2")).keys(bars("C4 E4 G4 | F3 A3 C4 | G3 B3 D4 | G3 B3 D4")).looped();
    let db7 = Some(chord("Db7"));
    let free = read(&music, &[], 6).unwrap();
    let r = read(&music, &[Pin { start: 3 * BAR, end: 4 * BAR, chord: db7 }], 6).unwrap();
    assert_eq!(shown(&r), ["C", "F", "G", "Db7/G"], "{}", render(&r));
    // As the loop goes round, its first chord follows Db7, not G; and a chord starts for sure there, where the chord
    // given ends.
    let first = |r: &ChordReading| r.spans[0].readings[0].chance.to_bits();
    assert_ne!(first(&r), first(&free), "{}", render(&r));
    assert!((r.spans[0].change - 1.0).abs() < 1e-9 && free.spans[0].change < 1.0 - 1e-9, "{}", render(&r));
    let last = |r: &ChordReading| r.spans.last().and_then(|s| s.label);
    assert_eq!((r.before, free.before), (last(&r), last(&free)), "{}", render(&r));
    assert_ne!(r.before, free.before);
    // Given only its ends, the last bar says nothing of the chord before the first.
    let ends = read(&music, &[Pin { start: 3 * BAR, end: 4 * BAR, chord: None }], 6).unwrap();
    assert_eq!(shown(&ends), ["C", "F", "G", "G"], "{}", render(&ends));
    assert!(ends.spans[0].change < 1.0 - 1e-9, "{}", render(&ends));
    assert_eq!(ends.before, free.before, "{}", render(&ends));
    // A pin before the loop's start is left out: the copied bar is never pinned on its own.
    let before = read(&music, &[Pin { start: -BAR, end: 0, chord: db7 }], 6).unwrap();
    assert_eq!(before, free);
}

#[test]
fn a_loop_reads_the_chord_before_its_first_as_it_goes_round() {
    // Dm9 | G13 | Cmaj9 goes round from Cmaj9 to Dm9.
    let r = case("10 Dm9 | G13 | Cmaj9");
    assert_eq!((r.before, r.spans.len()), (r.spans[2].label, 3), "{}", render(&r));
    assert!(r.before.is_some() && r.before != r.spans[0].label, "{}", render(&r));
    // C C/E | F F#dim7 | C/G goes round from C/G into C, and a one-chord loop into itself: the same chord carried
    // round, held on.
    for name in ["6 C C/E | F F#dim7 | C/G", "1 C"] {
        let r = case(name);
        assert!(r.before.is_some() && r.before == r.spans[0].label, "{name}: {}", render(&r));
    }
    // A song has nothing before its first chord.
    let r = read(&two_five_one(), &[], 2).unwrap();
    assert_eq!((r.looped, r.before), (false, None), "{}", render(&r));
}

/// Whether a reading keeps the rules that hold for any music, and if not, why: the spans run from the reading's start
/// to its end, each from where the one before ends, on the units heard; each keeps at most `keep` readings, and the
/// chance of none of them is what they leave; a rest has none; the answers follow the rules (a chord given is one
/// chord, one line alone is implied, one chord and the same notes are likely enough); a span shows a chord when its
/// answer gives one, and the chart spells that chord; Chord check's first choice is the first reading; and the chart
/// read back agrees with itself.
fn keeps_the_rules(hearing: &Hearing, r: &ChordReading, keep: usize) -> Result<(), String> {
    let check = |yes: bool, why: &dyn Fn() -> String| if yes { Ok(()) } else { Err(why()) };
    let reported = hearing.reported();
    let spans = &r.spans;
    let all = || format!("{r:?}");
    check(spans.first().map(|s| (s.units.start, s.start)) == Some((0, r.start)), &all)?;
    check(spans.last().map(|s| (s.units.end, s.end)) == Some((reported.len(), r.end)), &all)?;
    check(spans.windows(2).all(|w| w[0].units.end == w[1].units.start && w[0].end == w[1].start), &all)?;
    check(r.looped == (reported.start > 0) && r.notes > 0 && r.unit == hearing.unit(), &all)?;
    check(r.looped || r.before.is_none(), &all)?;
    let (one, same) = SHIPPED.thresholds.at(r.texture);
    let chart = r.names(None);
    for (i, s) in spans.iter().enumerate() {
        let why = || format!("{s:?} in {}", render(r));
        let first = s.readings.first();
        let from = |unit: usize| hearing.start(reported.start + unit);
        check(!s.units.is_empty() && s.start == from(s.units.start) && s.end == from(s.units.end), &why)?;
        check(s.heard_start <= s.start && (0.0..=1.0).contains(&s.change), &why)?;
        let chance = |c: f64| (0.0..=1.0).contains(&c);
        let kept: f64 = s.readings.iter().map(|x| x.chance).sum();
        check(s.readings.len() <= keep && s.readings.iter().all(|x| chance(x.chance)), &why)?;
        check(chance(s.no_chord) && kept + s.no_chord <= 1.0 + 1e-9, &why)?;
        check(s.other.to_bits() == (1.0 - kept).clamp(0.0, 1.0).to_bits(), &why)?;
        let given = s.flags.contains(SpanFlags::GIVEN);
        let answer_holds = match s.answer {
            ChordAnswer::NoChord(NoChord::Rest) => s.readings.is_empty() && !given,
            ChordAnswer::NoChord(NoChord::TooFewNotes) => !given,
            ChordAnswer::Implied => r.texture == Texture::SingleLine && !given,
            ChordAnswer::Chord if given => s.readings.len() == 1,
            ChordAnswer::Chord => {
                let rootless = first.is_some_and(|x| x.rootless) && s.flags.contains(SpanFlags::NO_BASS);
                r.texture != Texture::SingleLine && one < 1.0 && first.is_some_and(|x| x.chance >= one) && !rootless
            }
            ChordAnswer::SameNotes => r.texture != Texture::SingleLine && same < 1.0 && first.is_some(),
            ChordAnswer::Unclear => r.texture != Texture::SingleLine && !given,
        };
        check(answer_holds, &why)?;
        check(s.label.is_none() || first.map(|x| x.label) == s.label, &why)?;
        let reads = s.label.is_some() || s.answer == ChordAnswer::Implied;
        check(s.shown().is_some() == (reads && s.answer.shows_chord() && first.is_some()), &why)?;
        check(chart.get(i).copied().flatten().map(|c| c.chord()) == s.shown().map(|x| x.chord), &why)?;
        let choices = s.choices(3);
        check(choices.first().map(|c| c.chord) == first.map(|x| x.chord) && choices.len() <= 3, &why)?;
    }
    let shown: Vec<Chord> = spans.iter().filter_map(|s| s.shown()).map(|x| x.chord).collect();
    check(r.check_names(&shown).iter().all(|&c| c == NameCheck::Agrees), &all)?;
    Ok(())
}

/// Up to three pins anywhere in some music, in its own ticks (before it, past its end, empty, over each other and off
/// the units too), on a chord or only at their ends.
pub(super) fn random_pins(dice: &mut Dice, hearing: &Hearing) -> Vec<Pin> {
    let units = hearing.units();
    let (from, to) = (hearing.start(0), hearing.start(units));
    let span = (to - from).max(1);
    let mut pins = Vec::new();
    for _ in 0..dice.below(4) {
        let start = from - span / 4 + dice.below(span as u64 * 3 / 2) as i64;
        let end = start + dice.below(span as u64 / 2 + 1) as i64;
        let names = ["C", "Db7", "F#m7b5", "Bbmaj9", "E7(#9)", "G13sus4", "Abdim7", "D/C", "Am7/G"];
        let chord = (dice.below(3) > 0).then(|| chord(dice.pick(&names)).transposed(dice.below(12) as i32));
        pins.push(Pin { start, end, chord });
    }
    pins
}

#[test]
fn any_music_with_any_pins_is_read_by_the_rules() {
    let mut dice = Dice(0x6c07_8965_9a3f_10b7);
    let mut reader = ChordReader::new();
    let mut hearing = Hearing::new();
    let mut read = 0;
    for round in 0..300 {
        let music = if round % 2 == 0 { random_music(&mut dice) } else { small_music(&mut dice) };
        if !music.hear_into(&mut hearing) {
            continue;
        }
        let pins = random_pins(&mut dice, &hearing);
        let keep = dice.pick(&[0, 1, 2, 6, 9]);
        let parts: Vec<Part> = music.parts.iter().map(|(role, notes)| Part { role: *role, notes }).collect();
        let cx = ChordContext { span: music.span, pins: &pins, readings: keep };
        let what = format!("round {round}: {music:?}, pins {pins:?}, keep {keep}");
        let r = reader.read(&parts, music.grid, &cx).unwrap_or_else(|| panic!("nothing read: {what}"));
        if let Err(why) = keeps_the_rules(reader.hearing(), &r, usize::from(keep.clamp(1, MOST_READINGS))) {
            panic!("{why}\n{what}");
        }
        read += 1;
    }
    assert!(read > 250, "{read} pieces read");
}

#[test]
fn the_music_and_the_hand_cases_are_read_by_the_rules() {
    let mut reader = ChordReader::new();
    for (music, pins) in the_music_in_ticks() {
        let parts: Vec<Part> = music.parts.iter().map(|(role, notes)| Part { role: *role, notes }).collect();
        for keep in [1, 2, MOST_READINGS] {
            let cx = ChordContext { span: music.span, pins: &pins, readings: keep };
            let r = reader.read(&parts, music.grid, &cx).unwrap();
            if let Err(why) = keeps_the_rules(reader.hearing(), &r, usize::from(keep)) {
                panic!("{why}\n{music:?}");
            }
        }
    }
}

#[test]
fn the_answers_never_depend_on_how_many_readings_are_kept() {
    for (music, pins) in the_music_in_ticks() {
        let all = read(&music, &pins, MOST_READINGS).unwrap();
        for keep in [0, 1, 2, 3, 7, u8::MAX] {
            let some = read(&music, &pins, keep).unwrap();
            let kept = usize::from(keep.clamp(1, MOST_READINGS));
            assert_eq!(some.spans.len(), all.spans.len());
            for (s, a) in some.spans.iter().zip(&all.spans) {
                let first_few = a.readings.get(..kept.min(a.readings.len())).unwrap();
                assert_eq!(
                    (s.answer, &s.units, s.change, s.no_chord, s.flags),
                    (a.answer, &a.units, a.change, a.no_chord, a.flags)
                );
                assert_eq!(s.readings, first_few, "keeping {keep}: {}", render(&all));
            }
        }
        // The context's own default keeps two.
        let parts: Vec<Part> = music.parts.iter().map(|(role, notes)| Part { role: *role, notes }).collect();
        let cx = ChordContext { pins: &pins, ..ChordContext::new(music.span) };
        assert_eq!(read_chords(&parts, music.grid, &cx), read(&music, &pins, 2));
    }
}

/// Everything a reading holds, as bits, with each chord moved back by `shift` semitones and each tick put back to 480
/// a beat from `ppq`.
fn everything(r: &ChordReading, shift: i32, ppq: i64) -> Vec<String> {
    let tick = |t: i64| t * 480 / ppq;
    let back = |c: Chord| c.transposed(-shift);
    let (start, end) = (tick(r.start), tick(r.end));
    let before = r.before.map(|l| Label::new(l.root.up(-shift), l.core));
    let mut all =
        vec![format!("{:?} {:?} {start} {end} {} {} {} {before:?}", r.texture, r.unit, r.notes, r.truncated, r.looped)];
    for s in &r.spans {
        let (start, end, heard) = (tick(s.start), tick(s.end), tick(s.heard_start));
        let (change, none, other) = (s.change.to_bits(), s.no_chord.to_bits(), s.other.to_bits());
        let (units, answer, flags) = (&s.units, s.answer, s.flags.bits());
        all.push(format!("{units:?} {start} {end} {heard} {change:x} {answer:?} {none:x} {other:x} {flags}"));
        for x in &s.readings {
            let (roots, doubtful) = (x.roots.transposed(-shift).bits(), x.doubtful.transposed(-shift).bits());
            let (chance, also, bass, rootless, group) =
                (x.chance.to_bits(), x.also.map(back), x.bass, x.rootless, x.group);
            let c = back(x.chord);
            all.push(format!("  {c:?} {also:?} {roots:x} {bass:?} {rootless} {doubtful:x} {chance:x} {group}"));
        }
    }
    all
}

#[test]
fn the_same_notes_moved_read_the_same_moved() {
    for (music, pins) in the_music_in_ticks() {
        let want = everything(&read(&music, &pins, MOST_READINGS).unwrap(), 0, 480);
        for shift in [-6, -1, 1, 5] {
            let moved = music.map(|n| Note { key: (i32::from(n.key) + shift) as u8, ..n });
            let up = |pin: &Pin| Pin { chord: pin.chord.map(|c| c.transposed(shift)), ..*pin };
            let pins: Vec<Pin> = pins.iter().map(up).collect();
            let r = read(&moved, &pins, MOST_READINGS).unwrap();
            assert_eq!(everything(&r, shift, 480), want, "moved {shift}");
        }
    }
}

#[test]
fn the_same_music_at_another_resolution_or_in_another_order_reads_the_same() {
    for (music, pins) in the_music_in_ticks() {
        let want = everything(&read(&music, &pins, MOST_READINGS).unwrap(), 0, 480);
        for ppq in [96, 960] {
            let scale = |t: i64| t * ppq / 480;
            let mut other = music.map(|n| Note { start: scale(n.start), length: scale(n.length), ..n });
            other.grid.ticks_per_beat = scale(i64::from(music.grid.ticks_per_beat)) as u32;
            let pins: Vec<Pin> = pins.iter().map(|p| Pin { start: scale(p.start), end: scale(p.end), ..*p }).collect();
            let r = read(&other, &pins, MOST_READINGS).unwrap();
            assert_eq!(everything(&r, 0, ppq), want, "at {ppq} ticks a beat");
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
            assert_eq!(everything(&read(&other, &pins, MOST_READINGS).unwrap(), 0, 480), want, "{what}");
        }
    }
}

#[test]
fn a_reader_kept_and_used_again_reads_the_same() {
    let mut reader = ChordReader::new();
    let music = the_music_in_ticks();
    for (music, pins) in music.iter().chain(music.iter().rev()) {
        let parts: Vec<Part> = music.parts.iter().map(|(role, notes)| Part { role: *role, notes }).collect();
        let cx = ChordContext { span: music.span, pins, readings: 3 };
        assert_eq!(reader.read(&parts, music.grid, &cx), read_chords(&parts, music.grid, &cx));
    }
    // Nothing heard leaves nothing behind.
    let cx = ChordContext::new(Span::Song);
    assert_eq!(reader.read(&[], Grid::four_four(480), &cx), None);
}

#[test]
fn damaged_notes_are_read_without_a_panic() {
    let wild = [
        Note { start: i64::MAX - 5, length: i64::MAX, key: 255, velocity: 0 },
        Note { start: i64::MIN, length: -7, key: 127, velocity: u16::MAX },
        Note { start: 0, length: 0, key: 0, velocity: 1 },
        Note { start: 0, length: 480, key: 60, velocity: VELOCITY },
        Note { start: 240, length: 960, key: 64, velocity: VELOCITY },
        Note { start: 10_000_000 * 480, length: 480, key: 60, velocity: VELOCITY },
        Note { start: 10_000_000 * 480 + 240, length: 480, key: 131, velocity: VELOCITY },
    ];
    let pins = [
        Pin { start: i64::MIN, end: i64::MAX, chord: None },
        Pin { start: i64::MAX, end: i64::MIN, chord: Some(chord("Db7")) },
        Pin { start: 0, end: 960, chord: Some(chord("C")) },
        Pin { start: 240, end: 10_000_000 * 480, chord: Some(chord("Bbm7")) },
    ];
    let mut reader = ChordReader::new();
    for (beats_per_bar, ticks_per_beat) in [(4, 480), (0, 0), (u32::MAX, u32::MAX), (1, u32::MAX)] {
        for bar_line in [0, i64::MIN, i64::MAX] {
            let grid = Grid { ticks_per_beat, beats_per_bar, bar_line };
            for span in [Span::Song, Span::Loop] {
                for (k, pins) in [&pins[..0], &pins[..], &pins[1..2], &pins[2..]].into_iter().enumerate() {
                    let parts =
                        [Part { role: Role::Harmony, notes: &wild }, Part { role: Role::Bass, notes: &wild[3..] }];
                    let keep = [0, MOST_READINGS, 1, u8::MAX][k];
                    let cx = ChordContext { span, pins, readings: keep };
                    let r = reader.read(&parts, grid, &cx).unwrap_or_else(|| panic!("{grid:?} {span:?}"));
                    let kept = usize::from(keep.clamp(1, MOST_READINGS));
                    if let Err(why) = keeps_the_rules(reader.hearing(), &r, kept) {
                        panic!("{why}\n{grid:?} {span:?} {pins:?}");
                    }
                    assert!(r.truncated);
                    let _ =
                        (r.names(Some(Key::minor(PitchClass::new(11)))), r.check_names(&[chord("C"), chord("F#7")]));
                }
            }
        }
    }
}

/// What the hand cases, the tests' music and some music made here read at each version of the reader, as a hash
/// ([`readings_hash`]). A change that moves any of them needs `CHORD_READER_VERSION` bumped, and a line added here.
const READINGS: &[(u32, u64)] = &[(1, 0xe3a9_cd16_4f69_5725)];

/// The music whose readings the hash covers, with its pins: every hand case, the music the rules are checked on, and
/// music made here so every kind of answer is read (one line, one note, all twelve, a rest, chords given) and what
/// is one line and what isn't (a triad in a tune part, short stabs, a double stop in a long tune).
fn hashed_music() -> Vec<(Music, Vec<Pin>)> {
    let mut music = the_music_in_ticks();
    let given = |start: i64, end: i64, name: &str| Pin { start, end, chord: Some(chord(name)) };
    music.extend([
        (Music::new().tune(line("C4 E4 G4 E4", 0, BEAT)), Vec::new()),
        (Music::new().keys(notes("C4", 0, BAR)), Vec::new()),
        (Music::new().keys(notes("C4 C#4 D4 D#4 E4 F4 F#4 G4 G#4 A4 A#4 B4", 0, BAR)), Vec::new()),
        (Music::new().bass(bars("C2 | | | F2")).keys(bars("C4 E4 G4 | | | F3 A3 C4")), Vec::new()),
        (two_five_one(), vec![given(BAR, 2 * BAR, "Db7"), Pin { start: 2 * BAR, end: 4 * BAR, chord: None }]),
        (Music::new().bass(bars("C2 | | F2")).keys(bars("C4 E4 G4 | | F3 A3 C4")), vec![given(BAR, 2 * BAR, "Db7")]),
        (two_five_one().looped(), vec![given(3 * BAR, 4 * BAR, "Db7"), Pin { start: 0, end: BAR, chord: None }]),
        (Music::new().tune(notes("C4 E4 G4", 0, BAR)), Vec::new()),
        (Music::new().keys((0..8).flat_map(|beat| notes("C4 E4 G4", beat * BEAT, 20)).collect()), Vec::new()),
        (
            Music::new()
                .tune([line(&"C4 E4 G4 C5 G4 E4 C4 G3 ".repeat(3), 0, BEAT), notes("E3", 23 * BEAT, BEAT)].concat()),
            Vec::new(),
        ),
    ]);
    music
}

/// A hash of what the music the version covers reads ([`hashed_music`]), with every reading kept: each span's place,
/// answer and flags, its chances to six places, and each reading's chord, the other way to write it, its roots, its
/// bass, whether it's rootless, its doubtful tensions, its chance to six places and its group; and the fact and the
/// summary the MIDI page keeps of it.
///
/// The chances come through `exp`, whose last bit can differ from one machine's maths library to another's. Rounding to
/// six places makes that very unlikely to tip a number over, and the fact's chances, to the nearest 255th, are
/// coarser still; CI runs this on Windows, macOS and Linux, so if it ever does, CI says so on that machine and the
/// rounding can be made coarser.
fn readings_hash() -> u64 {
    let six = |x: f64| (x * 1e6).round() as i128;
    let chord = |h: &mut Fnv, c: Chord| {
        h.number(i128::from(c.root().number()));
        h.number(i128::from(c.notes().bits()));
        h.number(i128::from(c.bass().number()));
    };
    let mut h = Fnv::new();
    // The numbers the reader is built from, so a change to one is a change to what the version covers, though no
    // reading here moves.
    h.number(i128::from(numbers_hash(&SHIPPED)));
    for (music, pins) in hashed_music() {
        let Some(r) = read(&music, &pins, MOST_READINGS) else {
            h.bytes(&[0]);
            continue;
        };
        h.bytes(format!("{:?} {:?} {} {} {}", r.texture, r.unit, r.notes, r.truncated, r.looped).as_bytes());
        // What the MIDI page keeps of it: a change to either needs it read again.
        h.bytes(&r.fact().0);
        h.bytes(&r.summary().0);
        h.number(i128::from(r.start));
        h.number(i128::from(r.end));
        for s in &r.spans {
            h.bytes(format!("{:?} {:?} {}", s.units, s.answer, s.flags.bits()).as_bytes());
            for t in [s.start, s.end, s.heard_start] {
                h.number(i128::from(t));
            }
            for x in [s.change, s.no_chord, s.other] {
                h.number(six(x));
            }
            for x in &s.readings {
                chord(&mut h, x.chord);
                match x.also {
                    Some(also) => chord(&mut h, also),
                    None => h.bytes(&[0]),
                }
                h.bytes(format!("{:?} {} {}", x.bass, x.rootless, x.group).as_bytes());
                h.number(i128::from(x.roots.bits()));
                h.number(i128::from(x.doubtful.bits()));
                h.number(six(x.chance));
            }
        }
    }
    h.finish()
}

#[test]
fn a_change_that_moves_a_reading_bumps_the_version() {
    let now = readings_hash();
    let kept = READINGS.iter().find(|(v, _)| *v == CHORD_READER_VERSION).map(|(_, h)| *h);
    // Once CHORD_READER_VERSION is bumped, its readings only need adding.
    let fix = match kept {
        None => format!(
            "no readings are kept for version {CHORD_READER_VERSION} yet: add ({CHORD_READER_VERSION}, {now:#018x})"
        ),
        Some(_) => format!(
            "what the music reads differs from what was kept for version {CHORD_READER_VERSION}: if the change is \
             meant, bump CHORD_READER_VERSION and add ({}, {now:#018x})",
            CHORD_READER_VERSION + 1
        ),
    };
    assert_eq!(kept, Some(now), "{fix} to READINGS in chordread/reader_tests.rs");
    assert!(READINGS.windows(2).all(|w| w[0].0 < w[1].0), "versions only go up");
    // The hash covers every kind of answer, and chords given.
    let mut kinds: Vec<ChordAnswer> = Vec::new();
    let mut given = false;
    for (music, pins) in hashed_music() {
        for s in read(&music, &pins, 2).map(|r| r.spans).unwrap_or_default() {
            given |= s.flags.contains(SpanFlags::GIVEN);
            if !kinds.contains(&s.answer) {
                kinds.push(s.answer);
            }
        }
    }
    assert!(given && ChordAnswer::ALL.iter().all(|a| kinds.contains(a)), "{kinds:?}");
}

#[test]
fn the_version_is_bumped_when_a_number_changes() {
    let (hash, version) = SHIPPED_HASH;
    let now = numbers_hash(&SHIPPED);
    assert_eq!(
        now,
        hash,
        "a number the reader is built from has changed: write SHIPPED_HASH = {now:#018x} and HASH_VERSION = {} in \
         chordread/weights.rs, and bump CHORD_READER_VERSION in chordread/mod.rs to match",
        CHORD_READER_VERSION + 1
    );
    // The version the numbers were first given with; a bump for a change to the code alone leaves it behind.
    assert!(
        version <= CHORD_READER_VERSION,
        "weights.rs was written for version {version}: bump CHORD_READER_VERSION in chordread/mod.rs to {version}"
    );
    // Every number counts: the smallest change to any one of them changes the hash.
    let nudge = |x: f64| f64::from_bits(x.to_bits() + 1);
    let (mut features, mut pair, mut none) = (*SHIPPED.weights, *SHIPPED.weights, *SHIPPED.weights);
    let last = features.features.0.len() - 1;
    features.features.0[last] = nudge(features.features.0[last]);
    pair.pair = nudge(pair.pair);
    none.none = nudge(none.none);
    let (mut core, mut tension, mut motion) = (*SHIPPED.tables, *SHIPPED.tables, *SHIPPED.tables);
    core.core[15] = nudge(core.core[15]);
    tension.tension[3][11] = nudge(tension.tension[3][11]);
    motion.motion[2][7][1] = nudge(motion.motion[2][7][1]);
    let (mut one, mut same) = (*SHIPPED.thresholds, *SHIPPED.thresholds);
    one.one[2] = nudge(one.one[2]);
    same.same[3] = nudge(same.same[3]);
    for changed in [
        Model { weights: &features, ..SHIPPED },
        Model { weights: &pair, ..SHIPPED },
        Model { weights: &none, ..SHIPPED },
        Model { tables: &core, ..SHIPPED },
        Model { tables: &tension, ..SHIPPED },
        Model { tables: &motion, ..SHIPPED },
        Model { thresholds: &one, ..SHIPPED },
        Model { thresholds: &same, ..SHIPPED },
    ] {
        assert_ne!(numbers_hash(&changed), now);
    }
}

#[test]
#[ignore = "prints what each hand case reads, with its answers and chances"]
fn show_the_answers() {
    for (name, music) in hand_cases() {
        match read(&music, &[], 6) {
            Some(r) => println!("{name}: {}", render(&r)),
            None => println!("{name}: nothing"),
        }
    }
}
