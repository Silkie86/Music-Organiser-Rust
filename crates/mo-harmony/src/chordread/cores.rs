//! The kinds of chord the reader decides between, and which notes each one takes.
//!
//! A reading is a root and one of 16 cores ([`Core`]): the chord with its tensions left out and its seventh (or sixth)
//! kept, because the seventh decides which tensions the chord can carry. To each core, every other note is a tension
//! it can carry or a note that doesn't belong ([`ToneClass`]): a ♭9 is a tension on a dominant seventh, and a wrong
//! note on a major seventh. A minor third beside a major third and a ♭7 is a ♯9 on a dominant seventh, while a major
//! third is a wrong note on a minor seventh, so a chord with both thirds can't read as minor without paying for it.
//!
//! Before anything is scored, the notes heard decide which cores are worth trying on each root. With the root
//! sounding ([`valid`]), the notes that say which chord it is must sound too: the third of a triad, the third and the
//! seventh (or sixth) of a four-note chord, the ♭3 and ♭5 of a diminished one, the 3rd and ♯5 of an augmented one, the
//! 4th or 2nd of a sus chord and the fifth of a power chord. With the root not sounding ([`valid_rootless`]), as a
//! pianist plays over a bass player, only a seventh or sixth chord (or a m7b5) is tried, and only when its third (or
//! 4th), its seventh (or sixth), a m7b5's ♭5 and at least one of its tensions sound: the A and B voicings, upper
//! structures, a diminished seventh standing for four 7(♭9) chords, and the two dominants a tritone shares. Both
//! answers come from tables with one entry for each of the 4,096 sets of notes, worked out when the app is built.

use std::fmt;

use crate::chord::Family;
use crate::pitch::PcSet;

/// One of the 16 kinds of chord the reader decides between: a chord with its tensions left out (C9, C13(♭9) and
/// C7alt are all [`Core::Dominant7`]). Their order here is the order exact ties are broken in, and each fits in four
/// bits ([`Core::index`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum Core {
    /// 1 3 5.
    Major,
    /// 1 ♭3 5.
    Minor,
    /// 1 3 5 ♭7.
    Dominant7,
    /// 1 3 5 7.
    Major7,
    /// 1 ♭3 5 ♭7.
    Minor7,
    /// 1 3 5 6.
    Major6,
    /// 1 ♭3 5 6.
    Minor6,
    /// 1 ♭3 5 7.
    MinorMajor7,
    /// 1 ♭3 ♭5 ♭7 (m7b5).
    HalfDiminished,
    /// 1 ♭3 ♭5 𝄫7.
    Diminished7,
    /// 1 ♭3 ♭5.
    Diminished,
    /// 1 3 ♯5.
    Augmented,
    /// 1 4 5.
    Sus4,
    /// 1 2 5.
    Sus2,
    /// 1 4 5 ♭7: also 9sus4 and 13sus4, which are this with tensions.
    Dominant7Sus4,
    /// 1 5.
    Power,
}

/// What a note is to a core, by its interval above the root.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToneClass {
    /// One of the core's own notes: its root, third (or 4th or 2nd), fifth, and seventh or sixth.
    Core,
    /// A note the core can carry: a 9, 11 or 13 or their altered forms, as the core allows, and a seventh no other
    /// core has (the major seventh of Cmaj7♯5, Cmaj7sus4 and Cdim7(maj7), the ♭7 of C7(no3)).
    Tension,
    /// A note that doesn't belong: an avoid note (a natural 11 over a major third), a second seventh beside the
    /// core's own (a major seventh on a dominant), or a seventh or sixth that belongs to another core (the ♭7 over a
    /// major triad is [`Core::Dominant7`]'s, so a reading with it moves there).
    Other,
}

impl Core {
    /// Every core, in order.
    pub const ALL: [Core; 16] = [
        Core::Major,
        Core::Minor,
        Core::Dominant7,
        Core::Major7,
        Core::Minor7,
        Core::Major6,
        Core::Minor6,
        Core::MinorMajor7,
        Core::HalfDiminished,
        Core::Diminished7,
        Core::Diminished,
        Core::Augmented,
        Core::Sus4,
        Core::Sus2,
        Core::Dominant7Sus4,
        Core::Power,
    ];

    /// The cores a reading may take with its root not sounding: the seventh and sixth chords a pianist voices without
    /// the root (dominant, minor and major sevenths, sixths and 7sus4) and the m7b5 of a minor ii-V. A diminished
    /// seventh needs none (any of its notes can be its root), and a minor-major seventh is left out: it is a tonic
    /// chord, played over its root.
    pub const ROOTLESS: CoreSet = CoreSet::EMPTY
        .with(Core::Dominant7)
        .with(Core::Minor7)
        .with(Core::Major7)
        .with(Core::Major6)
        .with(Core::Minor6)
        .with(Core::HalfDiminished)
        .with(Core::Dominant7Sus4);

