//! The readings worth scoring for a stretch of music: the labels (a root and a core) the notes heard there let in.
//!
//! Scoring all 192 labels (12 roots, 16 cores) for every stretch would be slow, and most of them would be nonsense, so a
//! label is only tried when the notes that say which chord it is sound. There are four ways in:
//!
//! - **Played** ([`valid`]): the root sounds, and so do the core's third (or 4th or 2nd), its seventh or sixth, and
//!   the fifth of a diminished, augmented or power chord.
//! - **Rootless** ([`valid_rootless`]): the root doesn't sound, but a seventh or sixth chord's third, its seventh or
//!   sixth and one of its tensions do, as a pianist plays over a bass player.
//! - **Open fifth** ([`open_fifth`]): the root sounds with its fifth and no third, so the major and the minor triad
//!   are both tried. Pop piano often leaves the third to the tune, or plays a sus shape, under a major or minor chord.
//! - **Escape** ([`escape`]): on a root that sounds, the label whose own notes sound longest of all those no other way
//!   lets in, so a real chord with a note missing (C G B♭ D, a C9 with no third) always has a way in.
//!
//! What scores each label comes later; this only says which are worth scoring. Everything here moves with the music:
//! the list runs from the lowest note up (by root, then by core), so moving the notes moves every label to its new
//! root and keeps the list in the same order, and the escape's ties are broken in that order too.

use std::fmt;

use super::cores::{Core, CoreSet, valid, valid_rootless};
use crate::pitch::{PcSet, PerPitch, PitchClass, SetSums};

/// A reading of a stretch before it's named: a root and one of the 16 cores (the design's label). Its tensions, its
/// bass and its spelling come later, from the notes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Label {
    pub root: PitchClass,
    pub core: Core,
}

impl Label {
    pub const fn new(root: PitchClass, core: Core) -> Label {
        Label { root, core }
    }

    /// Its own notes (the design's C): the root, the third (or 4th or 2nd), the fifth, and the seventh or sixth.
    pub const fn tones(self) -> PcSet {
        self.core.intervals().on(self.root)
    }

    /// The notes it can carry as tensions (the design's X).
    pub const fn tensions(self) -> PcSet {
        self.core.tensions().on(self.root)
    }

    /// The notes that don't belong to it (the design's O).
    pub const fn others(self) -> PcSet {
        self.core.others().on(self.root)
    }
}

impl fmt::Display for Label {
    /// As a chart writes the core: C, Dm7, F#m7b5, Bbmaj7, G7sus4.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.root, self.core.name())
    }
}

/// The way a label got in among the candidates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Route {
    /// Its root sounds, and so do the notes that say which core it is ([`valid`]).
    Played,
    /// Its root doesn't sound, but its third, its seventh or sixth and a tension do ([`valid_rootless`]).
    Rootless,
    /// A major or minor triad on a root heard with its fifth and no third ([`open_fifth`]).
    OpenFifth,
    /// The label whose own notes sound longest among those no other way lets in ([`escape`]).
    Escape,
}

/// A label worth scoring, and the way it got in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Candidate {
    pub label: Label,
    pub route: Route,
}

/// The cores the open-fifth way lets in on a root, for the notes heard as intervals above it: the major and the minor
/// triad, when the root and its fifth sound and neither third does (an open fifth, or a sus shape such as 1 4 5).
/// Nothing in the notes says which of the two it is, so the scoring and the chords around decide.
pub const fn open_fifth(heard: PcSet) -> CoreSet {
    if heard.has(0) && heard.has(7) && !heard.has(3) && !heard.has(4) {
        CoreSet::EMPTY.with(Core::Major).with(Core::Minor)
    } else {
        CoreSet::EMPTY
    }
}

/// The cores the played, rootless and open-fifth ways let in on a root, for the notes heard as intervals above it.
pub fn allowed(heard: PcSet) -> CoreSet {
    if heard.has(0) { valid(heard).union(open_fifth(heard)) } else { valid_rootless(heard) }
}

/// The escape label: on a root that sounds, the core whose own notes have the most ticks (`ticks`, such as
/// [`super::Hearing::presence_ticks`]) of all the labels `taken` turns away. A tie goes to the root nearest above the
/// lowest note sounding (`lowest`), then to the first core in order ([`Core::ALL`]): the order [`candidates()`] lists
/// labels in, so the choice moves with the music. `None` when no root sounds or `taken` takes every label.
pub fn escape(heard: PcSet, ticks: &PerPitch<u64>, lowest: PitchClass, taken: impl Fn(Label) -> bool) -> Option<Label> {
    // Where the ticks all add up to less than the top, any set's add up exactly in three lookups.
    if ticks.total() < u64::MAX {
        let sums = SetSums::new(ticks);
        return first_most(heard, lowest, taken, |label| sums.sum(label.tones()));
    }
    first_most(heard, lowest, taken, |label| label.tones().iter().map(|pc| u128::from(ticks[pc])).sum::<u128>())
}

