//! Tests of the evidence ([`super::score`]): each piece of evidence on small music, the rules that hold for any music
//! (moved, at another resolution, in another order), how scores add up and rank, the hand-set weights' rules, and the
//! design's hand cases as far as one stretch read on its own decides them.

use std::ops::Range;

use super::hear_tests::{BAR, BEAT, Music, keys_only, line, notes, with_bass};
use super::{
    Candidate, Core, EVEN, Evidence, Feature, HAND, Hearing, Label, Mass, PerFeature, Push, Route, Scored, SpanFacts,
};
use crate::notes::{Note, Role};
use crate::pitch::PitchClass;

/// Notes bar by bar: bars split by "|", each a run of notes held the whole bar, or groups of notes each followed by
/// the beats they sound in brackets (from 1, in 4/4): "C2 (1-2) B1 (3-4) | A1".
fn bars(text: &str) -> Vec<Note> {
    let mut out = Vec::new();
    for (b, bar) in text.split('|').enumerate() {
        let start = b as i64 * BAR;
        let mut names: Vec<&str> = Vec::new();
        for word in bar.split_whitespace() {
            if let Some(beats) = word.strip_prefix('(').and_then(|w| w.strip_suffix(')')) {
                let (from, to) = beats.split_once('-').unwrap_or((beats, beats));
                let (from, to): (i64, i64) = (from.parse().unwrap(), to.parse().unwrap());
                out.extend(notes(&names.join(" "), start + (from - 1) * BEAT, (to - from + 1) * BEAT));
                names.clear();
            } else {
                names.push(word);
            }
        }
        if !names.is_empty() {
            out.extend(notes(&names.join(" "), start, BAR));
        }
    }
    out
}

/// The units of bar `k` (a bar of 4/4 is 8 units).
fn bar(k: usize) -> Range<usize> {
    8 * k..8 * k + 8
}

/// Every label: 12 roots, 16 cores.
fn all_labels() -> impl Iterator<Item = Label> {
    PitchClass::ALL.into_iter().flat_map(|root| Core::ALL.map(|core| Label::new(root, core)))
}

/// A label by its name, as it's written: "C", "F#m7b5", "Bb7sus4".
fn label(name: &str) -> Label {
    all_labels().find(|l| l.to_string() == name).unwrap_or_else(|| panic!("no label {name}"))
}

/// What a span holds: `units` counted from the first unit reported (a loop's copied bar left out).
fn facts_on(hearing: &Hearing, units: Range<usize>, push: Option<&Push>) -> SpanFacts {
    let mut evidence = Evidence::new();
    evidence.prepare(hearing);
    let first = hearing.reported().start;
    let mut facts = SpanFacts::new();
    evidence.facts(hearing, first + units.start..first + units.end, push, &mut facts);
    facts
}

/// The evidence about a label, with the tables before counting.
fn evidence(facts: &SpanFacts, name: &str) -> PerFeature<f64> {
    let mut out = PerFeature::default();
    facts.features(label(name), &EVEN, &mut out);
    out
}

/// The candidates ranked by the hand-set weights.
fn ranking(facts: &SpanFacts) -> Vec<Scored> {
    let mut candidates = Vec::new();
    facts.candidates(&mut candidates);
    let mut out = Vec::new();
    facts.rank(&candidates, &HAND, &EVEN, &mut out);
    out
}

/// Checks some pieces of evidence, each to within a rounding error, and that every other one is 0 (the priors left
/// out).
#[track_caller]
fn expect(features: &PerFeature<f64>, want: &[(Feature, f64)]) {
    for f in Feature::ALL {
        if matches!(f, Feature::CorePrior | Feature::TensionPrior) {
            continue;
        }
        let expected = want.iter().find(|(k, _)| *k == f).map_or(0.0, |&(_, v)| v);
        assert!((features[f] - expected).abs() < 1e-12, "{}: {} where {expected} was expected", f.code(), features[f]);
    }
}

#[test]
fn the_chords_notes_are_shared_out_by_what_each_is_to_the_label() {
    use Feature::*;
    // C E G with D and F over them, a bar each, no bass: a fifth each. Read as C, D is a 9 and F an 11, which a
    // major triad doesn't carry; read as F, C is its fifth, G its 9, and E (a major seventh) and D (a sixth) don't
    // belong, while A, its third, doesn't sound.
    let facts = facts_on(&Music::new().looped().keys(notes("C4 E4 G4 D5 F5", 0, BAR)).hear(), bar(0), None);
    assert_eq!(facts.beats(), 4.0);
    let starts = [(StruckAtStart, 1.0), (OnBar, 1.0), (RootPlayed, 1.0)];
    let c = evidence(&facts, "C");
    let leaning = (1.0 + 0.3 + 0.5 + 0.1) / 5.0;
    let want = [(ChordTones, 0.6), (Tensions, 0.2), (WrongStruck, 0.2), (RootSupport, leaning), (LowestIsRoot, 1.0)];
    expect(&c, &[&want[..], &starts].concat());
    let f = evidence(&facts, "F");
    let leaning = (1.0 + 0.5 + 0.1) / 5.0;
    let want = [(ChordTones, 0.4), (Tensions, 0.2), (WrongStruck, 0.4), (NoThird, 1.0), (RootSupport, leaning)];
    expect(&f, &[&want[..], &starts].concat());
    // The priors before counting: every core as likely, each tension played half the time (C's are D and F#).
    assert_eq!(c[CorePrior], (1.0f64 / 16.0).ln());
    assert_eq!(c[TensionPrior], 2.0 * 0.5f64.ln());
}

