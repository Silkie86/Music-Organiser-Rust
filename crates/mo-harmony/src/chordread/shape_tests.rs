//! Tests of the lines and the shapes of their notes ([`super::Shaped`]) and of pushed chords ([`super::Push`]): the
//! design's hand cases 36 to 48 and 53, suspensions and appoggiaturas, inner voices, and the rules that hold for any
//! notes (moved, at another resolution, shuffled, doubled an octave up).

use super::hear_tests::{BAR, BEAT, Music, key, keys_only, line, notes, with_bass};
use super::{Hearing, Line};
use crate::notes::{Note, Role};

/// A key's name, C4 being 60, with flats but F#: "Ab2".
fn name(key: u8) -> String {
    const NAMES: [&str; 12] = ["C", "Db", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];
    format!("{}{}", NAMES[usize::from(key % 12)], i32::from(key / 12) - 1)
}

fn line_name(line: Line) -> &'static str {
    match line {
        Line::Bass => "bass",
        Line::Tune(_) => "tune",
        Line::Chords => "chords",
        Line::Broken(_) => "broken",
    }
}

/// The shapes heard, a loop's copied bar left out: "bass Passing E2@480 (D F)", with where the note is struck in the
/// music's own ticks, and its anchors.
fn shapes(h: &Hearing) -> Vec<String> {
    h.shaped()
        .iter()
        .filter(|s| !h.notes()[s.note].copy)
        .map(|s| {
            let n = h.notes()[s.note];
            format!("{} {:?} {}@{} ({})", line_name(s.line), s.shape, name(n.key), h.tick(n.start), s.anchors)
        })
        .collect()
}

/// The pushes heard: "1920 <- 1680 C E G A", the felt beat and where the chord is struck in the music's own ticks, and
/// its pitch classes.
fn pushes(h: &Hearing) -> Vec<String> {
    h.pushes().iter().map(|p| format!("{} <- {} {}", h.start(p.unit), h.tick(p.at), p.pcs)).collect()
}

/// Every shape and push, for comparing music moved by `shift` semitones (moved back) or rescaled (ticks divided by
/// `scale`), in order.
fn described(h: &Hearing, shift: i32, scale: i64) -> Vec<String> {
    described_with(h, shift, |t| t / scale)
}

/// [`described`], with each tick told by `tick`.
fn described_with(h: &Hearing, shift: i32, tick: impl Fn(i64) -> i64) -> Vec<String> {
    let mut out: Vec<String> = h
        .shaped()
        .iter()
        .map(|s| {
            let (line, pc, anchors) = (line_name(s.line), s.pc.up(-shift), s.anchors.transposed(-shift));
            format!("{line} {:?} {pc} {}..{} ({anchors})", s.shape, tick(s.start), tick(s.end))
        })
        .collect();
    out.extend(h.pushes().iter().map(|p| format!("push {} {} {}", p.unit, tick(p.at), p.pcs.transposed(-shift))));
    out.sort();
    out
}

#[test]
fn a_walking_bass_steps_through_passing_notes() {
    // The design's case 36, a 1-bar loop: D2 E2 F2 G2 under F3 A3 C4. E2 passes from D2 to F2 (a step up a scale, so
    // no approach); F2 is on a stronger beat than the notes either side; G2 leaps to D2 as the loop goes round.
    let h = Music::new().looped().bass(line("D2 E2 F2 G2", 0, BEAT)).keys(notes("F3 A3 C4", 0, BAR)).hear();
    assert_eq!(shapes(&h), ["bass Passing E2@480 (D F)"]);
    // The design's case 37: with G2 held through bar 2, the G2 on beat 4 comes early to the bar of G.
    let h = Music::new()
        .looped()
        .bass([line("D2 E2 F2 G2", 0, BEAT), notes("G2", BAR, BAR)].concat())
        .keys([notes("F3 A3 C4", 0, BAR), notes("F3 A3 B3 E4", BAR, BAR)].concat())
        .hear();
    assert_eq!(shapes(&h), ["bass Passing E2@480 (D F)", "bass Anticipation G2@1440 (G)"]);
    let g2 = h.shaped().iter().find(|s| s.pc == crate::pitch::PitchClass::G && !h.notes()[s.note].copy).unwrap();
    assert_eq!((h.tick(g2.start), h.tick(g2.end)), (3 * BEAT, BAR), "shaped until the bar it comes early to");
}

