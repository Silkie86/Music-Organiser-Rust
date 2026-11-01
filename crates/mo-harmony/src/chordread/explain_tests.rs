//! Tests of the explanations ([`super::ChordReader::explain`]): what each note does in the design's hand cases and in
//! music made for each kind of note, where the bass comes from, the evidence as the scoring weighs it, the rules every
//! explanation keeps for any music, and that the same notes moved or at another resolution explain the same.

use super::decode_tests::small_music;
use super::hear_tests::{BAR, BEAT, Dice, Music, line, notes, random_music};
use super::name::push_to;
use super::reader_tests::{hand_cases, random_pins, the_music_in_ticks};
use super::{
    BassFrom, ChordContext, ChordReader, ChordReading, ChordSpan, Evidence, Explanation, Feature, Hearing,
    MOST_READINGS, NoteDoes, NoteWhy, PerFeature, Pin, Priors, SHIPPED, SpanFacts, SpanFlags, Stream, TERMS_SHOWN,
    Term,
};
use crate::notes::{Note, Part, Role};
use crate::pitch::{PcSet, PitchClass};

/// A key's name, C4 being 60, with flats but F#: "Ab2".
fn name(key: u8) -> String {
    const NAMES: [&str; 12] = ["C", "Db", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];
    format!("{}{}", NAMES[usize::from(key % 12)], i32::from(key / 12) - 1)
}

fn parts(music: &Music) -> Vec<Part<'_>> {
    music.parts.iter().map(|(role, notes)| Part { role: *role, notes }).collect()
}

/// What some music reads with some pins, keeping `readings` a span, and each span explained, by one reader.
fn explained(music: &Music, pins: &[Pin], readings: u8) -> (ChordReading, Vec<Option<Explanation>>) {
    let parts = parts(music);
    let cx = ChordContext { span: music.span, pins, readings };
    let mut reader = ChordReader::new();
    let reading = reader.read(&parts, music.grid, &cx).expect("nothing read");
    let all = (0..reading.spans.len()).map(|s| reader.explain(&parts, music.grid, &cx, &reading, s)).collect();
    (reading, all)
}

/// A hand case's explanation of one span, every reading kept.
fn case(name: &str, span: usize) -> Explanation {
    let (_, music) = hand_cases().into_iter().find(|(n, _)| *n == name).unwrap_or_else(|| panic!("no case {name}"));
    let (_, all) = explained(&music, &[], MOST_READINGS);
    all[span].clone().unwrap_or_else(|| panic!("{name} [{span}]: no explanation"))
}

/// What a note does, in a few words: "chords E4 ext 9", "bass D2 on 1", "tune F5 Clash".
fn note(w: &NoteWhy) -> String {
    let heard = match w.heard_as {
        Stream::Bass => "bass",
        Stream::Melody => "tune",
        Stream::Harmony => "chords",
    };
    let does = match w.does {
        NoteDoes::Bass(d) => format!("on {d}"),
        NoteDoes::ChordTone(d) => d.to_string(),
        NoteDoes::Extension(d) => format!("ext {d}"),
        NoteDoes::Doubtful(d) => format!("doubtful {d}"),
        NoteDoes::Colour(d) => format!("colour {d}"),
        NoteDoes::TuneOver(d) => format!("over {d}"),
        other => format!("{other:?}"),
    };
    format!("{heard} {} {does}", name(w.key))
}

fn notes_of(e: &Explanation) -> Vec<String> {
    e.notes.iter().map(note).collect()
}

/// An explanation in one line: "Dm9 (Fmaj7/D) BassPart: bass D2 on 1, ... | B1 3.60 0.00, ... | 17.41 11.82".
fn render(e: &Explanation) -> String {
    let terms: Vec<String> = e
        .shown_terms()
        .iter()
        .map(|t| {
            let term = match t.term {
                Term::Feature(f) => f.code().to_owned(),
                Term::Pair => "pair".to_owned(),
            };
            format!("{term} {:.2} {:.2}", t.scores[0], t.scores[1])
        })
        .collect();
    let second = e.second.map_or("-".to_owned(), |r| r.chord.name());
    let [a, b] = e.totals;
    let notes = notes_of(e).join(", ");
    format!("{} ({second}) {:?}: {notes} | {} | {a:.2} {b:.2}", e.first.chord.name(), e.bass_from, terms.join(", "))
}

