//! Tests of what the MIDI page keeps of a reading ([`super::fact`]): a fact read back is the reading it was made from
//! (its chances to the nearest 255th), for any music with any pins; its bytes are laid out as the design says; a fact
//! of another layout, or a damaged one, reads as none and never panics; and the summary holds the progression in any
//! key, the cores and tensions the chords are written with, and what else they have.

use super::decode_tests::small_music;
use super::fact::{BASS_KINDS, answer_code, bass_code};
use super::hear_tests::{BAR, Dice, Music, VELOCITY, notes, random_music};
use super::reader_tests::{case, chord, random_pins, read, the_music_in_ticks};
use super::score_tests::bars;
use super::{
    BassKind, ChordAnswer, ChordFact, ChordReading, ChordSpan, ChordSummary, Core, CoreSet, Extensions, FACT_LAYOUT,
    Hearing, Label, MOST_READINGS, MOST_UNITS, NoChord, Pin, Reading, SpanFlags, SummaryFlags, Texture, Unit,
};
use crate::key::Key;
use crate::notes::{Grid, Note};
use crate::pitch::PitchClass;

/// Whether a fact read back is the reading it was made from, and if not, why: the same header and grid; the same
/// stretches, each with its answer, its flags, its first reading (its chance to the nearest 255th) and its runner-up;
/// the same chart in any key and the same summary; and each stretch's ticks where the reading has them.
fn kept(r: &ChordReading) -> Result<(), String> {
    let fact = r.fact();
    let back = fact.read().ok_or_else(|| format!("not read back: {fact:?}"))?;
    let check = |yes: bool, what: &str| if yes { Ok(()) } else { Err(format!("{what} differs: {back:?}")) };
    let near = |a: f64, b: f64| (a - b).abs() <= 0.5 / 255.0 + 1e-12;
    check(fact.0.len() == 22 + 11 * r.spans.len(), "the length")?;
    let head = (back.unit, back.texture, back.looped, back.truncated, back.start);
    check(head == (r.unit, r.texture, r.looped, r.truncated, r.start), "the header")?;
    let grid = (back.grid.ticks_per_beat, back.grid.beats_per_bar, back.grid.bar_line);
    check(grid == (r.grid.ticks_per_beat, r.grid.beats_per_bar, r.start), "the grid")?;
    check(back.units == r.spans.last().map_or(0, |s| s.units.end) && back.spans.len() == r.spans.len(), "the units")?;
    for (b, s) in back.spans.iter().zip(&r.spans) {
        let (first, second) = (s.readings.first(), s.readings.get(1));
        let stretch = (&b.units, b.answer, b.flags, b.decoded);
        check(stretch == (&s.units, s.answer, s.flags, s.label.is_some()), "a stretch")?;
        check((b.label, b.chord) == (first.map(|x| x.label), first.map(|x| x.chord)), "a first reading")?;
        let stands = |x: &Reading| (x.bass, x.rootless, !x.doubtful.is_empty(), x.also.is_some());
        let none = (BassKind::Root, false, false, false);
        check((b.bass, b.rootless, b.doubtful, b.also) == first.map_or(none, stands), "how a first reading stands")?;
        check(near(b.chance, first.map_or(0.0, |x| x.chance)), "a first reading's chance")?;
        let same = |x: &Reading| first.is_some_and(|f| f.group == x.group);
        check(b.runner_up.map(|x| (x.label, x.same_notes)) == second.map(|x| (x.label, same(x))), "a runner-up")?;
        check(near(b.runner_up.map_or(0.0, |x| x.chance), second.map_or(0.0, |x| x.chance)), "a runner-up's chance")?;
        check(b.shown() == s.shown().map(|x| x.chord), "the chord shown")?;
        check(back.tick(s.units.start) == s.start && back.tick(s.units.end) == s.end, "the ticks")?;
    }
    let keys = [Key::major(PitchClass::C), Key::minor(PitchClass::new(3)), Key::major(PitchClass::new(6))];
    for key in [None].into_iter().chain(keys.map(Some)) {
        check(back.names(key) == r.names(key), "the chart")?;
    }
    check(back.summary() == r.summary(), "the summary")
}

