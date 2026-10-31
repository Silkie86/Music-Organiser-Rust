//! Tests of the answer given from the chances: the thresholds, ties, each kind of answer, colours, the eight-byte
//! fact, and checking a key someone gave.

use super::*;
use crate::key::Mode;
use crate::pitch::{PerPitch, PitchClass};

fn pc(n: i32) -> PitchClass {
    PitchClass::new(n)
}

const C_MAJOR: Key = Key::major(PitchClass::C);
const C_MINOR: Key = Key::minor(PitchClass::C);
const EB_MAJOR: Key = Key::major(PitchClass::new(3));
const G_MAJOR: Key = Key::major(PitchClass::G);
const A_MINOR: Key = Key::minor(PitchClass::A);

/// Chances with these keys at these values, the rest sharing what's left evenly.
fn chances_with(top: &[(Key, f64)]) -> [f64; 24] {
    let used: f64 = top.iter().map(|(_, p)| p).sum();
    let mut c = [(1.0 - used) / (24 - top.len()) as f64; 24];
    for (k, p) in top {
        c[k.index()] = *p;
    }
    c
}

/// Plenty of notes, well inside one scale, cut from a song.
fn grading(bars: u32) -> Grading {
    Grading { bars_heard: bars, notes: 20, diatonic_fit: 0.95, span: Span::Window }
}

fn up(key: Key, semitones: i32) -> Key {
    Key { tonic: key.tonic.up(semitones), mode: key.mode }
}

/// The chances of notes moved up this many semitones.
fn turned(chances: &[f64; 24], semitones: i32) -> [f64; 24] {
    let mut out = [0.0; 24];
    for k in Key::ALL {
        out[up(k, semitones).index()] = chances[k.index()];
    }
    out
}

/// The answer for notes moved up this many semitones.
fn moved(answer: KeyAnswer, semitones: i32) -> KeyAnswer {
    match answer {
        KeyAnswer::Key(k) => KeyAnswer::Key(up(k, semitones)),
        KeyAnswer::OrRelative(k) => KeyAnswer::OrRelative(up(k, semitones)),
        KeyAnswer::HomeNote { tonic, leaning } => KeyAnswer::HomeNote { tonic: tonic.up(semitones), leaning },
        KeyAnswer::Unclear { why, leaning } => KeyAnswer::Unclear { why, leaning: leaning.map(|k| up(k, semitones)) },
    }
}

// Thresholds.

#[test]
fn thresholds_fall_with_more_music_and_never_jump() {
    let t = SHIPPED.thresholds;
    assert_eq!(t.at(1, Span::Window), (0.85, 0.9, 0.85));
    assert_eq!(t.at(4, Span::Window), (0.8, 0.9, 0.85));
    assert_eq!(t.at(32, Span::Window), (0.7, 0.9, 0.85));
    // Past the last knot it stays; a song takes the last knot's; a loop is a window's; no bars is one bar.
    assert_eq!(t.at(1000, Span::Window), t.at(32, Span::Window));
    assert_eq!(t.at(1, Span::Song), t.at(32, Span::Window));
    assert_eq!(t.at(8, Span::Loop), t.at(8, Span::Window));
    assert_eq!(t.at(0, Span::Window), t.at(1, Span::Window));
    let mut last = t.at(1, Span::Window);
    for bars in 2..=64 {
        let now = t.at(bars, Span::Window);
        for (a, b) in [(last.0, now.0), (last.1, now.1), (last.2, now.2)] {
            assert!(b <= a && a - b <= 0.05, "{bars} bars: {a} then {b}");
        }
        last = now;
    }
    // Every knot of the shipped thresholds is a chance, and none rises with more music.
    for knots in [t.one, t.pair, t.home] {
        assert!(knots.iter().all(|x| (0.0..=1.0).contains(x)));
        assert!(knots.windows(2).all(|w| w[1] <= w[0]), "{knots:?}");
    }
}

// Ranking and ties.