#[test]
#[ignore = "prints every hand case's explanations"]
fn show_the_explanations() {
    for (case, music) in hand_cases() {
        let (_, all) = explained(&music, &[], MOST_READINGS);
        for (i, e) in all.iter().enumerate() {
            println!("{case} [{i}]: {}", e.as_ref().map_or("none".to_owned(), render));
        }
    }
}

#[test]
fn each_note_of_the_hand_cases_does_what_a_player_would_say() {
    // Dm9 over its root: the keys' F A C E are its b3, 5, b7 and the 9 it names.
    let e = case("10 Dm9 | G13 | Cmaj9", 0);
    assert_eq!(e.first.chord.name(), "Dm9");
    assert_eq!(notes_of(&e), ["bass D2 on 1", "chords F3 b3", "chords A3 5", "chords C4 b7", "chords E4 ext 9"]);
    assert_eq!(e.bass_from, BassFrom::BassPart);
    // G9sus4 over G: F is its b7, A its 9, C its 4 (F/G written as the sus chord on the bass).
    let e = case("7 G9sus4", 0);
    assert_eq!(notes_of(&e), ["bass G1 on 1", "chords F3 b7", "chords A3 ext 9", "chords C4 4"]);
    // D/C: the C under it is a b7 that isn't one of its notes.
    let e = case("3 D/C", 0);
    assert_eq!(notes_of(&e), ["bass C2 on b7", "chords D4 1", "chords F#4 3", "chords A4 5"]);
    // C7(#9): the Eb over its major third is a #9.
    let e = case("15b C7(#9)", 0);
    assert!(notes_of(&e).contains(&"chords Eb4 ext #9".to_owned()), "{}", render(&e));
    // C#dim7 with no bass, before Dm: each note by its degree above the C# it's written on.
    let e = case("25 C#dim7, no bass, before Dm", 0);
    assert_eq!(e.first.chord.name(), "C#dim7");
    assert_eq!(notes_of(&e), ["chords E4 b3", "chords G4 b5", "chords Bb4 bb7", "chords Db5 1"]);
    assert_eq!(e.bass_from, BassFrom::NoBass);
    // A walking bass under F A C: D the root, E passing from D to F, F the b3, and G walking off the strong beats.
    let e = case("36 a walk under F A C", 0);
    assert_eq!(e.first.chord.name(), "Dm7");
    let want = [
        "bass D2 on 1",
        "chords F3 b3",
        "chords A3 5",
        "chords C4 b7",
        "bass E2 Passing",
        "bass F2 on b3",
        "bass G2 Walking",
    ];
    assert_eq!(notes_of(&e), want);
    // Before the bar of G, the walk's G comes early to it.
    let e = case("37 Dm7 | G13 over a walk", 0);
    assert_eq!(notes_of(&e).last().map(String::as_str), Some("bass G2 Anticipation"), "{}", render(&e));
    // D F A Ab | G: the Ab leads by a step into the G on the bar.
    let e = case("38 Dm7 | G7 | Cmaj7 over a walk", 0);
    assert_eq!(notes_of(&e).last().map(String::as_str), Some("bass Ab2 Approach"), "{}", render(&e));
    // C7 over C then B: the B on the middle of the bar, held two beats, clashes with it.
    let e = case("4 C7, never cut where the bass plays B", 0);
    assert_eq!(notes_of(&e).last().map(String::as_str), Some("bass B1 Clash"), "{}", render(&e));
    // Keys alone, C D E G E D C G: both Ds pass by.
    let e = case("41 C, D4 passing", 0);
    let passing: Vec<String> = notes_of(&e).into_iter().filter(|n| n.contains("D4")).collect();
    assert_eq!(passing, ["chords D4 Passing", "chords D4 Passing"]);
    assert_eq!(e.bass_from, BassFrom::NoBass);
    // C | Am: the tune's A5 struck before the bar of Am comes early to it; in the Am it's its root.
    let e = case("45 C | Am, A5 anticipated", 0);
    assert_eq!(notes_of(&e).last().map(String::as_str), Some("tune A5 Anticipation"), "{}", render(&e));
    let e = case("45 C | Am, A5 anticipated", 1);
    assert_eq!(notes_of(&e).first().map(String::as_str), Some("tune A5 1"), "{}", render(&e));
    // G under a tune D5 E5 D5: the E5 steps away and back.
    let e = case("47 G, E5 a neighbour", 0);
    assert!(notes_of(&e).contains(&"tune E5 Neighbour".to_owned()), "{}", render(&e));
    // Cmaj7 under a held tune: D5 is the 9 it names, F5 clashes, and C5 D5 E5's D5 passes by.
    let e = case("48a Cmaj9, D5 held", 0);
    assert_eq!(notes_of(&e).last().map(String::as_str), Some("tune D5 ext 9"), "{}", render(&e));
    let e = case("48b Cmaj7, F5 held", 0);
    assert_eq!(notes_of(&e).last().map(String::as_str), Some("tune F5 Clash"), "{}", render(&e));
    let e = case("48c Cmaj7, C5 D5 E5", 0);
    assert!(notes_of(&e).ends_with(&["tune C5 1".into(), "tune D5 Passing".into(), "tune E5 3".into()]));
    // Cmaj7 rung on half a beat into the bar of Fmaj7: all four ring on from the chord before.
    let e = case("51 Cmaj7 | Fmaj7, B4 held over", 1);
    let rung: Vec<String> = notes_of(&e).into_iter().filter(|n| n.ends_with("HeldOver")).collect();
    assert_eq!(rung, ["chords C4 HeldOver", "chords E4 HeldOver", "chords G4 HeldOver", "chords B4 HeldOver"]);
    // Shells with no bass part: the bass is the lowest note in the keys, a fifth or more under the rest.
    let e = case("14 shells, the bass by the gap", 0);
    assert_eq!(notes_of(&e), ["bass D3 on 1", "chords C4 b7", "chords F4 b3"]);
    assert_eq!(e.bass_from, BassFrom::LowestKeys);
}