#[test]
fn wrong_notes_carried_in_or_held_over_cost_less_than_struck_ones() {
    use Feature::*;
    // B3 struck on beat 3 and held to the end of bar 2, under C E G struck in bar 2: in bar 2, B is carried in. Read
    // as C it doesn't belong; as Cmaj7 it does.
    let b = notes("B3", 2 * BEAT, 6 * BEAT);
    let facts = facts_on(&Music::new().keys([b, notes("C4 E4 G4", BAR, BAR)].concat()).hear(), bar(1), None);
    let c = evidence(&facts, "C");
    assert_eq!((c[WrongHeld], c[WrongStruck]), (0.25, 0.0));
    assert_eq!(evidence(&facts, "Cmaj7")[ChordTones], 1.0);
    // Struck with them, it costs more.
    let facts = facts_on(&Music::new().keys(notes("C4 E4 G4 B3", 0, BAR)).hear(), bar(0), None);
    let c = evidence(&facts, "C");
    assert_eq!((c[WrongStruck], c[WrongHeld]), (0.25, 0.0));
    // A 9 carried in isn't a tension played with the chord: it counts neither way.
    let d = notes("D4", 2 * BEAT, 6 * BEAT);
    let facts = facts_on(&Music::new().keys([d, notes("C4 E4 G4", BAR, BAR)].concat()).hear(), bar(1), None);
    let c = evidence(&facts, "C");
    assert_eq!((c[ChordTones], c[Tensions], c[WrongHeld]), (0.75, 0.0, 0.0));
    // D E G B rung half a beat into a bar of F A C: smear from the chord before. E and D (a major seventh and a
    // sixth) don't belong to F, while G and B are its 9 and #11; held over, E and D cost as a share of the chords'
    // sound in the bar, and the tensions held over count for nothing.
    let keys = [notes("D4 E4 G4 B4", 0, BAR + BEAT / 2), notes("F3 A3 C4", BAR, BAR)].concat();
    let hearing = Music::new().keys(keys).hear();
    assert_eq!(hearing.mass(bar(1), Mass::HeldOver)[PitchClass::E], 240);
    let f = evidence(&facts_on(&hearing, bar(1), None), "F");
    assert_eq!((f[ChordTones], f[Tensions], f[WrongStruck]), (1.0, 0.0, 0.0));
    assert_eq!(f[WrongHeld], 2.0 * 240.0 / (3.0 * 1920.0));
    // With more held over than the bar's own chords sound (five notes rung over, F A struck for a beat), what's held
    // over is shared out of what's held over, so it never counts for more than all of it.
    let keys = [notes("C4 E4 G4 B4 D5", 0, BAR + BEAT / 2), notes("F3 A3", BAR, BEAT)].concat();
    let f = evidence(&facts_on(&Music::new().keys(keys).hear(), bar(1), None), "F");
    assert_eq!(f[WrongHeld], 2.0 / 5.0);
}

#[test]
fn a_note_passing_by_costs_a_little_under_the_labels_it_passes_between() {
    use Feature::*;
    // Chords on beats 1 to 3 with E4 F4 G4 on top, then C E broken on beat 4: F4 passes from E to G on the chords'
    // top line. Read as C, F only passes by (and the A3 struck with it, a sixth, doesn't belong); read as Am, F's G
    // isn't Am's, so F is a b6, a tension; read as F, it's the root.
    let keys = [notes("C4 E4", 0, BEAT), notes("A3 F4", BEAT, BEAT), notes("C4 G4", 2 * BEAT, BEAT)].concat();
    let hearing = Music::new().keys([keys, line("E4 C4", 3 * BEAT, BEAT / 2)].concat()).hear();
    assert_eq!(hearing.shaped().len(), 1);
    let facts = facts_on(&hearing, bar(0), None);
    let chords = hearing.mass(bar(0), Mass::Harmony);
    let total: u64 = chords.0.iter().sum();
    let share = |pc: PitchClass| chords[pc] as f64 / total as f64;
    let c = evidence(&facts, "C");
    assert_eq!((c[Passing], c[WrongStruck], c[Tensions]), (share(PitchClass::F), share(PitchClass::A), 0.0));
    let am = evidence(&facts, "Am");
    assert_eq!((am[Passing], am[Tensions]), (0.0, share(PitchClass::F)));
    assert_eq!(evidence(&facts, "F")[Passing], 0.0);
}

#[test]
fn the_tune_counts_after_its_shapes() {
    use Feature::*;
    // The design's case 48: C2 under E3 G3 B3, with a tune.
    let cmaj7 = |tune: Vec<Note>| {
        let music = Music::new().looped().bass(notes("C2", 0, BAR)).keys(notes("E3 G3 B3", 0, BAR)).tune(tune);
        facts_on(&music.hear(), bar(0), None)
    };
    let tune = |f: PerFeature<f64>| (f[TuneTones], f[TuneTensions], f[TuneWrong]);
    // (a) D5 held: a 9 on top. (b) F5 held: an 11, which a major seventh chord doesn't carry.
    assert_eq!(tune(evidence(&cmaj7(notes("D5", 0, BAR)), "Cmaj7")), (0.0, 1.0, 0.0));
    assert_eq!(tune(evidence(&cmaj7(notes("F5", 0, BAR)), "Cmaj7")), (0.0, 0.0, 1.0));
    // (c) Eighths C5 D5 E5: D5 passes from C to E under Cmaj7, so it counts neither way. Read as G, C and E are
    // nothing to it and don't pass by.
    let c = cmaj7(line("C5 D5 E5", 0, BEAT / 2));
    assert_eq!(tune(evidence(&c, "Cmaj7")), (2.0 / 3.0, 0.0, 0.0));
    assert_eq!(tune(evidence(&c, "G")), (1.0 / 3.0, 0.0, 2.0 / 3.0));
    // E5 F5 G5: F, an 11 a major seventh chord doesn't carry, passes by and costs it nothing here.
    assert_eq!(tune(evidence(&cmaj7(line("E5 F5 G5", 0, BEAT / 2)), "Cmaj7")), (2.0 / 3.0, 0.0, 0.0));
    // In quarters, from the second half of the D5 that passes by: it still passes by.
    let music = Music::new().looped().bass(notes("C2", 0, BAR)).keys(notes("E3 G3 B3", 0, BAR));
    let hearing = music.tune(line("C5 D5 E5", 0, BEAT)).hear();
    assert_eq!(tune(evidence(&facts_on(&hearing, 3..8, None), "Cmaj7")), (2.0 / 3.0, 0.0, 0.0));
    // The design's case 45: A5 struck half a beat before the bar of A minor and held into it. In the bar of C, the A
    // comes early to the next bar, so it isn't a 6th that doesn't belong.
    let music = Music::new()
        .looped()
        .bass([notes("C2", 0, BAR), notes("A1", BAR, BAR)].concat())
        .keys([notes("C4 E4 G4", 0, BAR), notes("A3 C4 E4", BAR, BAR)].concat())
        .tune(notes("A5", BAR - BEAT / 2, BEAT + BEAT / 2));
    assert_eq!(tune(evidence(&facts_on(&music.hear(), bar(0), None), "C")), (0.0, 0.0, 0.0));
}