#[test]
fn keys_are_ranked_by_chance_then_by_their_place_above_the_first() {
    // C major first; then three keys tied, in order of how far their tonics are above C, major first.
    let c = chances_with(&[(C_MAJOR, 0.4), (A_MINOR, 0.1), (G_MAJOR, 0.1), (Key::minor(PitchClass::G), 0.1)]);
    let (ranked, clear) = ranked(&c);
    assert!(clear);
    let keys: Vec<Key> = ranked.iter().take(4).map(|(k, _)| *k).collect();
    assert_eq!(keys, [C_MAJOR, G_MAJOR, Key::minor(PitchClass::G), A_MINOR]);
    // Moved to any key, the order moves with it.
    for s in 0..12 {
        let (moved, clear) = super::ranked(&turned(&c, s));
        assert!(clear);
        for (a, b) in moved.iter().zip(&ranked) {
            assert_eq!(a.0, up(b.0, s), "moved {s}");
            assert_eq!(a.1.to_bits(), b.1.to_bits());
        }
    }
}

#[test]
fn a_tie_for_the_first_is_clear_only_between_a_relative_or_parallel_pair() {
    for s in 0..12 {
        // C major and A minor as likely: C major first, whatever key it's moved to.
        let (r, clear) = ranked(&turned(&chances_with(&[(C_MAJOR, 0.45), (A_MINOR, 0.45)]), s));
        assert!(clear);
        assert_eq!((r[0].0, r[1].0), (up(C_MAJOR, s), up(A_MINOR, s)), "moved {s}");
        // C minor and C major: the major first.
        let (r, clear) = ranked(&turned(&chances_with(&[(C_MINOR, 0.45), (C_MAJOR, 0.45)]), s));
        assert!(clear);
        assert_eq!((r[0].0, r[1].0), (up(C_MAJOR, s), up(C_MINOR, s)));
        // C major and G major: no way to choose that moves with the notes.
        let (_, clear) = ranked(&turned(&chances_with(&[(C_MAJOR, 0.45), (G_MAJOR, 0.45)]), s));
        assert!(!clear);
        // Three keys tied, even with a pair among them.
        let three = chances_with(&[(C_MAJOR, 0.3), (A_MINOR, 0.3), (C_MINOR, 0.3)]);
        assert!(!ranked(&turned(&three, s)).1);
    }
    // Every key as likely, or chances that aren't numbers: nothing is clear.
    assert!(!ranked(&[1.0 / 24.0; 24]).1);
    assert!(!ranked(&[f64::NAN; 24]).1);
}

// Each kind of answer.

#[test]
fn too_few_notes_then_no_scale_come_first() {
    let sure = chances_with(&[(C_MAJOR, 0.99)]);
    let few = Grading { notes: MIN_NOTES - 1, ..grading(4) };
    assert_eq!(grade(&sure, few, &SHIPPED), (KeyAnswer::Unclear { why: Why::TooFewNotes, leaning: None }, 0.0));
    for fit in [0.7, 0.0, f64::NAN] {
        let atonal = Grading { diatonic_fit: fit, ..grading(4) };
        assert_eq!(grade(&sure, atonal, &SHIPPED), (KeyAnswer::Unclear { why: Why::NotTonal, leaning: None }, 0.0));
    }
    // Exactly enough notes, exactly at the line: a key.
    let edge = Grading { notes: MIN_NOTES, diatonic_fit: SHIPPED.not_tonal_below, ..grading(4) };
    assert_eq!(grade(&sure, edge, &SHIPPED).0, KeyAnswer::Key(C_MAJOR));
}