/// The first span of some music, read as a loop, explained.
fn first_explained(music: Music) -> Explanation {
    let (_, all) = explained(&music.looped(), &[], MOST_READINGS);
    all[0].clone().expect("no explanation")
}

#[test]
fn suspensions_appoggiaturas_and_tensions_too_little_to_name_are_told_apart() {
    // F5 held from the bar of F into the bar of C, then E5: a suspension in the C.
    let music = Music::new()
        .bass([notes("F2", 0, BAR), notes("C2", BAR, BAR)].concat())
        .keys([notes("F3 A3 C4", 0, BAR), notes("C4 E4 G4", BAR, BAR)].concat())
        .tune([notes("F5", 0, BAR + 2 * BEAT), notes("E5", BAR + 2 * BEAT, 2 * BEAT)].concat());
    let (reading, all) = explained(&music.looped(), &[], MOST_READINGS);
    let shown: Vec<String> = reading.spans.iter().map(|s| s.shown().unwrap().chord.name()).collect();
    assert_eq!(shown, ["F", "C"]);
    let e = all[1].clone().unwrap();
    assert!(notes_of(&e).contains(&"tune F5 Suspension".to_owned()), "{}", render(&e));
    // In the F it's struck in, it's the F's root.
    let e = all[0].clone().unwrap();
    assert!(notes_of(&e).contains(&"tune F5 1".to_owned()), "{}", render(&e));
    // D5 on beat 4 of G, A5 leaping onto the bar of C, G5 a step down: an appoggiatura.
    let music = Music::new()
        .bass([notes("G2", 0, BAR), notes("C2", BAR, BAR)].concat())
        .keys([notes("G3 B3 D4", 0, BAR), notes("C4 E4 G4", BAR, BAR)].concat())
        .tune([notes("D5", 3 * BEAT, BEAT), notes("A5", BAR, BEAT), notes("G5", BAR + BEAT, 3 * BEAT)].concat());
    let (_, all) = explained(&music.looped(), &[], MOST_READINGS);
    let e = all[1].clone().unwrap();
    assert_eq!(e.first.chord.name(), "C");
    assert!(notes_of(&e).contains(&"tune A5 Appoggiatura".to_owned()), "{}", render(&e));
    // Cmaj7 held a bar, a D4 struck with it on beat 3 for an eighth, inside the chord: voiced, too little to name.
    let e = first_explained(
        Music::new()
            .bass(notes("C2", 0, BAR))
            .keys([notes("C4 E4 G4 B4", 0, BAR), notes("D4", 2 * BEAT, BEAT / 2)].concat()),
    );
    assert_eq!(e.first.chord.name(), "Cmaj7");
    assert!(notes_of(&e).contains(&"chords D4 doubtful 9".to_owned()), "{}", render(&e));
    assert_eq!(e.first.doubtful, PcSet::EMPTY.with(PitchClass::D));
    // C E G struck for a beat over a held C, a D5 alone on beat 2 for half a beat, then C E G again: colour too little
    // to list (it isn't struck with two of the chord's notes, and it's short).
    let e = first_explained(Music::new().bass(notes("C2", 0, BAR)).keys(
        [notes("C4 E4 G4", 0, BEAT), notes("D5", BEAT, BEAT / 2), notes("C4 E4 G4", 2 * BEAT, 2 * BEAT)].concat(),
    ));
    assert_eq!(e.first.chord.name(), "C");
    assert!(notes_of(&e).contains(&"chords D5 colour 9".to_owned()), "{}", render(&e));
    assert_eq!(e.first.doubtful, PcSet::EMPTY);
    // Cmaj7 under a tune that leaps from G5 to a D5 an eighth long, off the beat, and back: the tune over the chord.
    let e = first_explained(Music::new().bass(notes("C2", 0, BAR)).keys(notes("E3 G3 B3", 0, BAR)).tune(
        [notes("G5", 0, BEAT), notes("D5", BEAT + BEAT / 2, BEAT / 2), notes("G5", 2 * BEAT, 2 * BEAT)].concat(),
    ));
    assert_eq!(e.first.chord.name(), "Cmaj7");
    assert!(notes_of(&e).contains(&"tune D5 over 9".to_owned()), "{}", render(&e));
    // The same with an F5 (the 11 a major seventh can't carry): it clashes.
    let e = first_explained(Music::new().bass(notes("C2", 0, BAR)).keys(notes("E3 G3 B3", 0, BAR)).tune(
        [notes("G5", 0, BEAT), notes("F5", BEAT + BEAT / 2, BEAT / 2), notes("C6", 2 * BEAT, 2 * BEAT)].concat(),
    ));
    assert!(notes_of(&e).contains(&"tune F5 Clash".to_owned()), "{}", render(&e));
}

