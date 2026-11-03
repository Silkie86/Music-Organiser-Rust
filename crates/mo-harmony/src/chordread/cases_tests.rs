//! Tests of the hand cases as a book ([`super::cases`]): the built-in reader keeps every gate, other weights are told
//! which they break and why, the cases the fit learns from are labelled as their gates read, and a slip in a case's
//! notes is said, not a panic.

use super::cases::{Book, Step, Writer, fit_cases, hand_cases, held_out, named_cases, named_targets, read_whole_cases};
use super::{ChordFitter, HAND, Model, SHIPPED, Weights};

#[test]
fn the_built_in_reader_keeps_every_gate() {
    let book = Book::read(&SHIPPED).unwrap();
    let broken: Vec<String> =
        book.broken().iter().map(|v| format!("{} ({:?}): {:?}", v.name, v.step, v.wrong)).collect();
    assert!(broken.is_empty(), "{broken:#?}");
    // Every case is judged: one stretch at a time, read whole (the stretch cases again), named, and the targets.
    let count = |step: Step, gate: bool| book.verdicts.iter().filter(|v| v.step == step && v.gate == gate).count();
    let stretches = hand_cases().unwrap().len();
    assert_eq!(count(Step::Stretch, true), stretches);
    assert_eq!(count(Step::Whole, true), read_whole_cases().unwrap().len());
    assert_eq!(read_whole_cases().unwrap().len(), stretches + super::cases::whole_cases().unwrap().len());
    assert_eq!(count(Step::Named, true), named_cases().unwrap().len());
    assert_eq!(count(Step::Named, false), named_targets().unwrap().len());
    // The design's targets met so far (harmony/design/chord-reader-design.md, "As built (reader PR)"): 36, 39, 46.
    assert_eq!(book.targets(), (3, named_targets().unwrap().len()));
    let met: Vec<&str> = book.verdicts.iter().filter(|v| !v.gate && v.holds()).map(|v| v.name.as_str()).collect();
    assert_eq!(met, ["36 T: Dm7", "39 T: bar 1 Cmaj7", "46 T: Fmaj7 pushed from the loop's end | G7"]);
}

#[test]
fn weights_that_read_wrongly_are_told_which_gates_they_break_and_why() {
    let wrong = Weights::from_numbers(&HAND.numbers().map(|x| -x));
    let book = Book::read(&Model { weights: &wrong, ..SHIPPED }).unwrap();
    let broken = book.broken();
    for step in Step::ALL {
        assert!(broken.iter().any(|v| v.step == step), "no {step:?} gate broken");
    }
    for v in &broken {
        assert!(v.gate && !v.wrong.is_empty() && !v.read.is_empty(), "{v:?}");
    }
}

#[test]
fn the_held_out_cases_are_the_designs_g_star_ones() {
    for name in ["6 C C/E | F F#dim7 | C/G", "15a C13(#11)", "32 Dm11", "34 Cmaj9", "w34 Cmaj9", "35 Cm | Cm(maj7)"] {
        assert!(held_out(name), "{name}");
    }
    for name in ["1", "3 D/C", "16 C7(#9)", "w4 Am", "h6 Cmaj9 | Am11", "53 C | G7 in 3/4", "36 a walk"] {
        assert!(!held_out(name), "{name}");
    }
}

#[test]
fn the_fit_learns_the_readings_the_gates_want_and_never_a_held_out_case() {
    let cases = fit_cases().unwrap();
    assert!(cases.len() > 40, "{} cases", cases.len());
    let mut fitter = ChordFitter::new();
    for (k, case) in cases.iter().enumerate() {
        assert_eq!(case.names.first(), Some(&case.name));
        assert!(case.names.iter().all(|n| !held_out(n)), "{:?}", case.names);
        assert!(!case.labels.is_empty(), "{}", case.name);
        // Each music is one piece, however many cases read it.
        assert!(cases.iter().skip(k + 1).all(|other| other.music != case.music), "{}", case.name);
        let end = case.music.parts.iter().flat_map(|(_, n)| n).map(|n| n.start + n.length).max().unwrap();
        for label in &case.labels {
            assert!(!label.accepts.is_empty() && 0 <= label.start && label.start < label.end, "{}", case.name);
            assert!(label.end <= (end + 1919) / 1920 * 1920, "{}: {label:?}", case.name);
        }
        // The built-in reader allows the labelled ways and finds them likely: it keeps every gate.
        let read = fitter
            .read(&case.music.parts(), case.music.grid, case.music.span, &case.labels, SHIPPED.weights, SHIPPED.tables)
            .unwrap();
        let (lab, all) = (read.sums.ln_z_labelled, read.sums.ln_z);
        assert!(lab.is_finite() && lab <= all + 1e-9, "{}: {lab} against {all}", case.name);
    }
    // Case 4 (C C/B | Am): its two bars, C or Cmaj7 then Am, from the loop's own first tick. Its stretch cases read
    // whole ("w4 C C/B" over the first bar, "w4 Am" over the second) are the same music: one piece, the same labels.
    let four = cases.iter().find(|c| c.name.starts_with("4 C C/B")).unwrap();
    let labels: Vec<(i64, i64, Vec<String>)> =
        four.labels.iter().map(|l| (l.start, l.end, l.accepts.iter().map(|a| a.to_string()).collect())).collect();
    assert_eq!(labels, [(0, 1920, vec!["C".to_owned(), "Cmaj7".to_owned()]), (1920, 3840, vec!["Am".to_owned()])]);
    assert_eq!(four.names, ["4 C C/B | Am", "w4 C C/B", "w4 Am"]);
    // A stretch case on music of its own: "w22 Am7" over its bar.
    let am7 = cases.iter().find(|c| c.name == "w22 Am7").unwrap();
    assert_eq!((am7.names.len(), am7.labels.len(), am7.labels[0].start, am7.labels[0].end), (1, 1, 0, 1920));
    // A stretch that must have two readings on top accepts either.
    let accepts = |name: &str| -> Vec<String> {
        let case = cases.iter().find(|c| c.names.iter().any(|n| n == name)).unwrap();
        case.labels.iter().flat_map(|l| l.accepts.iter().map(|a| a.to_string())).collect()
    };
    assert_eq!(accepts("w22 no bass"), ["C6", "Am7"]);
    // A gate that only asks for readings to be kept says nothing about what's read: never learned from.
    let learned = |name: &str| cases.iter().any(|c| c.names.iter().any(|n| n == name));
    for kept in ["12 rootless, no bass", "13 G13 rootless, no bass", "20 G7alt or Db13, no bass", "w13 G13 rootless"] {
        assert!(!learned(kept), "{kept}");
    }
    assert!(learned("w4 Am") && learned("h1 C6 held 8 bars"));
}

#[test]
fn a_slip_in_a_cases_notes_is_said_not_a_panic() {
    let w = Writer::new();
    assert_eq!(w.notes("C4 E4 G4", 0, 480).len(), 3);
    assert!(w.notes("C4 H4", 0, 480).len() == 1 && w.line("C4 E9x", 0, 240).len() == 1);
    assert_eq!(w.bars("C2 (1-x) | A1").len(), 1);
    let said = w.done().unwrap_err();
    assert!(said.contains("not a note: H4") && said.contains("not a note: E9x") && said.contains("not beats: (1-x)"));
    assert_eq!(Writer::new().done(), Ok(()));
}