#[test]
fn each_kind_of_answer_and_its_chance() {
    // One key at its threshold (0.8 at 4 bars), and just under it.
    assert_eq!(grade(&chances_with(&[(C_MAJOR, 0.8)]), grading(4), &SHIPPED), (KeyAnswer::Key(C_MAJOR), 0.8));
    let under = chances_with(&[(C_MAJOR, 0.79)]);
    assert_eq!(
        grade(&under, grading(4), &SHIPPED),
        (KeyAnswer::Unclear { why: Why::Spread, leaning: Some(C_MAJOR) }, 0.79)
    );
    // The same chance is enough for a song (0.7), not for one bar (0.85).
    let c = chances_with(&[(C_MAJOR, 0.75)]);
    assert_eq!(grade(&c, Grading { span: Span::Song, ..grading(200) }, &SHIPPED).0, KeyAnswer::Key(C_MAJOR));
    assert_ne!(grade(&c, grading(1), &SHIPPED).0, KeyAnswer::Key(C_MAJOR));

    // A key and its relative: C minor, then Eb major.
    let pair = chances_with(&[(C_MINOR, 0.6), (EB_MAJOR, 0.35)]);
    let (answer, chance) = grade(&pair, grading(4), &SHIPPED);
    assert_eq!(answer, KeyAnswer::OrRelative(C_MINOR));
    assert_eq!(chance, pair[C_MINOR.index()] + pair[EB_MAJOR.index()]);
    // Not when the runner-up isn't the relative.
    let fifth = chances_with(&[(C_MAJOR, 0.6), (G_MAJOR, 0.35)]);
    assert_eq!(grade(&fifth, grading(4), &SHIPPED).0, KeyAnswer::Unclear { why: Why::Spread, leaning: Some(C_MAJOR) });

    // A home note, leaning major...
    let home = chances_with(&[(C_MAJOR, 0.5), (C_MINOR, 0.4)]);
    let (answer, chance) = grade(&home, grading(4), &SHIPPED);
    assert_eq!(answer, KeyAnswer::HomeNote { tonic: PitchClass::C, leaning: Some(Mode::Major) });
    assert_eq!(chance, 0.5 + 0.4);
    // ...or to neither mode.
    let even = chances_with(&[(C_MINOR, 0.45), (C_MAJOR, 0.45)]);
    assert_eq!(grade(&even, grading(4), &SHIPPED).0, KeyAnswer::HomeNote { tonic: PitchClass::C, leaning: None });

    // A relative pair as likely: given as the pair, major first.
    let tied = chances_with(&[(A_MINOR, 0.45), (C_MAJOR, 0.45)]);
    assert_eq!(grade(&tied, grading(4), &SHIPPED).0, KeyAnswer::OrRelative(C_MAJOR));
    // Two keys that aren't a pair as likely: no clear key, leaning nowhere.
    let split = chances_with(&[(C_MAJOR, 0.45), (G_MAJOR, 0.45)]);
    assert_eq!(grade(&split, grading(4), &SHIPPED), (KeyAnswer::Unclear { why: Why::Spread, leaning: None }, 0.0));
}

#[test]
fn every_answer_moves_with_the_notes() {
    let patterns = [
        chances_with(&[(C_MAJOR, 0.8)]),
        chances_with(&[(C_MINOR, 0.6), (EB_MAJOR, 0.35)]),
        chances_with(&[(C_MAJOR, 0.5), (C_MINOR, 0.4)]),
        chances_with(&[(C_MINOR, 0.45), (C_MAJOR, 0.45)]),
        chances_with(&[(A_MINOR, 0.45), (C_MAJOR, 0.45)]),
        chances_with(&[(C_MAJOR, 0.45), (G_MAJOR, 0.45)]),
        chances_with(&[(C_MAJOR, 0.5), (G_MAJOR, 0.3)]),
    ];
    for c in patterns {
        for bars in [1, 3, 8, 40] {
            let (answer, chance) = grade(&c, grading(bars), &SHIPPED);
            for s in 1..12 {
                let (a, p) = grade(&turned(&c, s), grading(bars), &SHIPPED);
                assert_eq!(a, moved(answer, s), "{answer:?} moved {s}");
                assert_eq!(p.to_bits(), chance.to_bits());
            }
        }
    }
}

#[test]
fn what_an_answer_names() {
    let pair = KeyAnswer::OrRelative(C_MINOR);
    assert!(pair.names(C_MINOR) && pair.names(EB_MAJOR) && !pair.names(C_MAJOR));
    let home = KeyAnswer::HomeNote { tonic: PitchClass::C, leaning: None };
    assert!(home.names(C_MAJOR) && home.names(C_MINOR) && !home.names(A_MINOR));
    assert_eq!(home.leaning_key(), None);
    assert_eq!(home.tonic(), Some(PitchClass::C));
    let leaning = KeyAnswer::HomeNote { tonic: PitchClass::C, leaning: Some(Mode::Minor) };
    assert_eq!(leaning.leaning_key(), Some(C_MINOR));
    let unclear = KeyAnswer::Unclear { why: Why::Spread, leaning: Some(G_MAJOR) };
    assert!(!unclear.names(G_MAJOR));
    assert_eq!((unclear.leaning_key(), unclear.tonic()), (Some(G_MAJOR), None));
}

// Colours.

/// What was heard: how long each note sounds above C (tune and other parts together), and the bass.
fn heard_with(time: &[(i32, u64)], bass_c: u64, flat7_on_c: u64) -> Heard {
    let mut heard = Heard::default();
    for &(d, t) in time {
        heard.harmony[pc(d)] = t;
    }
    heard.bass[PitchClass::C] = bass_c;
    heard.flat7_on_bass[PitchClass::C] = flat7_on_c;
    heard
}