#[test]
fn a_chromatic_step_into_a_bar_is_an_approach() {
    // The design's case 38: Ab2 and Db3 lead by semitones into the bars of G7 and Cmaj7, and B2 (leaping in) into
    // bar 4's C3. D3 in bar 4 passes up the scale.
    let h = Music::new()
        .looped()
        .bass(
            [
                line("D2 F2 A2 Ab2", 0, BEAT),
                line("G2 B2 D3 Db3", BAR, BEAT),
                line("C3 E3 G3 B2", 2 * BAR, BEAT),
                line("C3 D3 E3 G2", 3 * BAR, BEAT),
            ]
            .concat(),
        )
        .keys(
            [
                notes("F3 C4", 0, BAR),
                notes("F3 B3", BAR, BAR),
                notes("E3 B3", 2 * BAR, BAR),
                notes("E3 B3", 3 * BAR, BAR),
            ]
            .concat(),
        )
        .hear();
    let want = [
        "bass Approach Ab2@1440 (G)",
        "bass Approach Db3@3360 (C)",
        "bass Approach B2@5280 (C)",
        "bass Passing D3@6240 (C E)",
    ];
    assert_eq!(shapes(&h), want);
    // The design's case 39: G2 Ab2 | A2.
    let h = Music::new()
        .looped()
        .bass([line("C2 E2 G2 Ab2", 0, BEAT), notes("A2", BAR, BAR)].concat())
        .keys([notes("E4 G4 B4", 0, BAR), notes("C4 E4 G4", BAR, BAR)].concat())
        .hear();
    assert_eq!(shapes(&h), ["bass Approach Ab2@1440 (A)"]);
    // Played as a song, the last note has nothing after it.
    let h = Music::new().bass(line("C2 E2 G2 Ab2", 0, BEAT)).keys(notes("E4 G4 B4", 0, BAR)).hear();
    assert_eq!(shapes(&h), Vec::<String>::new());
}

#[test]
fn an_arpeggio_passes_through_the_scale() {
    // The design's case 41: the keys alone, in eighths, C4 D4 E4 G4 E4 D4 C4 G3. It's a broken chord, and both D4s
    // pass by.
    let h = Music::new().looped().keys(line("C4 D4 E4 G4 E4 D4 C4 G3", 0, BEAT / 2)).hear();
    assert_eq!(shapes(&h), ["broken Passing D4@240 (C E)", "broken Passing D4@1200 (C E)"]);
    assert!(h.notes().iter().filter(|n| !n.copy).all(|n| n.broken));
    // With the left hand's C2 held under it in the same part, the same: the C2 struck with the first C4 isn't broken,
    // but doesn't stop the broken line.
    let keys = [line("C4 D4 E4 G4 E4 D4 C4 G3", 0, BEAT / 2), notes("C2", 0, BAR)].concat();
    let h = Music::new().looped().keys(keys).hear();
    assert_eq!(shapes(&h), ["broken Passing D4@240 (C E)", "broken Passing D4@1200 (C E)"]);
    // A4 on the bar, leaping in from the E4 before it as the loop goes round and stepping down to G4: an appoggiatura
    // in the arpeggio, with or without the C2 struck with it.
    for bass in [Vec::new(), notes("C2", 0, BAR)] {
        let keys = [line("A4 G4 E4 C4 E4 G4 C5 E4", 0, BEAT / 2), bass].concat();
        let h = Music::new().looped().keys(keys).hear();
        assert_eq!(shapes(&h), ["broken Appoggiatura A4@0 (G)"]);
    }
}

