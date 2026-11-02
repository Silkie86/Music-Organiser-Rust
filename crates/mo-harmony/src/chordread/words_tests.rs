//! Tests of the plain words ([`super::Words`]): what the design's hand cases and music made for each kind of note say,
//! the rules every explanation's words keep for any music with any pins, that the same notes at another resolution
//! say the same, and that the words spell the chords as the chart does in a key.

use super::decode_tests::small_music;
use super::explain_tests::{case_music, explained};
use super::hear_tests::{BAR, BEAT, Dice, Music, notes, random_music};
use super::reader_tests::{chord, hand_cases, random_pins, the_music_in_ticks};
use super::words::{Kind, Topic, kind_of, topic_of};
use super::{ChordAnswer, ChordReading, Explanation, Hearing, MOST_READINGS, NoChord, Pin, SpanFlags, Words};
use crate::key::Key;
use crate::notes::Note;
use crate::pitch::PitchClass;

/// Each stretch's words, the chart spelled in `key`.
fn words_of(reading: &ChordReading, all: &[Option<Explanation>], key: Option<Key>) -> Vec<Option<Words>> {
    let chart = reading.names(key);
    all.iter().map(|e| e.as_ref().map(|e| e.words(reading, &chart, key))).collect()
}

/// A hand case's words for one stretch, every reading kept, the chart in no key.
fn case(name: &str, span: usize) -> Words {
    let (reading, all) = explained(&case_music(name), &[], MOST_READINGS);
    words_of(&reading, &all, None).swap_remove(span).unwrap_or_else(|| panic!("{name} [{span}]: no words"))
}

/// The first stretch of some music, read as a loop, in words.
fn first_words(music: Music) -> Words {
    let (reading, all) = explained(&music.looped(), &[], MOST_READINGS);
    words_of(&reading, &all, None).swap_remove(0).expect("no words")
}

fn render(w: &Words) -> String {
    let mut out = vec![w.chord.clone()];
    out.extend(w.notes.iter().map(|l| format!("    {l}")));
    out.extend(w.why.iter().map(|l| format!("    why: {l}")));
    out.join("\n")
}

/// Whether some words have a line, among the notes or the why.
fn has(w: &Words, line: &str) -> bool {
    w.notes.iter().chain(&w.why).any(|l| l == line)
}

#[test]
#[ignore = "prints every hand case's words"]
fn show_the_words() {
    for (case, music) in hand_cases() {
        let (reading, all) = explained(&music, &[], MOST_READINGS);
        for (i, w) in words_of(&reading, &all, None).iter().enumerate() {
            println!("{case} [{i}]: {}", w.as_ref().map_or("none".to_owned(), render));
        }
    }
}