#[test]
fn a_fact_read_back_is_the_reading_it_was_made_from() {
    for (music, pins) in the_music_in_ticks() {
        for keep in [1, 2, MOST_READINGS] {
            let r = read(&music, &pins, keep).unwrap();
            assert_eq!(r.grid, music.grid);
            kept(&r).unwrap_or_else(|why| panic!("{why}\n{music:?}"));
        }
    }
    // Any music with any pins, on any grid.
    let mut dice = Dice(0x2f8b_31c4_77d0_9e15);
    let mut hearing = Hearing::new();
    let mut read_back = 0;
    for round in 0..300 {
        let music = if round % 2 == 0 { random_music(&mut dice) } else { small_music(&mut dice) };
        if !music.hear_into(&mut hearing) {
            continue;
        }
        let pins = random_pins(&mut dice, &hearing);
        let r = read(&music, &pins, dice.pick(&[1, 2, 6])).unwrap();
        kept(&r).unwrap_or_else(|why| panic!("round {round}: {why}\n{music:?}, pins {pins:?}"));
        read_back += 1;
    }
    assert!(read_back > 250, "{read_back} read back");
}

// The layout's numbers are written out again here, not taken from the code, so the tests check the layout.

/// A label in a byte, as a fact keeps it.
fn label_byte(label: Label) -> u8 {
    label.root.number() << 4 | label.core.index() as u8
}

/// A chance out of 255, as a fact keeps it.
fn out_of_255(chance: f64) -> u8 {
    (chance * 255.0).round() as u8
}

/// The last four bytes of a stretch read as one chord, with a runner-up: its answer (the best way reads a chord
/// there), its chance and its runner-up's.
fn answer_and_chances(s: &ChordSpan) -> [u8; 4] {
    let (first, second) = (&s.readings[0], &s.readings[1]);
    let same = u8::from(first.group == second.group) << 3;
    [0x10 | same, out_of_255(first.chance), label_byte(second.label), out_of_255(second.chance)]
}

#[test]
fn a_fact_is_laid_out_as_the_design_says() {
    // C | F over their roots, a bar each: read on half beats, with a bass and chords, from tick 0, in 16 units, on
    // 4/4 at 480 ticks a beat; each stretch is one chord, read on its root and written there over its root, its notes
    // the major triad's.
    let r = read(&Music::new().bass(bars("C2 | F2")).keys(bars("C4 E4 G4 | F3 A3 C4")), &[], 2).unwrap();
    let grid = [0, 0, 0x01, 0xE0, 0, 0, 0, 4];
    let mut want = vec![FACT_LAYOUT, 0b0010, 0, 16, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0];
    want.extend(grid);
    want.extend([0, 0, 0x00, 0x00, 0x00, 0x91, 0]);
    want.extend(answer_and_chances(&r.spans[0]));
    want.extend([0, 8, 0x50, 0x50, 0x00, 0x91, 0]);
    want.extend(answer_and_chances(&r.spans[1]));
    assert_eq!(r.fact().0, want);
    // G9sus4 | rest | C as a loop: G A C F over G is written on G, the same notes written another way too (F/G); the
    // rest has no reading, no bass and nothing read there.
    let looped = Music::new().bass(bars("G1 | | C2")).keys(bars("F3 A3 C4 | | C4 E4 G4")).looped();
    let r = read(&looped, &[], 2).unwrap();
    let mut want = vec![FACT_LAYOUT, 0b1010, 0, 24, 0, 3, 0, 0, 0, 0, 0, 0, 0, 0];
    want.extend(grid);
    want.extend([0, 0, label_byte(r.spans[0].readings[0].label), 0x70, 0x44, 0x25, 0]);
    want.extend(answer_and_chances(&r.spans[0]));
    want.extend([0, 8, 0xFF, 0, 0, 0, 0x40, 0x04, 0, 0xFF, 0]);
    want.extend([0, 16, 0x00, 0x00, 0x00, 0x91, 0]);
    want.extend(answer_and_chances(&r.spans[2]));
    assert_eq!(r.fact().0, want);
    assert_eq!(r.spans[1].answer, ChordAnswer::NoChord(NoChord::Rest));
}

/// A fact made by hand: on half beats with no bass, starting a beat in, two bars of 4/4 at 480 ticks a beat long.
/// First a diminished seventh read on E, written on C# (as before Dm), sure at 0.8, the same notes as its runner-up A7
/// at 0.1; then a rest.
fn by_hand() -> ChordFact {
    let mut bytes = vec![FACT_LAYOUT, 0b0100, 0, 16, 0, 2, 0, 0, 0, 0, 0, 0, 0x01, 0xE0, 0, 0, 0x01, 0xE0, 0, 0, 0, 4];
    bytes.extend([0, 0, 0x49, 0x10, 0x02, 0x49, 0x44, 0x18, 204, 0x92, 26]);
    bytes.extend([0, 8, 0xFF, 0, 0, 0, 0x40, 0x04, 0, 0xFF, 0]);
    ChordFact(bytes)
}