#[test]
fn a_broken_line_goes_on_to_the_top_of_a_chord_played_together() {
    // A 1-bar loop of eighths, E4 G4 C5 G4 E4 G4 E4 D4: as it goes round, D4 turns round E4 (a neighbour).
    let h = Music::new().looped().keys(line("E4 G4 C5 G4 E4 G4 E4 D4", 0, BEAT / 2)).hear();
    assert_eq!(shapes(&h), ["broken Neighbour D4@1680 (E)"]);
    // With the loop's first beat C4 E4 G4 played together, D4 goes on to the chord's top note, G4: a leap, so it's
    // nothing (not to the E4 struck after the chord). The chord's G4 isn't shaped on the broken line.
    let keys = [notes("C4 E4 G4", 0, BEAT), line("E4 G4 C5 G4 E4 D4", BEAT, BEAT / 2)].concat();
    let h = Music::new().looped().keys(keys).hear();
    assert_eq!(shapes(&h), Vec::<String>::new());
    // With C4 E4 played together there instead, F4 after the chord turns round its top note, E4, and so does D4 as
    // the loop goes round.
    let keys = [notes("C4 E4", 0, BEAT), line("F4 E4 C4 G4 E4 D4", BEAT, BEAT / 2)].concat();
    let h = Music::new().looped().keys(keys).hear();
    assert_eq!(shapes(&h), ["broken Neighbour F4@480 (E)", "broken Neighbour D4@1680 (E)"]);
    // Chords played together on beats 1 to 3, their tops E4 F4 G4, then beat 4 broken: F4 passes on the top line of
    // the chords, and only there.
    let keys = [notes("C4 E4", 0, BEAT), notes("A3 F4", BEAT, BEAT), notes("C4 G4", 2 * BEAT, BEAT)];
    let h = Music::new().keys([keys.concat(), line("E4 C4", 3 * BEAT, BEAT / 2)].concat()).hear();
    assert_eq!(shapes(&h), ["chords Passing F4@480 (E G)"]);
}

#[test]
fn tune_notes_pass_by_turn_round_and_come_early() {
    // The design's case 47: D5 for three beats, E5 an eighth on beat 4, D5 on its second half. E5 turns round D5 (a
    // neighbour). The last D5 isn't an anticipation of the bar: D sounds through the beat before it, so it's the chord
    // going on, not one coming early (it would change nothing here, but a 9 held on top must not be hidden).
    let h = Music::new()
        .looped()
        .bass(notes("G2", 0, BAR))
        .keys(notes("G3 B3 D4", 0, BAR))
        .tune([notes("D5", 0, 3 * BEAT), line("E5 D5", 3 * BEAT, BEAT / 2)].concat())
        .hear();
    assert_eq!(shapes(&h), ["tune Neighbour E5@1440 (D)"]);
    // The design's case 48 (c): eighths C5 D5 E5 from beat 1; D5 passes by.
    let h = Music::new()
        .looped()
        .bass(notes("C2", 0, BAR))
        .keys(notes("E3 G3 B3", 0, BAR))
        .tune(line("C5 D5 E5", 0, BEAT / 2))
        .hear();
    assert_eq!(shapes(&h), ["tune Passing D5@240 (C E)"]);
    // The design's case 45: A5 struck an eighth before the bar of A minor and held into it. A tune note is never
    // pushed.
    let h = Music::new()
        .looped()
        .bass([notes("C2", 0, BAR), notes("A1", BAR, BAR)].concat())
        .keys([notes("C4 E4 G4", 0, BAR), notes("A3 C4 E4", BAR, BAR)].concat())
        .tune(notes("A5", BAR - BEAT / 2, BEAT + BEAT / 2))
        .hear();
    assert_eq!(shapes(&h), ["tune Anticipation A5@1680 (A)"]);
    assert_eq!(pushes(&h), Vec::<String>::new());
}

