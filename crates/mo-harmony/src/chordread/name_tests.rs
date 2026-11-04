//! Tests of the namer ([`super::name`]): the design's hand cases read whole and named as a player writes them, and the
//! rules that hold for any music (a name never claims a note that isn't played, the bass kind fits the name, the
//! readings' chances add up to at most 1, the same notes moved are named the same, moved, and the same music at another
//! resolution or in another order is named the same).

use std::ops::Range;

use super::cases::{NameWant as Want, Named, WrittenSpan as Span, named_verdicts, render_named as render};
use super::decode_tests::{dense, random_pins, small_music, song, the_music, whole_cases};
use super::hear_tests::{BEAT, Dice, Music, VELOCITY, line, random_music};
use super::score_tests::{bars, hand_cases};
use super::{
    BassKind, COUNTED, Decoder, Decoding, HAND, Hearing, Label, Namer, Naming, PinnedSpan, Reading, SHIPPED, SpanFlags,
};
use crate::chord::Chord;
use crate::keyfind::Span as Stretch;
use crate::notes::{Grid, Note, Part, Role};
use crate::pitch::PcSet;

impl Named {
    /// Some music read whole on the counted tables, as the reader reads it, and named.
    fn read(music: &Music, pins: &[PinnedSpan], most: usize) -> Named {
        Named::of(music, &HAND, &COUNTED, pins, most).expect("nothing read")
    }
}

/// A bar of 4/4, counted in units.
const B: usize = 8;

/// The design's hand cases, named ([`super::cases::named_cases`]).
pub(super) fn named_cases() -> Vec<(&'static str, Music, Vec<Want>)> {
    super::cases::named_cases().unwrap()
}

