//! Tests of what the fit adds up ([`super::ChordFitter`], the decode's fit sums): ln Z and the evidence expected,
//! over every way and over the ways the labels allow, against the slow way (every way through what the decode kept,
//! added up one by one, its evidence worked out piece by piece); the labels' readings kept on their spans; the sums
//! over every way the decode's own; and which ways the labels allow.

use super::decode::{FitWork, Gold, IdSet, Kind, Lattice};
use super::decode_tests::{count_ways, every_way, small_music, step, the_music};
use super::hear_tests::{BAR, BEAT, Dice, Music, notes, random_music};
use super::{
    COUNTED, ChordFitter, Core, Decoder, Decoding, Evidence, Feature, FitLabel, FitSums, HAND, Hearing,
    LABEL_TOLERANCE, Label, NUMBERS, PerFeature, Priors, SHARPNESS, SpanFacts, Weights,
};
use crate::notes::Part;
use crate::pitch::PitchClass;

/// The motion prior between two readings, as the pair term's trust weighs it: none into or out of no chord, nor for the
/// same chord.
fn motion(a: Option<Label>, b: Option<Label>) -> f64 {
    match (a, b) {
        (Some(a), Some(b)) => COUNTED.pair_prior(a, b),
        _ => 0.0,
    }
}

/// ln of the sum of e to each of some numbers; minus infinity for none.
fn ln_sum(values: impl Iterator<Item = f64> + Clone) -> f64 {
    let top = values.clone().fold(f64::NEG_INFINITY, f64::max);
    if top == f64::NEG_INFINITY {
        return top;
    }
    top + values.map(|v| (v - top).exp()).sum::<f64>().ln()
}

/// The fit's sums the slow way: every way through what the last decode kept (`lattice`), its evidence worked out step
/// by step from the facts of each span, checked to weigh up to the way's score, and whether the labels allow it.
fn slow_sums(lattice: &Lattice, hearing: &Hearing, weights: &Weights, gold: &Gold, what: &str) -> FitSums {
    let ways = every_way(lattice, weights);
    assert!(!ways.is_empty(), "{what}: no way");
    let priors = Priors::of(&COUNTED);
    let (mut evidence, mut facts, mut features) = (Evidence::new(), SpanFacts::new(), PerFeature::default());
    evidence.prepare(hearing);
    let push = |unit: usize, pushed: bool| hearing.pushes().iter().find(|p| pushed && p.unit == unit);
    let mut each: Vec<(f64, bool, [f64; NUMBERS])> = Vec::new();
    for way in &ways {
        let (mut sum, mut allowed, mut before) = ([0.0; NUMBERS], true, None);
        for &(s, k) in &way.steps {
            let (_, start) = step(lattice, weights, before, s, k).expect("a step the rules allow");
            let (from, to, _, readings) = &lattice.spans[s];
            let label = readings[k].0;
            let ((u0, pushed0, kind), (u1, pushed1, _)) = (lattice.places[*from], lattice.places[*to]);
            evidence.facts(hearing, u0..u1, push(u0, pushed0), push(u1, pushed1), &mut facts);
            let beats = facts.beats();
            match label {
                None => sum[Weights::NONE] += beats,
                Some(label) => {
                    facts.features(label, &priors, &mut features);
                    for (x, f) in sum.iter_mut().zip(Feature::ALL) {
                        *x += if !f.once() {
                            beats * features[f]
                        } else if start {
                            features[f]
                        } else {
                            0.0
                        };
                    }
                }
            }
            if let (true, Some((b, j))) = (start, before) {
                sum[Weights::PAIR] += motion(lattice.spans[b].3[j].0, label);
            }
            // A rest is no chord whatever the labels say; elsewhere the reading must be one they accept, and a chord
            // may start only near a labelled change.
            let label_ok = lattice.rests[s] || gold.accepts(u0..u1).is_none_or(|set| set.has_label(label));
            let start_ok = !start || matches!(kind, Kind::Start | Kind::Edge) || gold.may_start(u0);
            allowed &= label_ok && start_ok;
            before = Some((s, k));
        }
        let weighed: f64 = sum.iter().zip(weights.numbers()).map(|(x, w)| x * w).sum();
        assert!(
            (weighed - way.score).abs() <= 1e-9 * way.score.abs().max(1.0),
            "{what}: a way's evidence weighs {weighed}, its score is {}",
            way.score
        );
        each.push((SHARPNESS * way.score, allowed, sum));
    }
    let ln_z = ln_sum(each.iter().map(|w| w.0));
    let ln_z_labelled = ln_sum(each.iter().filter(|w| w.1).map(|w| w.0));
    let average = |ln: f64, only_allowed: bool| {
        let mut out = [0.0; NUMBERS];
        if ln == f64::NEG_INFINITY {
            return out;
        }
        for (score, allowed, sum) in &each {
            if only_allowed && !allowed {
                continue;
            }
            let p = (score - ln).exp();
            for (o, x) in out.iter_mut().zip(sum) {
                *o += p * x;
            }
        }
        out
    };
    FitSums { ln_z, ln_z_labelled, expected: average(ln_z, false), expected_labelled: average(ln_z_labelled, true) }
}