#[test]
fn a_note_held_into_the_bar_and_stepping_down_is_a_suspension() {
    // F5 from beat 3 held into the bar of C, then E5: a suspension from the bar line to the E.
    let h = Music::new()
        .bass([notes("F2", 0, BAR), notes("C2", BAR, BAR)].concat())
        .keys([notes("F3 A3 C4", 0, BAR), notes("C4 E4 G4", BAR, BAR)].concat())
        .tune([notes("F5", 2 * BEAT, 3 * BEAT), notes("E5", BAR + BEAT, 3 * BEAT)].concat())
        .hear();
    assert_eq!(shapes(&h), ["tune Suspension F5@960 (E)"]);
    let s = h.shaped()[0];
    assert_eq!((h.tick(s.start), h.tick(s.end)), (BAR, BAR + BEAT));
    // Struck with the bar of F and held over the bar line of C: prepared a bar before, a suspension from the bar line.
    let h = Music::new()
        .bass([notes("F2", 0, BAR), notes("C2", BAR, BAR)].concat())
        .keys([notes("F3 A3 C4", 0, BAR), notes("C4 E4 G4", BAR, BAR)].concat())
        .tune([notes("F5", 0, BAR + 2 * BEAT), notes("E5", BAR + 2 * BEAT, 2 * BEAT)].concat())
        .hear();
    assert_eq!(shapes(&h), ["tune Suspension F5@0 (E)"]);
    let s = h.shaped()[0];
    assert_eq!((h.tick(s.start), h.tick(s.end)), (BAR, BAR + 2 * BEAT));
    // Struck an eighth before the bar and held past beat 2: from the bar line, the strongest beat it's held into.
    let h = Music::new()
        .bass([notes("F2", 0, BAR), notes("C2", BAR, BAR)].concat())
        .keys([notes("F3 A3 C4", 0, BAR), notes("C4 E4 G4", BAR, BAR)].concat())
        .tune([notes("F5", BAR - BEAT / 2, 2 * BEAT), notes("E5", BAR + 3 * BEAT / 2, 5 * BEAT / 2)].concat())
        .hear();
    assert_eq!(shapes(&h), ["tune Suspension F5@1680 (E)"]);
    let s = h.shaped()[0];
    assert_eq!((h.tick(s.start), h.tick(s.end)), (BAR, BAR + 3 * BEAT / 2));
    // Held longer than the note it steps down to, it's the music's own (a sus4 chord's 4th, say).
    let h = Music::new()
        .bass([notes("F2", 0, BAR), notes("C2", BAR, BAR)].concat())
        .keys([notes("F3 A3 C4", 0, BAR), notes("C4 E4 G4", BAR, BAR)].concat())
        .tune([notes("F5", 2 * BEAT, 4 * BEAT), notes("E5", BAR + 2 * BEAT, BEAT)].concat())
        .hear();
    assert_eq!(shapes(&h), Vec::<String>::new());
    // Struck where the bar it's held into strikes its pitch class, it comes early to that bar instead: F5 an eighth
    // before the bar of F.
    let h = Music::new()
        .bass([notes("C2", 0, BAR), notes("F2", BAR, BAR)].concat())
        .keys([notes("C4 E4 G4", 0, BAR), notes("F3 A3 C4", BAR, BAR)].concat())
        .tune([notes("F5", BAR - BEAT / 2, BEAT + BEAT / 2), notes("E5", BAR + BEAT, 3 * BEAT)].concat())
        .hear();
    assert_eq!(shapes(&h), ["tune Anticipation F5@1680 (F)"]);
    // Held only into an off-beat (F5 from a sixteenth after the bar, to just past beat 2), it's nothing: a
    // suspension is held into a felt beat.
    let h = Music::new()
        .bass(notes("C2", 0, BAR))
        .keys(notes("C4 E4 G4", 0, BAR))
        .tune([notes("F5", BEAT / 4, BEAT), notes("E5", BEAT + BEAT / 4, 2 * BEAT)].concat())
        .hear();
    assert_eq!(shapes(&h), Vec::<String>::new());
    // In 3/4, held from beat 1's second half over beats 2 and 3, as strong as each other: from the first.
    let h = Music::new()
        .grid(3, 480)
        .bass(notes("C2", 0, 3 * BEAT))
        .keys(notes("C4 E4 G4", 0, 3 * BEAT))
        .tune([notes("F5", BEAT / 2, 2 * BEAT), notes("E5", 5 * BEAT / 2, 2 * BEAT)].concat())
        .hear();
    assert_eq!(shapes(&h), ["tune Suspension F5@240 (E)"]);
    let s = h.shaped()[0];
    assert_eq!((h.tick(s.start), h.tick(s.end)), (BEAT, 5 * BEAT / 2));
    // Never in the bass.
    let h = Music::new()
        .bass([notes("F2", 2 * BEAT, 3 * BEAT), notes("E2", BAR + BEAT, 3 * BEAT)].concat())
        .keys([notes("F3 A3 C4", 0, BAR), notes("C4 E4 G4", BAR, BAR)].concat())
        .hear();
    assert_eq!(shapes(&h), Vec::<String>::new());
}