#[test]
fn the_hand_cases_are_named_as_a_player_writes_them() {
    let verdicts = named_verdicts(&SHIPPED, false).unwrap().into_iter();
    let failed: Vec<String> =
        verdicts.filter(|v| !v.holds()).map(|v| format!("{}: {}\n{}", v.name, v.wrong.join(", "), v.read)).collect();
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
/// sorted after the first, adding up to at most 1 with the chance of no chord; no two readings are named the same;
/// a name holds only notes that sound, and its root when it isn't rootless (over silence, it's written in full, as the
/// decode read it); the bass kind fits the name, and is the
/// same kind of bass on every reading of a span; and readings share a same-notes group when they hold the same notes.
fn keeps_the_rules(
    hearing: &Hearing,
    decoding: &Decoding,
    pins: &[PinnedSpan],
    naming: &Naming,
    most: usize,
) -> Result<(), String> {
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
        // A pinned stretch is never cut in two, and a chord given for it (one the decode can read) is its only reading,
        // written as given: in root position over the bass held under it.
        // The decode says which: a chord given is one of the pins' (a pin's own, or round a loop, one given over its
        // last bar).
        let given = decoded.given;
        check(!decoded.pinned || !split, &why)?;
        check(given.is_none() || decoded.pinned, &why)?;
        check(given.is_none_or(|g| pins.iter().any(|p| p.given() == Some(g))), &why)?;
        check(given.is_some() == s.flags.contains(SpanFlags::GIVEN), &why)?;
        if let Some(given) = given {
            let [r] = readings else { return Err(format!("not one reading: {}", why())) };
            let c = r.chord;
            let bass = match hearing.bass(s.sound.clone()).held() {
                Some(held) if given.bass() == given.root() => held,
                _ => given.bass(),
            };
            check(c.root() == given.root() && c.notes() == given.notes() && c.bass() == bass, &why)?;
            check(r.also.is_none() && r.doubtful.is_empty() && r.group == 0, &why)?;
            continue;
        }
        let sounding = sounding_in(hearing, s.sound.clone());
        let silent = hearing.heard(s.sound.clone()).is_empty();
        let mut groups: Vec<PcSet> = Vec::new();
        for (i, r) in readings.iter().enumerate() {
            let why = || format!("{} ({r:?}) in {}", r.chord.name(), why());
            let (c, written) = (r.chord, r.written);
            check(written == c.as_written(), &why)?;
            let key = written.name_key();
            check(!readings.get(..i).unwrap_or_default().iter().any(|o| o.written.name_key() == key), &why)?;
            if silent {
                // Over silence, written in full.
                check(c.notes() == r.label.tones() && !r.rootless && r.also.is_none(), &why)?;
            } else {
                check(sounding.contains_all(c.notes().union(r.doubtful)), &why)?;
                // Rootless as written: a chord written on its bass has its root heard.
                check(r.rootless || sounding.contains(written.root()), &why)?;
            }
            check(r.doubtful.intersection(c.notes()).is_empty() && r.roots.contains(written.root()), &why)?;
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
        if let Err(why) = keeps_the_rules(&hearing, &decoding, &pins, &naming, most) {
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
            if let Err(why) = keeps_the_rules(&named.hearing, &named.decoding, &pins, &named.naming, most) {
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
                chord: pin.chord.map(|c| c.transposed(shift)),
            };
            let pins: Vec<PinnedSpan> = pins.iter().map(up).collect();
            assert_eq!(everything(&Named::read(&moved, &pins, 6).naming, shift, 480), want, "moved {shift}");
        }
    }
    // Named as a player names them, moved: C#dim7 before Dm is Ddim7 before Ebm, Bb13sus4 is B13sus4, D over C (D7) is
    // Eb7, and G7 over F is Ab7.
    for (case, want) in [("24 C#dim7", "Ddim7"), ("8 Bb13sus4", "B13sus4"), ("3 D/C", "Eb7"), ("2 G7/F", "Ab7")] {
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
    // A pinned chord is written as given, however unlikely, over the bass held under it, and named from all its notes:
    // Db7 pinned over C E G with C in the bass is Db7(maj7) (the C is its major seventh), rootless; with no bass,
    // Db7. Given with a bass of its own, it keeps it, and a bass in its notes adds nothing to the name.
    let chord = |name: &str| Chord::parse(name).unwrap();
    for (bass, given, written, kind) in [
        ("C2", "Db7", "Db7(maj7)", BassKind::Slash),
        ("", "Db7", "Db7", BassKind::None),
        ("C2", "Am7", "Am7", BassKind::Inversion),
        ("C2", "Db7/F", "Db7", BassKind::Inversion),
        ("", "C/G", "C", BassKind::Inversion),
    ] {
        let music = Music::new().bass(bars(bass)).keys(bars("C4 E4 G4"));
        let pin = PinnedSpan { units: 0..B, chord: Some(chord(given)) };
        let named = Named::read(&music, std::slice::from_ref(&pin), 4);
        let spans = named.spans();
        let span = named.naming.spans().first().unwrap();
        let [r] = named.naming.readings(span) else { panic!("{}", render(&spans)) };
        assert!(span.flags.contains(SpanFlags::GIVEN), "{}", render(&spans));
        assert_eq!((r.chord.name(), r.bass), (written.to_owned(), kind), "{given}: {}", render(&spans));
        assert!(Some(r.label) == pin.label() && r.rootless == !r.chord.name().starts_with('C'));
    }
    // And never cut in two, however its bass moves: Db9 pinned over C C/E is one chord.
    let music = Music::new().bass(bars("C2 (1-2) E2 (3-4) | F2")).keys(bars("C4 E4 G4 | F3 A3 C4"));
    let pin = PinnedSpan { units: 0..B, chord: Some(chord("Db9")) };
    let spans = Named::read(&music, std::slice::from_ref(&pin), 4).spans();
    assert_eq!(spans.iter().map(|s| s.name()).collect::<Vec<_>>(), ["Db9", "F"], "{}", render(&spans));
    // Pinned over silence, it's written in full: C | Db7 | F, with nothing played in the second bar.
    let music = Music::new().bass(bars("C2 | | F2")).keys(bars("C4 E4 G4 | | F3 A3 C4"));
    let pin = PinnedSpan { units: B..2 * B, chord: Some(chord("Db7")) };
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
                if let Err(why) = keeps_the_rules(&hearing, &decoding, &[], &naming, 4) {
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
    let targets = named_verdicts(&SHIPPED, true).unwrap();
    for v in &targets {
        let verdict = if v.holds() { "met".to_string() } else { format!("not met: {}", v.wrong.join(", ")) };
        println!("{}: {verdict}\n{}", v.name, v.read);
    }
    println!("{} of {} met", targets.iter().filter(|v| v.holds()).count(), targets.len());
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
