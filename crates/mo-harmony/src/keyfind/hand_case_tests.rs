//! Tests over the hand cases ([`cases`]): what they're heard as whatever the weights, the rules every case keeps for any
//! weights (the design's case 32), the gates for the shipped weights, and the checks that the finder's version is
//! bumped when an answer can change.

use super::cases::{Book, HandCase, hand_cases};
use super::version::Fnv;
use super::*;
use crate::notes::{Note, Role};
use crate::pitch::PitchClass;

const C_MAJOR: Key = Key::major(PitchClass::C);
const C_MINOR: Key = Key::minor(PitchClass::C);
const D_MINOR: Key = Key::minor(PitchClass::D);
const A_MINOR: Key = Key::minor(PitchClass::A);

/// The hand cases' readings at each version of the finder, as a hash ([`readings_hash`]). A change that moves any of
/// them needs `KEY_FINDER_VERSION` bumped, and a line added here.
const READINGS: &[(u32, u64)] = &[(1, 0xe4a5_05c2_3ac2_57d6)];

/// A hash of every hand case's reading: its fact (answer, keys, chances shown, colours), each key's chance to nine
/// places (so a last-bit difference in a maths library on another machine doesn't count), and what was heard.
fn readings_hash(book: &Book) -> u64 {
    let mut h = Fnv::new();
    for (case, read) in &book.readings {
        h.bytes(case.name.as_bytes());
        match read {
            None => h.bytes(&[0]),
            Some((_, r)) => {
                h.bytes(&r.fact().0);
                for p in r.chances {
                    h.number((p * 1e9).round() as i128);
                }
                h.number(i128::from(r.bars_heard));
                h.number(i128::from(r.notes));
                h.number((r.diatonic_fit * 1e9).round() as i128);
            }
        }
    }
    h.finish()
}

fn reading<'a>(book: &'a Book, name: &str) -> &'a (Heard, KeyReading) {
    book.readings.iter().find(|(c, _)| c.name == name).and_then(|(_, r)| r.as_ref()).unwrap()
}

#[test]
fn the_hand_cases_are_written_as_the_design_says() {
    let cases = hand_cases().unwrap();
    let mut names: Vec<&str> = cases.iter().map(|c| c.name).collect();
    assert_eq!(names.len(), 41);
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), 41, "every name once");
    let notes = |name: &str| -> Vec<(Role, usize)> {
        let case = cases.iter().find(|c| c.name == name).unwrap();
        case.parts.iter().map(|(role, notes)| (*role, notes.len())).collect()
    };
    // Four triads a bar each over their roots; twelve sevenths; a held chord; a tune alone.
    assert_eq!(notes("1"), [(Role::Harmony, 12), (Role::Bass, 4)]);
    assert_eq!(notes("6"), [(Role::Harmony, 48), (Role::Bass, 12)]);
    assert_eq!(notes("7"), [(Role::Harmony, 48), (Role::Bass, 12), (Role::Melody, 75)]);
    assert_eq!(notes("19"), [(Role::Harmony, 4)]);
    assert_eq!(notes("13"), [(Role::Melody, 17)]);
    assert_eq!(notes("8"), [(Role::Melody, 16)]);
    // Chord notes from middle C up to the B above, roots from C2.
    let first = cases.iter().find(|c| c.name == "1").unwrap();
    let keys: Vec<u8> = first.parts[0].1.iter().take(3).map(|n| n.key).collect();
    assert_eq!(keys, [60, 63, 67]);
    assert_eq!(first.parts[1].1.iter().map(|n| n.key).collect::<Vec<_>>(), [36, 44, 39, 46]);
}

#[test]
fn the_hand_cases_hear_what_the_design_says() {
    // These hold whatever the weights: the notes alone decide them.
    let book = Book::read(&SHIPPED).unwrap();
    let colours = |name: &str, key: Key| colour::of_key(&reading(&book, name).0, key);
    assert_eq!(colours("6", C_MAJOR), Colours::BLUES);
    assert_eq!(colours("7", C_MAJOR), Colours::BLUES);
    assert_eq!(colours("9", C_MAJOR), Colours::MIXOLYDIAN);
    assert_eq!(colours("10", C_MINOR), Colours::DORIAN);
    assert_eq!(colours("11", D_MINOR), Colours::DORIAN);
    assert_eq!(colours("13", C_MINOR), Colours::NO_SIXTH);
    assert_eq!(colours("15", C_MINOR), Colours::HARMONIC_MINOR);
    assert_eq!(colours("19", C_MINOR), Colours::NO_SIXTH);
    assert_eq!(colours("26", A_MINOR), Colours::HARMONIC_MINOR);
    assert_eq!(colours("27", C_MAJOR), Colours::MIXOLYDIAN.with(Colours::I7));
    // Scales with no key in them, and a blues-scale tune that has one (the design gives about 0.67, 0.63, 0.58).
    for (name, fit) in [("23a", 22.0 / 32.0), ("23b", 20.0 / 32.0), ("23c", 19.0 / 32.0)] {
        let (_, r) = reading(&book, name);
        assert_eq!(r.diatonic_fit, fit, "{name}");
        assert_eq!(r.answer, KeyAnswer::Unclear { why: Why::NotTonal, leaning: None }, "{name}");
    }
    assert!(reading(&book, "8").1.diatonic_fit >= SHIPPED.not_tonal_below);
    // A chord held for four bars is one bar heard; inverted with nothing under it, it has no bass.
    assert_eq!(reading(&book, "19").1.bars_heard, 1);
    assert!(reading(&book, "19").0.bass[PitchClass::C] > 0);
    assert!(reading(&book, "20").0.bass.0.iter().all(|c| *c == 0));
    // Too few notes, and drums alone.
    for name in ["22a", "22b", "31a"] {
        assert!(matches!(reading(&book, name).1.answer, KeyAnswer::Unclear { why: Why::TooFewNotes, .. }), "{name}");
    }
    assert!(book.readings.iter().any(|(c, r)| c.name == "31b" && r.is_none()));
}