    /// Its place in [`Core::ALL`], from 0 to 15.
    pub const fn index(self) -> usize {
        self as usize
    }

    /// The core at this place in [`Core::ALL`]; `None` past the end.
    pub fn from_index(index: usize) -> Option<Core> {
        Core::ALL.get(index).copied()
    }

    /// Its own notes, as intervals above the root (bit 0 the root).
    pub const fn intervals(self) -> PcSet {
        // Each set is worked out when the app is built.
        match self {
            Core::Major => const { PcSet::of(&[0, 4, 7]) },
            Core::Minor => const { PcSet::of(&[0, 3, 7]) },
            Core::Dominant7 => const { PcSet::of(&[0, 4, 7, 10]) },
            Core::Major7 => const { PcSet::of(&[0, 4, 7, 11]) },
            Core::Minor7 => const { PcSet::of(&[0, 3, 7, 10]) },
            Core::Major6 => const { PcSet::of(&[0, 4, 7, 9]) },
            Core::Minor6 => const { PcSet::of(&[0, 3, 7, 9]) },
            Core::MinorMajor7 => const { PcSet::of(&[0, 3, 7, 11]) },
            Core::HalfDiminished => const { PcSet::of(&[0, 3, 6, 10]) },
            Core::Diminished7 => const { PcSet::of(&[0, 3, 6, 9]) },
            Core::Diminished => const { PcSet::of(&[0, 3, 6]) },
            Core::Augmented => const { PcSet::of(&[0, 4, 8]) },
            Core::Sus4 => const { PcSet::of(&[0, 5, 7]) },
            Core::Sus2 => const { PcSet::of(&[0, 2, 7]) },
            Core::Dominant7Sus4 => const { PcSet::of(&[0, 5, 7, 10]) },
            Core::Power => const { PcSet::of(&[0, 7]) },
        }
    }

    /// Its third, or the 4th or 2nd of a sus chord, which stands in for it; none for a power chord. (A 7sus4 may also
    /// sound its 2 in place of its 4th: see [`Core::fits`].)
    pub const fn third(self) -> PcSet {
        // Each set is worked out when the app is built.
        match self {
            Core::Major | Core::Dominant7 | Core::Major7 | Core::Major6 | Core::Augmented => const { PcSet::of(&[4]) },
            Core::Minor
            | Core::Minor7
            | Core::Minor6
            | Core::MinorMajor7
            | Core::HalfDiminished
            | Core::Diminished7
            | Core::Diminished => const { PcSet::of(&[3]) },
            Core::Sus4 | Core::Dominant7Sus4 => const { PcSet::of(&[5]) },
            Core::Sus2 => const { PcSet::of(&[2]) },
            Core::Power => const { PcSet::of(&[]) },
        }
    }

    /// Its seventh, or the sixth of a sixth chord (and the 𝄫7 of a diminished seventh); none for a triad.
    pub const fn seventh(self) -> PcSet {
        // Each set is worked out when the app is built.
        match self {
            Core::Dominant7 | Core::Minor7 | Core::HalfDiminished | Core::Dominant7Sus4 => const { PcSet::of(&[10]) },
            Core::Major7 | Core::MinorMajor7 => const { PcSet::of(&[11]) },
            Core::Major6 | Core::Minor6 | Core::Diminished7 => const { PcSet::of(&[9]) },
            // The triads and the power chord.
            _ => const { PcSet::of(&[]) },
        }
    }

    /// The fifth that says which core it is, so must sound: the ♭5 of the diminished chords, the ♯5 of an augmented
    /// one and a power chord's fifth. Any other chord's plain fifth may be left out, as shells leave it out.
    pub const fn fifth(self) -> PcSet {
        // Each set is worked out when the app is built.
        match self {
            Core::HalfDiminished | Core::Diminished7 | Core::Diminished => const { PcSet::of(&[6]) },
            Core::Augmented => const { PcSet::of(&[8]) },
            Core::Power => const { PcSet::of(&[7]) },
            _ => const { PcSet::of(&[]) },
        }
    }