#[test]
fn each_chord_is_told_by_its_bass_and_its_notes_degrees() {
    let cases = [
        // A rootless voicing over the root in the bass, as the design has it.
        (
            "10 Dm9 | G13 | Cmaj9",
            0,
            "Dm9: D in the bass is its root, and the chords play F, A, C and E, its b3, 5, b7 and 9 (a rootless \
             voicing over the root in the bass).",
        ),
        (
            "10 Dm9 | G13 | Cmaj9",
            1,
            "G13: G in the bass is its root, and the chords play B, F, A and E, its 3, b7, 9 and 13 (a rootless \
             voicing over the root in the bass).",
        ),
        // A bass that's only a bass, and the same notes written the other way.
        (
            "3 D/C",
            0,
            "D/C (also D7/C): C in the bass is a b7 under the chord, and the chords play D, F# and A, its root, 3 \
             and 5.",
        ),
        ("2 C/E", 0, "C/E: E in the bass is its third, and the chords play C, E and G, its root, 3 and 5."),
        ("2 G7/F", 0, "G7/F: F in the bass is its seventh, and the chords play G, B, D and F, its root, 3, 5 and b7."),
        // A diminished seventh on its other roots, each spelled as the chart would.
        (
            "27 C C#dim7 | Dm7",
            1,
            "C#dim7 (also Edim7, Gdim7 or A#dim7): C# in the bass is its root, and the chords play E, G and Bb, its \
             b3, b5 and bb7.",
        ),
        // A walking bass: no slash.
        (
            "37 Dm7 | G13 over a walk",
            0,
            "Dm7: the bass moves under it through D and F, its root and b3 (so no slash is written), and the chords \
             play F, A and C, its b3, 5 and b7.",
        ),
        // No bass part: the lowest note, a fifth or more under the rest.
        (
            "14 shells, the bass by the gap",
            0,
            "Dm7: D, the lowest note (no bass part plays), is its root, and the chords play F and C, its b3 and b7.",
        ),
        (
            "22 no bass",
            0,
            "C6 or Am7 (the same notes): no bass plays, and the chords play C, E, G and A, its root, 3, 5 and 6.",
        ),
        (
            "19 Db13",
            0,
            "Db13: Db in the bass is its root, and the chords play F, Cb, Eb and Bb, its 3, b7, 9 and 13 (a \
             rootless voicing over the root in the bass).",
        ),
        (
            "15b C7(#9)",
            0,
            "C7(#9): C in the bass is its root, and the chords play E, G, Bb and D#, its 3, 5, b7 and #9 (a \
             rootless voicing over the root in the bass).",
        ),
    ];
    for (name, span, want) in cases {
        assert_eq!(case(name, span).chord, want, "{name} [{span}]");
    }
    assert!(case("38 Dm7 | G7 | Cmaj7 over a walk", 0).chord.starts_with("Unclear, most likely Dm7: "));
}

#[test]
fn how_a_chord_is_played_is_told() {
    // A chord the decode reads as one, the name cuts in two where its bass moves to its seventh.
    let w = case("4 C C/B | Am", 0);
    assert!(has(
        &w,
        "With the next stretch it's one chord, Cmaj7, written in two where the bass moves to B, its seventh."
    ));
    assert!(has(&w, "C is read as Cmaj7, whose seventh, B, isn't heard."), "{}", render(&w));
    let w = case("4 C C/B | Am", 1);
    assert_eq!(
        w.chord,
        "C/B (also Cmaj7/B): B in the bass is a 7 under the chord, and the chords play C, E and G, its root, 3 and 5."
    );
    assert_eq!(
        w.notes,
        ["With the stretch before it's one chord, Cmaj7, written in two where the bass moves to B, its seventh: no \
          new chord starts."]
    );
    // A loop's first chord carried round from its end.
    assert_eq!(case("1 C", 0).notes, ["As the loop goes round, C goes on here: no new chord starts."]);
    // A chord written as another than the decode reads.
    let w = case("3 D/C", 0);
    assert!(has(&w, "The reader reads it as D7; it's written D/C from the notes it plays."), "{}", render(&w));
    assert!(has(&w, "C in the bass is the seventh of D7 and would be the third of Am6(add11)/C."), "{}", render(&w));
    let w = case("25 C#dim7, no bass, before Dm", 0);
    assert!(has(&w, "The reader reads it as Edim7; it's written C#dim7 from the notes it plays."), "{}", render(&w));
    // Over a pedal, and pushed.
    assert!(has(&case("9 C | F/C | G/C | C over a pedal", 1), "It stands over a pedal on C."));
    assert!(has(&case("43 Cmaj7 | Am7 pushed", 1), "Pushed: struck half a beat early, on the and of 4."));
    assert!(has(&case("44 C | F pushed", 1), "Pushed: struck half a beat early, on the and of 4."));
}