/// The notes of a case, changed note by note.
fn each_note(parts: &[(Role, Vec<Note>)], f: impl Fn(Note) -> Note) -> Vec<(Role, Vec<Note>)> {
    parts.iter().map(|(role, notes)| (*role, notes.iter().map(|n| f(*n)).collect())).collect()
}

fn read(reader: &mut KeyReader, case: &HandCase, parts: &[(Role, Vec<Note>)], model: &Model) -> Option<KeyReading> {
    let parts: Vec<Part> = parts.iter().map(|(role, notes)| Part { role: *role, notes }).collect();
    reader.hear(&parts, case.grid).map(|heard| read_heard(&heard, case.span, model))
}

fn up(key: Key, semitones: i32) -> Key {
    Key { tonic: key.tonic.up(semitones), mode: key.mode }
}

#[test]
fn every_hand_case_moves_and_keeps_exactly() {
    // The design's case 32, with weights that have no pattern: the rules hold for any weights.
    let weights: [f64; FEATURES] = std::array::from_fn(|i| ((i * 7919 % 1013) as f64 / 1013.0 - 0.5) * 4.0);
    let calibration = Calibration([0.6, 0.8, 1.1, 1.3, 1.6, -0.2, -0.3, -0.1, 0.4, 0.2, 0.1, -0.1, -0.2, 0.3]);
    let model = Model { weights: &weights, calibration: &calibration, ..SHIPPED };
    let mut reader = KeyReader::new();
    let bits = |r: &KeyReading| r.chances.map(f64::to_bits);
    for case in hand_cases().unwrap() {
        let name = case.name;
        let Some(base) = read(&mut reader, &case, &case.parts, &model) else {
            assert_eq!(name, "31b");
            continue;
        };
        // Moved from 6 semitones down to 5 up: the chances turn with the keys, and the answer, runner-up, colours and
        // chance shown move with them.
        for s in -6..=5 {
            let moved = each_note(&case.parts, |n| Note { key: (i32::from(n.key) + s) as u8, ..n });
            let r = read(&mut reader, &case, &moved, &model).unwrap();
            for k in Key::ALL {
                assert_eq!(r.chance(up(k, s)).to_bits(), base.chance(k).to_bits(), "{name} moved {s}");
            }
            let answer = match base.answer {
                KeyAnswer::Key(k) => KeyAnswer::Key(up(k, s)),
                KeyAnswer::OrRelative(k) => KeyAnswer::OrRelative(up(k, s)),
                KeyAnswer::HomeNote { tonic, leaning } => KeyAnswer::HomeNote { tonic: tonic.up(s), leaning },
                KeyAnswer::Unclear { why, leaning } => KeyAnswer::Unclear { why, leaning: leaning.map(|k| up(k, s)) },
            };
            assert_eq!(r.answer, answer, "{name} moved {s}");
            assert_eq!(r.runner_up().map(|(k, _)| k), base.runner_up().map(|(k, _)| up(k, s)), "{name} moved {s}");
            assert_eq!((r.colours, r.shown_chance.to_bits()), (base.colours, base.shown_chance.to_bits()));
        }
        // Each part backwards; each part split in two of its kind; a drum part added; every velocity halved; a ghost
        // note added to each part; every note doubled an octave up: the same chances, to the last bit.
        let backwards: Vec<_> =
            case.parts.iter().map(|(role, notes)| (*role, notes.iter().rev().copied().collect())).collect();
        let split: Vec<_> = case
            .parts
            .iter()
            .flat_map(|(role, notes)| {
                let odd = notes.iter().skip(1).step_by(2).copied().collect();
                [(*role, notes.iter().step_by(2).copied().collect()), (*role, odd)]
            })
            .collect();
        let mut drums = case.parts.clone();
        drums.insert(
            0,
            (
                Role::Drums,
                (0..16)
                    .map(|i| Note { start: i * 240, length: 60, key: 36 + (i % 3) as u8, velocity: 1 << 15 })
                    .collect(),
            ),
        );
        let quiet = each_note(&case.parts, |n| Note { velocity: n.velocity / 2, ..n });
        let ghosts: Vec<_> = case
            .parts
            .iter()
            .map(|(role, notes)| {
                let mut notes = notes.clone();
                if let Some(first) = notes.first().copied() {
                    notes.push(Note { length: 15, key: first.key + 1, velocity: first.velocity / 16, ..first });
                }
                (*role, notes)
            })
            .collect();
        let doubled: Vec<_> = case
            .parts
            .iter()
            .map(|(role, notes)| (*role, notes.iter().flat_map(|n| [*n, Note { key: n.key + 12, ..*n }]).collect()))
            .collect();
        for (what, parts) in [
            ("backwards", backwards),
            ("split", split),
            ("with drums", drums),
            ("quieter", quiet),
            ("with ghost notes", ghosts),
            ("doubled", doubled),
        ] {
            let r = read(&mut reader, &case, &parts, &model).unwrap();
            assert_eq!(bits(&r), bits(&base), "{name} {what}");
        }
    }
}