    /// The notes it can carry ([`ToneClass::Tension`]), as intervals above the root: the 9ths, 11ths and 13ths its
    /// scale allows, as [`crate::chord::role`] gives them for the core's own notes, and a seventh no other core has.
    /// Three notes are judged again (the tests list them): the 13 of a major seventh is a tension, and the ♭13 of a
    /// minor sixth and the ♭9 of a sus2 sit a semitone under one of the chord's own notes, so don't belong.
    ///
    /// The core's own notes include its fifth, so whether the fifth sounds is left to the scoring, where a missing
    /// fifth costs a little: a ♭13 is a minor chord's tension even where it takes the fifth's place (C E♭ A♭ is also
    /// A♭/C, and the bass decides between them).
    pub const fn tensions(self) -> PcSet {
        // Each set is worked out when the app is built.
        match self {
            // 9, ♯11.
            Core::Major | Core::Major6 => const { PcSet::of(&[2, 6]) },
            // 9, ♯11, and the 7 of Cmaj7♯5.
            Core::Augmented => const { PcSet::of(&[2, 6, 11]) },
            // 9, 11, ♭13.
            Core::Minor | Core::HalfDiminished => const { PcSet::of(&[2, 5, 8]) },
            // 9, 11, ♭13, and the 7 (of Cdim(maj7), and the fourth tension of the diminished scale).
            Core::Diminished | Core::Diminished7 => const { PcSet::of(&[2, 5, 8, 11]) },
            // ♭9, 9, ♯9, ♯11, ♭13, 13.
            Core::Dominant7 => const { PcSet::of(&[1, 2, 3, 6, 8, 9]) },
            // 9, ♯11, 13.
            Core::Major7 => const { PcSet::of(&[2, 6, 9]) },
            // 9, 11, ♭13, 13.
            Core::Minor7 | Core::MinorMajor7 => const { PcSet::of(&[2, 5, 8, 9]) },
            // 9, 11.
            Core::Minor6 => const { PcSet::of(&[2, 5]) },
            // ♭9, 9, 13, and the 7 of Cmaj7sus4.
            Core::Sus4 => const { PcSet::of(&[1, 2, 9, 11]) },
            // ♭9, 9, 13.
            Core::Dominant7Sus4 => const { PcSet::of(&[1, 2, 9]) },
            // 13 (its 6), and the 7 of Cmaj7sus2.
            Core::Sus2 => const { PcSet::of(&[9, 11]) },
            // The ♭7 of C7(no3) and the 7 of Cmaj7(no3).
            Core::Power => const { PcSet::of(&[10, 11]) },
        }
    }

    /// The notes that don't belong, as intervals above the root: every note that is neither its own nor a tension.
    pub const fn others(self) -> PcSet {
        PcSet::ALL.difference(self.intervals()).difference(self.tensions())
    }

    /// What the note `interval` semitones above the root is to this core (any whole number, round the octave).
    pub const fn class(self, interval: i32) -> ToneClass {
        if self.intervals().has(interval) {
            ToneClass::Core
        } else if self.tensions().has(interval) {
            ToneClass::Tension
        } else {
            ToneClass::Other
        }
    }

    /// The core of a chord with these intervals above its root, as the chord names it: its tensions dropped and its
    /// seventh or sixth kept, where a core has that seventh (Cmaj13 is [`Core::Major7`], C6/9 [`Core::Major6`], Cm11
    /// [`Core::Minor7`], C7♯5 [`Core::Dominant7`], C9sus4 [`Core::Dominant7Sus4`]); a seventh no core has stays a
    /// note its core carries (Cmaj7♯5 is [`Core::Augmented`] with a major seventh). `None` when the notes have no
    /// third, no 4th or 2nd and no fifth: there is no chord to name.
    pub const fn of(intervals: PcSet) -> Option<Core> {
        let (six, b7, maj7) = (intervals.has(9), intervals.has(10), intervals.has(11));
        Some(match Family::of(intervals) {
            Family::Dominant => Core::Dominant7,
            Family::Major if maj7 => Core::Major7,
            Family::Major if six => Core::Major6,
            Family::Major => Core::Major,
            Family::Augmented => Core::Augmented,
            // A major seventh first, as the names have it: Cm(maj7)(b7).
            Family::Minor if maj7 => Core::MinorMajor7,
            Family::Minor if b7 => Core::Minor7,
            Family::Minor if six => Core::Minor6,
            Family::Minor => Core::Minor,
            Family::HalfDiminished => Core::HalfDiminished,
            Family::Diminished if six => Core::Diminished7,
            Family::Diminished => Core::Diminished,
            Family::Sus if b7 => Core::Dominant7Sus4,
            Family::Sus if intervals.has(5) => Core::Sus4,
            Family::Sus => Core::Sus2,
            Family::Power if intervals.has(7) => Core::Power,
            Family::Power => return None,
        })
    }