#[test]
fn the_colours_of_a_major_key() {
    use colour::of_key;
    // The seven notes of C major: no colour.
    let plain = [(0, 30), (2, 10), (4, 20), (5, 10), (7, 20), (9, 5), (11, 5)];
    assert_eq!(of_key(&heard_with(&plain, 0, 0), C_MAJOR), Colours::NONE);
    // A flat seventh at 4% and no major seventh: Mixolydian (the total is 100, so each count is a share).
    let mixolydian = [(0, 30), (2, 11), (4, 20), (5, 10), (7, 20), (9, 5), (10, 4)];
    assert_eq!(of_key(&heard_with(&mixolydian, 0, 0), C_MAJOR), Colours::MIXOLYDIAN);
    // Under 4%: none.
    let few = [(0, 31), (2, 11), (4, 20), (5, 10), (7, 20), (9, 5), (10, 3)];
    assert_eq!(of_key(&heard_with(&few, 0, 0), C_MAJOR), Colours::NONE);
    // A major seventh more than half the flat seventh's: none.
    let both = [(0, 30), (2, 9), (4, 20), (5, 10), (7, 20), (9, 3), (10, 5), (11, 3)];
    assert_eq!(of_key(&heard_with(&both, 0, 0), C_MAJOR), Colours::NONE);
    // C7 over the tonic on a fifth of its bass beats: C7 as home.
    let home = of_key(&heard_with(&mixolydian, 10, 2), C_MAJOR);
    assert_eq!(home, Colours::MIXOLYDIAN.with(Colours::I7));
    assert_eq!(of_key(&heard_with(&mixolydian, 11, 2), C_MAJOR), Colours::MIXOLYDIAN);
    // Both thirds and the flat seventh: the blues, and nothing else.
    let blues = [(0, 30), (3, 4), (4, 16), (5, 10), (7, 20), (9, 5), (10, 15)];
    assert_eq!(of_key(&heard_with(&blues, 10, 10), C_MAJOR), Colours::BLUES);
}

#[test]
fn the_colours_of_a_minor_key() {
    use colour::of_key;
    // C natural minor: none.
    let aeolian = [(0, 30), (2, 10), (3, 20), (5, 10), (7, 20), (8, 5), (10, 5)];
    assert_eq!(of_key(&heard_with(&aeolian, 0, 0), C_MINOR), Colours::NONE);
    // A raised seventh at 3% and at least half the flat seventh: harmonic minor.
    let harmonic = [(0, 30), (2, 10), (3, 20), (5, 10), (7, 22), (8, 5), (11, 3)];
    assert_eq!(of_key(&heard_with(&harmonic, 0, 0), C_MINOR), Colours::HARMONIC_MINOR);
    // Exactly half the flat seventh is enough; less isn't.
    let half = [(0, 30), (2, 10), (3, 20), (5, 10), (7, 16), (8, 5), (10, 6), (11, 3)];
    assert_eq!(of_key(&heard_with(&half, 0, 0), C_MINOR), Colours::HARMONIC_MINOR);
    let under = [(0, 30), (2, 10), (3, 20), (5, 10), (7, 15), (8, 5), (10, 7), (11, 3)];
    assert_eq!(of_key(&heard_with(&under, 0, 0), C_MINOR), Colours::NONE);
    // A major sixth at 4% and twice the minor sixth: Dorian; with the raised seventh too, both.
    let dorian = [(0, 30), (2, 10), (3, 20), (5, 10), (7, 20), (8, 2), (9, 4), (10, 4)];
    assert_eq!(of_key(&heard_with(&dorian, 0, 0), C_MINOR), Colours::DORIAN);
    let melodic = [(0, 30), (2, 10), (3, 20), (5, 10), (7, 22), (9, 4), (11, 4)];
    assert_eq!(of_key(&heard_with(&melodic, 0, 0), C_MINOR), Colours::DORIAN.with(Colours::HARMONIC_MINOR));
    // The two sixths under 2% together: no sixth.
    let no_sixth = [(0, 30), (2, 10), (3, 20), (5, 10), (7, 24), (8, 1), (10, 5)];
    assert_eq!(of_key(&heard_with(&no_sixth, 0, 0), C_MINOR), Colours::NO_SIXTH);
    let some_sixth = [(0, 30), (2, 10), (3, 20), (5, 10), (7, 23), (8, 2), (10, 5)];
    assert_eq!(of_key(&heard_with(&some_sixth, 0, 0), C_MINOR), Colours::NONE);
}