#[test]
fn what_the_notes_that_arent_the_chords_own_do_is_told() {
    let w = case("37 Dm7 | G13 over a walk", 0);
    assert_eq!(w.notes, ["G in the bass comes early to the next chord.", "E in the bass passes between D and F."]);
    assert!(has(&case("36 a walk under F A C", 0), "G in the bass walks between the chord's notes."));
    assert!(has(&case("38 Dm7 | G7 | Cmaj7 over a walk", 0), "G# in the bass leads by a step into G."));
    assert!(has(&case("51 Cmaj7 | Fmaj7, B4 held over", 1), "C, E, G and B ring on from the chord before."));
    assert!(has(&case("54 C | G | Am | F with a tune", 2), "B in the tune steps away from A and back."));
    assert!(has(&case("41 C, D4 passing", 0), "D in the chords passes between C and E (twice)."));
    assert!(has(&case("48b Cmaj7, F5 held", 0), "F in the tune clashes with Cmaj7."));
    assert!(has(&case("4 C7, never cut where the bass plays B", 0), "B in the bass clashes with C7."));
    // The music the explanations' tests make for each kind of note.
    let w = first_words(
        Music::new()
            .bass([notes("F2", 0, BAR), notes("C2", BAR, BAR)].concat())
            .keys([notes("F3 A3 C4", 0, BAR), notes("C4 E4 G4", BAR, BAR)].concat())
            .tune([notes("F5", 0, BAR + 2 * BEAT), notes("E5", BAR + 2 * BEAT, 2 * BEAT)].concat()),
    );
    assert_eq!(
        w.chord,
        "F: F in the bass is its root, and the chords and the tune play F, A and C, its root, 3 and 5."
    );
    let (reading, all) = explained(
        &Music::new()
            .bass([notes("F2", 0, BAR), notes("C2", BAR, BAR)].concat())
            .keys([notes("F3 A3 C4", 0, BAR), notes("C4 E4 G4", BAR, BAR)].concat())
            .tune([notes("F5", 0, BAR + 2 * BEAT), notes("E5", BAR + 2 * BEAT, 2 * BEAT)].concat())
            .looped(),
        &[],
        MOST_READINGS,
    );
    let w = words_of(&reading, &all, None).swap_remove(1).expect("no words");
    assert!(has(&w, "F in the tune is held over from before and steps down to E."), "{}", render(&w));
    let (reading, all) = explained(
        &Music::new()
            .bass([notes("G2", 0, BAR), notes("C2", BAR, BAR)].concat())
            .keys([notes("G3 B3 D4", 0, BAR), notes("C4 E4 G4", BAR, BAR)].concat())
            .tune([notes("D5", 3 * BEAT, BEAT), notes("A5", BAR, BEAT), notes("G5", BAR + BEAT, 3 * BEAT)].concat())
            .looped(),
        &[],
        MOST_READINGS,
    );
    let w = words_of(&reading, &all, None).swap_remove(1).expect("no words");
    assert!(has(&w, "A in the tune leaps in on the beat and steps to G."), "{}", render(&w));
    let w = first_words(
        Music::new()
            .bass(notes("C2", 0, BAR))
            .keys([notes("C4 E4 G4 B4", 0, BAR), notes("D4", 2 * BEAT, BEAT / 2)].concat()),
    );
    assert!(
        has(&w, "D, its 9, is heard too little to name: it's listed beside the chord as doubtful."),
        "{}",
        render(&w)
    );
    let w = first_words(Music::new().bass(notes("C2", 0, BAR)).keys(
        [notes("C4 E4 G4", 0, BEAT), notes("D5", BEAT, BEAT / 2), notes("C4 E4 G4", 2 * BEAT, 2 * BEAT)].concat(),
    ));
    assert!(has(&w, "D in the chords, its 9, is passing colour, heard too little to name."), "{}", render(&w));
    let w = first_words(Music::new().bass(notes("C2", 0, BAR)).keys(notes("E3 G3 B3", 0, BAR)).tune(
        [notes("G5", 0, BEAT), notes("D5", BEAT + BEAT / 2, BEAT / 2), notes("G5", 2 * BEAT, 2 * BEAT)].concat(),
    ));
    assert!(has(&w, "D in the tune, its 9, moves over the chord, too short or too soft to count."), "{}", render(&w));
}

