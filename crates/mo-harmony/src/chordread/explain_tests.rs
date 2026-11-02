//! Tests of the explanations ([`super::ChordExplainer`]): what each note does in the design's hand cases and in music
//! made for each kind of note, where the bass comes from, the evidence as the scoring weighs it (and as the decode
//! scored it), chords held on from the stretch before, the rules every explanation keeps for any music, that the same
//! notes moved or at another resolution explain the same, and that a reading of other notes or pins explains nothing.

use super::decode::{Reading as Kept, kept_pins};
use super::decode_tests::small_music;
use super::hear_tests::{BAR, BEAT, Dice, Music, line, notes, random_music};
use super::name::push_to;
use super::reader::pinned;
use super::reader_tests::{chord, hand_cases, random_pins, the_music_in_ticks};
use super::{
    BassFrom, ChordContext, ChordExplainer, ChordReader, ChordReading, ChordSpan, Decoder, Decoding, Evidence,
    Explanation, Feature, Hearing, Label, MOST_READINGS, NoteDoes, NoteWhy, PerFeature, Pin, PinnedSpan, Priors,
    SHIPPED, SpanFacts, SpanFlags, Stream, TERMS_SHOWN, Term, TermWhy,
};
use crate::notes::{Note, Part, Role};
use crate::pitch::{PcSet, PitchClass};

/// A key's name, C4 being 60, with flats but F#: "Ab2".
fn name(key: u8) -> String {
    const NAMES: [&str; 12] = ["C", "Db", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];
    format!("{}{}", NAMES[usize::from(key % 12)], i32::from(key / 12) - 1)
}

pub(super) fn parts(music: &Music) -> Vec<Part<'_>> {
    music.parts.iter().map(|(role, notes)| Part { role: *role, notes }).collect()
}

/// What some music reads with some pins, keeping `readings` a span, and each span explained, by one explainer;
/// explained all at once too, which must say the same.
pub(super) fn explained(music: &Music, pins: &[Pin], readings: u8) -> (ChordReading, Vec<Option<Explanation>>) {
    let parts = parts(music);
    let cx = ChordContext { span: music.span, pins, readings };
    let reading = ChordReader::new().read(&parts, music.grid, &cx).expect("nothing read");
    let mut explainer = ChordExplainer::new();
    let all: Vec<Option<Explanation>> =
        (0..reading.spans.len()).map(|s| explainer.explain(&parts, music.grid, &cx, &reading, s)).collect();
    let at_once = explainer.explain_all(&parts, music.grid, &cx, &reading).expect("not a reading of the notes");
    assert_eq!(at_once, all.iter().flatten().cloned().collect::<Vec<_>>(), "explained all at once");
    (reading, all)
}

/// Where every reading starts a chord, as the decode reads some pins: where each pin it keeps starts and ends.
fn edges(hearing: &Hearing, pins: &[Pin]) -> Vec<usize> {
    let pinned: Vec<PinnedSpan> = pins.iter().map(|p| pinned(hearing, p)).collect();
    kept_pins(hearing, &pinned).iter().flat_map(|(units, _)| [units.start, units.end]).collect()
}

/// Whether a reading of a stretch is the chord before it held on (for a loop's first stretch, the chord the decode
/// reads before it as the loop goes round): no pin starts or ends between them.
fn held_on(r: &ChordReading, span: usize, label: Label, edges: &[usize], reported: usize) -> bool {
    let before = match span.checked_sub(1) {
        Some(b) => r.spans[b].label,
        None => r.before,
    };
    before == Some(label) && !edges.contains(&(reported + r.spans[span].units.start))
}

/// Whether a piece of evidence is about where a chord starts.
fn about_the_start(t: &TermWhy) -> bool {
    matches!(t.term, Term::Feature(f) if f.once())
}

