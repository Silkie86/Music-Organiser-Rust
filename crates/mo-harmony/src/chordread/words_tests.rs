//! Tests of the plain words ([`super::Words`]): what the design's hand cases and music made for each kind of note say,
//! the rules every explanation's words keep for any music with any pins, that the same notes at another resolution
//! say the same, and that the words spell the chords as the chart does in a key.

use super::decode_tests::small_music;
use super::explain_tests::{case_music, explained};
use super::hear_tests::{BAR, BEAT, Dice, Music, notes, random_music};
use super::reader_tests::{chord, hand_cases, random_pins, the_music_in_ticks};
use super::words::{Kind, Topic, article, kind_of, pushed_words, topic_of, written_as_read};
use super::{
    ChordAnswer, ChordReading, Explanation, Feature, Hearing, MOST_READINGS, NoChord, Pin, SpanFlags, Term, TermWhy,
    Words,
};
use crate::key::Key;
use crate::notes::{Grid, Note};
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
        // A bass that's only a bass is one of the chord's notes as it's written (D over C is D7), and a bass on
        // another of its notes is written in root position.
        ("3 D/C", 0, "D7: C in the bass is its seventh, and the chords play D, F# and A, its root, 3 and 5."),
        ("2 C/E", 0, "C: E in the bass is its third, and the chords play C, E and G, its root, 3 and 5."),
        ("2 G7/F", 0, "G7: F in the bass is its seventh, and the chords play G, B, D and F, its root, 3, 5 and b7."),
        // A diminished seventh on its other roots, each spelled as the chart would.
        (
            "27 C C#dim7 | Dm7",
            1,
            "C#dim7 (also Edim7, Gdim7 or A#dim7): C# in the bass is its root, and the chords play E, G and Bb, its \
             b3, b5 and bb7.",
        ),
        // A walking bass.
        (
            "37 Dm7 | G13 over a walk",
            0,
            "Dm7: the bass moves under it through D and F, its root and b3, and the chords play F, A and C, its b3, 5 \
             and b7.",
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
            "C7#9: C in the bass is its root, and the chords play E, G, Bb and D#, its 3, 5, b7 and #9 (a rootless \
             voicing over the root in the bass).",
        ),
    ];
    for (name, span, want) in cases {
        assert_eq!(case(name, span).chord, want, "{name} [{span}]");
    }
    assert!(case("41 C, D4 passing", 0).chord.starts_with("Unclear, most likely C: "));
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
    assert_eq!(w.chord, "Cmaj7: B in the bass is its seventh, and the chords play C, E and G, its root, 3 and 5.");
    assert_eq!(
        w.notes,
        ["With the stretch before it's one chord, Cmaj7, written in two where the bass moves to B, its seventh: no \
          new chord starts."]
    );
    // A loop's first chord carried round from its end.
    assert_eq!(case("1 C", 0).notes, ["As the loop goes round, C goes on here: no new chord starts."]);
    // A chord written as another than the decode reads: G over a pedal on C is written G(add11), from all its notes.
    let w = case("9 C | F/C | G/C | C over a pedal", 2);
    assert!(has(&w, "The reader reads it as G; it's written G(add11) from the notes it plays."), "{}", render(&w));
    assert!(has(&w, "C in the bass is a note outside G and would be the root of Cmaj7sus2."), "{}", render(&w));
    // D over C is written D7, the chord the decode reads, so that isn't said.
    let w = case("3 D/C", 0);
    assert!(!w.notes.iter().any(|l| l.starts_with("The reader reads it as")), "{}", render(&w));
    assert!(w.why.iter().any(|l| l.starts_with("C in the bass is the seventh of D7 and would be ")), "{}", render(&w));
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
        w.why[1..4],
        [
            "D in the bass is the root of Dm9 and would be the sixth of F6.",
            "Of what the chords play, 25% is tensions Dm9 can carry; for F6, 25% is notes struck that don't belong to \
             it.",
            "Dm9's root, D, is only in the bass.",
        ]
    );
    assert!(has(
        &case("38 Dm7 | G7 | Cmaj7 over a walk", 1),
        "Dm7 to G7 to Cmaj7 is a more common way for the root to move than Dm7 to Dm6 to Cmaj7."
    ));
    // With no bass, the lowest note; how common each kind of chord is; a fifth left out.
    let w = case("22 no bass", 0);
    assert!(has(
        &w,
        "C6's root, C, is the lowest note all of the time; the notes lean more towards C as the root than towards A."
    ));
    assert!(has(&w, "Of the chords in the songs counted, 1.1% are major sixth chords and 6.6% minor seventh chords."));
    assert!(has(&case("13 G13 rootless, no bass", 0), "Fmaj7#11's fifth, C, isn't heard."));
    // A shell with no bass part, D F C: F5 over D is written Dm7 too, so it's one reading with Dm7, and the runner-up
    // told is the next one named another way, G7 (written G7sus4 from the notes it plays), whose third isn't heard.
    let w = case("14 shells, the bass by the gap", 0);
    assert!(has(&w, "The runner-up, G7sus4, is read as G7."));
    assert!(has(&w, "G7's third, B, isn't heard."));
    // How usual its tensions are for its kind of chord.
    assert!(has(
        &case("5 C | F, never C/G", 0),
        "As the songs counted name tensions, C is a more usual way to write a major chord than Gsus4(add6) is to write \
         a sus4 chord; of the chords in the songs counted, 52% are major chords and 1.7% sus4 chords."
    ));
    // Where a chord starts, and a chord held on.
    assert!(has(
        &case("4 C C/B | Am", 1),
        "Cmaj7 is the chord before, C, going on, so where a chord starts doesn't count for it; Am9 would start in the \
         middle of a bar."
    ));
    assert!(has(
        &case("6 C C/E | F F#dim7 | C/G", 1),
        "C is the chord before going on, so where a chord starts doesn't count for it; the bass moves to Emaddb13's \
         root, E, where it would start; Emaddb13 would start in the middle of a bar."
    ));
    assert!(has(
        &case("36 a walk under F A C", 0),
        "Dm7 is the chord before going on, so where a chord starts doesn't count for it; nothing new is struck where F \
         would start; F would start on a bar line."
    ));
    // A moving bass.
    assert!(has(
        &case("h6 Cmaj9 | Am11 | Dm9 | G13 in sixteenths over eighths", 0),
        "Of what the moving bass plays (counting the strongest beats most), 53% is on Cmaj9's root; for Em7, 20% is on \
         its root and 27% on notes not its own, on a strong beat or held long."
    ));
    // The tune.
    assert!(has(
        &case("48b Cmaj7, F5 held", 0),
        "Of what the tune plays, all of it is notes that don't belong to Cmaj7; for G7, all of it is its own notes."
    ));
    // A runner-up written as another chord than the decode reads is told as the one it reads.
    let w = case("7 G9sus4 with D4", 0);
    assert_eq!(
        w.why[..2],
        [
            "The runner-up, Dm11, is read as D7sus4.",
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

#[test]
fn a_name_that_only_leaves_out_notes_not_heard_is_the_chord_read() {
    let as_read = |written: &str, read: &str| written_as_read(chord(written), chord(read));
    // The same kind of chord, with its tensions named or not.
    assert!(as_read("Dm9", "Dm7") && as_read("Dm7", "Dm9") && as_read("C", "Cmaj7") && as_read("C/B", "Cmaj7"));
    // Notes it doesn't sound left out: told as the name, with the notes left out told by the chord read.
    assert!(as_read("G", "G7") && as_read("G/B", "G7") && as_read("C5", "C") && as_read("Bdim", "Bm7b5"));
    // Its seventh heard as a bass under it, another root, another kind of chord.
    assert!(!as_read("D/C", "D7") && !as_read("C#dim7", "Edim7") && !as_read("Dm11/G", "D7sus4"));
    assert!(!as_read("Cm", "C") && !as_read("Cmaj7", "Cm") && !as_read("G7", "G"));
}

#[test]
fn a_push_is_told_by_how_early_and_where_its_struck() {
    let grid = Grid::four_four(480);
    let told = |early: i64| pushed_words(grid, 4 * 480, 4 * 480 - early);
    assert_eq!(told(240), "Pushed: struck half a beat early, on the and of 4.");
    assert_eq!(told(120), "Pushed: struck a quarter of a beat early, on the a of 4.");
    assert_eq!(told(360), "Pushed: struck three quarters of a beat early, on the e of 4.");
    assert_eq!(told(320), "Pushed: struck two thirds of a beat early, on the second triplet of 4.");
    assert_eq!(told(160), "Pushed: struck a third of a beat early, on the last triplet of 4.");
    // Any other amount, between the beats it's struck between.
    assert_eq!(told(10), "Pushed: struck a little early, between beat 4 and the bar line.");
    assert_eq!(told(200), "Pushed: struck less than half a beat early, between beat 4 and the bar line.");
    assert_eq!(told(470), "Pushed: struck less than a beat early, between beat 4 and the bar line.");
    // To the middle of a bar, and in 6/8 (its beat an eighth), a bar line away from the grid's.
    assert_eq!(pushed_words(grid, 2 * 480, 2 * 480 - 240), "Pushed: struck half a beat early, on the and of 2.");
    let six_eight = Grid { ticks_per_beat: 240, beats_per_bar: 6, bar_line: 120 };
    assert_eq!(pushed_words(six_eight, 1560, 1320), "Pushed: struck a beat early, on beat 6.");
    // A bar line far from the music (a damaged grid) reads as the same grid with its bar line moved near it by whole
    // bars, and ticks far apart don't overflow.
    for bar_line in [i64::MIN, i64::MAX, i64::MAX - 7] {
        let near = i64::try_from(i128::from(bar_line).rem_euclid(1920)).unwrap();
        let (far, near) = (Grid { bar_line, ..grid }, Grid { bar_line: near, ..grid });
        assert_eq!(pushed_words(far, 4 * 480, 4 * 480 - 240), pushed_words(near, 4 * 480, 4 * 480 - 240));
    }
    for (start, struck) in [(i64::MAX, i64::MIN), (i64::MIN, i64::MAX), (i64::MAX, i64::MAX - 240)] {
        for bar_line in [0, i64::MIN, i64::MAX] {
            let _ = pushed_words(Grid { bar_line, ..grid }, start, struck);
        }
    }
}

#[test]
fn evidence_about_two_notes_or_adding_two_shares_is_told_as_it_was_weighed() {
    let music = Music::new().bass(notes("G2", 0, BAR)).keys(notes("C4 D4 F4", 0, BAR));
    let (reading, all) = explained(&music.looped(), &[], 2);
    let mut e = all.into_iter().next().flatten().expect("no explanation");
    assert_eq!(e.first.chord.name(), "G7sus4");
    let chart = reading.names(None);
    let mut why = |f: Feature, value: f64| {
        e.terms = vec![TermWhy { term: Term::Feature(f), values: [value, 0.0], scores: [-value, 0.0] }];
        e.words(&reading, &chart, None).why.into_iter().last().unwrap_or_default()
    };
    // A 7sus4's 2 may stand in for its 4th: the evidence is about the one heard more.
    let partly = "G7sus4's 4th, C, and the 2 that may stand in for it, A, are at most partly heard.";
    assert_eq!(why(Feature::NoThird, 0.4), partly);
    assert_eq!(why(Feature::NoThird, 1.0), partly.replace("are at most partly heard", "aren't heard"));
    // What the chords carry in or hold over adds a share of what they hold over to one of their sound: no share.
    let held = "Of what the chords play, much is notes carried in or held over that don't fit G7sus4.";
    assert_eq!(why(Feature::WrongHeld, 1.2), held);
    assert_eq!(why(Feature::WrongHeld, 0.3), held.replace("much", "some"));
    assert_eq!(
        why(Feature::Passing, 0.05),
        "Of what the chords play, a little is notes that only pass by under G7sus4."
    );
    assert_eq!(why(Feature::ChordTones, 1.0), "Of what the chords play, all of it is G7sus4's own notes.");
}

#[test]
fn the_runner_up_is_told_by_the_chord_it_is_read_as() {
    // Fm(maj7) read as Fmaj7: the evidence tells it as Fmaj7, and the first reading as the chord it's read as, E+.
    let w = case("30 C+ with no bass, before F", 0);
    assert_eq!(w.why[0], "The runner-up, Fm(maj7), is read as Fmaj7.");
    assert!(has(
        &w,
        "Fmaj7's root, F, isn't played at all, and no bass plays; E+'s root, E, is the lowest note all of the time."
    ));
    // Fmaj13 read as F6: its E doesn't belong to a sixth chord, so the evidence tells it as F6.
    assert_eq!(case("10 Dm9 | G13 | Cmaj9", 0).why[0], "The runner-up, Fmaj13, is read as F6.");
    // G+ over C is written Cm(maj7), as the first reading is: the two are one reading, and the runner-up another.
    let w = case("35 Cm | Cm(maj7) | Cm7 | Cm6 over a held C", 1);
    assert!(w.why.iter().all(|l| !l.contains("G+") && !l.contains("B+")), "{}", render(&w));
    // A passing note between the notes it hangs on, the one below it first.
    assert!(has(&case("54 C | G | Am | F with a tune", 2), "B in the tune passes between A and C."));
    // What the chords carry in or hold over, told by how much.
    assert!(has(
        &case("44 C | F pushed", 1),
        "Of what the chords play, all of it is F's own notes; for C, 33% is its own notes and much is notes carried in \
         or held over that don't fit it."
    ));
}

#[test]
fn a_word_said_with_a_vowel_first_takes_an() {
    for (word, a) in [("augmented", "an"), ("11", "an"), ("#11", "a"), ("b13", "a"), ("major", "a"), ("7sus4", "a")] {
        assert_eq!(article(word), a, "{word}");
    }
    // An augmented triad (which has no runner-up: its roots are one chord) as the runner-up of C.
    let first = |keys: &str| {
        let music = Music::new().bass(notes("C2", 0, BAR)).keys(notes(keys, 0, BAR)).looped();
        let (reading, all) = explained(&music, &[], 2);
        (reading, all.into_iter().next().flatten().expect("no explanation"))
    };
    let (reading, mut e) = first("E4 G4 C5");
    e.second = Some(first("E4 G#4 C5").1.first);
    e.terms = vec![TermWhy { term: Term::Feature(Feature::TensionPrior), values: [-0.5, -1.0], scores: [-0.5, -1.0] }];
    let why = e.words(&reading, &reading.names(None), None).why;
    assert_eq!(
        why.last().map(String::as_str),
        Some(
            "As the songs counted name tensions, C is a more usual way to write a major chord than C+ is to write an \
              augmented chord."
        )
    );
}

#[test]
fn a_chord_given_over_a_bass_of_its_own_says_what_plays_in_the_bass() {
    // Given with a bass of its own, a chord stands on it (Db7 over F is written Db7), whatever plays there.
    let told = |bass: Vec<Note>, keys: &str, given: &str| {
        let mut music = Music::new().keys(notes(keys, 0, BAR));
        if !bass.is_empty() {
            music = music.bass(bass);
        }
        let pins = [Pin { start: 0, end: BAR, chord: Some(chord(given)) }];
        let (reading, all) = explained(&music.looped(), &pins, MOST_READINGS);
        words_of(&reading, &all, None).swap_remove(0).expect("no words").chord
    };
    let c_e_g = "C4 E4 G4";
    assert_eq!(
        told(notes("C2", 0, BAR), c_e_g, "Db7/F"),
        "Db7: it's given over F, its third, but C plays in the bass, and nothing plays its root, Db."
    );
    assert_eq!(
        told(Vec::new(), c_e_g, "Db7/F"),
        "Db7: it's given over F, its third, but no bass plays, and nothing plays its root, Db."
    );
    assert_eq!(
        told(Vec::new(), "C3 E4 G4", "C/G"),
        "C: it's given over G, its fifth, but the lowest note is C (no bass part plays), and the chords play E and G, \
         its 3 and 5."
    );
    let walk = ["C2", "E2", "G2", "A2"].iter().zip(0..).flat_map(|(n, k)| notes(n, k * BEAT, BEAT)).collect();
    assert_eq!(
        told(walk, c_e_g, "C/G"),
        "C: it's given over G, its fifth, but the bass moves, and the chords play C, E and G, its root, 3 and 5."
    );
    // Over the bass it's given with, it's told as any chord is.
    assert_eq!(
        told(notes("G2", 0, BAR), c_e_g, "C/G"),
        "C: G in the bass is its fifth, and the chords play C, E and G, its root, 3 and 5."
    );
}