#[test]
fn a_leap_onto_a_strong_beat_stepping_back_is_an_appoggiatura() {
    // D5 on beat 4, A5 leaping onto the bar of C, G5 a step down: an appoggiatura resolving to G.
    let h = Music::new()
        .bass([notes("G2", 0, BAR), notes("C2", BAR, BAR)].concat())
        .keys([notes("G3 B3 D4", 0, BAR), notes("C4 E4 G4", BAR, BAR)].concat())
        .tune([notes("D5", 3 * BEAT, BEAT), notes("A5", BAR, BEAT), notes("G5", BAR + BEAT, 3 * BEAT)].concat())
        .hear();
    assert_eq!(shapes(&h), ["tune Appoggiatura A5@1920 (G)"]);
    // On a weak beat, the same leap and step is nothing.
    let h = Music::new()
        .bass(notes("C2", 0, BAR))
        .keys(notes("C4 E4 G4", 0, BAR))
        .tune([notes("D5", 0, BEAT / 2), notes("A5", BEAT / 2, BEAT / 2), notes("G5", BEAT, BEAT / 2)].concat())
        .hear();
    assert_eq!(shapes(&h), Vec::<String>::new());
}

#[test]
fn inner_voices_are_never_lines() {
    // Block chords whose middle note moves E4 F4 E4: the top line is G4 throughout, so the F4 isn't a neighbour.
    let keys = [notes("C4 E4 G4", 0, BEAT), notes("C4 F4 G4", BEAT, BEAT), notes("C4 E4 G4", 2 * BEAT, 2 * BEAT)];
    let h = Music::new().bass(notes("C2", 0, BAR)).keys(keys.concat()).hear();
    assert_eq!(shapes(&h), Vec::<String>::new());
    // A tune holding G5 while a lower voice in the same part moves D5 E5 F5: only the G5 is the tune's line.
    let tune = [notes("G5", 0, BAR), line("D5 E5 F5", 0, BEAT)].concat();
    let h = Music::new().bass(notes("C2", 0, BAR)).keys(notes("C4 E4", 0, BAR)).tune(tune).hear();
    assert_eq!(shapes(&h), Vec::<String>::new());
    // A little legato overlap (10 ticks) doesn't make the next note an inner voice.
    let tune = [notes("C5", 0, BEAT / 2 + 10), notes("B4", BEAT / 2, BEAT / 2), notes("A4", BEAT, BEAT)].concat();
    let h = Music::new().bass(notes("A1", 0, BAR)).keys(notes("C4 E4", 0, BAR)).tune(tune).hear();
    assert_eq!(shapes(&h), ["tune Passing B4@240 (C A)"]);
    // Nor does a chord ringing a little past the bar line (held over): G4 B4 D5 to 100 ticks into bar 2, under whose
    // D5 the bar's C5 is still the top, then B4 passing down to A4.
    let keys = [
        notes("G4 B4 D5", 0, BAR + 100),
        notes("E4 A4", BAR, BEAT),
        notes("C5", BAR, BEAT / 2),
        notes("B4", BAR + BEAT / 2, BEAT / 2),
        notes("C4 F4 A4", BAR + BEAT, 3 * BEAT),
    ];
    let h = Music::new().keys(keys.concat()).hear();
    assert_eq!(shapes(&h), ["chords Passing B4@2160 (C A)"]);
}