#[test]
fn colours_move_with_the_notes_and_only_a_clear_answer_has_them() {
    let dorian = heard_with(&[(0, 30), (2, 10), (3, 20), (5, 10), (7, 20), (8, 2), (9, 4), (10, 4)], 10, 3);
    for s in 0..12 {
        let mut moved = Heard::default();
        moved.harmony = PerPitch(std::array::from_fn(|i| dorian.harmony[pc(i as i32 - s)]));
        assert_eq!(colour::of_key(&moved, up(C_MINOR, s)), Colours::DORIAN, "moved {s}");
    }
    let none = Heard::default();
    assert_eq!(colour::of_key(&none, C_MAJOR), Colours::NONE);
    assert_eq!(
        colour::colours(&dorian, KeyAnswer::Unclear { why: Why::Spread, leaning: Some(C_MINOR) }),
        Colours::NONE
    );
    assert_eq!(colour::colours(&dorian, KeyAnswer::OrRelative(C_MINOR)), Colours::DORIAN);
    // A home note leaning to neither mode is read as major.
    let mixolydian = heard_with(&[(0, 30), (2, 11), (4, 20), (5, 10), (7, 20), (9, 5), (10, 4)], 0, 0);
    let home = KeyAnswer::HomeNote { tonic: PitchClass::C, leaning: None };
    assert_eq!(colour::colours(&mixolydian, home), Colours::MIXOLYDIAN);
}

#[test]
fn colour_bits() {
    let all =
        [Colours::MIXOLYDIAN, Colours::I7, Colours::BLUES, Colours::DORIAN, Colours::HARMONIC_MINOR, Colours::NO_SIXTH];
    let mut every = Colours::NONE;
    for c in all {
        assert!(!every.has(c));
        every = every.with(c);
        assert!(every.has(c));
    }
    assert_eq!(every.bits(), 63);
    assert_eq!(Colours::from_bits(255), every);
    assert!(!every.has(Colours::NONE) && Colours::NONE.is_empty() && !every.is_empty());
}

// The fact.

/// A reading with these chances and this answer.
fn reading(chances: [f64; 24], answer: KeyAnswer, shown_chance: f64, colours: Colours) -> KeyReading {
    KeyReading {
        chances,
        answer,
        shown_chance,
        colours,
        bars_heard: 4,
        config: Config::Full,
        diatonic_fit: 0.95,
        notes: 20,
    }
}

#[test]
fn a_fact_holds_the_whole_answer() {
    let c = chances_with(&[(C_MINOR, 0.6), (EB_MAJOR, 0.35)]);
    let answers = [
        KeyAnswer::Key(C_MINOR),
        KeyAnswer::OrRelative(C_MINOR),
        KeyAnswer::HomeNote { tonic: PitchClass::C, leaning: Some(Mode::Minor) },
        KeyAnswer::HomeNote { tonic: pc(6), leaning: None },
        KeyAnswer::Unclear { why: Why::Spread, leaning: Some(C_MINOR) },
        KeyAnswer::Unclear { why: Why::Spread, leaning: None },
        KeyAnswer::Unclear { why: Why::NotTonal, leaning: None },
        KeyAnswer::Unclear { why: Why::TooFewNotes, leaning: None },
    ];
    for (i, answer) in answers.into_iter().enumerate() {
        let colours = Colours::from_bits(i as u8 * 9);
        let r = reading(c, answer, 0.95, colours);
        let fact = r.fact();
        assert_eq!(fact.0[0], KeyFact::LAYOUT);
        assert_eq!(fact.answer(), Some(answer));
        assert_eq!(fact.colours(), colours);
        assert_eq!(fact.tonic(), answer.tonic());
        assert_eq!(fact.runner_up(), Some(EB_MAJOR));
        assert!((fact.shown_chance() - 0.95).abs() <= 0.5 / 65_535.0);
        assert!((fact.best_chance() - 0.6).abs() <= 0.5 / 255.0);
    }
    // Search by mode only for one key or a pair.
    assert_eq!(reading(c, KeyAnswer::OrRelative(C_MINOR), 0.95, Colours::NONE).fact().mode(), Some(Mode::Minor));
    let home = KeyAnswer::HomeNote { tonic: PitchClass::C, leaning: Some(Mode::Minor) };
    assert_eq!(reading(c, home, 0.9, Colours::NONE).fact().mode(), None);
    // A tie for the first has no runner-up.
    let tied = reading([1.0 / 24.0; 24], KeyAnswer::Unclear { why: Why::Spread, leaning: None }, 0.0, Colours::NONE);
    assert_eq!(tied.fact().runner_up(), None);
    assert_eq!(tied.fact().0[2..4], [255, 255]);
}