fn close(a: f64, b: f64, by: f64) -> bool {
    a == b || (a - b).abs() <= by * a.abs().max(b.abs()).max(1.0)
}

/// Checks the fast sums against the slow ones.
fn agrees(fast: &FitSums, slow: &FitSums, what: &str) {
    assert!(close(fast.ln_z, slow.ln_z, 1e-9), "{what}: ln Z {} against {}", fast.ln_z, slow.ln_z);
    assert!(
        close(fast.ln_z_labelled, slow.ln_z_labelled, 1e-9),
        "{what}: ln Z labelled {} against {}",
        fast.ln_z_labelled,
        slow.ln_z_labelled
    );
    for (i, ((a, b), (c, d))) in fast
        .expected
        .iter()
        .zip(&slow.expected)
        .zip(fast.expected_labelled.iter().zip(&slow.expected_labelled))
        .enumerate()
    {
        let name = Weights::number_name(i);
        assert!(close(*a, *b, 1e-8), "{what}: {name} expected {a} against {b}");
        assert!(close(*c, *d, 1e-8), "{what}: {name} expected where labelled {c} against {d}");
    }
}

/// Checks the labels as a decode settled them (`settled`) against the labels it was given: every labelled change is
/// within a unit of a place a chord may change, and those that were moved to the nearest place; what each stretch
/// accepts is the same.
fn settles(given: &Gold, settled: &Gold, lattice: &Lattice, what: &str) {
    let places: Vec<usize> =
        lattice.places.iter().filter(|(_, _, kind)| *kind != Kind::Inside).map(|(unit, _, _)| *unit).collect();
    let mut by_hand = given.clone();
    by_hand.settle(&places);
    assert_eq!(format!("{by_hand:?}"), format!("{settled:?}"), "{what}");
    for (range, _) in settled.stretches() {
        for edge in [range.start, range.end] {
            let near = places.iter().any(|p| p.abs_diff(edge) <= LABEL_TOLERANCE);
            assert!(
                near || edge > *places.last().unwrap_or(&0),
                "{what}: a change at {edge}, no place near it ({places:?})"
            );
        }
    }
}

/// A random label: a root and a core.
fn any_label(dice: &mut Dice) -> Label {
    Label::new(PitchClass::new(dice.below(12) as i32), dice.pick(&Core::ALL))
}

/// Up to four labelled stretches anywhere in music `units` long (over each other or past the end too), each
/// accepting the reading a decode without them read where it starts (most of the time, so some ways are allowed),
/// some random ones, or none.
fn random_gold(dice: &mut Dice, units: usize, bar: usize, read: &Decoding) -> Gold {
    let mut stretches = Vec::new();
    for _ in 0..1 + dice.below(4) {
        let start = dice.below(units as u64 + 1) as usize;
        let end = start + 1 + dice.below(2 * bar as u64) as usize;
        let mut accepts = Vec::new();
        if dice.below(4) > 0 {
            let first = read.spans().iter().find(|s| s.units.contains(&start)).and_then(|s| s.label);
            accepts.extend(first);
        }
        for _ in 0..dice.below(3) {
            accepts.push(any_label(dice));
        }
        stretches.push((start..end, IdSet::of(&accepts)));
    }
    Gold::new(units, stretches)
}

/// Decodes some music with labels, and works out the fit's sums over it.
fn fit_sums(decoder: &mut Decoder, hearing: &Hearing, gold: &Gold, work: &mut FitWork) -> FitSums {
    decoder.set_gold(gold.clone());
    let mut out = Decoding::new();
    assert!(decoder.decode(hearing, &HAND, &COUNTED, &[], &mut out), "nothing read");
    let sums = decoder.fit_sums(hearing, &COUNTED, work);
    decoder.set_gold(Gold::default());
    sums
}