    /// Its quality as the chord names write it ([`crate::Chord::quality`]): "" for a major triad, "m", "7", "maj7",
    /// "m7b5", "dim7", "+", "7sus4", "5".
    pub const fn name(self) -> &'static str {
        match self {
            Core::Major => "",
            Core::Minor => "m",
            Core::Dominant7 => "7",
            Core::Major7 => "maj7",
            Core::Minor7 => "m7",
            Core::Major6 => "6",
            Core::Minor6 => "m6",
            Core::MinorMajor7 => "m(maj7)",
            Core::HalfDiminished => "m7b5",
            Core::Diminished7 => "dim7",
            Core::Diminished => "dim",
            Core::Augmented => "+",
            Core::Sus4 => "sus4",
            Core::Sus2 => "sus2",
            Core::Dominant7Sus4 => "7sus4",
            Core::Power => "5",
        }
    }

    /// Whether a reading on a root that sounds may be this core: `heard` holds the notes heard as intervals above the
    /// root, and the root and the notes that say which core it is ([`Core::third`], [`Core::seventh`] and
    /// [`Core::fifth`]) are among them. A 7sus4 may sound its 2 in place of its 4th (C D G B♭), as there is no 7sus2
    /// to read it as. Other notes heard don't rule a core out: they count against it when it is scored.
    pub const fn fits(self, heard: PcSet) -> bool {
        let third = match self {
            Core::Dominant7Sus4 => heard.has(5) || heard.has(2),
            _ => heard.contains_all(self.third()),
        };
        heard.has(0) && third && heard.contains_all(self.seventh()) && heard.contains_all(self.fifth())
    }

    /// Whether a reading on a root that doesn't sound may be this core: it is one of [`Core::ROOTLESS`], and its
    /// third (or 4th), its seventh (or sixth), a m7b5's ♭5 and at least one of its tensions are among the notes heard,
    /// read from the root (`heard`, without bit 0). Unlike [`Core::fits`], a 7sus4 needs its 4th here: with neither
    /// the root nor the 4th, nothing says it's a sus chord on that root (D B♭ A is B♭maj7, on a B♭ that sounds).
    pub const fn fits_rootless(self, heard: PcSet) -> bool {
        !heard.has(0)
            && Core::ROOTLESS.contains(self)
            && heard.contains_all(self.third())
            && heard.contains_all(self.seventh())
            && heard.contains_all(self.fifth())
            && !heard.intersection(self.tensions()).is_empty()
    }

    /// How a chord of this core tends to move on ([`Group`]).
    pub const fn group(self) -> Group {
        match self {
            Core::Dominant7 | Core::Dominant7Sus4 => Group::Dominant,
            Core::HalfDiminished | Core::Diminished7 | Core::Diminished | Core::Augmented => Group::DimAug,
            _ => Group::Other,
        }
    }
}

/// How a chord tends to move on to the next, for the pair term (the design's g): a dominant seventh, plain or sus,
/// goes up a fourth (V to I) or down a semitone (a tritone sub); a diminished or augmented chord, whose altered fifth
/// says which it is (dim, m7b5, dim7, +), moves by a step (C♯dim7 to Dm); any other chord moves freely.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Group {
    Dominant,
    DimAug,
    Other,
}

impl Group {
    /// Every group, in order.
    pub const ALL: [Group; 3] = [Group::Dominant, Group::DimAug, Group::Other];

    /// How many ways a chord can move on to another, as the pair term tells them apart: its root up one of 12
    /// intervals (the same root too: C to C7), to a chord in one of the groups.
    pub const MOVES: usize = 12 * Group::ALL.len();

    /// Its place in [`Group::ALL`], from 0 to 2.
    pub const fn index(self) -> usize {
        self as usize
    }

    /// Its name in the counted tables: "dominant", "dim/aug", "other".
    pub const fn name(self) -> &'static str {
        match self {
            Group::Dominant => "dominant",
            Group::DimAug => "dim/aug",
            Group::Other => "other",
        }
    }
}

/// A set of cores, one bit each (bit `k` for the core whose [`Core::index`] is `k`).
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct CoreSet(u16);

impl CoreSet {
    pub const EMPTY: CoreSet = CoreSet(0);

    pub const fn with(self, core: Core) -> CoreSet {
        CoreSet(self.0 | 1 << core as u16)
    }

    pub const fn contains(self, core: Core) -> bool {
        self.0 >> core as u16 & 1 != 0
    }

    pub const fn union(self, other: CoreSet) -> CoreSet {
        CoreSet(self.0 | other.0)
    }

    pub const fn difference(self, other: CoreSet) -> CoreSet {
        CoreSet(self.0 & !other.0)
    }

    /// Every core that isn't in it.
    pub const fn others(self) -> CoreSet {
        CoreSet(!self.0)
    }