#[test]
fn a_fact_made_by_hand_reads_as_its_bytes_say() {
    let back = by_hand().read().unwrap();
    let head = (back.unit, back.texture, back.looped, back.truncated, back.start, back.units);
    assert_eq!(head, (Unit::HalfBeat, Texture::HarmonyOnly, false, false, 480, 16));
    assert_eq!(back.grid, Grid { ticks_per_beat: 480, beats_per_bar: 4, bar_line: 480 });
    let [dim, rest] = &back.spans[..] else { panic!("{back:?}") };
    // The root it's read on isn't the one it's written on: the fact keeps both.
    assert_eq!(dim.label.map(|l| (l.root, l.core)), Some((PitchClass::E, Core::Diminished7)));
    assert_eq!(dim.chord, Some(chord("C#dim7")));
    let stretch = (dim.units.clone(), dim.bass, dim.answer, dim.flags, dim.decoded);
    assert_eq!(stretch, (0..8, BassKind::None, ChordAnswer::Chord, SpanFlags::NO_BASS, true));
    assert_eq!((dim.rootless, dim.doubtful, dim.also), (false, false, false));
    assert!((dim.chance - 0.8).abs() < 0.5 / 255.0);
    let runner_up = dim.runner_up.unwrap();
    assert_eq!(
        (runner_up.label.root, runner_up.label.core, runner_up.same_notes),
        (PitchClass::A, Core::Dominant7, true)
    );
    assert!((runner_up.chance - 0.1).abs() < 0.5 / 255.0);
    assert_eq!((rest.units.clone(), rest.label, rest.chord, rest.runner_up), (8..16, None, None, None));
    let how = (rest.answer, rest.flags, rest.decoded, rest.chance);
    assert_eq!(how, (ChordAnswer::NoChord(NoChord::Rest), SpanFlags::NO_BASS, false, 0.0));
    // Its chart, and where its units start: a beat after the bar line, half a beat a unit.
    let names: Vec<Option<String>> = back.names(None).iter().map(|s| s.map(|s| s.name())).collect();
    assert_eq!(names, [Some("C#dim7".to_owned()), None]);
    assert_eq!([0, 1, 8, 16, 17, usize::MAX].map(|u| back.tick(u)), [480, 720, 2400, 4320, 4320, 4320]);
}