#[test]
fn the_third_seventh_and_fifths_count_by_how_long_they_sound() {
    use Feature::*;
    let keys = |music: Vec<Note>| facts_on(&Music::new().keys(music).hear(), bar(0), None);
    // C G Bb held, with E sounding an eighth of the bar: E counts four times over (twice as a chord note, twice
    // again for presence), so it's half there.
    let facts = keys([notes("C4 G4 Bb4", 0, BAR), notes("E4", 0, BEAT / 2)].concat());
    let c7 = evidence(&facts, "C7");
    assert_eq!((c7[NoThird], c7[NoSeventh], c7[NoFifth], c7[NoAlteredFifth]), (0.5, 0.0, 0.0, 0.0));
    // A shell, C E Bb: no fifth. C Eb Bb as a half-diminished chord: no flat fifth (and a plain fifth is no part of
    // it). C E G as Cmaj7: no seventh.
    assert_eq!(evidence(&keys(notes("C4 E4 Bb4", 0, BAR)), "C7")[NoFifth], 1.0);
    let half = evidence(&keys(notes("C4 Eb4 Bb4", 0, BAR)), "Cm7b5");
    assert_eq!((half[NoAlteredFifth], half[NoFifth]), (1.0, 0.0));
    assert_eq!(evidence(&keys(notes("C4 E4 G4", 0, BAR)), "Cmaj7")[NoSeventh], 1.0);
    assert_eq!(evidence(&keys(notes("C4 E4", 0, BAR)), "C+")[NoAlteredFifth], 1.0);
    // A 7sus4 may sound its 2 in place of its 4th; a sus4 triad may not.
    let facts = keys(notes("C4 D4 G4 Bb4", 0, BAR));
    assert_eq!((evidence(&facts, "C7sus4")[NoThird], evidence(&facts, "Csus4")[NoThird]), (0.0, 1.0));
}

#[test]
fn where_the_root_is_heard() {
    use Feature::*;
    let roots = |facts: &SpanFacts, name: &str| {
        let f = evidence(facts, name);
        [f[RootPlayed], f[RootInBass], f[RootlessOverBass], f[RootlessNoBass], f[LowestIsRoot]]
    };
    let with_bass = |bass: &str, keys: &str| {
        facts_on(&Music::new().bass(notes(bass, 0, BAR)).keys(notes(keys, 0, BAR)).hear(), bar(0), None)
    };
    let keys = |keys: &str| facts_on(&Music::new().keys(notes(keys, 0, BAR)).hear(), bar(0), None);
    // Played in the chords; only in the bass (Dm7 over D, the keys F A C); nowhere, over a bass (G7 over C, the keys
    // F A B E); nowhere, with no bass.
    assert_eq!(roots(&with_bass("C2", "C4 E4 G4"), "C"), [1.0, 0.0, 0.0, 0.0, 0.0]);
    assert_eq!(roots(&with_bass("D2", "F3 A3 C4"), "Dm7"), [0.0, 1.0, 0.0, 0.0, 0.0]);
    assert_eq!(roots(&with_bass("C2", "F3 A3 B3 E4"), "G7"), [0.0, 0.0, 1.0, 0.0, 0.0]);
    assert_eq!(roots(&keys("B3 E4 F4 A4"), "G7"), [0.0, 0.0, 0.0, 1.0, 0.0]);
    // A tune's held note plays the root too.
    let music = Music::new().bass(notes("C2", 0, BAR)).keys(notes("E4 G4", 0, BAR)).tune(notes("C5", 0, BAR));
    assert_eq!(roots(&facts_on(&music.hear(), bar(0), None), "C")[..2], [1.0, 0.0]);
    // With no bass, the lowest chord note where the span starts leans towards its own root (E G B D: Em7, not G6).
    let facts = keys("E3 G3 B3 D4");
    assert_eq!((roots(&facts, "Em7")[4], roots(&facts, "G6")[4]), (1.0, 0.0));
}