    /// How many cores are in it.
    pub const fn len(self) -> u32 {
        self.0.count_ones()
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The sixteen bits, bit 0 for [`Core::Major`].
    pub const fn bits(self) -> u16 {
        self.0
    }

    /// The cores in it, in order.
    pub fn iter(self) -> impl Iterator<Item = Core> {
        let mut bits = self.0;
        std::iter::from_fn(move || {
            let core = Core::from_index(bits.trailing_zeros() as usize)?;
            bits &= bits - 1;
            Some(core)
        })
    }
}

impl FromIterator<Core> for CoreSet {
    fn from_iter<I: IntoIterator<Item = Core>>(cores: I) -> CoreSet {
        cores.into_iter().fold(CoreSet::EMPTY, CoreSet::with)
    }
}

impl fmt::Debug for CoreSet {
    /// The cores by name: {Major, Dominant7}.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}

/// For every set of notes heard (the index's twelve bits, read from a root), the cores [`Core::fits`] allows.
static VALID: [CoreSet; 4096] = table(false);

/// For every set of notes heard, the cores [`Core::fits_rootless`] allows.
static VALID_ROOTLESS: [CoreSet; 4096] = table(true);

/// One of the tables, worked out when the app is built.
// The loops stay inside the table (4,096 entries) and the list of cores (16).
#[allow(clippy::indexing_slicing)]
const fn table(rootless: bool) -> [CoreSet; 4096] {
    let mut out = [CoreSet::EMPTY; 4096];
    let mut bits = 0;
    while bits < out.len() {
        let heard = PcSet::from_bits(bits as u16);
        let mut k = 0;
        while k < Core::ALL.len() {
            let core = Core::ALL[k];
            let fits = if rootless { core.fits_rootless(heard) } else { core.fits(heard) };
            if fits {
                out[bits] = out[bits].with(core);
            }
            k += 1;
        }
        bits += 1;
    }
    out
}

/// The cores a reading may be on a root that sounds, for the notes heard as intervals above that root (bit 0 the
/// root): [`Core::fits`], from a table. Empty when the root isn't heard.
pub fn valid(heard: PcSet) -> CoreSet {
    // A set has twelve bits, so it is always in the table.
    VALID.get(usize::from(heard.bits())).copied().unwrap_or_default()
}

/// The cores a reading may be on a root that doesn't sound, for the notes heard as intervals above that root:
/// [`Core::fits_rootless`], from a table. Empty when the root is heard.
pub fn valid_rootless(heard: PcSet) -> CoreSet {
    VALID_ROOTLESS.get(usize::from(heard.bits())).copied().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chord::{Chord, ToneRole, role};
    use crate::pitch::PitchClass;

    /// Every set of notes, as intervals above a root.
    fn every_set() -> impl Iterator<Item = PcSet> {
        (0..4096u16).map(PcSet::from_bits)
    }

    fn above(root: &str, notes: &str) -> PcSet {
        let root = crate::names::parse_note(root).unwrap().0;
        notes.split(' ').map(|n| crate::names::parse_note(n).unwrap().0).collect::<PcSet>().above(root)
    }

    #[test]
    fn every_core_is_named_as_the_app_names_its_notes() {
        for (k, core) in Core::ALL.into_iter().enumerate() {
            assert_eq!(core.index(), k);
            assert_eq!(Core::from_index(k), Some(core));
            assert_eq!(Core::of(core.intervals()), Some(core), "{core:?}");
            assert_eq!(Chord::of(PitchClass::C, core.intervals()).quality(), core.name(), "{core:?}");
        }
        assert_eq!(Core::from_index(16), None);
        assert!(Core::ALL.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn the_core_of_a_chord_keeps_its_seventh_and_drops_its_tensions() {
        for (name, core) in [
            ("Cmaj13", Core::Major7),
            ("C6/9", Core::Major6),
            ("Cadd9", Core::Major),
            ("Cm11", Core::Minor7),
            ("Cm13", Core::Minor7),
            ("Cm6/9", Core::Minor6),
            ("Cm(maj9)", Core::MinorMajor7),
            ("C13(b9)", Core::Dominant7),
            ("C7alt", Core::Dominant7),
            ("C7#5", Core::Dominant7),
            ("C7b5", Core::Dominant7),
            ("Cmaj7#5", Core::Augmented),
            ("C+", Core::Augmented),
            ("Cm9b5", Core::HalfDiminished),
            ("Cdim7(9)", Core::Diminished7),
            ("C9sus4", Core::Dominant7Sus4),
            ("C13sus4", Core::Dominant7Sus4),
            ("Csus4(add9)", Core::Sus4),
            ("C5", Core::Power),
        ] {
            assert_eq!(Core::of(Chord::parse(name).unwrap().intervals()), Some(core), "{name}");
        }
        // C, C D♭ or C G♭: no chord.
        assert_eq!(Core::of(PcSet::of(&[0])), None);
        assert_eq!(Core::of(PcSet::of(&[0, 6, 10])), None);
    }

    #[test]
    fn each_note_is_a_cores_own_a_tension_or_a_note_that_doesnt_belong() {
        for core in Core::ALL {
            assert!(core.intervals().intersection(core.tensions()).is_empty(), "{core:?}");
            assert_eq!(core.intervals().union(core.tensions()).union(core.others()), PcSet::ALL);
            assert_eq!(core.class(0), ToneClass::Core);
            // The notes that say which core it is are its own.
            let says = core.third().union(core.seventh()).union(core.fifth());
            assert_eq!(says.difference(core.intervals()), PcSet::EMPTY, "{core:?}");
            for i in 0..12 {
                let class = core.class(i);
                assert_eq!(class == ToneClass::Core, core.intervals().has(i));
                assert_eq!(class == ToneClass::Tension, core.tensions().has(i));
                assert_eq!(core.class(i + 12), class, "round the octave");
            }
        }
        // The design's own examples: under a dominant the ♭3 is a ♯9; under a minor seventh a major third is a wrong
        // note; a ♭9 is a tension on a dominant and a wrong note on a major seventh; C9sus4 is a 7sus4 with a 9.
        assert_eq!(Core::Dominant7.class(3), ToneClass::Tension);
        assert_eq!(Core::Minor7.class(4), ToneClass::Other);
        assert_eq!(Core::Dominant7.class(1), ToneClass::Tension);
        assert_eq!(Core::Major7.class(1), ToneClass::Other);
        assert_eq!(Core::Dominant7Sus4.class(2), ToneClass::Tension);
        // A seventh or sixth a core doesn't have belongs to another core.
        assert_eq!(Core::Major.class(10), ToneClass::Other);
        assert_eq!(Core::Major.class(9), ToneClass::Other);
        assert_eq!(Core::Minor.class(9), ToneClass::Other);
        assert_eq!(Core::Sus4.class(10), ToneClass::Other);
        // A natural 11 over a major third is an avoid note, and so is a second seventh.
        assert_eq!(Core::Dominant7.class(5), ToneClass::Other);
        assert_eq!(Core::Dominant7.class(11), ToneClass::Other);
        // A seventh no other core has is carried: Cmaj7♯5, Cmaj7sus4, C7(no3).
        assert_eq!(Core::Augmented.class(11), ToneClass::Tension);
        assert_eq!(Core::Sus4.class(11), ToneClass::Tension);
        assert_eq!(Core::Power.class(10), ToneClass::Tension);
        // 51 (core, tension) pairs in all, for the tension counts the reader learns.
        assert_eq!(Core::ALL.iter().map(|c| c.tensions().len()).sum::<u32>(), 51);
    }

    #[test]
    fn a_seventh_belongs_to_the_core_that_has_it() {
        for core in Core::ALL {
            let iv = core.intervals();
            for seventh in [10, 11].into_iter().filter(|&s| !iv.has(s)) {
                let class = core.class(seventh);
                if iv.has(10) || iv.has(11) {
                    // Two sevenths clash.
                    assert_eq!(class, ToneClass::Other, "{core:?} {seventh}");
                } else {
                    // C with a B♭ is C7, so the B♭ is wrong in a C major reading; C+ with a B is still C+.
                    let moves = Core::of(iv.with(PitchClass::new(seventh))) != Some(core);
                    assert_eq!(class == ToneClass::Other, moves, "{core:?} {seventh}");
                }
            }
        }
    }

    #[test]
    fn a_chord_moves_on_as_its_group_does() {
        for (k, group) in Group::ALL.into_iter().enumerate() {
            assert_eq!(group.index(), k);
        }
        // The dominants are the cores with a major third's ♭7 or a 4th's ♭7; dim/aug those whose altered fifth says
        // which chord they are (a power chord's fifth is a plain one); every other core moves freely.
        for core in Core::ALL {
            let dominant = core.intervals().has(10) && !core.intervals().has(3) && !core.intervals().has(6);
            let altered = !core.fifth().is_empty() && core != Core::Power;
            let want = if dominant {
                Group::Dominant
            } else if altered {
                Group::DimAug
            } else {
                Group::Other
            };
            assert_eq!(core.group(), want, "{core:?}");
        }
        assert_eq!(Group::ALL.map(Group::name), ["dominant", "dim/aug", "other"]);
    }

    #[test]
    fn the_tensions_are_the_chord_names_own_with_a_few_notes_judged_again() {
        // `chord::role` on each core's own notes, with the notes it calls a tension, and those it is overruled on.
        let mut differ = Vec::new();
        for core in Core::ALL {
            let iv = core.intervals();
            for i in (1..12).filter(|&i| !iv.has(i)) {
                let by_role = role(iv, Family::of(iv), i) == ToneRole::Tension;
                if by_role != core.tensions().has(i) {
                    differ.push((core, i, by_role));
                }
            }
        }
        assert_eq!(
            differ,
            [
                // Cmaj13: `role` counts a major chord's 6 as its seventh even beside a major seventh.
                (Core::Major7, 9, false),
                // Cm6 with an A♭: a semitone under the chord's own 6 (C D E♭ F G A B♭ is its scale).
                (Core::Minor6, 8, true),
                // A seventh no other core has (`a_seventh_belongs_to_the_core_that_has_it`): Cdim(maj7), Cdim7 with a
                // B (the diminished scale's fourth tension), Cmaj7♯5, Cmaj7sus4, Cmaj7sus2, C7(no3), Cmaj7(no3).
                (Core::Diminished7, 11, false),
                (Core::Diminished, 11, false),
                (Core::Augmented, 11, false),
                (Core::Sus4, 11, false),
                // Csus2 with a D♭: a semitone under the chord's own 2.
                (Core::Sus2, 1, true),
                (Core::Sus2, 11, false),
                (Core::Power, 10, false),
                (Core::Power, 11, false),
            ]
        );
    }

    #[test]
    fn the_tables_hold_the_rules_for_every_set_of_notes() {
        for heard in every_set() {
            let fits: CoreSet = Core::ALL.into_iter().filter(|c| c.fits(heard)).collect();
            let fits_rootless: CoreSet = Core::ALL.into_iter().filter(|c| c.fits_rootless(heard)).collect();
            assert_eq!(valid(heard), fits, "{heard}");
            assert_eq!(valid_rootless(heard), fits_rootless, "{heard}");
            // Each table is about one kind of reading.
            assert!(heard.has(0) || valid(heard).is_empty());
            assert!(!heard.has(0) || valid_rootless(heard).is_empty());
            assert_eq!(valid_rootless(heard).bits() & !Core::ROOTLESS.bits(), 0);
        }
    }

    #[test]
    fn the_rules_say_what_the_design_says_for_every_set_of_notes() {
        // The same rules worked out another way, from what `chord::role` says each of the core's notes does: its
        // third, its seventh (or sixth) and an altered fifth must sound; a power chord's fifth; a 7sus4's 4th or 2nd.
        let says = |core: Core| -> PcSet {
            let iv = core.intervals();
            let family = Family::of(iv);
            let mut notes: PcSet = (1..12)
                .filter(|&i| iv.has(i))
                .filter(|&i| match role(iv, family, i) {
                    ToneRole::Third | ToneRole::Seventh => true,
                    ToneRole::Fifth => i != 7,
                    _ => false,
                })
                .map(PitchClass::new)
                .collect();
            if core == Core::Power {
                notes = notes.with(PitchClass::new(7));
            }
            notes
        };
        let tensions = |core: Core| -> PcSet {
            let iv = core.intervals();
            (1..12).filter(|&i| core.class(i) == ToneClass::Tension && !iv.has(i)).map(PitchClass::new).collect()
        };
        for heard in every_set() {
            for core in Core::ALL {
                let mut need = says(core);
                let mut played = heard.contains_all(need);
                if core == Core::Dominant7Sus4 && !played {
                    // C D G B♭: the 2 standing in for the 4th.
                    need = need.without(PitchClass::new(5)).with(PitchClass::new(2));
                    played = heard.contains_all(need);
                }
                assert_eq!(valid(heard).contains(core), heard.has(0) && played, "{core:?} {heard}");
                let rootless = matches!(
                    core,
                    Core::Dominant7
                        | Core::Minor7
                        | Core::Major7
                        | Core::Major6
                        | Core::Minor6
                        | Core::HalfDiminished
                        | Core::Dominant7Sus4
                ) && !heard.has(0)
                    && heard.contains_all(says(core))
                    && !heard.intersection(tensions(core)).is_empty();
                assert_eq!(valid_rootless(heard).contains(core), rootless, "{core:?} rootless {heard}");
                if rootless {
                    assert!(heard.len() >= 3, "{heard}");
                }
            }
        }
    }

    #[test]
    fn every_chord_is_tried_as_its_own_core() {
        // Whatever the notes on a root, the core the chord's name gives it is among the cores tried, so no chord is
        // ruled out before it is scored.
        let mut no_chord = 0;
        for heard in every_set().filter(|s| s.has(0)) {
            match Core::of(heard) {
                Some(core) => assert!(valid(heard).contains(core), "{core:?} {heard}"),
                None => no_chord += 1,
            }
        }
        // The sets with no third, no 4th or 2nd and no fifth: the root with any of D♭, G♭, A♭, A, B♭ and B.
        assert_eq!(no_chord, 64);
    }

    #[test]
    fn hearing_more_notes_never_rules_a_core_out() {
        for heard in every_set() {
            for i in 0..12 {
                let more = heard.with(PitchClass::new(i));
                assert_eq!(valid(heard).bits() & !valid(more).bits(), 0, "{heard} + {i}");
                if i != 0 {
                    assert_eq!(valid_rootless(heard).bits() & !valid_rootless(more).bits(), 0, "{heard} + {i}");
                }
            }
        }
    }

    #[test]
    fn a_core_is_tried_only_when_the_notes_that_say_it_sound() {
        let cores = |root: &str, notes: &str| valid(above(root, notes)).iter().collect::<Vec<_>>();
        use Core::*;
        // A shell: the third and seventh are enough, and the triad is still tried.
        assert_eq!(cores("C", "C E Bb"), [Major, Dominant7]);
        // No ♭5, so no m7b5.
        assert_eq!(cores("C", "C Eb Bb"), [Minor, Minor7]);
        assert_eq!(cores("C", "C Eb Gb Bb"), [Minor, Minor7, HalfDiminished, Diminished]);
        assert_eq!(cores("C", "C Eb Gb A"), [Minor, Minor6, Diminished7, Diminished]);
        // Both thirds: either is tried, and the ♯9 decides when they're scored. A fifth over the root always lets a
        // power chord be tried too; the third then counts against it.
        assert_eq!(cores("C", "C E G Bb Eb"), [Major, Minor, Dominant7, Minor7, Power]);
        // C D G B♭: a 7sus with its 2 in place of its 4th, a sus2 or a power chord; never a sus4.
        assert_eq!(cores("C", "C D G Bb"), [Sus2, Dominant7Sus4, Power]);
        assert_eq!(cores("C", "C G"), [Power]);
        assert_eq!(cores("C", "C E G# B"), [Major, Major7, Augmented]);
        // F A C E is Fmaj7 on F. On D it is Dm9 only once D sounds; a 9 lets the sus chords be tried as well.
        assert_eq!(cores("F", "F A C E"), [Major, Major7, Power]);
        assert!(cores("D", "F A C E").is_empty());
        assert_eq!(cores("D", "D F A C E"), [Minor, Minor7, Sus2, Dominant7Sus4, Power]);
        // A root that doesn't sound is never tried here.
        assert!(cores("C", "E G B D").is_empty());
    }

    #[test]
    fn a_rootless_voicing_is_tried_on_the_root_a_pianist_leaves_out() {
        let rootless = |root: &str, notes: &str| valid_rootless(above(root, notes)).iter().collect::<Vec<_>>();
        let roots = |notes: &str, core: Core| -> Vec<u8> {
            (0..12)
                .filter(|&r| valid_rootless(above("C", notes).above(PitchClass::new(r))).contains(core))
                .map(|r| r as u8)
                .collect()
        };
        use Core::*;
        // Bill Evans's A and B voicings in a ii-V-I in C: Dm9, G13 and Cmaj9 without their roots.
        // G13's voicing is also tried as a G6/9 without its root, where its F is a wrong note.
        assert_eq!(rootless("D", "F A C E"), [Minor7]);
        assert_eq!(rootless("G", "F A B E"), [Dominant7, Major6]);
        assert_eq!(rootless("C", "E G B D"), [Major7]);
        // An upper structure: a D triad over C7's third and seventh (and again C6/9, where the B♭ is wrong).
        assert_eq!(rootless("C", "E Bb D F# A"), [Dominant7, Major6]);
        // C9sus4 without C: its 4th, 7th and 9. Without its 4th too, it isn't tried (D B♭ A is B♭maj7).
        assert_eq!(rootless("C", "F Bb D"), [Dominant7Sus4]);
        assert!(rootless("C", "D Bb A").is_empty());
        // Dm9b5 in a minor ii-V, without its D (and as a Dm9, where the A♭ is wrong).
        assert_eq!(rootless("D", "F Ab C E"), [Minor7, HalfDiminished]);
        // A m7b5 needs its ♭5: without it, it is a m7.
        assert_eq!(rootless("D", "F C E"), [Minor7]);
        // The third and seventh alone are a shell, not a rootless voicing: a tension must sound too.
        assert!(rootless("C", "E Bb").is_empty());
        assert!(rootless("C", "E G Bb").is_empty());
        // A diminished seventh is four 7(♭9) chords without their roots: G, E, C♯ and B♭, and nothing else.
        assert_eq!(roots("B D F Ab", Dominant7), [1, 4, 7, 10]);
        for core in [Minor7, Major7, Major6, Minor6, HalfDiminished, Dominant7Sus4] {
            assert!(roots("B D F Ab", core).is_empty(), "{core:?}");
        }
        // A tritone with a 9 is C9, and also G♭7(♭13), the dominant a tritone away.
        assert_eq!(roots("E Bb D", Dominant7), [0, 6]);
        // A root that sounds is never tried here.
        assert!(rootless("C", "C E Bb D").is_empty());
    }

    #[test]
    fn a_set_of_cores_lists_them_in_order() {
        for bits in 0..=u16::MAX {
            let set = CoreSet(bits);
            let listed: Vec<Core> = set.iter().collect();
            let wanted: Vec<Core> = Core::ALL.into_iter().filter(|&core| set.contains(core)).collect();
            assert_eq!(listed, wanted);
            assert_eq!(set.others().bits(), !bits);
            assert!(Core::ALL.iter().all(|&core| set.contains(core) != set.others().contains(core)));
        }
    }
}