#[test]
fn the_fit_sums_agree_with_adding_up_every_way_one_by_one() {
    let mut dice = Dice(0x2545_f491_4f6c_dd1d);
    let (mut decoder, mut out, mut hearing, mut work) =
        (Decoder::new(), Decoding::new(), Hearing::new(), FitWork::default());
    let (mut tried, mut labelled) = (0, 0);
    for round in 0..300 {
        let music = small_music(&mut dice);
        if !music.hear_into(&mut hearing) {
            continue;
        }
        assert!(decoder.decode(&hearing, &HAND, &COUNTED, &[], &mut out));
        let gold = random_gold(&mut dice, hearing.units(), hearing.bar_units().max(1), &out);
        let what = format!("round {round}: {music:?}, labels {gold:?}");
        let fast = fit_sums(&mut decoder, &hearing, &gold, &mut work);
        decoder.set_gold(gold.clone());
        assert!(decoder.decode(&hearing, &HAND, &COUNTED, &[], &mut out));
        // The labels as the decode settled them on its places: the ways they allow.
        let settled = decoder.gold().clone();
        decoder.set_gold(Gold::default());
        let lattice = decoder.lattice();
        settles(&gold, &settled, &lattice, &what);
        if count_ways(&lattice, &HAND) > 20_000.0 {
            continue;
        }
        agrees(&fast, &slow_sums(&lattice, &hearing, &HAND, &settled, &what), &what);
        tried += 1;
        labelled += usize::from(fast.ln_z_labelled > f64::NEG_INFINITY);
    }
    // Enough music was added up the slow way, much of it with some way the labels allow (random labels often accept
    // no reading over two units of the same span).
    assert!(tried >= 150 && labelled >= tried / 3, "{tried} tried, {labelled} with an allowed way");
}

#[test]
fn the_fit_sums_over_every_way_are_the_decodes_own_and_each_unit_is_read_once() {
    let mut dice = Dice(0x9e37_79b9_7f4a_7c15);
    let (mut decoder, mut out, mut hearing, mut work) =
        (Decoder::new(), Decoding::new(), Hearing::new(), FitWork::default());
    let mut musics: Vec<Music> = the_music().into_iter().map(|(m, _)| m).collect();
    for _ in 0..60 {
        musics.push(random_music(&mut dice));
    }
    for (round, music) in musics.iter().enumerate() {
        if !music.hear_into(&mut hearing) {
            continue;
        }
        let units = hearing.units();
        assert!(decoder.decode(&hearing, &HAND, &COUNTED, &[], &mut out));
        let gold = random_gold(&mut dice, units, hearing.bar_units().max(1), &out);
        let what = format!("music {round}: labels {gold:?}");
        let sums = fit_sums(&mut decoder, &hearing, &gold, &mut work);
        decoder.set_gold(gold.clone());
        assert!(decoder.decode(&hearing, &HAND, &COUNTED, &[], &mut out));
        let gold = decoder.gold().clone();
        decoder.set_gold(Gold::default());
        let lattice = decoder.lattice();
        assert!(
            close(sums.ln_z, lattice.total, 1e-9),
            "{what}: ln Z {} against the decode's {}",
            sums.ln_z,
            lattice.total
        );
        // Over every way, each reading's chance is the decode's own.
        let readings = lattice.spans.iter().flat_map(|s| s.3.iter());
        for ((started, held), reading) in work.chances(false).into_iter().zip(readings) {
            assert!(
                (started + held - reading.3).abs() < 1e-9,
                "{what}: {:?} {} against {}",
                reading.0,
                started + held,
                reading.3
            );
        }
        // Each unit is read by exactly one span of each way: over every way, and over the allowed ones.
        for labelled in [false, true] {
            if labelled && sums.ln_z_labelled == f64::NEG_INFINITY {
                assert!(sums.expected_labelled.iter().all(|x| *x == 0.0), "{what}");
                continue;
            }
            let chances = work.chances(labelled);
            let mut per_unit = vec![0.0; units];
            let mut k = 0;
            for (from, to, _, readings) in &lattice.spans {
                let (u0, u1) = (lattice.places[*from].0, lattice.places[*to].0);
                for _ in readings {
                    let (started, held) = chances[k];
                    for p in &mut per_unit[u0..u1] {
                        *p += started + held;
                    }
                    k += 1;
                }
            }
            for (u, p) in per_unit.iter().enumerate() {
                assert!((p - 1.0).abs() < 1e-9, "{what}: unit {u} read {p} times over (labelled: {labelled})");
            }
        }
        // The readings the labels accept are kept on every span inside a labelled stretch.
        for (s, (from, to, _, readings)) in lattice.spans.iter().enumerate() {
            let (u0, u1) = (lattice.places[*from].0, lattice.places[*to].0);
            let Some(set) = gold.accepts(u0..u1).filter(|_| !lattice.rests[s]) else { continue };
            for id_label in (0..12).flat_map(|r| Core::ALL.map(|c| Label::new(PitchClass::new(r), c))) {
                if set.has_label(Some(id_label)) {
                    assert!(
                        readings.iter().any(|r| r.0 == Some(id_label)),
                        "{what}: {id_label} not kept on {u0}..{u1}"
                    );
                }
            }
        }
    }
}