#[test]
fn a_chord_struck_just_before_the_beat_with_the_bass_on_it_may_be_pushed() {
    // The design's case 43: Cmaj7 to 1680, then Am7 struck at 1680 and held, the bass's A1 on the bar.
    let h = Music::new()
        .looped()
        .bass([notes("C2", 0, BAR), notes("A1", BAR, BAR)].concat())
        .keys([notes("C4 E4 G4 B4", 0, BAR - BEAT / 2), notes("A3 C4 E4 G4", BAR - BEAT / 2, BAR + BEAT / 2)].concat())
        .hear();
    assert_eq!(pushes(&h), ["1920 <- 1680 C E G A"]);
    let keys: Vec<u8> = h.push_notes(&h.pushes()[0]).map(|(_, n)| n.key).collect();
    assert_eq!(keys, [key("A3"), key("C4"), key("E4"), key("G4")]);
    // The design's case 44: the bass pushed with the chord.
    let h = Music::new()
        .looped()
        .bass([notes("C2", 0, BAR - BEAT / 2), notes("F2", BAR - BEAT / 2, BAR + BEAT / 2)].concat())
        .keys([notes("C4 E4 G4", 0, BAR - BEAT / 2), notes("F3 A3 C4", BAR - BEAT / 2, BAR + BEAT / 2)].concat())
        .hear();
    assert_eq!(pushes(&h), ["1920 <- 1680 C F A"]);
    // A sixteenth early is pushed too (it isn't snapped: a quarter of a unit is a 32nd).
    let h = Music::new()
        .bass([notes("C2", 0, BAR), notes("F2", BAR, BAR)].concat())
        .keys([notes("C4 E4 G4", 0, BAR - BEAT / 4), notes("F3 A3 C4", BAR - BEAT / 4, BAR)].concat())
        .hear();
    assert_eq!(pushes(&h), ["1920 <- 1800 C F A"]);
}

#[test]
fn a_chord_isnt_pushed_over_a_bass_held_from_before_or_when_it_is_one_note() {
    let early = |keys: &str| notes(keys, BAR - BEAT / 2, BAR);
    let before = notes("C4 E4 G4", 0, BAR - BEAT / 2);
    // F A C struck early over a G2 held from bar 1, to the middle of bar 2: the bass doesn't move with it. (Held only
    // a beat into bar 2, the G2 would be held over: legato, which leaves the bar to the chord.)
    let h = Music::new().bass(notes("G2", 0, BAR + 2 * BEAT)).keys([before.clone(), early("F3 A3 C4")].concat()).hear();
    assert_eq!(pushes(&h), Vec::<String>::new());
    // One note early.
    let h = Music::new().bass(notes("C2", 0, 2 * BAR)).keys([before.clone(), early("E4")].concat()).hear();
    assert_eq!(pushes(&h), Vec::<String>::new());
    // Two notes early that are less than half of the chord sounding on the beat.
    let on_the_beat = notes("D4 E4 G4 B4", BAR, BEAT);
    let h = Music::new().bass(notes("C2", 0, 2 * BAR)).keys([before, early("F3 A3"), on_the_beat].concat()).hear();
    assert_eq!(pushes(&h), Vec::<String>::new());
    // Not held a unit past the beat: struck early and let go.
    let h = Music::new()
        .bass([notes("C2", 0, BAR), notes("F2", BAR, BAR)].concat())
        .keys([notes("C4 E4 G4", 0, BAR - BEAT / 2), notes("F3 A3 C4", BAR - BEAT / 2, BEAT / 2 + BEAT / 4)].concat())
        .hear();
    assert_eq!(pushes(&h), Vec::<String>::new());
}

#[test]
fn a_chord_struck_at_a_loops_end_may_be_pushed_to_its_start() {
    // The design's case 46: a 2-bar loop, silent to 960, G7 to 3600, then Fmaj7 struck at 3600 and held to the
    // loop's end. As the loop goes round, it may be counted from the loop's start.
    let h = Music::new()
        .looped()
        .bass([notes("G2", 960, 2640), notes("F2", 3600, 240)].concat())
        .keys([notes("F4 B4 D5", 960, 2640), notes("E4 A4 C5", 3600, 240)].concat())
        .hear();
    assert_eq!(pushes(&h), ["0 <- -240 C E F A"]);
    assert!(h.push_notes(&h.pushes()[0]).all(|(_, n)| n.copy && n.wraps));
}