#[test]
fn the_shipped_weights_keep_every_gate() {
    let book = Book::read(&SHIPPED).unwrap();
    let verdicts = book.verdicts();
    assert_eq!(verdicts.iter().map(|v| v.case).collect::<Vec<_>>(), (1..=31).collect::<Vec<u8>>());
    assert!(verdicts.iter().all(|v| !v.about.is_empty()));
    if FITTED {
        let broken: Vec<&String> = verdicts.iter().flat_map(|v| v.broken()).collect();
        assert!(broken.is_empty(), "the shipped weights break a hand case's gate: {broken:#?}");
    } else {
        // The placeholder gives every key the same chance, so nothing has a key.
        for (case, read) in &book.readings {
            if let Some((_, r)) = read {
                assert!(matches!(r.answer, KeyAnswer::Unclear { .. }), "{}", case.name);
            }
        }
    }
}

#[test]
fn the_gates_can_fail() {
    // Weights that make every minor key likelier than any major one: case 2 (Eb major) must fail its gate, and say
    // why.
    let mut weights = [0.0; FEATURES];
    weights[MINOR] = 50.0;
    let model = Model { weights: &weights, ..SHIPPED };
    let verdicts = Book::read(&model).unwrap().verdicts();
    let two = verdicts.iter().find(|v| v.case == 2).unwrap();
    assert!(!two.passes_gate());
    assert!(two.broken()[0].starts_with("2: Eb major on top"), "{:?}", two.gate);
    // Cases with no target or no gate say so; others say whether they keep them.
    let case = |n: u8| verdicts.iter().find(|v| v.case == n).unwrap();
    assert_eq!(case(2).meets_target(), None);
    assert!(case(1).meets_target().is_some());
    assert!(case(9).gate.is_none() && case(17).gate.is_none() && case(9).passes_gate());
    assert!(verdicts.iter().filter(|v| v.case != 9 && v.case != 17).all(|v| v.gate.is_some()));
}

#[test]
fn the_version_is_bumped_when_a_number_changes() {
    let (hash, version) = SHIPPED_HASH;
    let now = numbers_hash(&SHIPPED);
    assert_eq!(
        now,
        hash,
        "a number the finder is built from has changed: write SHIPPED_HASH = {now:#018x} and HASH_VERSION = {} in \
         keyfind/weights.rs, and bump KEY_FINDER_VERSION in keyfind/mod.rs to match",
        KEY_FINDER_VERSION + 1
    );
    assert_eq!(version, KEY_FINDER_VERSION, "weights.rs was written for version {version}: bump KEY_FINDER_VERSION");
    // Every number counts: the smallest change to any one of them changes the hash.
    let nudge = |x: f64| f64::from_bits(x.to_bits() + 1);
    let mut weights = *SHIPPED.weights;
    weights[FEATURES - 1] = nudge(weights[FEATURES - 1]);
    let mut calibration = *SHIPPED.calibration;
    calibration.0[3] = nudge(calibration.0[3]);
    let mut thresholds = *SHIPPED.thresholds;
    thresholds.home[5] = nudge(thresholds.home[5]);
    for changed in [
        Model { weights: &weights, ..SHIPPED },
        Model { calibration: &calibration, ..SHIPPED },
        Model { thresholds: &thresholds, ..SHIPPED },
        Model { not_tonal_below: nudge(SHIPPED.not_tonal_below), ..SHIPPED },
    ] {
        assert_ne!(numbers_hash(&changed), now);
    }
}

#[test]
fn a_change_that_moves_an_answer_bumps_the_version() {
    let now = readings_hash(&Book::read(&SHIPPED).unwrap());
    let kept = READINGS.iter().find(|(v, _)| *v == KEY_FINDER_VERSION).map(|(_, h)| *h);
    assert_eq!(
        kept,
        Some(now),
        "the hand cases' readings differ from those kept for version {KEY_FINDER_VERSION}: if the change is meant, \
         bump KEY_FINDER_VERSION and add ({}, {now:#018x}) to READINGS in keyfind/hand_case_tests.rs",
        KEY_FINDER_VERSION + 1
    );
    assert!(READINGS.windows(2).all(|w| w[0].0 < w[1].0), "versions only go up");
}