#[test]
fn labelled_stretches_say_which_readings_may_be_taken_and_where_a_chord_may_start() {
    let c = Label::new(PitchClass::C, Core::Major);
    let cmaj7 = Label::new(PitchClass::C, Core::Major7);
    let f = Label::new(PitchClass::F, Core::Major);
    // C over units 0 to 8 (C or Cmaj7), F over 8 to 16, nothing labelled from 16 to 24, then C to 32.
    let gold = Gold::new(
        32,
        vec![
            (0..8, IdSet::of(&[c, cmaj7])),
            (8..16, IdSet::of(&[f])),
            (24..40, IdSet::of(&[c])),
            (20..20, IdSet::of(&[f])),
        ],
    );
    let accepts =
        |units| gold.accepts(units).map(|set| [Some(c), Some(cmaj7), Some(f), None].map(|l| set.has_label(l)));
    // Inside a stretch, its readings; reaching one unit into the next is allowed, two units isn't.
    assert_eq!(accepts(0..8), Some([true, true, false, false]));
    assert_eq!(accepts(2..9), Some([true, true, false, false]));
    assert_eq!(accepts(2..10), Some([false, false, false, false]));
    assert_eq!(accepts(7..16), Some([false, false, true, false]));
    // Where nothing is labelled, anything; reaching in from there, what the stretch accepts.
    assert_eq!(accepts(16..24), None);
    assert_eq!(accepts(17..25), None);
    assert_eq!(accepts(14..20), Some([false, false, true, false]));
    assert_eq!(accepts(20..28), Some([true, false, false, false]));
    // A chord may start within a unit of a labelled change, and anywhere nothing is labelled or next to it.
    let starts: Vec<usize> = (0..=32).filter(|&u| gold.may_start(u)).collect();
    assert_eq!(starts, [0, 1, 7, 8, 9, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 31, 32]);
    assert_eq!(LABEL_TOLERANCE, 1);
}

#[test]
fn a_labelled_change_no_way_can_make_moves_to_the_nearest_place_a_chord_may_change() {
    let (c, f, g) = (
        Label::new(PitchClass::C, Core::Major),
        Label::new(PitchClass::F, Core::Major),
        Label::new(PitchClass::G, Core::Major),
    );
    let ranges = |gold: &Gold| gold.stretches().iter().map(|(r, _)| r.clone()).collect::<Vec<_>>();
    // A chord may change at units 0, 8, 12, 16 and 24 (the music's end). C's end at 10 is two units from both 8 and
    // 12, and goes to the earlier; F's start goes with it, while its end, a unit from 12, stays; G's end at 21 goes to
    // 24, three units away rather than five.
    let places = [0, 8, 12, 16, 24];
    let mut gold = Gold::new(24, vec![(0..10, IdSet::of(&[c])), (10..13, IdSet::of(&[f])), (13..21, IdSet::of(&[g]))]);
    assert_eq!(gold.settle(&places), 3);
    assert_eq!(ranges(&gold), [0..8, 8..13, 13..24]);
    let accepts =
        |gold: &Gold, units| gold.accepts(units).map(|set| [Some(c), Some(f), Some(g)].map(|l| set.has_label(l)));
    assert_eq!(accepts(&gold, 0..8), Some([true, false, false]));
    assert_eq!(accepts(&gold, 8..12), Some([false, true, false]));
    assert_eq!(accepts(&gold, 16..24), Some([false, false, true]));
    let starts: Vec<usize> = (0..=24).filter(|&u| gold.may_start(u)).collect();
    assert_eq!(starts, [0, 1, 7, 8, 9, 12, 13, 14, 23, 24]);
    // Labels the places already follow stay as they are.
    let before = format!("{gold:?}");
    assert_eq!(gold.settle(&places), 0);
    assert_eq!(format!("{gold:?}"), before);
    // An end past the music's stays; a stretch whose ends both go to one place goes.
    let mut gold = Gold::new(24, vec![(20..30, IdSet::of(&[c])), (3..4, IdSet::of(&[f]))]);
    assert_eq!(gold.settle(&[0, 8, 16, 24]), 3);
    assert_eq!(gold.stretches().len(), 1);
    assert_eq!(gold.stretches()[0].0, 16..30);
}