#[test]
fn why_a_chord_reads_as_it_does_is_told_from_the_evidence() {
    // The bass, the chords' notes, the root and how the root moves.
    let w = case("10 Dm9 | G13 | Cmaj9", 0);
    assert_eq!(
        w.why[..3],
        [
            "D in the bass is the root of Dm9 and would be a tension of Fmaj7/D (its 13).",
            "Of what the chords play, 75% is Dm9's own notes and 25% tensions it can carry; for Fmaj7/D, all of it is \
             its own notes.",
            "Dm9's root, D, is only in the bass; the notes lean more towards F as the root than towards D.",
        ]
    );
    assert!(has(
        &case("10 Dm9 | G13 | Cmaj9", 1),
        "Dm9 to G13 to Cmaj9 is a more common way for the root to move than Dm9 to Fmaj7(#11)/G to Cmaj9."
    ));
    // With no bass, the lowest note; how common each kind of chord is; a fifth left out.
    let w = case("22 no bass", 0);
    assert!(has(
        &w,
        "C6's root, C, is the lowest note all of the time; the notes lean more towards C as the root than towards A."
    ));
    assert!(has(&w, "Of the chords in the songs counted, 1.1% are major sixth chords and 6.6% minor seventh chords."));
    assert!(has(&case("14 shells, the bass by the gap", 0), "Dm7's fifth, A, isn't heard."));
    // How usual its tensions are for its kind of chord.
    assert!(has(
        &case("1 C", 0),
        "As the songs counted name tensions, C is a more usual way to write a major chord than Gsus4(add6)/C is to \
         write a sus4 chord; of the chords in the songs counted, 52% are major chords and 1.7% sus4 chords."
    ));
    // Where a chord starts, and a chord held on.
    assert!(has(
        &case("4 C C/B | Am", 1),
        "C/B is the chord before, C, going on, so where a chord starts doesn't count for it; the bass moves to \
         Bsus4(b9)(addb13)'s root, B, where it would start; Bsus4(b9)(addb13) would start in the middle of a bar."
    ));
    assert!(has(
        &case("36 a walk under F A C", 0),
        "Dm7 is the chord before going on, so where a chord starts doesn't count for it; nothing new is struck where F \
         would start; F would start on a bar line."
    ));
    // A moving bass.
    assert!(has(
        &case("38 Dm7 | G7 | Cmaj7 over a walk", 1),
        "Of what the moving bass plays (counting the strongest beats most), 47% is on G7's other notes; for Dm6, 20% \
         is on its other notes and 27% on notes not its own, on a strong beat or held long."
    ));
    // The tune.
    assert!(has(
        &case("48b Cmaj7, F5 held", 0),
        "Of what the tune plays, all of it is notes that don't belong to Cmaj7; for G13/C, all of it is its own notes."
    ));
    // A runner-up written as another chord than the decode reads is told as the one it reads.
    let w = case("7 G9sus4 with D4", 0);
    assert_eq!(
        w.why[..2],
        [
            "The runner-up, Dm11/G, is read as D7sus4.",
            "G in the bass is the root of G9sus4 and would be the 4th of D7sus4."
        ]
    );
}

#[test]
fn a_chord_given_is_told_as_given() {
    let music = Music::new()
        .bass([notes("C2", 0, BAR), notes("A1", BAR, BAR)].concat())
        .keys([notes("C4 E4 G4", 0, BAR), notes("C4 E4 A4", BAR, BAR)].concat());
    let pins = [Pin { start: BAR, end: 2 * BAR, chord: Some(chord("Am")) }];
    let (reading, all) = explained(&music, &pins, 2);
    let w = words_of(&reading, &all, None).swap_remove(1).expect("no words");
    assert!(reading.spans[1].flags.contains(SpanFlags::GIVEN));
    assert_eq!(w.notes[0], "You gave this chord (a fix or a Chord check answer), so it's written as given.");
}