#[test]
fn a_held_bass_leans_towards_the_label_by_what_it_is_to_it() {
    use Feature::*;
    let held = |bass: &str, name: &str| {
        let music = Music::new().bass(notes(bass, 0, BAR)).keys(notes("C4 E4 G4", 0, BAR));
        let f = evidence(&facts_on(&music.hear(), bar(0), None), name);
        [BassRoot, BassThird, BassFifth, BassSeventh, BassTension, BassWrong].map(|k| f[k])
    };
    assert_eq!(held("E2", "Em"), [1.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
    assert_eq!(held("E2", "C"), [0.0, 1.0, 0.0, 0.0, 0.0, 0.0]);
    assert_eq!(held("G1", "C"), [0.0, 0.0, 1.0, 0.0, 0.0, 0.0]);
    assert_eq!(held("Bb1", "C7"), [0.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    assert_eq!(held("D2", "C"), [0.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
    assert_eq!(held("Bb1", "C"), [0.0, 0.0, 0.0, 0.0, 0.0, 1.0]);
}

#[test]
fn a_moving_bass_leans_by_how_strong_each_beat_is() {
    use Feature::*;
    // The design's case 36, a 1-bar loop: D2 E2 F2 G2 under F A C. Each half beat is weighed by its level (4 1 2 1
    // 3 1 2 1), so D2 weighs 1200, E2 720, F2 960 and G2 720 of 3600. E2 passes by under Dm7. Read as F, the D2 on
    // the bar line is a strong note that doesn't belong (240 ticks there, weighed 4).
    let music = Music::new().looped().bass(line("D2 E2 F2 G2", 0, BEAT)).keys(notes("F3 A3 C4", 0, BAR));
    let facts = facts_on(&music.hear(), bar(0), None);
    let walk = |name: &str| {
        let f = evidence(&facts, name);
        [f[WalkingRoot], f[WalkingTones], f[WalkingWrong], f[BassRoot]]
    };
    assert_eq!(walk("Dm7"), [1200.0 / 3600.0, 960.0 / 3600.0, 0.0, 0.0]);
    assert_eq!(walk("F"), [960.0 / 3600.0, 0.0, 960.0 / 3600.0, 0.0]);
    // D2 for a beat and a half, then G2 held to the bar's end: a note played longer than a felt beat counts on every
    // unit it sounds, weak ones too (1 3 1 2 1 of 240 ticks: 1920 of 3600).
    let left = [notes("D2", 0, 720), notes("G2", 720, 1200)].concat();
    let music = Music::new().looped().bass(left.clone()).keys(notes("F3 A3 C4", 0, BAR));
    let f = evidence(&facts_on(&music.hear(), bar(0), None), "Dm7");
    assert_eq!([f[WalkingRoot], f[WalkingWrong]], [1680.0 / 3600.0, 1920.0 / 3600.0]);
    // The same played by the left hand in the keys part: its notes are the bass by the gap below the chords.
    let music = Music::new().looped().keys([left, notes("F3 A3 C4", 0, BAR)].concat());
    let f = evidence(&facts_on(&music.hear(), bar(0), None), "Dm7");
    assert_eq!([f[WalkingRoot], f[WalkingWrong]], [1680.0 / 3600.0, 1920.0 / 3600.0]);
}

#[test]
fn where_a_span_starts() {
    use Feature::*;
    let once = |f: &PerFeature<f64>| {
        [BassStrikesRoot, StruckAtStart, NothingStruck, OnBar, MidBar, OnBeat, OffBeat, Pushed].map(|k| f[k])
    };
    // D2 struck under G C F (the design's case 33): its first bass note is Dm7's root, not G7sus4's.
    let music = Music::new().looped().bass(notes("D2", 0, BAR)).keys(notes("G3 C4 F4", 0, BAR));
    let facts = facts_on(&music.hear(), bar(0), None);
    assert_eq!(once(&evidence(&facts, "Dm7")), [1.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0]);
    assert_eq!(once(&evidence(&facts, "G7sus4"))[..2], [0.0, 1.0]);
    // A C2 held from bar 1 under the keys' bar 3: no bass struck there.
    let music = Music::new().bass(notes("C2", 0, 4 * BAR)).keys(bars("C4 E4 G4 | C4 F4 A4 | B3 D4 G4"));
    assert_eq!(once(&evidence(&facts_on(&music.hear(), bar(2), None), "Csus2"))[0], 0.0);
    // C E G struck on the bar and B on beat 3: of Cmaj7's notes heard, three of four are struck where it starts.
    let music = Music::new().keys([notes("C4 E4 G4", 0, BAR), notes("B4", 2 * BEAT, 2 * BEAT)].concat());
    let hearing = music.hear();
    assert_eq!(evidence(&facts_on(&hearing, bar(0), None), "Cmaj7")[StruckAtStart], 0.75);
    // Spans starting inside it: on the middle of the bar (where B is struck), on beat 2 and between beats, with
    // nothing struck.
    let cmaj7 = |units: Range<usize>| once(&evidence(&facts_on(&hearing, units, None), "Cmaj7"));
    assert_eq!(cmaj7(4..8), [0.0, 0.25, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0]);
    assert_eq!(cmaj7(2..8), [0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    assert_eq!(cmaj7(1..8), [0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
}

#[test]
fn a_pushed_chord_counts_as_struck_where_the_span_starts() {
    use Feature::*;
    // The design's case 44: C E G over C2, then F A C struck half a beat early with the bass's F2, held into bar 2.
    let music = Music::new()
        .looped()
        .bass([notes("C2", 0, BAR - BEAT / 2), notes("F2", BAR - BEAT / 2, BAR + BEAT / 2)].concat())
        .keys([notes("C4 E4 G4", 0, BAR - BEAT / 2), notes("F3 A3 C4", BAR - BEAT / 2, BAR + BEAT / 2)].concat());
    let hearing = music.hear();
    let push = hearing.pushes().first().cloned().expect("the chord may be pushed");
    let first = hearing.reported().start;
    let units = push.unit - first..push.unit - first + 8;
    let starts = |push: Option<&Push>| {
        let f = evidence(&facts_on(&hearing, units.clone(), push), "F");
        [f[BassStrikesRoot], f[StruckAtStart], f[NothingStruck], f[Pushed]]
    };
    // Counted from the bar, with nothing struck there, or from the push: F A C and the bass's F struck there.
    assert_eq!(starts(None), [0.0, 0.0, 1.0, 0.0]);
    assert_eq!(starts(Some(&push)), [1.0, 1.0, 0.0, 1.0]);
    // A push to another unit is left out.
    let mut elsewhere = push.clone();
    elsewhere.unit += 1;
    assert_eq!(starts(Some(&elsewhere)), starts(None));
    // With a G in the pushed chord: counted from the push, it's a 9 struck with F; from the bar, it was carried in.
    let music = Music::new()
        .looped()
        .bass([notes("C2", 0, BAR - BEAT / 2), notes("F2", BAR - BEAT / 2, BAR + BEAT / 2)].concat())
        .keys([notes("C4 E4 G4", 0, BAR - BEAT / 2), notes("F3 A3 C4 G4", BAR - BEAT / 2, BAR + BEAT / 2)].concat());
    let hearing = music.hear();
    let push = hearing.pushes().first().cloned().expect("the chord may be pushed");
    let ninth = |push: Option<&Push>| evidence(&facts_on(&hearing, units.clone(), push), "F")[Tensions];
    assert_eq!((ninth(None), ninth(Some(&push))), (0.0, 0.25));
    // The keys alone, the left hand's F2 under the pushed chord: it's the bass, struck with it.
    let music = Music::new()
        .looped()
        .keys([notes("C2 C4 E4 G4", 0, BAR - BEAT / 2), notes("F2 F3 A3 C4", BAR - BEAT / 2, BAR + BEAT / 2)].concat());
    let hearing = music.hear();
    let push = hearing.pushes().first().cloned().expect("the chord may be pushed");
    let root = |push: Option<&Push>| evidence(&facts_on(&hearing, units.clone(), push), "F")[BassStrikesRoot];
    assert_eq!((root(None), root(Some(&push))), (0.0, 1.0));
}

#[test]
fn the_priors_read_the_counted_tables() {
    use Feature::*;
    // Half of all chords major; a major triad's 9 played 90% of the time and its #11 20%. C E G with D: the 9 is
    // heard and the #11 isn't.
    let mut tables = EVEN;
    tables.core[Core::Major.index()] = 0.5;
    tables.tension[Core::Major.index()][2] = 0.9;
    tables.tension[Core::Major.index()][6] = 0.2;
    let facts = facts_on(&Music::new().keys(notes("C4 E4 G4 D5", 0, BAR)).hear(), bar(0), None);
    let mut f = PerFeature::default();
    facts.features(label("C"), &tables, &mut f);
    assert_eq!(f[CorePrior], 0.5f64.ln());
    assert_eq!(f[TensionPrior], 0.9f64.ln() + 0.8f64.ln());
    // A D carried in from before, struck for a 32nd of the bar (under an eighth of it), or passing by in a broken
    // chord (the design's case 41, C4 D4 E4 G4 E4 D4 C4 G3 in eighths) isn't heard as the 9.
    let carried = [notes("D5", 2 * BEAT, 6 * BEAT), notes("C4 E4 G4", BAR, BAR)].concat();
    let short = [notes("C4 E4 G4", 0, BAR), notes("D5", 0, BEAT / 8)].concat();
    let broken = line("C4 D4 E4 G4 E4 D4 C4 G3", 0, BEAT / 2);
    for (keys, units) in [(carried, bar(1)), (short, bar(0)), (broken, bar(0))] {
        let facts = facts_on(&Music::new().keys(keys).hear(), units, None);
        facts.features(label("C"), &tables, &mut f);
        assert_eq!(f[TensionPrior], (1.0f64 - 0.9).ln() + (1.0f64 - 0.2).ln());
    }
}

#[test]
fn a_score_adds_up_the_evidence_over_the_beats_and_once() {
    // C E G held for two bars, read as one span and as each bar.
    let hearing = Music::new().bass(notes("C2", 0, 2 * BAR)).keys(notes("C4 E4 G4", 0, 2 * BAR)).hear();
    let score = |units: Range<usize>| {
        let facts = facts_on(&hearing, units, None);
        let f = evidence(&facts, "C");
        let (per_beat, once): (Vec<Feature>, Vec<Feature>) = Feature::ALL.iter().partition(|f| !f.once());
        let sum = |set: &[Feature]| set.iter().map(|&k| HAND.features[k] * f[k]).sum::<f64>();
        let by_hand = facts.beats() * sum(&per_beat) + sum(&once);
        assert!((facts.score(&f, &HAND) - by_hand).abs() < 1e-9);
        (facts.score(&f, &HAND), sum(&once), facts.none_score(&HAND))
    };
    let (whole, _, none) = score(0..16);
    let (first, _, _) = score(bar(0));
    let (second, second_once, _) = score(bar(1));
    // What counts for each beat adds up, so cutting a held chord in two gains nothing: the second bar only adds
    // where it starts (nothing struck there).
    assert!((whole - (first + second - second_once)).abs() < 1e-9);
    assert!(second_once < 0.0);
    assert_eq!(none, 8.0 * HAND.none);
}

#[test]
fn the_ranking_is_best_first_and_equal_scores_keep_the_order_from_the_lowest_note_up() {
    // C Eb Gb A with no bass: the four diminished seventh readings explain the same notes. C is the lowest, so Cdim7
    // leads; the other three score the same and keep the candidates' order, from C up.
    for (shift, want) in [(0, ["Cdim7", "Ebdim7", "F#dim7", "Adim7"]), (1, ["Dbdim7", "Edim7", "Gdim7", "Bbdim7"])] {
        let keys = notes("C4 Eb4 Gb4 A4", 0, BAR).into_iter().map(|n| Note { key: n.key + shift, ..n }).collect();
        let ranked = ranking(&facts_on(&Music::new().looped().keys(keys).hear(), bar(0), None));
        assert!(ranked.windows(2).all(|w| w[0].score >= w[1].score));
        let dim7: Vec<&Scored> = ranked.iter().filter(|s| s.label.core == Core::Diminished7).collect();
        let names: Vec<String> = dim7.iter().map(|s| s.label.to_string()).collect();
        assert_eq!(names, want);
        assert_eq!(ranked[0].label.to_string(), want[0]);
        assert!(dim7[0].score > dim7[1].score && dim7[1].score == dim7[2].score && dim7[2].score == dim7[3].score);
    }
}

#[test]
fn equal_scores_keep_the_candidates_order_however_long_the_list() {
    // All twelve notes held: every label on every root is let in, and each core's readings on the eleven roots above
    // the lowest note score the same.
    let keys = notes("C4 C#4 D4 D#4 E4 F4 F#4 G4 G#4 A4 A#4 B4", 0, BAR);
    let facts = facts_on(&Music::new().looped().keys(keys).hear(), bar(0), None);
    let mut candidates = Vec::new();
    facts.candidates(&mut candidates);
    let ranked = ranking(&facts);
    assert_eq!((candidates.len(), ranked.len()), (192, 192));
    let place = |label: Label| candidates.iter().position(|c| c.label == label);
    let mut ties = 0;
    for w in ranked.windows(2) {
        assert!(w[0].score >= w[1].score);
        if w[0].score == w[1].score {
            ties += 1;
            assert!(place(w[0].label) < place(w[1].label), "{} came before {}", w[0].label, w[1].label);
        }
    }
    assert!(ties >= 16 * 10, "{ties} ties");
}

/// The sample musics, with pushes and loops: what the rules that hold for any music are checked on.
fn samples() -> Vec<Music> {
    let pushed = Music::new()
        .looped()
        .bass([notes("C2", 0, BAR), notes("A1", BAR, BAR)].concat())
        .keys([notes("C4 E4 G4 B4", 0, BAR - BEAT / 2), notes("A3 C4 E4 G4", BAR - BEAT / 2, BAR + BEAT / 2)].concat())
        .tune([notes("E5", 0, BEAT), line("D5 C5 B4 C5", BEAT, BEAT / 2), notes("A5", BAR, 2 * BEAT)].concat());
    let walking = Music::new()
        .looped()
        .bass([line("D2 E2 F2 G2", 0, BEAT), notes("G2", BAR, BAR)].concat())
        .keys([notes("F3 A3 C4", 0, BAR), notes("F3 A3 B3 E4", BAR, BAR)].concat());
    vec![with_bass(), keys_only(), with_bass().looped(), pushed, walking]
}

/// Spans of up to two bars from every unit, in the hearing's own units, and a bar from each push with the push.
fn spans(hearing: &Hearing) -> Vec<(Range<usize>, Option<Push>)> {
    let units = hearing.units();
    let mut out = Vec::new();
    for start in 0..units {
        for length in [1, 2, 3, 4, 6, 8, 12, 16] {
            if start + length <= units {
                out.push((start..start + length, None));
            }
        }
    }
    for push in hearing.pushes() {
        out.push((push.unit..(push.unit + 8).min(units), Some(push.clone())));
    }
    out
}

/// Every candidate's evidence on every span, as bits, with its label moved back by `shift` semitones, and its score:
/// in the order ranked, which the same music must give again.
fn everything(hearing: &Hearing, shift: i32) -> Vec<(Label, Route, [u64; Feature::COUNT], u64)> {
    let mut evidence = Evidence::new();
    evidence.prepare(hearing);
    let (mut facts, mut candidates, mut ranked) = (SpanFacts::new(), Vec::<Candidate>::new(), Vec::new());
    let mut f = PerFeature::default();
    let mut out = Vec::new();
    for (units, push) in spans(hearing) {
        evidence.facts(hearing, units, push.as_ref(), &mut facts);
        facts.candidates(&mut candidates);
        facts.rank(&candidates, &HAND, &EVEN, &mut ranked);
        for s in &ranked {
            facts.features(s.label, &EVEN, &mut f);
            let back = Label::new(s.label.root.up(-shift), s.label.core);
            out.push((back, s.route, f.0.map(f64::to_bits), s.score.to_bits()));
        }
    }
    out
}

#[test]
fn the_same_notes_moved_give_the_same_evidence_to_the_last_bit() {
    for music in samples() {
        let want = everything(&music.hear(), 0);
        assert!(want.len() > 1000);
        for shift in [-6, -5, -1, 1, 2, 5] {
            let moved = music.map(|n| Note { key: (i32::from(n.key) + shift) as u8, ..n });
            assert!(everything(&moved.hear(), shift) == want, "moved {shift}");
        }
    }
}

#[test]
fn the_evidence_is_the_same_at_another_resolution_and_in_another_order() {
    for music in samples() {
        let want = everything(&music.hear(), 0);
        for ppq in [96, 960] {
            let scale = |t: i64| t * ppq / 480;
            let mut other = music.map(|n| Note { start: scale(n.start), length: scale(n.length), ..n });
            other.grid.ticks_per_beat = ppq as u32;
            assert!(everything(&other.hear(), 0) == want, "at {ppq} ticks a beat");
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
            assert!(everything(&other.hear(), 0) == want, "{what}");
        }
    }
}

#[test]
fn every_piece_of_evidence_stays_in_its_range_on_any_span() {
    use Feature::*;
    let shares = [ChordTones, Tensions, WrongStruck, Passing, TuneTones, TuneTensions, TuneWrong, RootSupport];
    let flags = [RootPlayed, RootInBass, RootlessOverBass, RootlessNoBass, LowestIsRoot, BassRoot, BassWrong];
    let mut evidence = Evidence::new();
    let mut facts = SpanFacts::new();
    let mut f = PerFeature::default();
    for music in samples() {
        let hearing = music.hear();
        evidence.prepare(&hearing);
        let mut spans = spans(&hearing);
        // Spans past the end, and empty ones, hold nothing.
        let units = hearing.units();
        spans.extend([(units..units + 8, None), (units + 3..units + 1, None), (5..5, None)]);
        for (units, push) in spans {
            evidence.facts(&hearing, units, push.as_ref(), &mut facts);
            assert!(facts.beats() >= 0.0 && facts.units().end <= hearing.units());
            for l in all_labels() {
                facts.features(l, &EVEN, &mut f);
                assert!(f.0.iter().all(|v| v.is_finite()));
                assert!(shares.iter().all(|&k| (0.0..=1.0).contains(&f[k])), "{l} {:?}", f.0);
                assert!(flags.iter().all(|&k| f[k] == 0.0 || f[k] == 1.0));
                let chords = f[ChordTones] + f[Tensions] + f[WrongStruck] + f[Passing];
                assert!(chords <= 1.0 + 1e-12 && f[TuneTones] + f[TuneTensions] + f[TuneWrong] <= 1.0 + 1e-12);
                assert!(f[WalkingRoot] + f[WalkingTones] <= 1.0 + 1e-12 && (0.0..=2.0).contains(&f[WrongHeld]));
                assert!(f[CorePrior] <= 0.0 && f[TensionPrior] <= 0.0);
            }
        }
    }
    // Nothing heard at all.
    let nothing = Hearing::new();
    evidence.prepare(&nothing);
    evidence.facts(&nothing, 0..8, None, &mut facts);
    assert_eq!((facts.beats(), facts.units()), (0.0, 0..0));
    let mut candidates = Vec::new();
    facts.candidates(&mut candidates);
    assert!(candidates.is_empty());
}

#[test]
fn the_hand_weights_keep_every_rule_and_broken_ones_are_named() {
    use Feature::*;
    assert_eq!(HAND.check(), Vec::<&str>::new());
    // One weight moved out of its place at a time, and the rules it breaks.
    let cases: [(Feature, f64, &[&str]); 25] = [
        (ChordTones, -1.0, &["T1 counts for the label", "T2 is between 0 and T1"]),
        (Tensions, 4.0, &["T2 is between 0 and T1"]),
        (WrongStruck, 1.0, &["T3 counts against the label", "T4 is between T3 and 0", "T5 is between T3 and 0"]),
        (WrongHeld, 0.5, &["T4 is between T3 and 0"]),
        (Passing, -4.0, &["T5 is between T3 and 0"]),
        (TuneTensions, -0.1, &["T6 and T7 count for the label"]),
        (TuneWrong, 0.1, &["T8 counts against the label"]),
        (NoSeventh, 0.1, &["T9 to T11 count against"]),
        (NoFifth, -3.0, &["T12 is between T9 and 0"]),
        (RootSupport, -1.0, &["T15 counts for the label"]),
        (RootlessOverBass, -2.0, &["R3 is between R4 and 0"]),
        (LowestIsRoot, -0.1, &["L1 counts for the label"]),
        (BassThird, 2.0, &["B2 is between 0 and B1"]),
        (BassFifth, -0.1, &["B3 is between 0 and B1"]),
        (BassSeventh, 1.5, &["B4 is between 0 and B1"]),
        (BassTension, -0.5, &["B5 is between B6 and 0"]),
        (WalkingRoot, -1.0, &["B7 counts for the label", "B8 is between 0 and B7"]),
        (WalkingTones, 2.0, &["B8 is between 0 and B7"]),
        (WalkingWrong, 0.1, &["B9 counts against the label"]),
        (StruckAtStart, -0.1, &["E1 and E2 count for the label"]),
        (Pushed, 0.1, &["E3 and E8 count against the label"]),
        (OnBar, 0.1, &["E4 is 0 or less"]),
        (MidBar, 0.1, &["E5 is at most E4"]),
        (OffBeat, 0.0, &["E7 is at most E6, at most E5"]),
        (TensionPrior, -0.1, &["the trusts are 0 or more"]),
    ];
    for (feature, value, rules) in cases {
        let mut w = HAND;
        w.features[feature] = value;
        assert_eq!(w.check(), rules, "{} at {value}", feature.code());
    }
    let mut w = HAND;
    w.pair = -1.0;
    assert_eq!(w.check(), ["the trusts are 0 or more"]);
    w = HAND;
    w.none = f64::NAN;
    assert_eq!(w.check(), ["every number is finite"]);
    w = HAND;
    w.features[Tensions] = f64::INFINITY;
    assert_eq!(w.check(), ["T2 is between 0 and T1", "every number is finite"]);
}

/// What a hand case's stretch must read on its own.
#[derive(Clone, Copy, Debug)]
enum Want {
    /// The first reading is one of these (the design's gate allows any, or the namer turns it into the name wanted).
    First(&'static [&'static str]),
    /// These are the first two readings, or the first two hold the one given.
    TopTwo(&'static [&'static str]),
    /// These are kept for deciding where the chords change: among the best eight, or the best rootless reading. Their
    /// gates need what decides between neighbouring stretches, or the counted tables.
    Kept(&'static [&'static str]),
}

/// The design's hand cases that one stretch read on its own decides, as far as it decides them
/// (harmony/design/chord-reader-design.md, "Hand cases"): a name, the music, the units, and what it must read.
fn hand_cases() -> Vec<(&'static str, Music, Range<usize>, Want)> {
    use Want::*;
    let keys_bass = |bass: &str, keys: &str| Music::new().bass(bars(bass)).keys(bars(keys));
    let keys = |keys: &str| Music::new().keys(bars(keys));
    let pedal = || Music::new().bass(notes("C2", 0, 4 * BAR)).keys(bars("C4 E4 G4 | C4 F4 A4 | B3 D4 G4 | C4 E4 G4"));
    let inversion = || keys_bass("C2 (1-2) B1 (3-4) | A1", "C4 E4 G4 | A3 C4 E4");
    let two_feel = || keys_bass("C2 (1-2) G2 (3-4) | F2", "C4 E4 G4 | F3 A3 C4");
    let ii_v_i = || keys_bass("D2 | G2 | C2 | C2", "F3 A3 C4 E4 | F3 A3 B3 E4 | E3 G3 B3 D4 | E3 G3 B3 D4");
    let low = || keys_bass("D2 | G2 | C2 | C2", "C3 E3 F3 A3 | B2 E3 F3 A3 | B2 D3 E3 G3 | B2 D3 E3 G3");
    let rootless = || keys("F4 A4 C5 E5 | F4 A4 B4 E5 | E4 G4 B4 D5 | E4 G4 B4 D5");
    let shells = || keys("D3 C4 F4 | G2 B3 F4 | C3 B3 E4");
    let dim7 = || keys_bass("C#2 | D2", "E3 G3 Bb3 | F3 A3 C4");
    let chromatic = || keys_bass("D2 | Db2 | C2", "F3 A3 C4 | F3 B3 Eb4 | E3 G3 B3");
    let sus = || keys_bass("G1", "G3 C4 D4 F4 (1-2) G3 B3 D4 F4 (3-4)");
    let tune = |t: Vec<Note>| Music::new().bass(bars("G2")).keys(bars("G3 B3 D4")).tune(t);
    let cmaj7 = |t: Vec<Note>| Music::new().bass(bars("C2")).keys(bars("E3 G3 B3")).tune(t);
    let turn = [notes("D5", 0, 3 * BEAT), line("E5 D5", 3 * BEAT, BEAT / 2)].concat();
    vec![
        ("1", keys_bass("C2", "C4 E4 G4"), bar(0), First(&["C"])),
        ("2 C/E", keys_bass("E2", "C4 E4 G4"), bar(0), First(&["C"])),
        ("2 C/E in octaves", keys_bass("E2 E3", "C4 E4 G4"), bar(0), First(&["C"])),
        ("2 C/G", keys_bass("G1", "C4 E4 G4"), bar(0), First(&["C"])),
        ("2 G7/F", keys_bass("F2", "G3 B3 D4 F4"), bar(0), First(&["G7"])),
        ("3 D/C", keys_bass("C2", "D4 F#4 A4"), bar(0), First(&["D7", "D"])),
        ("4 C C/B", inversion(), bar(0), First(&["C", "Cmaj7"])),
        ("4 Am", inversion(), bar(1), First(&["Am"])),
        ("5 C", two_feel(), bar(0), First(&["C"])),
        ("5 F", two_feel(), bar(1), First(&["F"])),
        ("7 G9sus4", keys_bass("G1", "F3 A3 C4"), bar(0), First(&["G7sus4", "F"])),
        ("7 G9sus4 with D", keys_bass("G1", "F3 A3 C4 D4"), bar(0), First(&["G7sus4"])),
        ("7 C9sus4", keys_bass("C2", "Bb3 D4 F4"), bar(0), First(&["C7sus4", "Bb"])),
        ("7 Bb9sus4", keys_bass("Bb1", "Ab3 C4 Eb4"), bar(0), First(&["Bb7sus4", "Ab"])),
        ("8 Bb13sus4", keys_bass("Bb1", "F3 Ab3 C4 Eb4 G4"), bar(0), First(&["Bb7sus4"])),
        ("9 C", pedal(), bar(0), First(&["C"])),
        ("9 F/C", pedal(), bar(1), First(&["F"])),
        ("9 G/C", pedal(), bar(2), First(&["G"])),
        ("10 Dm9", ii_v_i(), bar(0), First(&["Dm7"])),
        ("10 G13", ii_v_i(), bar(1), First(&["G7"])),
        ("10 Cmaj9", ii_v_i(), 16..32, First(&["Cmaj7"])),
        ("11 Dm9", low(), bar(0), First(&["Dm7"])),
        ("11 G13", low(), bar(1), First(&["G7"])),
        ("11 Cmaj9", low(), 16..32, First(&["Cmaj7"])),
        ("12 Fmaj7 or Dm9", rootless(), bar(0), Kept(&["Fmaj7", "Dm7"])),
        ("12 G13 or Fmaj7(#11)", rootless(), bar(1), Kept(&["G7", "Fmaj7"])),
        ("12 Em7 or Cmaj9", rootless(), 16..32, Kept(&["Em7", "Cmaj7"])),
        ("13 G13 rootless", keys("B2 E3 F3 A3"), bar(0), Kept(&["G7"])),
        ("14 Dm7", shells(), bar(0), First(&["Dm7"])),
        ("14 G7", shells(), bar(1), First(&["G7"])),
        ("14 Cmaj7", shells(), bar(2), First(&["Cmaj7"])),
        ("16 C7(#9)", keys_bass("C2", "E3 Bb3 Eb4"), bar(0), First(&["C7"])),
        ("16 E7(#9)", keys_bass("E2", "G#3 D4 G4"), bar(0), First(&["E7"])),
        ("16 E7(#9) keys alone", keys("E3 G#3 D4 G4"), bar(0), First(&["E7"])),
        ("17 C7alt", keys_bass("C2", "E3 Bb3 D#4 G#4"), bar(0), First(&["C7"])),
        ("18 G7alt", keys_bass("G2", "B3 F4 Ab4 Bb4 Eb5"), bar(0), First(&["G7"])),
        ("19 Db13", keys_bass("Db2", "F3 B3 Eb4 Bb4"), bar(0), First(&["Db7"])),
        ("19 G7alt", keys_bass("G1", "F3 B3 Eb4 Bb4"), bar(0), First(&["G7"])),
        // G7alt waits for the counted tables: Abm6, rootless too, explains more of the notes.
        ("20 no bass", keys("F3 B3 Eb4 Bb4"), bar(0), Kept(&["Db7"])),
        ("21 Am6", keys_bass("A1", "C4 E4 F#4 A4"), bar(0), First(&["Am6"])),
        ("21 F#m7b5", keys_bass("F#2", "C4 E4 F#4 A4"), bar(0), First(&["F#m7b5"])),
        ("21 D9", keys_bass("D2", "C4 E4 F#4 A4"), bar(0), First(&["D7"])),
        ("21 no bass", keys("C4 E4 F#4 A4"), bar(0), TopTwo(&["Am6", "F#m7b5"])),
        ("22 C6", keys_bass("C2", "C4 E4 G4 A4"), bar(0), First(&["C6"])),
        ("22 Am7", keys_bass("A1", "C4 E4 G4 A4"), bar(0), First(&["Am7"])),
        ("22 Fmaj9", keys_bass("F1", "C4 E4 G4 A4"), bar(0), First(&["Fmaj7"])),
        ("22 no bass", keys("C4 E4 G4 A4"), bar(0), TopTwo(&["C6", "Am7"])),
        ("23 Cmaj9/E", keys_bass("E2", "G3 B3 C4 D4"), bar(0), TopTwo(&["Cmaj7", "Em7"])),
        ("24 C#dim7", dim7(), bar(0), First(&["Dbdim7", "Edim7", "Gdim7", "Bbdim7"])),
        ("24 Dm7", dim7(), bar(1), First(&["Dm7"])),
        ("26 C7(b9)", keys_bass("C2", "E3 G3 Bb3 Db4"), bar(0), First(&["C7"])),
        ("30 C+", keys_bass("C2", "E3 G#3 C4"), bar(0), First(&["C+", "E+", "Ab+"])),
        ("31 G7sus4", sus(), 0..4, First(&["G7sus4"])),
        ("31 G7", sus(), 4..8, First(&["G7"])),
        ("33 Em11", keys_bass("E2", "E3 A3 D4 G4 B4"), bar(0), First(&["Em7"])),
        ("33 Dm11", keys_bass("D2", "G3 C4 F4"), bar(0), First(&["Dm7"])),
        ("33 no bass", keys("E3 A3 D4 G4"), bar(0), TopTwo(&["A7sus4", "Em7"])),
        ("34 Cmaj9", keys_bass("C2", "D4 E4 G4 B4"), bar(0), First(&["Cmaj7"])),
        ("47 G, E5 a neighbour", tune(turn), bar(0), First(&["G"])),
        ("48a Cmaj9, D5 held", cmaj7(bars("D5")), bar(0), First(&["Cmaj7"])),
        ("48b Cmaj7, F5 held", cmaj7(bars("F5")), bar(0), First(&["Cmaj7"])),
        ("48c Cmaj7, C5 D5 E5", cmaj7(line("C5 D5 E5", 0, BEAT / 2)), bar(0), First(&["Cmaj7"])),
        ("49 Cmaj9 keys alone", keys("C2 G2 C3 E4 G4 B4 D5"), bar(0), First(&["Cmaj7"])),
        ("50 Cmaj9/E keys alone", keys("E2 E3 C4 G4 B4 D5"), bar(0), TopTwo(&["Cmaj7"])),
        ("55 Dm7", chromatic(), bar(0), First(&["Dm7"])),
        ("55 Db9", chromatic(), bar(1), First(&["Db7"])),
        ("55 Cmaj7", chromatic(), bar(2), First(&["Cmaj7"])),
    ]
}

#[test]
fn the_hand_cases_read_as_a_player_reads_them_one_stretch_at_a_time() {
    for (name, music, units, want) in hand_cases() {
        let ranked = ranking(&facts_on(&music.looped().hear(), units, None));
        let names: Vec<String> = ranked.iter().map(|s| s.label.to_string()).collect();
        let among = |these: &[&str], most: usize| these.iter().all(|t| names.iter().take(most).any(|n| n == t));
        let ok = match want {
            Want::First(any) => any.contains(&names[0].as_str()),
            Want::TopTwo(these) => among(these, 2),
            Want::Kept(these) => {
                let best_rootless = ranked.iter().find(|s| s.route == Route::Rootless).map(|s| s.label.to_string());
                these.iter().all(|t| among(&[t], 8) || best_rootless.as_deref() == Some(*t))
            }
        };
        let read: Vec<String> = ranked.iter().take(8).map(|s| format!("{} {:.2}", s.label, s.score)).collect();
        assert!(ok, "case {name}: wanted {want:?}, read {}", read.join(", "));
    }
}

#[test]
#[ignore = "prints the hand cases' rankings, for tuning the hand-set weights (SHOW=n for more than 6)"]
fn show_the_hand_cases() {
    let most = std::env::var("SHOW").ok().and_then(|n| n.parse().ok()).unwrap_or(6);
    for (name, music, units, _) in hand_cases() {
        let ranked = ranking(&facts_on(&music.looped().hear(), units, None));
        let top: Vec<String> = ranked.iter().take(most).map(|s| format!("{} {:.2}", s.label, s.score)).collect();
        println!("{name:<28} {}", top.join(", "));
    }
}