/// C, F, G7, C, a bar each, with a held bass.
fn four_bars() -> Music {
    Music::new()
        .bass(
            [notes("C2", 0, BAR), notes("F2", BAR, BAR), notes("G2", 2 * BAR, BAR), notes("C2", 3 * BAR, BAR)].concat(),
        )
        .keys(
            [
                notes("C4 E4 G4", 0, BAR),
                notes("C4 F4 A4", BAR, BAR),
                notes("B3 D4 F4 G4", 2 * BAR, BAR),
                notes("C4 E4 G4", 3 * BAR, BAR),
            ]
            .concat(),
        )
}

fn label(root: PitchClass, core: Core) -> Label {
    Label::new(root, core)
}

#[test]
fn the_fitter_reads_labels_in_ticks_and_keeps_their_readings() {
    let music = four_bars();
    let parts: Vec<Part> = music.parts.iter().map(|(role, notes)| Part { role: *role, notes }).collect();
    let mut fitter = ChordFitter::new();
    let read = |fitter: &mut ChordFitter, labels: &[FitLabel]| {
        fitter.read(&parts, music.grid, music.span, labels, &HAND, &COUNTED).expect("something heard")
    };
    let chart = |chords: [Label; 4]| -> Vec<FitLabel> {
        let at = |bar: i64| bar * BAR;
        (0..4).map(|b| FitLabel { start: at(b), end: at(b + 1), accepts: vec![chords[b as usize]] }).collect()
    };
    use PitchClass as P;
    let right = chart([
        label(P::C, Core::Major),
        label(P::F, Core::Major),
        label(P::G, Core::Dominant7),
        label(P::C, Core::Major),
    ]);
    let ok = read(&mut fitter, &right);
    assert_eq!(ok.units, 32);
    // The labels are what the music plays, so nearly every way the decode weighs is one they allow.
    assert!(ok.sums.ln_z_labelled <= ok.sums.ln_z && ok.sums.ln_z - ok.sums.ln_z_labelled < 0.1, "{ok:?}");
    // Labels half a beat early still allow the ways that change chord on the bar lines, a unit away: nothing moves.
    let early: Vec<FitLabel> = right
        .iter()
        .map(|l| FitLabel { start: (l.start - BEAT / 2).max(0), end: l.end - BEAT / 2, ..l.clone() })
        .collect();
    let read_early = read(&mut fitter, &early);
    assert!(read_early.sums.ln_z_labelled > f64::NEG_INFINITY && read_early.moved == 0, "{read_early:?}");
    // A beat late, no chord can change within a unit of them (only on the bar lines, two units before), so each
    // change moves to the nearest bar line: the labels read as if they were right. Seven ends move: each stretch's
    // start, and the ends of the first three (the last one's end is past the music's).
    let late: Vec<FitLabel> =
        right.iter().map(|l| FitLabel { start: l.start + BEAT, end: l.end + BEAT, ..l.clone() }).collect();
    let read_late = read(&mut fitter, &late);
    assert_eq!(read_late.moved, 7, "{read_late:?}");
    assert_eq!(read_late.sums, ok.sums);
    // A wrong label is kept on its bar whatever its rank, so a way through it is still summed: far less likely.
    let wrong = chart([
        label(P::new(1), Core::Major),
        label(P::F, Core::Major),
        label(P::G, Core::Dominant7),
        label(P::C, Core::Major),
    ]);
    let bad = read(&mut fitter, &wrong);
    assert!(bad.forced > 0 && bad.sums.ln_z_labelled > f64::NEG_INFINITY, "{bad:?}");
    assert!(bad.sums.ln_z - bad.sums.ln_z_labelled > 5.0, "{bad:?}");
    // Labels that accept nothing leave the music free: every way is allowed.
    let free = read(&mut fitter, &[FitLabel { start: 0, end: 4 * BAR, accepts: Vec::new() }]);
    assert_eq!(free.sums.ln_z_labelled, free.sums.ln_z);
    assert_eq!(free.sums.expected_labelled, free.sums.expected);
    // A fitter used again reads the same as a new one, and leaves nothing behind for a plain decode.
    assert_eq!(read(&mut ChordFitter::new(), &wrong), bad);
    assert_eq!(read(&mut fitter, &right), ok);
}