#[test]
fn a_fact_of_another_layout_or_a_damaged_one_reads_as_none() {
    let fact = by_hand();
    let with = |at: usize, byte: u8| {
        let mut damaged = fact.clone();
        damaged.0[at] = byte;
        damaged.read()
    };
    // Another layout; a bit no layout-1 header sets; more units than a reader hears; a unit the grid doesn't read on
    // (a beat on 4/4, or half a beat on 6/8); stretches that aren't as many as it says, or none at all.
    assert_eq!(with(0, FACT_LAYOUT + 1), None);
    for (at, byte) in [(1, 0x24), (1, 0x44), (1, 0x84), (2, 0x40), (1, 0b0101), (21, 6), (5, 0), (5, 1), (5, 3)] {
        assert_eq!(with(at, byte), None, "byte {at} as {byte:#04x}");
    }
    assert!(usize::from(u16::from_be_bytes([0x40, 16])) > MOST_UNITS);
    let (mut short, mut long, mut empty) = (fact.clone(), fact.clone(), fact.clone());
    short.0.pop();
    long.0.push(0);
    empty.0.truncate(22);
    empty.0[5] = 0;
    for damaged in [short, long, empty, ChordFact(Vec::new()), ChordFact(fact.0[..21].to_vec())] {
        assert_eq!(damaged.read(), None, "{damaged:?}");
    }
    // Stretches out of order: the first not at the start, one not after the one before, the last not before the end.
    assert_eq!(with(23, 1), None);
    assert_eq!(with(34, 0), None);
    assert_eq!(with(3, 8), None);
    // Roots and basses that aren't pitch classes; a bit no layout-1 stretch sets; a bass, an answer or a runner-up no
    // fact has; a rest with a reading; a stretch with no reading that has some of one; a runner-up's chance or same
    // notes with no runner-up.
    for (at, byte) in [
        (24, 0xC9),
        (25, 0xC0),
        (25, 0x0C),
        (26, 0x82),
        (28, 0x45),
        (28, 0x47),
        (29, 0x16),
        (29, 0x58),
        (29, 0x98),
        (31, 0xC2),
        (29, 0x14),
        (24, 0xFF),
        (36, 0x10),
        (37, 0x01),
        (39, 0x41),
        (40, 0x14),
        (41, 1),
        (42, 0x92),
        (43, 1),
        (40, 0x0C),
    ] {
        assert_eq!(with(at, byte), None, "byte {at} as {byte:#04x}");
    }
    // Another grid with the same unit (3/4 at 448 ticks a beat), another runner-up, a sure first reading, or a rest
    // with a bass under it, read.
    for (at, byte) in [(21, 3), (17, 0xC0), (31, 0x2E), (32, 255), (39, 0)] {
        assert!(with(at, byte).is_some(), "byte {at} as {byte:#04x}");
    }
    // Damaged at random, or not a fact at all: read as none, or as a reading the page can show, never with a panic.
    let mut dice = Dice(0x9e37_79b9_7f4a_7c15);
    let real = read(&Music::new().bass(bars("C2 | F2 | G2")).keys(bars("C4 E4 G4 | F3 A3 C4 | G3 B3 D4")), &[], 2);
    let facts = [fact.0.clone(), real.unwrap().fact().0];
    let mut read_back = 0;
    for round in 0..20_000 {
        let mut bytes = facts[round % 2].clone();
        if round % 5 == 0 {
            bytes = (0..dice.below(48)).map(|_| dice.below(256) as u8).collect();
        } else {
            for _ in 0..=dice.below(3) {
                let at = dice.below(bytes.len() as u64) as usize;
                bytes[at] = dice.below(256) as u8;
            }
        }
        let Some(back) = ChordFact(bytes).read() else { continue };
        read_back += 1;
        let spans = &back.spans;
        assert_eq!((spans[0].units.start, spans[spans.len() - 1].units.end), (0, back.units), "{back:?}");
        assert!(spans.windows(2).all(|w| w[0].units.end == w[1].units.start), "{back:?}");
        assert!(spans.iter().all(|s| !s.units.is_empty() && (0.0..=1.0).contains(&s.chance)), "{back:?}");
        let _ = (back.names(Some(Key::minor(PitchClass::new(1)))), back.summary(), back.tick(usize::MAX));
    }
    assert!(read_back > 1000, "{read_back} read back");
}

/// What chords given read, a bar each over their own notes (the bass under the rest), as a song: written as given,
/// whatever the numbers read, so the summary's rules are checked on chords that stay put.
fn given(names: &[&str]) -> ChordReading {
    given_as(names, false)
}

/// What chords given read ([`given`]), as a song or a loop.
fn given_as(names: &[&str], looped: bool) -> ChordReading {
    let (mut bass, mut keys, mut pins) = (Vec::new(), Vec::new(), Vec::new());
    for (bar, name) in names.iter().enumerate() {
        let c = chord(name);
        let start = bar as i64 * BAR;
        let note = |key: u8| Note { start, length: BAR, key, velocity: VELOCITY };
        bass.push(note(36 + c.bass().number()));
        keys.extend(c.notes().iter().map(|pc| note(60 + pc.number())));
        pins.push(Pin { start, end: start + BAR, chord: Some(c) });
    }
    let music = Music::new().bass(bass).keys(keys);
    let r = read(&if looped { music.looped() } else { music }, &pins, 2).unwrap();
    let written: Vec<String> = r.spans.iter().map(|s| s.shown().map_or("N".to_owned(), |x| x.chord.name())).collect();
    assert_eq!((written, r.looped), (names.iter().map(|&n| n.to_owned()).collect(), looped), "{r:?}");
    r
}