#[test]
fn the_evidence_is_what_the_scoring_weighs_the_biggest_lead_first() {
    let priors = Priors::of(SHIPPED.tables);
    let (w, pair) = (&SHIPPED.weights.features, SHIPPED.weights.pair);
    let mut evidence = Evidence::new();
    let mut facts = SpanFacts::new();
    for (music, pins) in the_music_in_ticks() {
        let hearing = music.hear();
        let (reading, all) = explained(&music, &pins, 2);
        evidence.prepare(&hearing);
        for (i, (s, e)) in reading.spans.iter().zip(&all).enumerate() {
            let Some(e) = e else { continue };
            // The evidence worked out again, on the span's units with the pushes to its ends.
            let units = hearing.reported().start + s.units.start..hearing.reported().start + s.units.end;
            let pushed = |s: &ChordSpan| s.flags.contains(SpanFlags::PUSHED);
            let start = if pushed(s) { push_to(&hearing, units.start) } else { None };
            let next = reading.spans.get(i + 1).is_some_and(pushed);
            let end = if next { push_to(&hearing, units.end) } else { None };
            evidence.facts(&hearing, units, start, end, &mut facts);
            let mut values = [PerFeature::default(); 2];
            facts.features(e.first.label, &priors, &mut values[0]);
            if let Some(second) = e.second {
                facts.features(second.label, &priors, &mut values[1]);
            }
            let before = match i.checked_sub(1) {
                Some(b) => reading.spans[b].label,
                None if reading.looped => reading.spans.last().unwrap().label,
                None => None,
            };
            let after = reading.spans.get(i + 1).and_then(|s| s.label);
            let pairs = [Some(e.first.label), e.second.map(|r| r.label)].map(|label| {
                label.map_or(0.0, |l| {
                    before.map_or(0.0, |b| SHIPPED.tables.pair_prior(b, l))
                        + after.map_or(0.0, |a| SHIPPED.tables.pair_prior(l, a))
                })
            });
            let what = format!("span {i}: {}", render(e));
            // Every piece that differs is there, as the scoring weighs it, and no other.
            let mut want = Vec::new();
            for f in Feature::ALL {
                let counted = if f.once() { 1.0 } else { facts.beats() };
                let scores = [w[f] * values[0][f] * counted, w[f] * values[1][f] * counted];
                if scores[0] != scores[1] {
                    want.push((Term::Feature(f), [values[0][f], values[1][f]], scores));
                }
            }
            if pairs[0] != pairs[1] {
                want.push((Term::Pair, pairs, pairs.map(|p| pair * p)));
            }
            let mut got: Vec<_> = e.terms.iter().map(|t| (t.term, t.values, t.scores)).collect();
            got.sort_by_key(|(t, _, _)| match t {
                Term::Feature(f) => *f as usize,
                Term::Pair => Feature::COUNT,
            });
            assert_eq!(got, want, "{what}");
            // The biggest lead first, and the first six shown.
            assert!(e.terms.windows(2).all(|t| t[0].lead().abs() >= t[1].lead().abs()), "{what}");
            assert_eq!(e.shown_terms(), &e.terms[..e.terms.len().min(TERMS_SHOWN)]);
            // The totals are the scoring's, with the pair term.
            let score = |v: &PerFeature<f64>| facts.score(v, SHIPPED.weights);
            let totals =
                [score(&values[0]) + pair * pairs[0], e.second.map_or(0.0, |_| score(&values[1]) + pair * pairs[1])];
            for (got, want) in e.totals.iter().zip(totals) {
                assert!((got - want).abs() <= 1e-9 * want.abs().max(1.0), "{what}: {got} not {want}");
            }
        }
    }
}