/// A hand case's explanation of one span, every reading kept.
fn case(name: &str, span: usize) -> Explanation {
    let (_, all) = explained(&case_music(name), &[], MOST_READINGS);
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
    let (mut facts, mut plain) = (SpanFacts::new(), SpanFacts::new());
    let mut held_where_pushed = 0;
    for (music, pins) in the_music_in_ticks() {
        let hearing = music.hear();
        let edges = edges(&hearing, &pins);
        let (reading, all) = explained(&music, &pins, 2);
        evidence.prepare(&hearing);
        for (i, (s, e)) in reading.spans.iter().zip(&all).enumerate() {
            let Some(e) = e else { continue };
            // The evidence worked out again, on the span's units with the pushes to its ends; and with none to its
            // start, for a reading held on (from where the chord before ends: never from a chord pushed in early).
            let units = hearing.reported().start + s.units.start..hearing.reported().start + s.units.end;
            let pushed = |s: &ChordSpan| s.flags.contains(SpanFlags::PUSHED);
            let start = if pushed(s) { push_to(&hearing, units.start) } else { None };
            let next = reading.spans.get(i + 1).is_some_and(pushed);
            let end = if next { push_to(&hearing, units.end) } else { None };
            evidence.facts(&hearing, units.clone(), start, end, &mut facts);
            evidence.facts(&hearing, units, None, end, &mut plain);
            let readings = [Some(e.first), e.second];
            let held =
                readings.map(|r| r.is_some_and(|r| held_on(&reading, i, r.label, &edges, hearing.reported().start)));
            let facts_of = |k: usize| if held[k] { &plain } else { &facts };
            held_where_pushed += held.iter().filter(|&&h| h && start.is_some()).count();
            let mut values = [PerFeature::default(); 2];
            for (k, (r, out)) in readings.iter().zip(&mut values).enumerate() {
                let Some(r) = r else { continue };
                facts_of(k).features(r.label, &priors, out);
                // A chord held on from the stretch before has none of the evidence about where a chord starts.
                if held[k] {
                    Feature::ALL.into_iter().filter(|f| f.once()).for_each(|f| out[f] = 0.0);
                }
            }
            let before = match i.checked_sub(1) {
                Some(b) => reading.spans[b].label,
                None => reading.before,
            };
            let after = reading.spans.get(i + 1).and_then(|s| s.label);
            let pairs = readings.map(|r| {
                r.map_or(0.0, |r| {
                    before.map_or(0.0, |b| SHIPPED.tables.pair_prior(b, r.label))
                        + after.map_or(0.0, |a| SHIPPED.tables.pair_prior(r.label, a))
                })
            });
            let what = format!("span {i}: {}", render(e));
            // Every piece that differs is there, as the scoring weighs it, and no other.
            let mut want = Vec::new();
            for f in Feature::ALL {
                let counted = |k: usize| if f.once() { 1.0 } else { facts_of(k).beats() };
                let scores = [w[f] * values[0][f] * counted(0), w[f] * values[1][f] * counted(1)];
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
            let score = |k: usize| facts_of(k).score(&values[k], SHIPPED.weights) + pair * pairs[k];
            let totals = [score(0), e.second.map_or(0.0, |_| score(1))];
            for (got, want) in e.totals.iter().zip(totals) {
                assert!((got - want).abs() <= 1e-9 * want.abs().max(1.0), "{what}: {got} not {want}");
            }
        }
    }
    assert!(held_where_pushed > 0);
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

/// The rules every explanation of a reading keeps, as the reason one is broken; and how many of its readings are held
/// on, first readings and runners-up.
fn keeps_the_rules(
    music: &Music,
    hearing: &Hearing,
    pins: &[Pin],
    r: &ChordReading,
    all: &[Option<Explanation>],
) -> Result<[usize; 2], String> {
    let check = |ok: bool, what: &str| if ok { Ok(()) } else { Err(what.to_owned()) };
    let edges = edges(hearing, pins);
    let mut held = [0, 0];
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
            // Only the bass walks or approaches, and only the bass is the bass, but where it doubles a bass written
            // under the chord that's only a bass.
            let doubles = matches!(w.does, NoteDoes::Bass(_)) && pc == chord.bass() && !chord.notes().contains(pc);
            let bass_like = matches!(w.does, NoteDoes::Bass(_) | NoteDoes::Walking | NoteDoes::Approach) && !doubles;
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
            // A note that passes by hangs on notes (an anticipation on its own), and no other note does.
            let anchors_ok = match w.does {
                NoteDoes::Anticipation => w.anchors == PcSet::EMPTY.with(pc),
                NoteDoes::Passing | NoteDoes::Neighbour | NoteDoes::Approach => {
                    !w.anchors.is_empty() && !w.anchors.contains(pc)
                }
                NoteDoes::Suspension | NoteDoes::Appoggiatura => !w.anchors.is_empty(),
                _ => w.anchors.is_empty(),
            };
            check(anchors_ok, &what(&format!("{} hangs on {:?}", note(w), w.anchors)))?;
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
        // The bass each one is weighed on: none for no runner-up, and the same for both but where only one of them is
        // held on (weighed on the stretch's own units when a chord is pushed to it).
        check(e.second.is_some() || e.held_bass[1].is_none(), &what("no runner-up's bass"))?;
        let may_differ = e.second.is_none() || e.held_on[0] != e.held_on[1];
        check(may_differ || e.held_bass[0] == e.held_bass[1], &what("the bass held"))?;
        // A reading held on from the chord before has nothing about where a chord starts.
        for (k, reading) in [Some(e.first), e.second].iter().enumerate() {
            let is_held = reading.is_some_and(|x| held_on(r, i, x.label, &edges, hearing.reported().start));
            check(e.held_on[k] == is_held, &what("held on, as said"))?;
            if is_held {
                check(e.terms.iter().filter(|t| about_the_start(t)).all(|t| t.values[k] == 0.0), &what("held on"))?;
                held[k] += 1;
            }
        }
    }
    Ok(held)
}

#[test]
fn any_music_with_any_pins_is_explained_by_the_rules() {
    let mut dice = Dice(0x3c6e_f372_fe94_f82b);
    let mut hearing = Hearing::new();
    let (mut explained_spans, mut held) = (0, [0, 0]);
    for round in 0..200 {
        let music = if round % 2 == 0 { random_music(&mut dice) } else { small_music(&mut dice) };
        if !music.hear_into(&mut hearing) {
            continue;
        }
        let pins = random_pins(&mut dice, &hearing);
        let keep = dice.pick(&[1, 2, 6]);
        let (r, all) = explained(&music, &pins, keep);
        match keeps_the_rules(&music, &hearing, &pins, &r, &all) {
            Ok([first, second]) => held = [held[0] + first, held[1] + second],
            Err(why) => panic!("round {round}: {why}\n{music:?}, pins {pins:?}"),
        }
        explained_spans += all.iter().flatten().count();
    }
    assert!(explained_spans > 500, "{explained_spans} spans explained");
    // First readings and runners-up held on are both met.
    assert!(held[0] > 10 && held[1] > 10, "held on: {held:?}");
    for (music, pins) in the_music_in_ticks() {
        let (r, all) = explained(&music, &pins, MOST_READINGS);
        if let Err(why) = keeps_the_rules(&music, &music.hear(), &pins, &r, &all) {
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
        .map(|w| {
            let anchors = w.anchors.transposed(-shift).bits();
            format!("{} {} {} {:?} {:?} {anchors:x}", w.part, w.note, i32::from(w.key) - shift, w.heard_as, w.does)
        })
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
    let (first, bass_from, held_on) = (back(e.first.chord), e.bass_from, e.held_on);
    let held_bass = e.held_bass.map(|b| b.map(|pc| pc.up(-shift)));
    format!("{} {first:?} {second:?} {bass_from:?} {notes:?} {terms:?} {totals:x?} {held_on:?} {held_bass:?}", e.span)
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
fn a_reading_of_other_notes_or_pins_a_rest_or_a_stretch_not_there_explains_nothing() {
    let music = Music::new()
        .bass([notes("C2", 0, BAR), notes("F2", 2 * BAR, BAR)].concat())
        .keys([notes("C4 E4 G4", 0, BAR), notes("F3 A3 C4", 2 * BAR, BAR)].concat());
    let parts = parts(&music);
    let cx = ChordContext::new(music.span);
    let r = ChordReader::new().read(&parts, music.grid, &cx).unwrap();
    let mut explainer = ChordExplainer::new();
    // C | rest | F: the rest has nothing to explain, and there's no fourth stretch.
    assert_eq!(r.spans.len(), 3);
    assert!(explainer.explain(&parts, music.grid, &cx, &r, 0).is_some());
    assert_eq!(explainer.explain(&parts, music.grid, &cx, &r, 1), None);
    assert_eq!(explainer.explain(&parts, music.grid, &cx, &r, 3), None);
    assert_eq!(explainer.explain_all(&parts, music.grid, &cx, &r).map(|all| all.len()), Some(2));
    // Other notes, another grid, a loop: not this reading.
    let other = music.map(|n| Note { start: n.start + BEAT, ..n });
    assert_eq!(explainer.explain(&self::parts(&other), music.grid, &cx, &r, 0), None);
    let mut grid = music.grid;
    grid.bar_line = BEAT;
    assert_eq!(explainer.explain(&parts, grid, &cx, &r, 0), None);
    let looped = ChordContext::new(crate::keyfind::Span::Loop);
    assert_eq!(explainer.explain(&parts, music.grid, &looped, &r, 0), None);
    let fewer = Music { parts: vec![(Role::Bass, notes("C2", 0, BAR)), music.parts[1].clone()], ..music.clone() };
    assert_eq!(explainer.explain(&self::parts(&fewer), music.grid, &cx, &r, 0), None);
    // Nothing heard.
    assert_eq!(explainer.explain(&[], music.grid, &cx, &r, 0), None);
    assert_eq!(explainer.explain_all(&[], music.grid, &cx, &r), None);

    // C held two bars, then F: read with some pins and explained with others (one left out, one added, another
    // chord given, or the C cut in two where its second bar starts, by a pin with only its ends), not this reading.
    let music = Music::new()
        .bass([notes("C2", 0, 2 * BAR), notes("F2", 2 * BAR, BAR)].concat())
        .keys([notes("C4 E4 G4", 0, 2 * BAR), notes("F3 A3 C4", 2 * BAR, BAR)].concat());
    let parts = self::parts(&music);
    let mut explains = |read_with: &[Pin], explained_with: &[Pin]| {
        let read = ChordContext { span: music.span, pins: read_with, readings: 2 };
        let r = ChordReader::new().read(&parts, music.grid, &read).unwrap();
        let cx = ChordContext { pins: explained_with, ..read };
        explainer.explain_all(&parts, music.grid, &cx, &r).is_some()
    };
    let g7 = [Pin { start: 2 * BAR, end: 3 * BAR, chord: Some(chord("G7")) }];
    let g9 = [Pin { chord: Some(chord("G9")), ..g7[0] }];
    let cut = [Pin { start: BAR, end: 2 * BAR, chord: None }];
    for pins in [&[][..], &g7, &g9, &cut] {
        assert!(explains(pins, pins), "{pins:?}");
    }
    for (read_with, explained_with) in
        [(&g7[..], &[][..]), (&[], &g7), (&g7, &g9), (&cut, &[]), (&[], &cut), (&g7, &cut)]
    {
        assert!(!explains(read_with, explained_with), "read with {read_with:?}, explained with {explained_with:?}");
    }
}

#[test]
fn an_explainer_kept_and_used_again_explains_as_a_new_one() {
    let mut kept = ChordExplainer::new();
    let music = the_music_in_ticks();
    for (music, pins) in music.iter().chain(music.iter().rev()) {
        let parts = parts(music);
        let cx = ChordContext { span: music.span, pins, readings: 2 };
        let reading = ChordReader::new().read(&parts, music.grid, &cx).unwrap();
        let fresh = ChordExplainer::new().explain_all(&parts, music.grid, &cx, &reading);
        assert!(fresh.is_some());
        assert_eq!(kept.explain_all(&parts, music.grid, &cx, &reading), fresh);
    }
    // Asked for a reading of other notes (a tune alone), then for one of the notes given.
    let tune = Music::new().tune(line("C5 D5 E5 G5", 0, BEAT));
    let music = case_music("10 Dm9 | G13 | Cmaj9");
    let cx = ChordContext { span: music.span, pins: &[], readings: 2 };
    let reading = ChordReader::new().read(&parts(&music), music.grid, &cx).unwrap();
    let fresh = ChordExplainer::new().explain_all(&parts(&music), music.grid, &cx, &reading);
    assert_eq!(fresh.as_ref().map(Vec::len), Some(3));
    assert_eq!(kept.explain_all(&parts(&tune), tune.grid, &cx, &reading), None);
    assert_eq!(kept.explain_all(&parts(&music), music.grid, &cx, &reading), fresh);
}

/// Whether two scores are the same up to rounding.
fn close(got: f64, want: f64) -> bool {
    (got - want).abs() <= 1e-9 * want.abs().max(1.0)
}

/// Checks each stretch of some music read with some pins against what the decode scored there, where the stretch is
/// one span of the decode's best way, read as a chord; counting the first readings checked, the runners-up, and the
/// runners-up held on into a stretch whose chord is pushed.
fn scored_as_decoded(music: &Music, pins: &[Pin], checked: &mut [usize; 3]) {
    // The decode's own scores, as its quick scoring worked them out: each kept reading's on each span, from a decode
    // of its own.
    let hearing = music.hear();
    let pinned: Vec<PinnedSpan> = pins.iter().map(|p| pinned(&hearing, p)).collect();
    let (mut decoder, mut decoding) = (Decoder::new(), Decoding::new());
    assert!(decoder.decode(&hearing, SHIPPED.weights, SHIPPED.tables, &pinned, &mut decoding));
    let lattice = decoder.lattice();
    let (reported, edges) = (hearing.reported().start, edges(&hearing, pins));
    let (_, alone) = explained(music, pins, 1);
    let (reading, all) = explained(music, pins, 2);
    for (i, ((s, alone), e)) in reading.spans.iter().zip(&alone).zip(&all).enumerate() {
        let (Some(alone), Some(e)) = (alone, e) else { continue };
        // Only a stretch that's one span of the decode's best way is scored as the decode scored it (a chord held
        // longer than a span, or cut in two, is read on all of it at once).
        let units = reported + s.units.start..reported + s.units.end;
        let at = |place: usize| lattice.places[place].0;
        let on_best: Vec<_> = lattice
            .best
            .iter()
            .filter(|&&(k, _)| at(lattice.spans[k].0) < units.end && at(lattice.spans[k].1) > units.start)
            .collect();
        let [&(k, label)] = on_best[..] else { continue };
        let &(from, to, _, ref kept) = &lattice.spans[k];
        // And one read as a chord: where the decode reads no chord, the first reading is only the likeliest.
        if (at(from), at(to)) != (units.start, units.end) || s.flags.contains(SpanFlags::SPLIT) || label.is_none() {
            continue;
        }
        let what = format!("span {i}: {}", render(e));
        assert_eq!(label, Some(e.first.label), "{what}");
        // Each one's score on the stretch is its total less its pair term (with no runner-up, the pair term is shown
        // whenever it adds anything; with one, it's the same for both when it isn't shown). Its kept score, or its
        // score held on when it's the chord before held on.
        let pair = |e: &Explanation, k: usize| e.terms.iter().find(|t| t.term == Term::Pair).map(|t| t.scores[k]);
        let kept_score = |kept: &[Kept], label: Label, held: bool| {
            kept.iter().find(|r| r.0 == Some(label)).map(|r| if held { r.2 } else { r.1 })
        };
        // The first reading starts a chord here: held on, the decode would have read the stretches as one.
        assert!(!held_on(&reading, i, e.first.label, &edges, reported), "{what}");
        let first = pair(alone, 0).unwrap_or(0.0);
        let want = kept_score(kept, e.first.label, false).expect("the best way's reading is kept");
        assert!(close(alone.totals[0] - first, want), "{what}: {} not {want}", alone.totals[0] - first);
        checked[0] += 1;
        let Some(second) = e.second else { continue };
        let held = held_on(&reading, i, second.label, &edges, reported);
        // Held on, it's held on from where the chord before ends (never from a chord pushed in early): its score on
        // the span from the place there with no chord pushed to it.
        let plain = match lattice.places[from] {
            (unit, true, _) if held => lattice.places.iter().position(|p| (p.0, p.1) == (unit, false)),
            _ => None,
        };
        let kept = match plain {
            Some(plain) => lattice.spans.iter().find(|span| (span.0, span.1) == (plain, to)).map_or(&[][..], |s| &s.3),
            None => kept,
        };
        if let Some(want) = kept_score(kept, second.label, held) {
            let got = e.totals[1] - pair(e, 1).unwrap_or(first);
            assert!(close(got, want), "{what}: the runner-up's {got} not {want}");
            checked[1] += 1;
            checked[2] += usize::from(plain.is_some());
        }
    }
}

#[test]
fn the_evidence_adds_up_to_what_the_decode_scored() {
    let mut checked = [0, 0, 0];
    for (music, pins) in the_music_in_ticks() {
        scored_as_decoded(&music, &pins, &mut checked);
    }
    assert!(checked[0] > 50 && checked[1] > 30, "{checked:?} checked");
    // And any music with any pins, where a runner-up held on into a stretch whose chord is pushed is met (rare: one
    // piece in several hundred has one the decode kept a score for).
    let mut dice = Dice(0x510e_527f_ade6_82d1);
    let mut hearing = Hearing::new();
    for round in 0..100 {
        let music = if round % 2 == 0 { random_music(&mut dice) } else { small_music(&mut dice) };
        if !music.hear_into(&mut hearing) {
            continue;
        }
        let pins = random_pins(&mut dice, &hearing);
        scored_as_decoded(&music, &pins, &mut checked);
    }
    assert!(checked[0] > 400 && checked[1] > 250 && checked[2] > 0, "{checked:?} checked");
}

#[test]
fn a_chord_held_on_from_the_stretch_before_has_no_evidence_about_where_a_chord_starts() {
    // C C/B | Am: the C/B is the C held on, its bass moving to its seventh halfway through the bar. Nothing about where
    // a chord starts counts for it, and the runner-up starts a chord in the middle of the bar.
    let (r, all) = explained(&case_music("4 C C/B | Am"), &[], 2);
    assert!(r.spans[1].flags.contains(SpanFlags::SPLIT), "{:?}", r.spans[1]);
    let e = all[1].clone().unwrap();
    assert_eq!((e.first.chord.name(), e.first.label), ("C/B".to_owned(), r.spans[0].label.unwrap()));
    assert!(e.terms.iter().filter(|t| about_the_start(t)).all(|t| t.values[0] == 0.0), "{}", render(&e));
    let mid_bar = e.terms.iter().find(|t| t.term == Term::Feature(Feature::MidBar)).expect("the runner-up's start");
    assert_eq!(mid_bar.values, [0.0, 1.0]);
    // Its first half starts the chord, after the Am going round (with no runner-up, all its evidence is told).
    let first_alone = |music: &Music, pins: &[Pin]| explained(music, pins, 1).1[0].clone().unwrap();
    let starts = |e: &Explanation| e.terms.iter().any(|t| about_the_start(t) && t.values[0] != 0.0);
    let e = first_alone(&case_music("4 C C/B | Am"), &[]);
    assert!(starts(&e), "{}", render(&e));
    // C C/E | F F#dim7 | C/G goes round from C/G into C: the same chord held on, its bass moving.
    let e = first_alone(&case_music("6 C C/E | F F#dim7 | C/G"), &[]);
    assert!(!starts(&e), "{}", render(&e));
    // A one-chord loop goes round into itself, held on; but a pin over it starts a chord where it starts.
    let music = case_music("1 C");
    let e = first_alone(&music, &[]);
    assert!(!starts(&e), "{}", render(&e));
    let hearing = music.hear();
    let (start, end) = (hearing.start(hearing.reported().start), hearing.start(hearing.units()));
    let e = first_alone(&music, &[Pin { start, end, chord: None }]);
    assert!(starts(&e), "{}", render(&e));
    // Dm9 | G13 | Cmaj9 goes round from Cmaj9 to Dm9: a chord starts there.
    let e = first_alone(&case_music("10 Dm9 | G13 | Cmaj9"), &[]);
    assert!(starts(&e), "{}", render(&e));
}

/// A hand case's music, read as a loop.
pub(super) fn case_music(name: &str) -> Music {
    hand_cases().into_iter().find(|(n, _)| *n == name).unwrap_or_else(|| panic!("no case {name}")).1
}

#[test]
fn the_bass_written_under_a_chord_doubled_in_the_tune_is_the_bass() {
    // D/C with the tune's C6 struck for an eighth off the beat: it doubles the bass the name writes.
    let music = Music::new().bass(notes("C2", 0, BAR)).keys(notes("D4 F#4 A4", 0, BAR)).tune(
        [notes("A5", 0, BEAT + BEAT / 2), notes("C6", BEAT + BEAT / 2, BEAT / 2), notes("A5", 2 * BEAT, 2 * BEAT)]
            .concat(),
    );
    let e = first_explained(music);
    assert_eq!(e.first.chord.name(), "D/C", "{}", render(&e));
    assert!(notes_of(&e).contains(&"tune C6 on b7".to_owned()), "{}", render(&e));
}

#[test]
fn a_bass_note_only_held_over_into_a_bar_isnt_the_bass_part_playing_there() {
    // The bass part's C2 rings half a beat into the bar of F, which the keys play with F2 under them: the bass there
    // is the keys' lowest note, and the C2 is held over.
    let music = Music::new()
        .bass(notes("C2", 0, BAR + BEAT / 2))
        .keys([notes("C4 E4 G4", 0, BAR), notes("F2 A3 C4 F4", BAR, BAR)].concat());
    let (r, all) = explained(&music, &[], 2);
    let shown: Vec<String> = r.spans.iter().map(|s| s.shown().unwrap().chord.name()).collect();
    assert_eq!(shown, ["C", "F"]);
    let e = all[1].clone().unwrap();
    assert_eq!(e.bass_from, BassFrom::LowestKeys, "{}", render(&e));
    assert!(notes_of(&e).contains(&"bass C2 HeldOver".to_owned()), "{}", render(&e));
    assert!(notes_of(&e).contains(&"bass F2 on 1".to_owned()), "{}", render(&e));
}