#[test]
fn the_progression_is_the_same_in_any_key_however_the_chords_are_voiced() {
    let two_five_one = given(&["Dm7", "G7", "Cmaj7"]).summary().progression();
    assert_ne!(two_five_one, 0);
    // In another key, with tensions or over other basses, the roots above the first one's and the cores are the same;
    // and a chord held over stretches side by side is counted once.
    for names in [
        &["Ebm7", "Ab7", "Dbmaj7"][..],
        &["Dm9", "G13", "Cmaj9"],
        &["Dm7/C", "G7/B", "Cmaj7/E"],
        &["Bm11", "E7alt", "Amaj9"],
        &["Dm7", "Dm9", "G7", "Cmaj7", "Cmaj7/E"],
    ] {
        assert_eq!(given(names).summary().progression(), two_five_one, "{names:?}");
    }
    // Another root, another core, another order (a loop's from its first chord) or more chords is another progression.
    for names in [
        &["Dm7", "Db7", "Cmaj7"][..],
        &["Dm7", "G7", "C"],
        &["G7", "Cmaj7", "Dm7"],
        &["Dm7", "G7", "Cmaj7", "Fmaj7"],
        &["Dm7", "G7"],
        &["Dm7", "G7", "Dm7", "Cmaj7"],
    ] {
        assert_ne!(given(names).summary().progression(), two_five_one, "{names:?}");
    }
    // Round a loop, a chord held over its seam counts once, and a loop that plays the same chords over again is the
    // loop played once: C F G C is C F G, as the same loop started a bar before (C C F G) is, and C F C F is C F. A
    // song has no seam, and a loop counts from the chord it starts on.
    let progression = |names: &[&str], looped: bool| given_as(names, looped).summary().progression();
    let c_f_g = progression(&["C", "F", "G"], false);
    for names in [&["C", "F", "G", "C"][..], &["C", "C", "F", "G"], &["C", "F", "G", "C", "F", "G"], &["C", "F", "G"]] {
        assert_eq!(progression(names, true), c_f_g, "{names:?}");
    }
    assert_eq!(progression(&["C", "F", "C", "F"], true), progression(&["C", "F"], false));
    assert_ne!(progression(&["C", "F", "G", "C"], false), c_f_g);
    assert_ne!(progression(&["C", "F", "C", "F"], false), progression(&["C", "F"], false));
    assert_ne!(progression(&["F", "G", "C"], true), c_f_g);
    // Stretches with no chord are left out: C | rest | F is C | F.
    let c_f = read(&Music::new().bass(bars("C2 | F2")).keys(bars("C4 E4 G4 | F3 A3 C4")), &[], 2).unwrap();
    let rest = read(&Music::new().bass(bars("C2 | | F2")).keys(bars("C4 E4 G4 | | F3 A3 C4")), &[], 2).unwrap();
    assert_eq!(rest.spans[1].answer, ChordAnswer::NoChord(NoChord::Rest));
    assert_eq!(rest.summary().progression(), c_f.summary().progression());
    // With no chord shown there's nothing to search by: one note alone.
    let one = read(&Music::new().keys(notes("C4", 0, BAR)), &[], 2).unwrap();
    assert_eq!(one.summary(), ChordSummary::default(), "{one:?}");
    // The whole summary is the same for the same music in any key.
    for (music, pins) in the_music_in_ticks() {
        let want = read(&music, &pins, 2).unwrap().summary();
        for shift in [-5, 1, 6] {
            let moved = music.map(|n| Note { key: (i32::from(n.key) + shift) as u8, ..n });
            let up = |p: &Pin| Pin { chord: p.chord.map(|c| c.transposed(shift)), ..*p };
            let pins: Vec<Pin> = pins.iter().map(up).collect();
            assert_eq!(read(&moved, &pins, 2).unwrap().summary(), want, "moved {shift}: {music:?}");
        }
    }
}