#[test]
fn with_no_runner_up_the_first_readings_evidence_is_told_biggest_first() {
    for (music, pins) in the_music_in_ticks() {
        let (one, all_one) = explained(&music, &pins, 1);
        let (_, all_two) = explained(&music, &pins, 2);
        for ((s, e), two) in one.spans.iter().zip(&all_one).zip(&all_two) {
            let (Some(e), Some(two)) = (e, two) else {
                assert!(e.is_none() && two.is_none() && s.readings.is_empty());
                continue;
            };
            assert_eq!(e.second, None);
            assert!(e.terms.iter().all(|t| t.values[1] == 0.0 && t.scores[1] == 0.0 && t.scores[0] != 0.0));
            assert_eq!(e.totals[1], 0.0);
            // The notes and the first reading's evidence are the same as with a runner-up.
            assert_eq!(
                (&e.notes, e.bass_from, e.first, e.totals[0]),
                (&two.notes, two.bass_from, two.first, two.totals[0])
            );
        }
    }
}

/// The rules every explanation of a reading keeps, as the reason one is broken.
fn keeps_the_rules(
    music: &Music,
    hearing: &Hearing,
    r: &ChordReading,
    all: &[Option<Explanation>],
) -> Result<(), String> {
    let check = |ok: bool, what: &str| if ok { Ok(()) } else { Err(what.to_owned()) };
    for (i, (s, e)) in r.spans.iter().zip(all).enumerate() {
        let Some(e) = e else {
            check(s.readings.is_empty(), &format!("span {i}: no explanation"))?;
            continue;
        };
        let what = |rule: &str| format!("span {i}: {rule}: {}", render(e));
        check(
            e.span == i && Some(&e.first) == s.readings.first() && e.second.as_ref() == s.readings.get(1),
            &what("readings"),
        )?;
        // Each note once, as given.
        let mut seen: Vec<(usize, usize)> = e.notes.iter().map(|w| (w.part, w.note)).collect();
        seen.sort_unstable();
        seen.dedup();
        check(seen.len() == e.notes.len(), &what("a note twice"))?;
        let (chord, label) = (e.first.chord, e.first.label);
        for w in &e.notes {
            let (role, notes) = &music.parts[w.part];
            check(notes[w.note].key == w.key, &what("the key"))?;
            let pc = PitchClass::of_key(w.key);
            let interval = i32::from(pc.above(chord.root()));
            let bass_like = matches!(w.does, NoteDoes::Bass(_) | NoteDoes::Walking | NoteDoes::Approach);
            let heard = match w.heard_as {
                Stream::Bass => *role != Role::Melody,
                Stream::Melody => *role == Role::Melody,
                Stream::Harmony => matches!(role, Role::Harmony | Role::Unknown),
            };
            check(heard && (!bass_like || w.heard_as == Stream::Bass), &what("heard as"))?;
            let ok = match w.does {
                NoteDoes::Bass(d) => d.interval() == interval,
                NoteDoes::ChordTone(d) => {
                    d.interval() == interval && (chord.notes().contains(pc) || label.tones().contains(pc))
                }
                NoteDoes::Extension(d) => d.interval() == interval && chord.notes().contains(pc),
                NoteDoes::Doubtful(d) => d.interval() == interval && e.first.doubtful.contains(pc),
                NoteDoes::Colour(d) => d.interval() == interval && w.heard_as == Stream::Harmony,
                NoteDoes::TuneOver(d) => d.interval() == interval && w.heard_as == Stream::Melody,
                NoteDoes::Suspension | NoteDoes::Appoggiatura => w.heard_as != Stream::Bass,
                NoteDoes::Clash => !chord.notes().contains(pc) && !label.tones().contains(pc),
                NoteDoes::Passing | NoteDoes::Neighbour | NoteDoes::Anticipation | NoteDoes::Approach => {
                    !label.tones().contains(pc)
                }
                NoteDoes::Walking | NoteDoes::HeldOver => true,
            };
            check(ok, &what(&note(w)))?;
        }
        // Every note struck in the span's own units is there (but in its last when the next chord is pushed to it).
        let reported = hearing.reported().start;
        let next_pushed = r.spans.get(i + 1).is_some_and(|n| n.flags.contains(SpanFlags::PUSHED));
        let last = reported + s.units.end - usize::from(next_pushed);
        for n in hearing.notes().iter().filter(|n| n.struck && !n.copy) {
            let unit = hearing.unit_of(n.start);
            if (reported + s.units.start..last).contains(&unit) {
                check(seen.binary_search(&(n.part, n.note)).is_ok(), &what(&format!("{} left out", name(n.key))))?;
            }
        }
        // The evidence: the biggest lead first, each piece once and differing, with the runner-up's none without one.
        check(e.terms.windows(2).all(|t| t[0].lead().abs() >= t[1].lead().abs()), &what("order"))?;
        check(e.terms.iter().all(|t| t.lead() != 0.0 && t.lead().is_finite()), &what("leads"))?;
        let mut terms: Vec<Term> = e.terms.iter().map(|t| t.term).collect();
        terms.dedup();
        check(terms.len() == e.terms.len() && terms.len() <= Feature::COUNT + 1, &what("each once"))?;
        let none = e.second.is_none();
        check(!none || e.terms.iter().all(|t| t.scores[1] == 0.0) && e.totals[1] == 0.0, &what("no runner-up"))?;
        check(e.totals.iter().all(|t| t.is_finite()), &what("totals"))?;
    }
    Ok(())
}