#[test]
fn a_push_in_twelve_eight_lands_on_the_middle_of_the_bar() {
    // The design's case 53, in 12/8 (240 ticks an eighth): Ab for eighths 1 to 6, Bbm7 for 7 to 12, and the twin
    // with Bbm7 struck on eighth 6: it may be counted from eighth 7, the middle of the bar.
    let ab = |length: i64| [notes("Ab2", 0, length), notes("Ab3 C4 Eb4", 0, length)].concat();
    let bbm7 = |at: i64| [notes("Bb2", at, 2880 - at), notes("Ab3 Db4 F4", at, 2880 - at)].concat();
    let h = Music::new().grid(12, 240).keys([ab(1440), bbm7(1440)].concat()).hear();
    assert_eq!(pushes(&h), Vec::<String>::new());
    let h = Music::new().grid(12, 240).keys([ab(1200), bbm7(1200)].concat()).hear();
    assert_eq!(pushes(&h), ["1440 <- 1200 Db F Ab Bb"]);
}

#[test]
fn shapes_and_pushes_move_with_the_notes() {
    for music in [with_bass(), keys_only(), with_bass().looped()] {
        let h = music.hear();
        let base = described(&h, 0, 1);
        // Moved.
        for shift in -6..=5 {
            let moved = music.map(|n| Note { key: (i32::from(n.key) + shift) as u8, ..n }).hear();
            assert_eq!(described(&moved, shift, 1), base, "moved {shift}");
        }
        // At another resolution.
        for ppq in [96, 960] {
            let scale = |t: i64| t * ppq / 480;
            let mut other = music.map(|n| Note { start: scale(n.start), length: scale(n.length), ..n });
            other.grid.ticks_per_beat = ppq as u32;
            let o = other.hear();
            if ppq == 960 {
                assert_eq!(described(&o, 0, 2), base, "ppq {ppq}");
            } else {
                assert_eq!(described(&o, 0, 1), described(&h, 0, 5), "ppq {ppq}");
            }
        }
        // The notes and the parts in another order.
        let mut shuffled = music.clone();
        shuffled.parts.reverse();
        for (_, notes) in &mut shuffled.parts {
            notes.reverse();
        }
        assert_eq!(described(&shuffled.hear(), 0, 1), base, "shuffled");
        // The chords doubled an octave up.
        let mut doubled = music.clone();
        for (role, notes) in &mut doubled.parts {
            if matches!(role, Role::Harmony | Role::Unknown) {
                let up: Vec<Note> = notes.iter().map(|n| Note { key: n.key + 12, ..*n }).collect();
                notes.extend(up);
            }
        }
        assert_eq!(described(&doubled.hear(), 0, 1), base, "doubled");
        // Played a little late, each note by its own few ticks (at most 40, a twelfth of a beat: less than the
        // quarter of a unit a note is still on the unit's start by, with the music's 10 ticks of legato added).
        let late = music.map(|n| Note { start: n.start + (i64::from(n.key) * 7 + n.start / 120) % 41, ..n }).hear();
        let on_units = |t: i64| t - t.rem_euclid(BEAT / 2);
        assert_eq!(described_with(&late, 0, on_units), described_with(&h, 0, on_units), "late");
    }
}

#[test]
fn the_sample_music_has_the_shapes_and_the_push_it_was_written_with() {
    // The walking bar's E2 passes and its F#2 leads into G2 (two semitone steps: an approach); the tune's D5 and C5
    // pass down from E5; the keys come in an eighth before bar 3, where the bass's G2 lands.
    let h = with_bass().hear();
    let want = [
        "bass Passing E2@2400 (D F)",
        "bass Approach F#2@3360 (G)",
        "tune Passing D5@480 (C E)",
        "tune Passing C5@720 (D B)",
    ];
    assert_eq!(shapes(&h), want);
    assert_eq!(pushes(&h), ["3840 <- 3600 D F G B"]);
}