#[test]
fn the_summary_holds_the_cores_and_tensions_the_chords_are_written_with() {
    use Core::{Dominant7, Dominant7Sus4, Major, Major6, Major7, Minor7};
    use Extensions as E;
    // Each core, with no tension: a sixth, and a sus chord's 2 and 4, are none.
    let plain = [
        "C", "Cm", "C7", "Cmaj7", "Cm7", "C6", "Cm6", "Cm(maj7)", "Cm7b5", "Cdim7", "Cdim", "C+", "Csus4", "Csus2",
        "C7sus4", "C5",
    ];
    for (name, core) in plain.into_iter().zip(Core::ALL) {
        let s = given(&[name]).summary();
        assert_eq!((s.cores(), s.extensions()), (CoreSet::EMPTY.with(core), E::NONE), "{name}");
    }
    assert_eq!(given(&plain).summary().cores(), Core::ALL.into_iter().collect::<CoreSet>());
    // The tensions as the names write them, the cores as the chords are written: Cmaj9 is a major seventh.
    let altered = E::ALTERED.with(E::FLAT_NINTH).with(E::SHARP_NINTH).with(E::SHARP_ELEVENTH).with(E::FLAT_THIRTEENTH);
    let ninths = &["C9", "Cm9", "Cmaj9", "C6/9", "Cadd9", "C9sus4"][..];
    let ii_v_i = [Minor7, Dominant7, Major7];
    for (names, cores, tensions) in [
        (ninths, &[Dominant7, Minor7, Major7, Major6, Major, Dominant7Sus4][..], E::NINTH),
        (&["C7(b9)"], &[Dominant7], E::FLAT_NINTH),
        (&["C7(#9)"], &[Dominant7], E::SHARP_NINTH),
        (&["Cm11"], &[Minor7], E::NINTH.with(E::ELEVENTH)),
        (&["Cmaj7(#11)", "C7(#11)"], &[Major7, Dominant7], E::SHARP_ELEVENTH),
        (&["C7(b13)"], &[Dominant7], E::FLAT_THIRTEENTH),
        (&["C13", "C13sus4"], &[Dominant7, Dominant7Sus4], E::NINTH.with(E::THIRTEENTH)),
        (&["C7(b9,b13)"], &[Dominant7], E::FLAT_NINTH.with(E::FLAT_THIRTEENTH)),
        // An altered dominant: its altered ninths, eleventh and thirteenth, and that it's altered.
        (&["C7alt"], &[Dominant7], altered),
        (&["Dm11", "G7alt", "Cmaj9"], &ii_v_i, altered.with(E::NINTH).with(E::ELEVENTH)),
    ] {
        let s = given(names).summary();
        assert_eq!((s.cores(), s.extensions()), (cores.iter().copied().collect(), tensions), "{names:?}");
    }
}

#[test]
fn the_summary_holds_what_else_the_chords_have() {
    use SummaryFlags as F;
    // How a bass stands under the chords written: on their roots, only a bass (D/C), or another of their notes.
    assert_eq!(given(&["C", "Dm7", "G7"]).summary().flags(), F::NONE);
    assert_eq!(given(&["D/C"]).summary().flags(), F::SLASH);
    assert_eq!(given(&["C/E", "G7/F"]).summary().flags(), F::INVERSION);
    // A rootless chord: Db7 given over C E G.
    let db7 = Pin { start: 0, end: BAR, chord: Some(chord("Db7")) };
    let r = read(&Music::new().keys(bars("C4 E4 G4")), &[db7], 2).unwrap();
    assert_eq!(r.summary().flags(), F::ROOTLESS, "{r:?}");
    // What's heard: a pedal, chords pushed, a walking bass, and a sus chord over its bass that's also a chord over a
    // bass that's only a bass.
    // A triad over its seventh is written another way too (D/C, also D7/C), but it's no sus chord: only a slash.
    for (name, want) in [
        ("3 D/C", F::SLASH),
        ("9 C | F/C | G/C | C over a pedal", F::PEDAL.with(F::INVERSION).with(F::SLASH)),
        ("44 C | F pushed", F::PUSHED),
        ("36 a walk under F A C", F::WALKING),
        ("7 G9sus4", F::HYBRID),
    ] {
        assert_eq!(case(name).summary().flags(), want, "{name}");
    }
    // Unclear for more than a quarter of the time the music plays, rests left out; and only the chords the chart shows
    // count.
    let mut r = given(&["C", "F", "G", "Cm7b5"]);
    r.spans[3].answer = ChordAnswer::Unclear;
    assert_eq!(r.summary().flags(), F::NONE);
    let rest = ChordAnswer::NoChord(NoChord::Rest);
    r.spans[0] = ChordSpan { answer: rest, label: None, readings: Vec::new(), ..r.spans[0].clone() };
    assert_eq!(r.summary().flags(), F::UNCLEAR);
    assert!(r.summary().cores().contains(Core::HalfDiminished));
    r.spans[3].label = None;
    assert!(!r.summary().cores().contains(Core::HalfDiminished));
}

#[test]
fn a_fact_numbers_answers_basses_and_textures_by_their_places() {
    for (i, answer) in ChordAnswer::ALL.into_iter().enumerate() {
        assert_eq!(usize::from(answer_code(answer)), i, "{answer:?}");
    }
    for (i, bass) in BASS_KINDS.into_iter().enumerate() {
        assert_eq!(usize::from(bass_code(bass)), i, "{bass:?}");
    }
    for (i, texture) in Texture::ALL.into_iter().enumerate() {
        assert_eq!(texture.index(), i, "{texture:?}");
    }
}