/// The label that isn't taken whose own notes have the most ticks (`own`), on a root that's heard: the first of equals,
/// from the lowest note up.
fn first_most<T: PartialOrd>(
    heard: PcSet,
    lowest: PitchClass,
    taken: impl Fn(Label) -> bool,
    own: impl Fn(Label) -> T,
) -> Option<Label> {
    let mut best: Option<(T, Label)> = None;
    for up in 0..12 {
        let root = lowest.up(up);
        if !heard.contains(root) {
            continue;
        }
        for core in Core::ALL {
            let label = Label::new(root, core);
            if taken(label) {
                continue;
            }
            let own = own(label);
            if best.as_ref().is_none_or(|(most, _)| own > *most) {
                best = Some((own, label));
            }
        }
    }
    best.map(|(_, label)| label)
}

/// The labels worth scoring for a stretch, each once, by the first way that lets it in: `heard` is the notes heard
/// there (the design's P, [`super::Hearing::heard`]), `ticks` how long each pitch class sounds there (for the escape)
/// and `lowest` the lowest note sounding there. In the order ties are broken: by root, from the lowest note up, then
/// by core ([`Core::ALL`]), with the escape in its place.
pub fn candidates(heard: PcSet, ticks: &PerPitch<u64>, lowest: PitchClass, out: &mut Vec<Candidate>) {
    out.clear();
    let mut allowed_on = PerPitch::<CoreSet>::default();
    for root in PitchClass::ALL {
        allowed_on[root] = allowed(heard.above(root));
    }
    let escape = if ticks.total() < u64::MAX {
        // The escape as [`escape`] finds it, quicker: from the cores each heard root doesn't let in.
        let sums = SetSums::new(ticks);
        let mut best: Option<(u64, Label)> = None;
        for root in heard.above(lowest).iter().map(|up| lowest.up(i32::from(up.number()))) {
            for core in allowed_on[root].others().iter() {
                let label = Label::new(root, core);
                let own = sums.sum(label.tones());
                if best.is_none_or(|(most, _)| own > most) {
                    best = Some((own, label));
                }
            }
        }
        best.map(|(_, label)| label)
    } else {
        escape(heard, ticks, lowest, |label| allowed_on[label.root].contains(label.core))
    };
    for up in 0..12 {
        let root = lowest.up(up);
        let (rel, allowed) = (heard.above(root), allowed_on[root]);
        let played = valid(rel);
        let escapes = escape.filter(|e| e.root == root).map(|e| CoreSet::EMPTY.with(e.core));
        for core in allowed.union(escapes.unwrap_or(CoreSet::EMPTY)).iter() {
            let route = if !allowed.contains(core) {
                Route::Escape
            } else if !rel.has(0) {
                Route::Rootless
            } else if played.contains(core) {
                Route::Played
            } else {
                Route::OpenFifth
            };
            out.push(Candidate { label: Label::new(root, core), route });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pitch classes by name: "C E G".
    fn pcs(names: &str) -> PcSet {
        names.split_whitespace().map(pc).collect()
    }

    fn pc(name: &str) -> PitchClass {
        let names = ["C", "Db", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];
        PitchClass::new(names.iter().position(|&n| n == name).unwrap_or_else(|| panic!("{name}?")) as i32)
    }

    /// The same ticks for each pitch class named, none for the rest.
    fn even(names: &str, ticks: u64) -> PerPitch<u64> {
        let mut out = PerPitch::default();
        for p in pcs(names).iter() {
            out[p] = ticks;
        }
        out
    }

    /// The candidates as "label route", in their order.
    fn described(heard: &str, ticks: &PerPitch<u64>, lowest: &str) -> Vec<String> {
        let mut out = Vec::new();
        candidates(pcs(heard), ticks, pc(lowest), &mut out);
        out.iter().map(|c| format!("{} {:?}", c.label, c.route)).collect()
    }

    #[test]
    fn a_label_is_a_root_and_a_core() {
        let d7 = Label::new(pc("D"), Core::Minor7);
        assert_eq!(d7.to_string(), "Dm7");
        assert_eq!((d7.tones(), d7.tensions()), (pcs("D F A C"), pcs("E G Bb B")));
        assert_eq!(d7.others(), pcs("Eb F# Ab Db"));
        assert_eq!(Label::new(pc("F#"), Core::HalfDiminished).to_string(), "F#m7b5");
        assert_eq!(Label::new(pc("Bb"), Core::Dominant7Sus4).to_string(), "Bb7sus4");
        assert_eq!(Label::new(pc("C"), Core::Major).to_string(), "C");
    }

    #[test]
    fn a_triad_lets_in_its_own_chord() {
        // C E G: C and C5 played on C; E G with C over them is an Em(b13); G with C is a Gsus4; and with no D, C E G
        // are a D9sus4's 7th, 9th and 4th (a C triad over a D bass is one). The escape is the label with the most
        // notes of the rest: C7, whose own C E G sound (its B♭ doesn't; the scoring will see that). The list runs
        // from the lowest note up, by root and then by core.
        let ticks = even("C E G", 480);
        assert_eq!(
            described("C E G", &ticks, "C"),
            ["C Played", "C7 Escape", "C5 Played", "D7sus4 Rootless", "Em Played", "Gsus4 Played"]
        );
        // Over an E in the bass, the same labels start from E.
        assert_eq!(
            described("C E G", &ticks, "E"),
            ["Em Played", "Gsus4 Played", "C Played", "C7 Escape", "C5 Played", "D7sus4 Rootless"]
        );
    }

    #[test]
    fn an_open_fifth_tries_the_major_and_the_minor_triad() {
        let ticks = even("C G", 480);
        assert_eq!(
            described("C G", &ticks, "C"),
            ["C OpenFifth", "Cm OpenFifth", "C7 Escape", "C5 Played", "Gsus4 Played"]
        );
        // A sus shape (1 4 5) too, beside its own sus chord; not when a third sounds.
        let sus = described("C F G", &even("C F G", 480), "C");
        for wanted in ["Csus4 Played", "C OpenFifth", "Cm OpenFifth"] {
            assert!(sus.contains(&wanted.to_owned()), "{sus:?}");
        }
        assert_eq!(open_fifth(pcs("C E G")), CoreSet::EMPTY);
        assert_eq!(open_fifth(pcs("C Eb G")), CoreSet::EMPTY);
        assert_eq!(open_fifth(pcs("C F")), CoreSet::EMPTY, "no fifth");
        assert_eq!(open_fifth(pcs("G")), CoreSet::EMPTY, "no root");
    }

    #[test]
    fn a_ninth_with_no_third_keeps_a_way_in() {
        // C G B♭ D over a C bass: C7sus4 (the 2 standing in for the 4th) and C5 are played; the C7 it is comes in
        // by the escape, its root, fifth and seventh sounding longer than Gm7's G B♭ D.
        let mut ticks = even("G Bb D", 960);
        ticks[pc("C")] = 3840;
        let out = described("C D G Bb", &ticks, "C");
        assert!(out.contains(&"C7sus4 Played".to_owned()), "{out:?}");
        let escapes: Vec<&String> = out.iter().filter(|c| c.ends_with("Escape")).collect();
        assert_eq!(escapes, ["C7 Escape"], "{out:?}");
    }

    #[test]
    fn a_rootless_voicing_is_let_in_with_its_root_not_sounding() {
        // E G B D: Em7 played, and Cmaj9 with no C (its 3rd, 7th and 9th), G6 and A9sus4 with no A.
        let out = described("E G B D", &even("E G B D", 480), "E");
        for wanted in ["Cmaj7 Rootless", "Em7 Played", "G6 Played", "A7sus4 Rootless"] {
            assert!(out.contains(&wanted.to_owned()), "{wanted}: {out:?}");
        }
        // A rootless reading needs its third, its seventh and a tension: two notes are never one.
        let out = described("E G", &even("E G", 480), "E");
        assert!(!out.iter().any(|c| c.ends_with("Rootless")), "{out:?}");
    }

    #[test]
    fn the_escape_breaks_ties_from_the_lowest_note() {
        // A tritone alone: nothing lets a label in, and every diminished chord on either note sounds both. The tie goes
        // to the lowest note's root, then to the first core: the m7b5.
        let ticks = even("C F#", 480);
        assert_eq!(described("C F#", &ticks, "C"), ["Cm7b5 Escape"]);
        assert_eq!(described("C F#", &ticks, "F#"), ["F#m7b5 Escape"]);
        // Sounding longer wins over the tie-breaks: over C D with a longer D, the D7 (D and C sounding) beats every
        // label on C turned away, which sound the C alone.
        let mut longer = even("C", 480);
        longer[pc("D")] = 960;
        let taken = |l: Label| allowed(pcs("C D").above(l.root)).contains(l.core);
        assert_eq!(escape(pcs("C D"), &longer, pc("C"), taken), Some(Label::new(pc("D"), Core::Dominant7)));
        // With nothing turned away, Csus2 sounds as long, and C comes first.
        assert_eq!(escape(pcs("C D"), &longer, pc("C"), |_| false), Some(Label::new(pc("C"), Core::Sus2)));
        // Nothing sounding: no escape.
        assert_eq!(escape(PcSet::EMPTY, &ticks, pc("C"), |_| false), None);
        assert_eq!(escape(pcs("C"), &ticks, pc("C"), |_| true), None);
    }

    /// A small random number maker (xorshift), so the random sets are the same on every run.
    struct Dice(u64);

    impl Dice {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
    }

    #[test]
    fn every_label_comes_in_once_by_the_first_way_that_lets_it_in() {
        let mut dice = Dice(0x9E37_79B9_7F4A_7C15);
        let mut out = Vec::new();
        for bits in 0..4096u16 {
            let heard = PcSet::from_bits(bits);
            let mut ticks = PerPitch::default();
            for p in heard.iter() {
                ticks[p] = 1 + dice.next() % 4;
            }
            let lowest = PitchClass::new((dice.next() % 12) as i32);
            candidates(heard, &ticks, lowest, &mut out);
            // In the order ties are broken (from the lowest note up, by root and then by core), so each label once.
            let place = |c: &Candidate| (c.label.root.above(lowest), c.label.core);
            assert!(out.windows(2).all(|w| w.first().map(place) < w.last().map(place)), "{heard}: out of order");
            for root in PitchClass::ALL {
                let rel = heard.above(root);
                for core in Core::ALL {
                    let route = out.iter().find(|c| c.label == Label::new(root, core)).map(|c| c.route);
                    let wanted = if rel.has(0) && valid(rel).contains(core) {
                        Some(Route::Played)
                    } else if !rel.has(0) && valid_rootless(rel).contains(core) {
                        Some(Route::Rootless)
                    } else if open_fifth(rel).contains(core) {
                        Some(Route::OpenFifth)
                    } else {
                        route.filter(|&r| r == Route::Escape)
                    };
                    assert_eq!(route, wanted, "{heard}: {}", Label::new(root, core));
                }
            }
            // The escape: on a root that sounds, sounding longest of the labels turned away, the first in order of
            // the tie-breaks.
            let escapes: Vec<Label> = out.iter().filter(|c| c.route == Route::Escape).map(|c| c.label).collect();
            let own = |l: Label| l.tones().iter().map(|p| ticks[p]).sum::<u64>();
            let rest: Vec<Label> = (0..12)
                .map(|up| lowest.up(up))
                .filter(|&root| heard.contains(root))
                .flat_map(|root| Core::ALL.map(|core| Label::new(root, core)))
                .filter(|l| !allowed(heard.above(l.root)).contains(l.core))
                .collect();
            let most = rest.iter().map(|&l| own(l)).max();
            let first = rest.iter().find(|&&l| Some(own(l)) == most).copied();
            assert_eq!(escapes, first.into_iter().collect::<Vec<_>>(), "{heard}");
        }
    }

    #[test]
    fn the_candidates_move_with_the_music() {
        let mut dice = Dice(0x0123_4567_89AB_CDEF);
        let (mut out, mut moved) = (Vec::new(), Vec::new());
        for _ in 0..2000 {
            let heard = PcSet::from_bits((dice.next() & 0xFFF) as u16);
            let mut ticks = PerPitch::default();
            for p in heard.iter() {
                ticks[p] = 1 + dice.next() % 3;
            }
            let lowest = PitchClass::new((dice.next() % 12) as i32);
            candidates(heard, &ticks, lowest, &mut out);
            for shift in 1..12 {
                let mut shifted = PerPitch::default();
                for p in PitchClass::ALL {
                    shifted[p.up(shift)] = ticks[p];
                }
                candidates(heard.transposed(shift), &shifted, lowest.up(shift), &mut moved);
                // The same labels moved, the same ways in, in the same order.
                let back: Vec<Candidate> = moved
                    .iter()
                    .map(|c| Candidate { label: Label::new(c.label.root.up(-shift), c.label.core), route: c.route })
                    .collect();
                assert_eq!(back, out, "{heard} moved {shift}");
            }
        }
    }
}