#[test]
fn a_fact_of_another_layout_or_damaged_gives_no_answer() {
    let fact = reading(chances_with(&[(C_MAJOR, 0.9)]), KeyAnswer::Key(C_MAJOR), 0.9, Colours::NONE).fact();
    let mut other = fact;
    other.0[0] = 2;
    assert_eq!((other.answer(), other.tonic(), other.mode()), (None, None, None));
    for kind in [7, 200, 255] {
        let mut damaged = fact;
        damaged.0[1] = kind;
        assert_eq!(damaged.answer(), None);
    }
    // A key that isn't one of the 24.
    for key in [24, 254, 255] {
        let mut damaged = fact;
        damaged.0[2] = key;
        assert_eq!(damaged.answer(), None);
    }
    // Chances out of range are kept in range.
    let wild = reading([f64::NAN; 24], KeyAnswer::Key(C_MAJOR), 7.0, Colours::NONE).fact();
    assert_eq!(wild.shown_chance(), 1.0);
}

// Hints.

#[test]
fn a_hint_against_each_kind_of_answer() {
    use HintCheck::{Agrees, Disagrees, Fifth, Ignored, InAnswer, Parallel, Relative};
    use HintSource::{KeySignatureEvent, Name, Typed};
    let key = KeyAnswer::Key(C_MAJOR);
    let check = |answer: KeyAnswer, hint: Key| answer.check_hint(hint, Typed);
    assert_eq!(check(key, C_MAJOR), Agrees);
    assert_eq!(check(key, A_MINOR), Relative);
    assert_eq!(check(key, C_MINOR), Parallel);
    assert_eq!(check(key, G_MAJOR), Fifth);
    assert_eq!(check(key, Key::major(PitchClass::F)), Fifth);
    assert_eq!(check(key, Key::minor(PitchClass::G)), Disagrees);
    assert_eq!(check(key, Key::major(PitchClass::D)), Disagrees);
    // A C major key signature says nothing (programs save it whatever the music); named, it counts.
    assert_eq!(key.check_hint(C_MAJOR, KeySignatureEvent), Ignored);
    assert_eq!(key.check_hint(C_MAJOR, Name), Agrees);
    assert_eq!(key.check_hint(A_MINOR, KeySignatureEvent), Relative);

    let pair = KeyAnswer::OrRelative(C_MINOR);
    assert_eq!(check(pair, C_MINOR), InAnswer);
    assert_eq!(check(pair, EB_MAJOR), InAnswer);
    assert_eq!(check(pair, C_MAJOR), Parallel);
    assert_eq!(check(pair, Key::minor(PitchClass::G)), Fifth);

    let home = KeyAnswer::HomeNote { tonic: PitchClass::C, leaning: Some(Mode::Minor) };
    assert_eq!((check(home, C_MAJOR), check(home, C_MINOR)), (InAnswer, InAnswer));
    assert_eq!(check(home, EB_MAJOR), Relative);
    let even = KeyAnswer::HomeNote { tonic: PitchClass::C, leaning: None };
    assert_eq!(check(even, A_MINOR), Relative);
    assert_eq!(check(even, EB_MAJOR), Disagrees);

    let leaning = KeyAnswer::Unclear { why: Why::Spread, leaning: Some(G_MAJOR) };
    assert_eq!(check(leaning, G_MAJOR), InAnswer);
    assert_eq!(check(leaning, Key::minor(PitchClass::E)), Relative);
    for nowhere in [Why::Spread, Why::NotTonal, Why::TooFewNotes] {
        assert_eq!(check(KeyAnswer::Unclear { why: nowhere, leaning: None }, C_MAJOR), Ignored);
    }
    // Moved to any key, the check is the same.
    for s in 0..12 {
        for hint in Key::ALL {
            for answer in [key, pair, home, even, leaning] {
                assert_eq!(moved(answer, s).check_hint(up(hint, s), Name), answer.check_hint(hint, Name));
            }
        }
    }
}