#[test]
fn the_words_spell_the_chords_as_the_chart_does_in_a_key() {
    let music = Music::new().bass(notes("F#2", 0, BAR)).keys(notes("F#3 A#3 C#4", 0, BAR));
    let (reading, all) = explained(&music.looped(), &[], 2);
    let in_key = |key: Option<Key>| words_of(&reading, &all, key).swap_remove(0).expect("no words").chord;
    // B major's fifth, and D flat major's fourth.
    let sharp = "F#: F# in the bass is its root, and the chords play F#, A# and C#, its root, 3 and 5.";
    assert_eq!(in_key(Some(Key::major(PitchClass::new(11)))), sharp);
    assert_eq!(
        in_key(Some(Key::major(PitchClass::new(1)))),
        sharp.replace("F#", "Gb").replace("A#", "Bb").replace("C#", "Db")
    );
}

/// Whether a line is plain words: printable ASCII, a capital first, one full stop at the end, and nothing left over
/// from making it (no doubled spaces or commas, no braces, no numbers that aren't).
fn plain(line: &str) -> Result<(), String> {
    let first = line.chars().next().ok_or("an empty line")?;
    let bad = ["  ", " ,", ",,", " .", "..", "{", "}", "NaN", "inf", "'s's", ";;", "( ", " )"];
    if !line.chars().all(|c| c.is_ascii_graphic() || c == ' ') {
        Err(format!("not plain ASCII: {line}"))
    } else if !first.is_ascii_uppercase() {
        Err(format!("no capital first: {line}"))
    } else if !line.ends_with('.') || line.ends_with("..") {
        Err(format!("no full stop at the end: {line}"))
    } else if let Some(b) = bad.iter().find(|b| line.contains(*b)) {
        Err(format!("{b:?} in: {line}"))
    } else {
        Ok(())
    }
}

/// What words keep for any reading: every line plain; the chord line names the chord as the chart spells it and says
/// what the answer says; a line for each kind of note that isn't the chord's own; for each way it's played; and for
/// each kind of evidence shown, with the runner-up's chord read first where it's written as another. Counts what it
/// met: answers implied, unclear and too few notes, rootless voicings, chords read as another, pushes, gifts.
fn keeps_the_rules(reading: &ChordReading, all: &[Option<Explanation>], met: &mut [usize; 7]) -> Result<(), String> {
    let chart = reading.names(None);
    for (e, w) in all.iter().zip(words_of(reading, all, None)) {
        let (Some(e), Some(w)) = (e, w) else { continue };
        let lines = std::iter::once(&w.chord).chain(&w.notes).chain(&w.why);
        lines.map(|l| plain(l)).collect::<Result<Vec<()>, String>>().map_err(|why| format!("[{}] {why}", e.span))?;
        let span = &reading.spans[e.span];
        let name = reading.spell(&chart, e.span, e.first.chord, None).name();
        let head = match span.answer {
            ChordAnswer::Unclear => format!("Unclear, most likely {name}"),
            ChordAnswer::NoChord(_) => format!("Too few notes for a chord, most likely {name}"),
            _ => name,
        };
        if !w.chord.starts_with(&format!("{head}:")) && !w.chord.starts_with(&format!("{head} ")) {
            return Err(format!("[{}] {:?} for {head}", e.span, w.chord));
        }
        let said = |what: &str| w.chord.contains(what);
        match span.answer {
            ChordAnswer::Implied if !said("(implied: only one line plays") => {
                return Err(format!("not implied: {}", w.chord));
            }
            ChordAnswer::Implied => met[0] += 1,
            ChordAnswer::Unclear => met[1] += 1,
            ChordAnswer::NoChord(NoChord::TooFewNotes) => met[2] += 1,
            _ => {}
        }
        if e.first.rootless {
            if !said("nothing plays its root, ") {
                return Err(format!("not rootless: {}", w.chord));
            }
            met[3] += 1;
        }
        // The notes: a line for each way it's played, and at least one for each kind of note.
        let starting = |start: &str| w.notes.iter().filter(|l| l.starts_with(start)).count();
        let read_as = starting("The reader reads it as ");
        met[4] += read_as;
        let pushed = span.flags.contains(SpanFlags::PUSHED) && span.heard_start < span.start;
        if usize::from(pushed) != starting("Pushed: struck ") {
            return Err(format!("[{}] pushed {pushed}: {:?}", e.span, w.notes));
        }
        met[5] += usize::from(pushed);
        let given = span.flags.contains(SpanFlags::GIVEN);
        if usize::from(given) != starting("You gave this chord") {
            return Err(format!("[{}] given {given}: {:?}", e.span, w.notes));
        }
        met[6] += usize::from(given);
        let held = w.notes.iter().filter(|l| l.ends_with("no new chord starts.")).count();
        if usize::from(e.held_on[0]) != held {
            return Err(format!("[{}] held on {}: {:?}", e.span, e.held_on[0], w.notes));
        }
        let mut kinds: Vec<Kind> = e.notes.iter().filter_map(|n| kind_of(n.does)).collect();
        kinds.dedup();
        kinds.sort_by_key(|k| *k as u8);
        kinds.dedup();
        let ways = read_as + usize::from(pushed) + usize::from(given) + starting("With the ") + starting("It stands");
        let ways = ways + w.notes.iter().filter(|l| l.ends_with("goes on here: no new chord starts.")).count();
        if w.notes.len() < ways + kinds.len() {
            return Err(format!("[{}] {} kinds of note, {} ways: {:?}", e.span, kinds.len(), ways, w.notes));
        }
        // The why: a line for each kind of evidence shown, after the runner-up's chord read where it's another.
        let mut topics: Vec<Topic> = e.shown_terms().iter().map(|t| topic_of(t.term)).collect();
        topics.sort_by_key(|t| *t as u8);
        topics.dedup();
        let runner_up = w.why.first().is_some_and(|l| l.starts_with("The runner-up, "));
        if w.why.len() != topics.len() + usize::from(runner_up) {
            return Err(format!("[{}] {} kinds of evidence: {:?}", e.span, topics.len(), w.why));
        }
    }
    Ok(())
}