#[test]
fn any_music_with_any_pins_is_explained_by_the_rules() {
    let mut dice = Dice(0x3c6e_f372_fe94_f82b);
    let mut hearing = Hearing::new();
    let mut explained_spans = 0;
    for round in 0..200 {
        let music = if round % 2 == 0 { random_music(&mut dice) } else { small_music(&mut dice) };
        if !music.hear_into(&mut hearing) {
            continue;
        }
        let pins = random_pins(&mut dice, &hearing);
        let keep = dice.pick(&[1, 2, 6]);
        let (r, all) = explained(&music, &pins, keep);
        if let Err(why) = keeps_the_rules(&music, &hearing, &r, &all) {
            panic!("round {round}: {why}\n{music:?}, pins {pins:?}");
        }
        explained_spans += all.iter().flatten().count();
    }
    assert!(explained_spans > 500, "{explained_spans} spans explained");
    for (music, pins) in the_music_in_ticks() {
        let (r, all) = explained(&music, &pins, MOST_READINGS);
        if let Err(why) = keeps_the_rules(&music, &music.hear(), &r, &all) {
            panic!("{why}\n{music:?}");
        }
    }
}

/// Everything an explanation holds, as bits, with each degree and the notes' keys moved back by `shift` semitones.
fn everything(e: &Explanation, shift: i32) -> String {
    let back = |c: crate::chord::Chord| c.transposed(-shift);
    let notes: Vec<String> = e
        .notes
        .iter()
        .map(|w| format!("{} {} {} {:?} {:?}", w.part, w.note, i32::from(w.key) - shift, w.heard_as, w.does))
        .collect();
    let terms: Vec<String> = e
        .terms
        .iter()
        .map(|t| {
            format!(
                "{:?} {:x} {:x} {:x} {:x}",
                t.term,
                t.values[0].to_bits(),
                t.values[1].to_bits(),
                t.scores[0].to_bits(),
                t.scores[1].to_bits()
            )
        })
        .collect();
    let second = e.second.map(|r| back(r.chord));
    let totals = e.totals.map(f64::to_bits);
    format!("{} {:?} {second:?} {:?} {notes:?} {terms:?} {totals:x?}", e.span, back(e.first.chord), e.bass_from)
}