#[test]
fn any_music_with_any_pins_is_told_by_the_rules() {
    let mut dice = Dice(0x9b05_688c_2b3e_6c1f);
    let mut hearing = Hearing::new();
    let mut met = [0; 7];
    for round in 0..200 {
        let music = if round % 2 == 0 { random_music(&mut dice) } else { small_music(&mut dice) };
        if !music.hear_into(&mut hearing) {
            continue;
        }
        let pins = random_pins(&mut dice, &hearing);
        let keep = dice.pick(&[1, 2, 6]);
        let (r, all) = explained(&music, &pins, keep);
        if let Err(why) = keeps_the_rules(&r, &all, &mut met) {
            panic!("round {round}: {why}\n{music:?}, pins {pins:?}");
        }
    }
    for (music, pins) in the_music_in_ticks() {
        let (r, all) = explained(&music, &pins, MOST_READINGS);
        if let Err(why) = keeps_the_rules(&r, &all, &mut met) {
            panic!("{why}\n{music:?}");
        }
    }
    // Every rule is met somewhere.
    assert!(met.iter().all(|&n| n > 0), "met {met:?}");
}

#[test]
fn the_same_notes_at_another_resolution_are_told_the_same() {
    for (music, pins) in the_music_in_ticks() {
        let (r, all) = explained(&music, &pins, 2);
        let want = words_of(&r, &all, None);
        for ppq in [96, 960] {
            let scale = |t: i64| t * ppq / 480;
            let mut other = music.map(|n| Note { start: scale(n.start), length: scale(n.length), ..n });
            other.grid.ticks_per_beat = scale(i64::from(music.grid.ticks_per_beat)) as u32;
            let pins: Vec<Pin> = pins.iter().map(|p| Pin { start: scale(p.start), end: scale(p.end), ..*p }).collect();
            let (r, all) = explained(&other, &pins, 2);
            assert_eq!(words_of(&r, &all, None), want, "at {ppq} ticks a beat");
        }
    }
}