#[test]
fn the_same_notes_moved_or_at_another_resolution_are_explained_the_same() {
    for (music, pins) in the_music_in_ticks() {
        let (_, all) = explained(&music, &pins, 2);
        let want: Vec<Option<String>> = all.iter().map(|e| e.as_ref().map(|e| everything(e, 0))).collect();
        for shift in [-6, -1, 1, 5] {
            let moved = music.map(|n| Note { key: (i32::from(n.key) + shift) as u8, ..n });
            let up = |pin: &Pin| Pin { chord: pin.chord.map(|c| c.transposed(shift)), ..*pin };
            let pins: Vec<Pin> = pins.iter().map(up).collect();
            let (_, all) = explained(&moved, &pins, 2);
            let got: Vec<Option<String>> = all.iter().map(|e| e.as_ref().map(|e| everything(e, shift))).collect();
            assert_eq!(got, want, "moved {shift}");
        }
        for ppq in [96, 960] {
            let scale = |t: i64| t * ppq / 480;
            let mut other = music.map(|n| Note { start: scale(n.start), length: scale(n.length), ..n });
            other.grid.ticks_per_beat = scale(i64::from(music.grid.ticks_per_beat)) as u32;
            let pins: Vec<Pin> = pins.iter().map(|p| Pin { start: scale(p.start), end: scale(p.end), ..*p }).collect();
            let (_, all) = explained(&other, &pins, 2);
            let got: Vec<Option<String>> = all.iter().map(|e| e.as_ref().map(|e| everything(e, 0))).collect();
            assert_eq!(got, want, "at {ppq} ticks a beat");
        }
    }
}

#[test]
fn a_reading_of_other_notes_a_rest_or_a_stretch_not_there_explains_nothing() {
    let music = Music::new()
        .bass([notes("C2", 0, BAR), notes("F2", 2 * BAR, BAR)].concat())
        .keys([notes("C4 E4 G4", 0, BAR), notes("F3 A3 C4", 2 * BAR, BAR)].concat());
    let parts = parts(&music);
    let cx = ChordContext::new(music.span);
    let mut reader = ChordReader::new();
    let r = reader.read(&parts, music.grid, &cx).unwrap();
    // C | rest | F: the rest has nothing to explain, and there's no fourth stretch.
    assert_eq!(r.spans.len(), 3);
    assert!(reader.explain(&parts, music.grid, &cx, &r, 0).is_some());
    assert_eq!(reader.explain(&parts, music.grid, &cx, &r, 1), None);
    assert_eq!(reader.explain(&parts, music.grid, &cx, &r, 3), None);
    // Other notes, another grid, a loop: not this reading.
    let other = music.map(|n| Note { start: n.start + BEAT, ..n });
    assert_eq!(reader.explain(&self::parts(&other), music.grid, &cx, &r, 0), None);
    let mut grid = music.grid;
    grid.bar_line = BEAT;
    assert_eq!(reader.explain(&parts, grid, &cx, &r, 0), None);
    let looped = ChordContext::new(crate::keyfind::Span::Loop);
    assert_eq!(reader.explain(&parts, music.grid, &looped, &r, 0), None);
    let fewer = Music { parts: vec![(Role::Bass, notes("C2", 0, BAR)), music.parts[1].clone()], ..music.clone() };
    assert_eq!(reader.explain(&self::parts(&fewer), music.grid, &cx, &r, 0), None);
    // Nothing heard.
    assert_eq!(reader.explain(&[], music.grid, &cx, &r, 0), None);
    // A reader kept and used again explains as a new one.
    let mut fresh = ChordReader::new();
    let tune = Music::new().tune(line("C5 D5 E5 G5", 0, BEAT));
    assert!(reader.read(&self::parts(&tune), tune.grid, &cx).is_some());
    assert_eq!(reader.explain(&parts, music.grid, &cx, &r, 2), fresh.explain(&parts, music.grid, &cx, &r, 2));
}
