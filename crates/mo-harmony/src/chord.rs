//! Chords as jazz and gospel players write them: a root, every note in the chord, and the bass under it, so "G13b9",
//! "Bbmaj9", "F#m7b5", "C#dim7" and "Ab7#9#5" all come out right. A chord is made from a typed name
//! ([`Chord::parse`]) or by the chord reader from notes.
//!
//! The names follow the C# app's, which Solomon has used and checked (its tests are carried over below), with three
//! changes he chose on 2026-11-03:
//!
//! - **Altered notes take no brackets**: C7b9, C13#11, C7#9#5, C7b9#11, Cmaj7#11. The other notes a usual name
//!   doesn't cover keep theirs: Cm(maj7), Cm(add9), C7(no3), Csus4(add9), C7(11).
//! - **No 7alt**: an altered dominant's notes are written out (C7#9#5, not C7alt). Typed, 7alt is still read.
//! - **No slash chords**: a chord is named from all its notes, the bass among them ([`Chord::as_written`]). D over C
//!   is D7, F over G is G9sus4, Am over G is Am7, C over E is C, E over C is Cmaj7#5. A chord still keeps its bass,
//!   for what it does and how the music is read; only its name leaves the slash out.
//!
//! A chord here is its pitch classes; how its notes are spelled (C♯ or D♭) is [`crate::spell`]'s job: each note is
//! written by its degree above the root ([`Chord::degree`]), so the root is the only note whose spelling is chosen.
//! With no key, roots are written as the C# app wrote them (flats for black notes except F♯, sharps for diminished
//! chords).

use std::fmt;

use crate::names::parse_note;
use crate::pitch::{PcSet, PitchClass};
use crate::spell::{Degree, Spelled, Tpc};

/// What sort of chord it is, which decides how it behaves: what it resolves to and which notes sit on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Family {
    Major,
    Minor,
    /// A major third and a flat seventh.
    Dominant,
    /// A minor third, a flat fifth and a flat seventh (m7b5).
    HalfDiminished,
    Diminished,
    Augmented,
    /// A fourth or a second in place of the third.
    Sus,
    /// Root and fifth only.
    Power,
}

/// What one note does in a chord.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToneRole {
    Root,
    Third,
    Fifth,
    /// The seventh, or the sixth of a sixth chord.
    Seventh,
    /// A colour the chord can carry: 9, 11, 13 and their altered forms, as the family allows.
    Tension,
    /// A note that doesn't belong: a natural 11 on a major chord, a major seventh on a dominant.
    Avoid,
}

impl Family {
    /// The family of a chord with these intervals above its root.
    pub const fn of(intervals: PcSet) -> Family {
        let m3 = intervals.has(3);
        let maj3 = intervals.has(4);
        let b7 = intervals.has(10);
        let p5 = intervals.has(7);
        let d5 = intervals.has(6);
        let a5 = intervals.has(8);
        if maj3 && b7 {
            // A minor third beside them is a #9.
            Family::Dominant
        } else if maj3 {
            if a5 && !p5 { Family::Augmented } else { Family::Major }
        } else if m3 {
            if d5 && !p5 { if b7 { Family::HalfDiminished } else { Family::Diminished } } else { Family::Minor }
        } else if intervals.has(5) || intervals.has(2) {
            Family::Sus
        } else {
            Family::Power
        }
    }
}

/// Whether a dominant chord is an altered one, as players call it (the C# app wrote it 7alt): a major third and flat
/// seventh, no perfect fifth, no natural 9, 11 or 13 and no major seventh (none of them is in the altered scale), a ♭9
/// or ♯9, and a ♯11 or ♯5. Its name writes its notes out: C7#9#5, C7b9#11.
pub const fn is_altered(intervals: PcSet) -> bool {
    intervals.has(4)
        && intervals.has(10)
        && !intervals.has(7)
        && !intervals.has(2)
        && !intervals.has(5)
        && !intervals.has(9)
        && !intervals.has(11)
        && (intervals.has(1) || intervals.has(3))
        && (intervals.has(6) || intervals.has(8))
}

/// How the note `interval` semitones above the root is written in a chord of this family with these intervals: its
/// degree, as the chord's name has it. Three semitones are a ♭3, or a ♯9 beside a major third; six a ♭5 in a
/// diminished chord, or in a dominant with no fifth and no 9 or 13 (C7b5), else a ♯11 (C9#11, C13#11, C7b9#11); eight
/// a ♯5 in an augmented chord or a dominant with no fifth (C7#5, C7#9#5), else a ♭13; nine a 𝄫7 in a diminished
/// seventh, a 13 over a seventh, else a 6. In a sus chord, two and five semitones are its 2 and 4.
pub const fn degree(intervals: PcSet, family: Family, interval: i32) -> Degree {
    let no_fifth_dominant = matches!(family, Family::Dominant) && !intervals.has(7);
    match PitchClass::new(interval).number() {
        0 => Degree::ROOT,
        1 => Degree::FLAT_NINTH,
        2 => {
            if matches!(family, Family::Sus) && !intervals.has(5) {
                Degree::SECOND
            } else {
                Degree::NINTH
            }
        }
        3 => {
            if intervals.has(4) {
                Degree::SHARP_NINTH
            } else {
                Degree::MINOR_THIRD
            }
        }
        4 => Degree::MAJOR_THIRD,
        5 => {
            if matches!(family, Family::Sus) {
                Degree::FOURTH
            } else {
                Degree::ELEVENTH
            }
        }
        6 => {
            if matches!(family, Family::Diminished | Family::HalfDiminished)
                || (no_fifth_dominant && !has_ninth_or_thirteenth(intervals))
            {
                Degree::FLAT_FIFTH
            } else {
                Degree::SHARP_ELEVENTH
            }
        }
        7 => Degree::FIFTH,
        8 => {
            if matches!(family, Family::Augmented) || no_fifth_dominant {
                Degree::SHARP_FIFTH
            } else {
                Degree::FLAT_THIRTEENTH
            }
        }
        9 => {
            if matches!(family, Family::Diminished) {
                Degree::DIMINISHED_SEVENTH
            } else if intervals.has(10) || intervals.has(11) {
                Degree::THIRTEENTH
            } else {
                Degree::SIXTH
            }
        }
        10 => Degree::MINOR_SEVENTH,
        _ => Degree::MAJOR_SEVENTH,
    }
}

/// Whether a 9 of any kind (♭9, 9, ♯9 beside a major third) or a 13 sounds over the root: then six semitones over a
/// dominant with no fifth are its ♯11, not its ♭5.
const fn has_ninth_or_thirteenth(intervals: PcSet) -> bool {
    intervals.has(1) || intervals.has(2) || intervals.has(3) || intervals.has(9)
}

/// Whether a chord over a bass that's only a bass is written as the sus chord on that bass ([`Chord::as_written`]):
/// the notes over it (`notes`, the bass not among them) hold its 4th and its ♭7 and no third. F A C over G is G9sus4,
/// Bb D F over C is C9sus4; D F# A over C, with no 4th over the C, is D7.
pub const fn sus_on_bass(notes: PcSet, bass: PitchClass) -> bool {
    let over = notes.above(bass);
    over.has(5) && over.has(10) && !over.has(3) && !over.has(4)
}

/// Whether a chord with these intervals above its root has a name players write as it is: it has a third, and its
/// usual name has nothing added and nothing in brackets (but a minor chord's major seventh, whose brackets are its
/// name's own), and names every note (a dominant chord's altered notes aside, which follow it: C7b9). Cmaj7, Cm7, C9,
/// C7b9, Cm7b5, Cmaj7#5 and Cm(maj7) are; Csus2, Cdim(maj7), C(add11) and C6b9 aren't. A chord over a bass that's
/// only a bass is written on the bass when it's one there ([`Chord::as_written`]). It's looked up: every set of
/// intervals is worked out the first time it's needed, from the names' text ([`named_as_it_is`]).
fn written_as_it_is(intervals: PcSet) -> bool {
    static YES: std::sync::OnceLock<Vec<bool>> = std::sync::OnceLock::new();
    let yes = YES.get_or_init(|| (0..4096u16).map(|bits| named_as_it_is(PcSet::from_bits(bits))).collect());
    yes.get(usize::from(intervals.bits())).copied().unwrap_or(false)
}

/// [`written_as_it_is`], worked out from the usual name's text.
fn named_as_it_is(intervals: PcSet) -> bool {
    let family = Family::of(intervals);
    let usual = usual_quality(intervals, family);
    let left = intervals.difference(intervals_of(&usual).unwrap_or_default());
    // A minor chord's major seventh is bracketed as part of its name (m(maj7), m(maj9)), not as a note added.
    let named = if family == Family::Minor { usual.replacen("m(maj", "mmaj", 1) } else { usual };
    let plain = !named.contains('(') && !named.contains("add");
    match family {
        Family::Sus | Family::Power => false,
        Family::Dominant => plain && left.iter().all(|pc| matches!(pc.number(), 1 | 3 | 6 | 8)),
        _ => plain && left.is_empty(),
    }
}

/// How a bass that isn't one of the chord's own notes is written above the root: within the octave, on the plain
/// side (the C under D over C is its ♭7, the G under F over G its 2, the F♯ under C over F♯ its ♯4).
const fn bass_only_degree(interval: u8) -> Degree {
    match interval {
        0 => Degree::ROOT,
        1 => Degree::new(2, -1),
        2 => Degree::new(2, 0),
        3 => Degree::new(3, -1),
        4 => Degree::new(3, 0),
        5 => Degree::new(4, 0),
        6 => Degree::new(4, 1),
        7 => Degree::new(5, 0),
        8 => Degree::new(6, -1),
        9 => Degree::new(6, 0),
        10 => Degree::new(7, -1),
        _ => Degree::new(7, 0),
    }
}

/// What the note `interval` semitones above the root does in a chord of this family with these intervals.
pub const fn role(intervals: PcSet, family: Family, interval: i32) -> ToneRole {
    let i = PitchClass::new(interval).number();
    if i == 0 {
        return ToneRole::Root;
    }
    let p5 = intervals.has(7);
    let b7 = intervals.has(10);
    let maj7 = intervals.has(11);
    match family {
        Family::Major => match i {
            4 => ToneRole::Third,
            7 => ToneRole::Fifth,
            // The 6 of a 6 chord counts as its seventh (a 13 with a maj7 too).
            11 | 9 => ToneRole::Seventh,
            // 9, #11.
            2 | 6 => ToneRole::Tension,
            // #5.
            8 if !p5 => ToneRole::Fifth,
            // Natural 11, b9, #9, b13.
            _ => ToneRole::Avoid,
        },
        Family::Augmented => match i {
            4 => ToneRole::Third,
            8 => ToneRole::Fifth,
            11 | 10 => ToneRole::Seventh,
            2 | 6 => ToneRole::Tension,
            _ => ToneRole::Avoid,
        },
        Family::Minor => match i {
            3 => ToneRole::Third,
            7 => ToneRole::Fifth,
            10 | 11 => ToneRole::Seventh,
            // m6.
            9 if !b7 && !maj7 => ToneRole::Seventh,
            // 9, 11, 13.
            2 | 5 | 9 => ToneRole::Tension,
            // b13 (Aeolian).
            8 if p5 => ToneRole::Tension,
            _ => ToneRole::Avoid,
        },
        Family::Dominant => match i {
            4 => ToneRole::Third,
            7 => ToneRole::Fifth,
            10 => ToneRole::Seventh,
            // #5 stands in for the fifth, and b5 when no 9 or 13 makes it a #11 ([`degree`]).
            8 if !p5 => ToneRole::Fifth,
            6 if !p5 && !has_ninth_or_thirteenth(intervals) => ToneRole::Fifth,
            // b9 9 #9 #11 b13 13.
            1 | 2 | 3 | 6 | 8 | 9 => ToneRole::Tension,
            // Natural 11 under a third, major 7.
            _ => ToneRole::Avoid,
        },
        Family::HalfDiminished => match i {
            3 => ToneRole::Third,
            6 => ToneRole::Fifth,
            10 => ToneRole::Seventh,
            2 | 5 | 8 => ToneRole::Tension,
            _ => ToneRole::Avoid,
        },
        Family::Diminished => match i {
            3 => ToneRole::Third,
            6 => ToneRole::Fifth,
            9 | 11 => ToneRole::Seventh,
            // A whole step above a chord note.
            2 | 5 | 8 => ToneRole::Tension,
            _ => ToneRole::Avoid,
        },
        Family::Sus => match i {
            5 => ToneRole::Third,
            2 if !intervals.has(5) => ToneRole::Third,
            7 => ToneRole::Fifth,
            10 => ToneRole::Seventh,
            11 if !b7 => ToneRole::Seventh,
            // 9, 13, and the b9 of the gospel and Phrygian sus.
            2 | 9 | 1 => ToneRole::Tension,
            _ => ToneRole::Avoid,
        },
        Family::Power => {
            if i == 7 {
                ToneRole::Fifth
            } else {
                ToneRole::Avoid
            }
        }
    }
}

/// A chord: its root, its own notes, and the bass under it. When the bass is the root, the chord is in root position.
///
/// The bass may be one of the chord's own notes (C over E, G7 over F) or only a bass (the C under D, the Lydian sound):
/// D over C and D7 over C sound the same four notes, and the chord keeps which it is, for what each note does and how
/// the music is read (chord labels write them D/C and D7/C). Its name has no slash: it's written from all its notes,
/// the bass among them, so both are D7 ([`Chord::as_written`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    root: PitchClass,
    notes: PcSet,
    bass: PitchClass,
}

impl Chord {
    /// A chord on `root` with these notes of its own over `bass`. The notes may leave the root out, as a pianist's
    /// rootless voicing does, and may leave the bass out when it's only a bass.
    pub const fn new(root: PitchClass, notes: PcSet, bass: PitchClass) -> Chord {
        Chord { root, notes, bass }
    }

    /// A chord in root position from its intervals above the root: `Chord::of(D, PcSet::of(&[0, 3, 7, 10]))` is Dm7.
    pub const fn of(root: PitchClass, intervals: PcSet) -> Chord {
        Chord::new(root, intervals.on(root).with(root), root)
    }

    pub const fn root(self) -> PitchClass {
        self.root
    }

    /// The chord's own notes, as pitch classes: the bass is among them unless it's only a bass.
    pub const fn notes(self) -> PcSet {
        self.notes
    }

    /// Every note that sounds: the chord's own and the bass.
    pub const fn sounding(self) -> PcSet {
        self.notes.with(self.bass)
    }

    pub const fn bass(self) -> PitchClass {
        self.bass
    }

    /// The notes from the root up: bit 0 the root, bit 4 a major third, bit 10 a flat seventh.
    pub const fn intervals(self) -> PcSet {
        self.notes.above(self.root)
    }

    pub const fn family(self) -> Family {
        Family::of(self.intervals())
    }

    /// Whether the note this many semitones above the root is in the chord.
    pub const fn has(self, interval: i32) -> bool {
        self.intervals().has(interval)
    }

    /// What the note `interval` semitones above the root does in this chord.
    pub const fn role(self, interval: i32) -> ToneRole {
        role(self.intervals(), self.family(), interval)
    }

    /// The degree the note `interval` semitones above the root has in this chord: [`degree`].
    pub const fn degree(self, interval: i32) -> Degree {
        degree(self.intervals(), self.family(), interval)
    }

    /// The chord's own notes by degree, from the root up: Cmaj9 is 1 3 5 7 9.
    pub fn degrees(self) -> Vec<Degree> {
        let (intervals, family) = (self.intervals(), self.family());
        let mut all: Vec<Degree> =
            (0..12).filter(|&i| intervals.has(i)).map(|i| degree(intervals, family, i)).collect();
        all.sort_unstable();
        all
    }

    /// The bass's degree above the root: its own degree when it's one of the chord's notes (the 3 of A7 over C#), else
    /// a plain one within the octave (the ♭7 of D over C).
    pub const fn bass_degree(self) -> Degree {
        let interval = self.bass.above(self.root);
        if self.notes.contains(self.bass) { self.degree(interval as i32) } else { bass_only_degree(interval) }
    }

    /// The chord as it's written, with no slash (Solomon, 2026-11-03): from all its notes, the bass among them. A chord
    /// whose bass is its root or another of its notes is written as it is, in root position (C over E is C). A bass
    /// that's only a bass is one more of its notes, and the chord is written on its own root (D over C is D7, Am over
    /// G is Am7, G over C is G(add11), Eb over E is Ebaddb9), except in two cases, where it's written on the bass:
    ///
    /// - the notes over the bass hold its 4th and its ♭7 and no third, a sus chord on the bass ([`sus_on_bass`]): F
    ///   over G is G9sus4, Fm9 over Bb is Bb13sus4, Am over B is B7sus4b9;
    /// - the notes make a chord on the bass whose name players write as it is ([`written_as_it_is`]): E over C is
    ///   Cmaj7#5, Am over F is Fmaj7, G over E is Em7, Bdim7 over G is G7b9, G+ over C is Cm(maj7).
    ///
    /// Its bass stays where it is.
    pub fn as_written(self) -> Chord {
        if self.bass.number() == self.root.number() || self.notes.contains(self.bass) {
            return self;
        }
        let notes = self.notes.with(self.bass);
        let on_bass = Chord { root: self.bass, notes, bass: self.bass };
        if sus_on_bass(self.notes, self.bass) || written_as_it_is(on_bass.intervals()) {
            on_bass
        } else {
            Chord { root: self.root, notes, bass: self.bass }
        }
    }

    /// The quality written after the root ([`Chord::as_written`]): "maj9", "m7b5", "13#11", "7#9#5", "9sus4".
    pub fn quality(self) -> String {
        let written = self.as_written();
        quality_of(written.intervals(), written.family())
    }

    /// The root as [`Chord::name`] writes it: sharp for a diminished chord, else flat except F#.
    pub fn root_name(self) -> String {
        Spelled::plain(self.as_written()).root().name()
    }

    /// "Dm9", "G13b9", "Cmaj7", "A7": written as charts write it when no key says otherwise, from all its notes with no
    /// slash ([`Chord::as_written`]). In a key, see [`crate::spell::spell_in_key`].
    pub fn name(self) -> String {
        Spelled::plain(self.as_written()).name()
    }

    /// A key to the chord's name ([`Chord::name`]): two chords are named the same, however their roots are spelled,
    /// just when their keys are the same. C over E and C over G are both C, and Bmaj13 is Bmaj13 with or without its 9
    /// (a name stands for notes it leaves out). It's quick: the name's quality is looked up, not written out.
    pub fn name_key(self) -> (PitchClass, u16) {
        let written = self.as_written();
        let quality = quality_numbers().get(usize::from(written.intervals().bits())).copied();
        (written.root(), quality.unwrap_or(u16::MAX))
    }

    /// The chord as chord datasets and labels write it, its bass after a slash: its own notes' name, then a bass that
    /// isn't its root (D/C, D7/C, C/E, F/G). Never shown in the app, whose names have no slash ([`Chord::name`]); it's
    /// for the bench and the hand cases, which tell D over C from D7 over C as the datasets do.
    pub fn slash_name(self) -> String {
        let spelled = Spelled::plain(self);
        let name = format!("{}{}", spelled.root().name(), quality_of(self.intervals(), self.family()));
        if self.bass.number() == self.root.number() {
            name
        } else {
            format!("{name}/{}", spelled.bass().without_doubles().name())
        }
    }

    /// Every way the name is written, for searching: "C#m7" and "Dbm7".
    pub fn spellings(self) -> Vec<String> {
        let written = self.as_written();
        let mut all = vec![self.name()];
        // All sharps (F up to A#), then all flats (Gb up to B).
        for lowest in [-1, -6] {
            all.push(Spelled::new(written, Tpc::in_window(written.root, lowest)).name());
        }
        let mut seen = std::collections::HashSet::new();
        all.retain(|n| seen.insert(n.clone()));
        all
    }

    /// The third and seventh (or sixth): the two notes that say what a chord is, and what jazz voice leading moves.
    pub fn guide_tones(self) -> PcSet {
        [3, 4, 5, 9, 10, 11]
            .into_iter()
            .filter(|&i| self.has(i) && matches!(self.role(i), ToneRole::Third | ToneRole::Seventh))
            .map(|i| self.root.up(i))
            .collect()
    }

    /// The chord's own notes (root, third, fifth, seventh or sixth), without its tensions, the root always among them.
    pub fn core_tones(self) -> PcSet {
        (0..12)
            .filter(|&i| {
                self.has(i)
                    && matches!(self.role(i), ToneRole::Root | ToneRole::Third | ToneRole::Fifth | ToneRole::Seventh)
            })
            .map(|i| self.root.up(i))
            .collect::<PcSet>()
            .with(self.root)
    }

    /// The chord moved up (down when negative) by this many semitones.
    pub const fn transposed(self, semitones: i32) -> Chord {
        Chord { root: self.root.up(semitones), notes: self.notes.transposed(semitones), bass: self.bass.up(semitones) }
    }

    /// The same chord with its root in the bass (for a part with no bassline of its own).
    pub const fn root_position(self) -> Chord {
        Chord { root: self.root, notes: self.notes.with(self.root), bass: self.root }
    }

    /// A chord from its name as typed: "Dm9", "G7b9#11", "G7(b9,#11)", "G7alt", "C6/9", "F#m7b5", "Bbmaj7#11",
    /// "Ebdim7", "Ab13sus", "Dø", "C°7", "CΔ9", "C-7", and with a bass, "C/E" and "D/C" (a chord label's slash: the
    /// chord keeps the bass, and is written from all its notes, [`Chord::as_written`]). `None` when it can't be read.
    pub fn parse(text: &str) -> Option<Chord> {
        let text = text.trim();
        let (root, used) = parse_note(text)?;
        let mut rest = text.get(used..)?;
        let mut bass = root;
        // "6/9" is a chord, not a bass note.
        if let Some(slash) = rest.rfind('/')
            && rest.get(slash + 1..).and_then(|after| after.chars().next()).is_some_and(|c| !c.is_ascii_digit())
        {
            let after = rest.get(slash + 1..)?;
            let (b, bass_used) = parse_note(after)?;
            if bass_used != after.len() {
                return None;
            }
            bass = b;
            rest = rest.get(..slash)?;
        }
        let intervals = intervals_of(rest)?;
        Some(Chord::new(root, intervals.on(root), bass))
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
    }
}

/// A number for each set of intervals above a root (by its bits, [`PcSet::bits`]), the same for two sets just when
/// their qualities are written the same ([`quality_of`]), for [`Chord::name_key`]. Worked out the first time it's
/// needed.
fn quality_numbers() -> &'static [u16] {
    static NUMBERS: std::sync::OnceLock<Vec<u16>> = std::sync::OnceLock::new();
    NUMBERS.get_or_init(|| {
        let mut seen = std::collections::HashMap::new();
        (0..4096u16)
            .map(|bits| {
                let intervals = PcSet::from_bits(bits);
                let next = u16::try_from(seen.len()).unwrap_or(u16::MAX);
                *seen.entry(quality_of(intervals, Family::of(intervals))).or_insert(next)
            })
            .collect()
    })
}

/// The quality as written after the root, for a chord of this family with these intervals above its root.
///
/// Every note the chord has is in its name: the usual name of its kind of chord ([`usual_quality`]), then each altered
/// note that doesn't cover, by its degree with no brackets (Solomon, 2026-11-03: "C7b9#11", "Cmaj7#11", "Cm7b5b13"),
/// then the other notes it doesn't cover, in brackets ("C7(11)", "Cdim7(9)", "C7(maj7)"). An altered note that would
/// follow a letter or a sign, or the root itself, starts with "add", so none reads as the root's own sharp or flat:
/// "Caddb9", not "Cb9"; "Cmadd#11", "C+add#11". The name may still stand for notes the chord leaves out, as charts do:
/// the fifth, the 9 under an 11 or 13, and the 11 of a minor 13.
pub fn quality_of(intervals: PcSet, family: Family) -> String {
    let mut q = usual_quality(intervals, family);
    let left = intervals.difference(intervals_of(&q).unwrap_or_default());
    // The other notes take "add" when the chord has no seventh, as in "C(add11)" beside "C7(11)".
    let add = !(intervals.has(10) || intervals.has(11) || (matches!(family, Family::Diminished) && intervals.has(9)));
    let mut other = Vec::new();
    for interval in left.iter().map(|pc| i32::from(pc.number())) {
        match written_tone(intervals, family, interval, add) {
            Some(Tone::Altered(tone)) => {
                if !q.ends_with(|c: char| c.is_ascii_digit() || c == ')') {
                    q.push_str("add");
                }
                q.push_str(tone);
            }
            Some(Tone::Other(tone)) => other.push(tone),
            None => {}
        }
    }
    if !other.is_empty() {
        q.push('(');
        q.push_str(&other.join(","));
        q.push(')');
    }
    q
}

/// A note the usual name leaves out, as it's written after it ([`written_tone`]).
enum Tone {
    /// An altered note: b9, #9, #11 or b5, b13 or #5, written with no brackets.
    Altered(&'static str),
    /// Any other: 9, 11, 13 (or 6), b7, maj7, written in brackets.
    Other(&'static str),
}

/// How a note the usual name leaves out is written after it, so that reading the name back gives the note: its degree
/// ([`degree`]), and for a note that isn't altered, with "add" in front when `add`. `None` for the root, the thirds
/// and the fifth, which every usual name covers.
fn written_tone(intervals: PcSet, family: Family, interval: i32, add: bool) -> Option<Tone> {
    let other = |with_add: &'static str, plain: &'static str| Some(Tone::Other(if add { with_add } else { plain }));
    match interval {
        1 => Some(Tone::Altered("b9")),
        2 => other("add9", "9"),
        // A minor third that isn't the chord's third is a #9.
        3 => Some(Tone::Altered("#9")),
        5 => other("add11", "11"),
        6 => Some(Tone::Altered(if degree(intervals, family, 6).alter() < 0 { "b5" } else { "#11" })),
        8 => Some(Tone::Altered(if degree(intervals, family, 8).alter() > 0 { "#5" } else { "b13" })),
        9 => other("add6", "13"),
        10 => Some(Tone::Other("b7")),
        11 => Some(Tone::Other("maj7")),
        _ => None,
    }
}

/// The usual name of the quality, from the notes that decide it and the natural ones it names (9, 11, 13);
/// [`quality_of`] adds what it leaves out, the altered notes among them.
fn usual_quality(intervals: PcSet, family: Family) -> String {
    let has = |i| intervals.has(i);
    let (n9, n11, n13) = (has(2), has(5), has(9));
    let (p5, b7, maj7) = (has(7), has(10), has(11));
    match family {
        Family::Major => {
            if !p5 && has(8) {
                return if maj7 { "maj7#5" } else { "+" }.into();
            }
            let mut q = String::from(if maj7 {
                if n13 {
                    "maj13"
                } else if n9 {
                    "maj9"
                } else {
                    "maj7"
                }
            } else if n13 {
                if n9 { "6/9" } else { "6" }
            } else if n9 {
                "add9"
            } else {
                ""
            });
            if n11 {
                q.push_str("(add11)");
            }
            q
        }
        Family::Augmented => {
            if maj7 {
                "maj7#5"
            } else if b7 {
                "7#5"
            } else {
                "+"
            }
        }
        .into(),
        Family::Minor => String::from(if maj7 {
            if n9 { "m(maj9)" } else { "m(maj7)" }
        } else if b7 {
            match (n9, n11, n13) {
                (_, true, true) | (true, false, true) => "m13",
                (_, true, false) => "m11",
                (false, false, true) => "m7(13)",
                (true, false, false) => "m9",
                (false, false, false) => "m7",
            }
        } else if n13 {
            if n9 { "m6/9" } else { "m6" }
        } else {
            match (n9, n11) {
                (true, true) => "m(add9,11)",
                (true, false) => "m(add9)",
                (false, true) => "m(add11)",
                (false, false) => "m",
            }
        }),
        // An altered dominant's notes are written out after it, as any chord's are: C7#9#5, not C7alt.
        Family::Dominant => if n13 {
            "13"
        } else if n9 {
            "9"
        } else {
            "7"
        }
        .into(),
        Family::HalfDiminished => if n11 {
            "m11b5"
        } else if n9 {
            "m9b5"
        } else {
            "m7b5"
        }
        .into(),
        Family::Diminished => match (has(9), maj7) {
            (true, true) => "dim7(maj7)",
            (true, false) => "dim7",
            (false, true) => "dim(maj7)",
            (false, false) => "dim",
        }
        .into(),
        Family::Sus => {
            let sus = if n11 { "sus4" } else { "sus2" };
            if b7 {
                if n13 {
                    format!("13{sus}")
                } else if n9 && n11 {
                    "9sus4".into()
                } else {
                    format!("7{sus}")
                }
            } else if maj7 {
                format!("maj7{sus}")
            } else if n11 && n9 {
                "sus4(add9)".into()
            } else {
                sus.into()
            }
        }
        // A seventh over a bare fifth: "C7(no3)", as rock charts write it.
        Family::Power if b7 => "7(no3)".into(),
        Family::Power if maj7 => "maj7(no3)".into(),
        Family::Power => "5".into(),
    }
}

/// The intervals a typed quality means: "", "m", "7", "maj9", "m7b5", "dim7", "13#11", "13(#11)", "7#9#5", "7alt",
/// "9sus4", "6/9", "add9", "addb9", "+", "ø", "°7", "Δ", "-7". `None` when it can't be read.
pub fn intervals_of(quality: &str) -> Option<PcSet> {
    let q: String = quality
        .chars()
        .filter(|c| *c != ' ')
        .collect::<String>()
        .replace('♭', "b")
        .replace('♯', "#")
        .replace(['Δ', '^'], "maj");
    let mut read = Reader { q: &q, at: 0 };
    // A major triad unless told otherwise.
    let mut iv = PcSet::of(&[0, 4, 7]);
    let set = |iv: &mut PcSet, i: &[i32]| *iv = iv.union(PcSet::of(i));
    let clear = |iv: &mut PcSet, i: &[i32]| *iv = iv.difference(PcSet::of(i));
    // 9, 11 and 13 bring the notes below them (an 11 chord leaves the third out, as players do; a 13 leaves the 11,
    // except on a minor chord, where the 11 doesn't clash). Any other number after a seventh chord's quality is a typo.
    let extend = |iv: &mut PcSet, n: u32, major7: bool| -> Option<()> {
        if !matches!(n, 7 | 9 | 11 | 13) {
            return None;
        }
        set(iv, &[if major7 { 11 } else { 10 }]);
        if n >= 9 {
            set(iv, &[2]);
        }
        if n >= 11 {
            set(iv, &[5]);
        }
        if n == 13 {
            set(iv, &[9]);
            if !iv.has(3) {
                clear(iv, &[5]);
            }
        }
        Some(())
    };

    if read.take("maj") || read.take("Maj") || read.take("M") {
        match read.number() {
            // "Cmaj6" and "CM6" are plain 6 chords, as "Cmaj6/9" is a 6/9.
            6 => set(&mut iv, &[9]),
            0 => extend(&mut iv, 7, true)?,
            n => extend(&mut iv, n, true)?,
        }
    } else if read.take("m7b5") || read.take("ø") || read.take("-7b5") || read.take("min7b5") {
        iv = PcSet::of(&[0, 3, 6, 10]);
        read.take("7");
    } else if read.take("dim") || read.take("°") || read.take("o") {
        iv = PcSet::of(&[0, 3, 6]);
        if read.take("7") {
            set(&mut iv, &[9]);
        }
    } else if read.take("aug") || read.take("+") {
        clear(&mut iv, &[7]);
        set(&mut iv, &[8]);
        match read.number() {
            0 => {}
            n => extend(&mut iv, n, false)?,
        }
    } else if read.take("min") || read.take("mi") || read.take("m") || read.take("-") {
        clear(&mut iv, &[4]);
        set(&mut iv, &[3]);
        // "Cm(maj7)".
        read.take("(");
        if read.take("maj") || read.take("M") {
            match read.number() {
                0 => extend(&mut iv, 7, true)?,
                n => extend(&mut iv, n, true)?,
            }
        } else {
            match read.number() {
                0 => {}
                6 => set(&mut iv, &[9]),
                n => extend(&mut iv, n, false)?,
            }
        }
    } else {
        match read.number() {
            0 => {}
            5 => iv = PcSet::of(&[0, 7]),
            6 => set(&mut iv, &[9]),
            2 => set(&mut iv, &[2]),
            // "C4" is a sus4, as some charts write it.
            4 => iv = PcSet::of(&[0, 5, 7]),
            n => {
                extend(&mut iv, n, false)?;
                // "C11" is a sus chord with a 9: C, F, Bb, D.
                if n == 11 {
                    clear(&mut iv, &[4]);
                }
            }
        }
    }
    // Then any of: 6/9, sus, sus2, sus4, add9, alt, b9, #9, #11, b13, b5, #5, 11, 13, omit3.
    while !read.done() {
        if read.take("(") || read.take(")") || read.take(",") {
        } else if read.take("/9") {
            set(&mut iv, &[2]);
        } else if read.take("sus2") {
            clear(&mut iv, &[3, 4]);
            set(&mut iv, &[2]);
        } else if read.take("sus4") || read.take("sus") {
            clear(&mut iv, &[3, 4]);
            set(&mut iv, &[5]);
        } else if read.take("add") {
            let added = if read.take("b9") {
                1
            } else if read.take("#9") {
                3
            } else if read.take("#11") {
                6
            } else if read.take("b13") {
                8
            } else {
                match read.number() {
                    2 | 9 => 2,
                    4 | 11 => 5,
                    6 | 13 => 9,
                    _ => return None,
                }
            };
            set(&mut iv, &[added]);
        } else if read.take("alt") {
            // Every alteration: b9, #9, #11 and b13.
            clear(&mut iv, &[7, 2, 9]);
            set(&mut iv, &[10, 1, 3, 6, 8]);
        } else if read.take("maj7") || read.take("maj") {
            // "dim(maj7)", "dim7(maj7)", as the app names them.
            set(&mut iv, &[11]);
        } else if read.take("b9") {
            set(&mut iv, &[1]);
        } else if read.take("b7") {
            // A flat seventh beside a major one, as "Cm(maj7)(b7)" has it.
            set(&mut iv, &[10]);
        } else if read.take("#9") {
            set(&mut iv, &[3]);
        } else if read.take("#11") {
            set(&mut iv, &[6]);
        } else if read.take("b13") {
            set(&mut iv, &[8]);
        } else if read.take("b5") {
            clear(&mut iv, &[7]);
            set(&mut iv, &[6]);
        } else if read.take("#5") {
            clear(&mut iv, &[7]);
            set(&mut iv, &[8]);
        } else if read.take("13") {
            set(&mut iv, &[9]);
        } else if read.take("11") {
            set(&mut iv, &[5]);
        } else if read.take("9") {
            set(&mut iv, &[2]);
        } else if read.take("7") {
            if !iv.has(11) {
                set(&mut iv, &[10]);
            }
        } else if read.take("6") {
            set(&mut iv, &[9]);
        } else if read.take("no3") || read.take("omit3") {
            clear(&mut iv, &[3, 4]);
        } else {
            return None;
        }
    }
    Some(iv)
}

/// Reads a chord quality from the left.
struct Reader<'a> {
    q: &'a str,
    at: usize,
}

impl Reader<'_> {
    fn rest(&self) -> &str {
        self.q.get(self.at..).unwrap_or_default()
    }

    fn done(&self) -> bool {
        self.rest().is_empty()
    }

    /// Takes `s` when the rest starts with it.
    fn take(&mut self, s: &str) -> bool {
        let found = self.rest().starts_with(s);
        if found {
            self.at += s.len();
        }
        found
    }

    /// A chord number: 11 and 13 are the only two-digit ones, so "69" is a 6 then a 9. 0 when there's none.
    fn number(&mut self) -> u32 {
        if self.take("11") {
            return 11;
        }
        if self.take("13") {
            return 13;
        }
        match self.rest().chars().next().and_then(|c| c.to_digit(10)) {
            Some(d) => {
                self.at += 1;
                d
            }
            None => 0,
        }
    }
}

/// A usual name as a lead sheet writes it when room is short ([`Chord::short_quality`]): Δ for maj, ° for dim, ø for
/// m7b5, sus for sus4, and nothing in brackets but a major seventh, which says what the chord is: "m(maj7)" is "mΔ7".
fn shorten(quality: &str) -> String {
    // "(maj7)" and "(maj9)" come out of their brackets; every other bracket goes, with what's in it.
    let mut kept = String::new();
    let mut rest = quality;
    while let Some(open) = rest.find('(') {
        kept.push_str(rest.get(..open).unwrap_or_default());
        let inside = rest.get(open + 1..).unwrap_or_default();
        let close = inside.find(')').unwrap_or(inside.len());
        let group = inside.get(..close).unwrap_or_default();
        if group.strip_prefix("maj").is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())) {
            kept.push_str(group);
        }
        rest = inside.get(close + 1..).unwrap_or_default();
    }
    kept.push_str(rest);
    kept.replace("m7b5", "ø")
        .replace("m9b5", "ø9")
        .replace("m11b5", "ø11")
        .replace("dim", "°")
        .replace("maj", "Δ")
        .replace("sus4", "sus")
}

impl Chord {
    /// The quality the way a lead sheet writes it when room is short: its usual name, with no tensions after it, Δ for
    /// maj, ° for dim, ø for m7b5 and sus for sus4. Cmaj7 is "Δ7", C13b9 "13", C7(11) "7", Cdim7 "°7", Cm(maj7) "mΔ7".
    pub fn short_quality(self) -> String {
        let written = self.as_written();
        shorten(&usual_quality(written.intervals(), written.family()))
    }

    /// The least of the quality that still says what kind of chord it is, for when even the short form won't fit: "m"
    /// for a minor chord, ° for diminished, ø for half-diminished, + for augmented, and nothing for major and dominant
    /// ones.
    pub fn least_quality(self) -> String {
        let q = self.short_quality();
        match q.chars().next() {
            Some('m') => "m".into(),
            Some(c @ ('°' | 'ø' | '+')) => c.into(),
            _ => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(name: &str) -> Chord {
        Chord::parse(name).unwrap_or_else(|| panic!("{name} doesn't read"))
    }

    #[test]
    fn a_typed_chord_reads_back_to_the_same_name() {
        for name in [
            "Dm9",
            "G13",
            "Cmaj9",
            "Bm7b5",
            "C#dim7",
            "C6/9",
            "F#m11",
            "Ab13#11",
            "G9sus4",
            "Bbmaj7",
            "Cm(maj7)",
            "E7b9",
            "Db7#9",
            "Adim(maj7)",
            "C#dim7(maj7)",
            "C7#9#5",
            "C7b9#11",
            "F#9#11",
            "Eb13b9",
            "Cmaj7#11",
            "Cmaj7#5",
            "Caddb9",
            "Cmadd#11",
            "C+add#11",
            "Bm7b5b13",
            "G7sus4b9",
            "C7(11)",
        ] {
            assert_eq!(chord(name).name(), name);
        }
    }

    #[test]
    fn chord_spellings_players_type_are_understood() {
        for (typed, name) in [
            ("CΔ9", "Cmaj9"),
            ("D-7", "Dm7"),
            ("Bø", "Bm7b5"),
            ("C°7", "Cdim7"),
            ("C11", "C9sus4"),
            ("Gsus", "Gsus4"),
            // Brackets are still read, and 7alt is every alteration.
            ("F#7(b9)", "F#7b9"),
            ("Bb7(#9)", "Bb7#9"),
            ("G7(b9,#11)", "G7b9#11"),
            ("C13(#11)", "C13#11"),
            ("A7alt", "A7b9#9#11#5"),
            ("E♭maj7", "Ebmaj7"),
            // A slash is read as the bass, and the name folds it in.
            ("Bbmaj7/D", "Bbmaj7"),
            ("D/C", "D7"),
        ] {
            assert_eq!(chord(typed).name(), name, "{typed}");
        }
    }

    #[test]
    fn every_name_the_app_writes_can_be_typed_back() {
        // Every set of three or more notes on a root, named as the app names it (the chord lane, the chart box).
        let mut unread = Vec::new();
        let mut renamed = Vec::new();
        for bits in (1..4096u16).step_by(2) {
            let iv = PcSet::from_bits(bits);
            if iv.len() < 3 {
                continue;
            }
            let c = Chord::of(PitchClass::C, iv);
            let name = c.name();
            match Chord::parse(&name) {
                None => unread.push(name),
                Some(back) if back.name() != name => renamed.push(format!("{name} -> {}", back.name())),
                Some(_) => {}
            }
        }
        unread.dedup();
        assert!(unread.is_empty(), "{unread:?}");
        renamed.sort();
        renamed.dedup();
        assert!(renamed.is_empty(), "{} names change when typed back: {renamed:?}", renamed.len());
    }

    #[test]
    fn a_crowded_chord_lane_writes_qualities_the_way_a_lead_sheet_does() {
        for (name, shorter, least) in [
            ("Cmaj7", "Δ7", ""),
            ("Dm7", "m7", "m"),
            ("F#m7b5", "ø", "ø"),
            ("C#dim7", "°7", "°"),
            ("G7b9", "7", ""),
            ("G13#11", "13", ""),
            ("G7(11)", "7", ""),
            ("G7sus4", "7sus", ""),
            ("Cmaj9", "Δ9", ""),
            ("Cm(maj7)", "mΔ7", "m"),
            ("Cm(maj9)", "mΔ9", "m"),
            ("Cmaj7#5", "Δ7#5", ""),
            ("C+", "+", "+"),
        ] {
            let c = chord(name);
            assert_eq!(c.short_quality(), shorter, "{name}");
            assert_eq!(c.least_quality(), least, "{name}");
            assert_eq!(format!("{}{}", c.root_name(), c.quality()), name);
        }
        // Over a bass that's only a bass, as the chord is written: D over C is D7, F over G is G9sus4.
        assert_eq!((chord("D/C").short_quality(), chord("D/C").quality()), ("7".into(), "7".into()));
        assert_eq!((chord("F/G").root_name(), chord("F/G").short_quality()), ("G".into(), "9sus".into()));
    }

    #[test]
    fn altered_notes_are_written_out_with_no_brackets() {
        let c = |iv: &[i32]| Chord::of(PitchClass::C, PcSet::of(iv)).name();
        // Solomon's (2026-11-03): no brackets on altered notes, and no 7alt.
        assert_eq!(c(&[0, 4, 7, 10, 2, 9, 6]), "C13#11");
        assert_eq!(c(&[0, 4, 7, 10, 2, 6]), "C9#11");
        assert_eq!(c(&[0, 4, 10, 3, 8]), "C7#9#5");
        assert_eq!(c(&[0, 4, 10, 1, 6]), "C7b9#11");
        assert_eq!(c(&[0, 4, 10, 1, 8]), "C7b9#5");
        assert_eq!(c(&[0, 4, 10, 1, 3, 6, 8]), "C7b9#9#11#5");
        // With no fifth, six semitones are a b5 unless a 9 or a 13 sounds, and eight are a #5.
        assert_eq!(c(&[0, 4, 10, 6]), "C7b5");
        assert_eq!(c(&[0, 4, 10, 2, 6]), "C9#11");
        assert_eq!(c(&[0, 4, 10, 9, 6]), "C13#11");
        assert_eq!(c(&[0, 4, 10, 6, 8]), "C7b5#5");
        // With the fifth sounding, eight semitones are a b13.
        assert_eq!(c(&[0, 4, 7, 10, 1, 8]), "C7b9b13");
        assert_eq!(c(&[0, 4, 7, 10, 3, 6]), "C7#9#11");
        // The other notes a usual name leaves out keep their brackets, after the altered ones.
        assert_eq!(c(&[0, 4, 10, 1, 8, 5]), "C7b9#5(11)");
        assert_eq!(c(&[0, 4, 7, 10, 3, 5]), "C7#9(11)");
        // An altered note after a letter or a sign takes "add", so it can't be read as the root's sharp or flat.
        assert_eq!(c(&[0, 4, 7, 1]), "Caddb9");
        assert_eq!(c(&[0, 3, 7, 6]), "Cmadd#11");
        assert_eq!(c(&[0, 4, 8, 6]), "C+add#11");
        // After a number it follows straight on.
        assert_eq!(c(&[0, 4, 7, 11, 6]), "Cmaj7#11");
        assert_eq!(c(&[0, 3, 6, 10, 8]), "Cm7b5b13");
        assert_eq!(c(&[0, 5, 7, 10, 1]), "C7sus4b9");
        // Typed, 7alt still stands for every alteration.
        assert_eq!(chord("C7alt").notes(), PcSet::of(&[0, 4, 10, 1, 3, 6, 8]));
    }

    #[test]
    fn every_note_is_in_the_name() {
        // For every set of notes on a root, reading its name back gives every note in it. The name may stand for a
        // few notes the chord leaves out, as charts do: the fifth, the 9 under an 11 or 13, and the 11 of a minor 13;
        // never a third or a seventh the chord doesn't have.
        for bits in (1..4096u16).step_by(2) {
            let c = Chord::of(PitchClass::C, PcSet::from_bits(bits));
            let name = c.name();
            let back = chord(&name);
            assert!(c.notes().difference(back.notes()).is_empty(), "{name} leaves out some of {}", c.notes());
            let added = back.notes().difference(c.notes());
            assert!(added.difference(PcSet::of(&[2, 5, 7])).is_empty(), "{name} adds {added} to {}", c.notes());
        }
        let c = |iv: &[i32]| Chord::of(PitchClass::C, PcSet::of(iv)).name();
        assert_eq!(c(&[0, 4, 7, 8]), "Caddb13");
        assert_eq!(c(&[0, 4, 7, 11, 8]), "Cmaj7b13");
        assert_eq!(c(&[0, 4, 10, 11]), "C7(maj7)");
        assert_eq!(c(&[0, 3, 8]), "Cmaddb13");
        assert_eq!(c(&[0, 3, 7, 10, 11]), "Cm(maj7)(b7)");
        assert_eq!(c(&[0, 3, 5, 9]), "Cm6(add11)");
        assert_eq!(c(&[0, 3, 10, 2, 5, 9]), "Cm13");
        assert_eq!(c(&[0, 3, 6, 9, 2]), "Cdim7(9)");
        assert_eq!(c(&[0, 3, 6, 10, 1]), "Cm7b5b9");
        assert_eq!(c(&[0, 4, 8, 2]), "C+(add9)");
        assert_eq!(c(&[0, 7, 10]), "C7(no3)");
        assert_eq!(c(&[0, 2]), "Csus2");
    }

    #[test]
    fn numbers_a_chord_cant_have_are_refused() {
        for (typed, name) in [("CM6", "C6"), ("Cmaj6", "C6"), ("Cmaj6/9", "C6/9"), ("C4", "Csus4"), ("C2", "Cadd9")] {
            assert_eq!(chord(typed).name(), name, "{typed}");
        }
        for typed in ["C1", "C3", "C8", "C10", "Cmaj2", "Cmaj5", "Cm5", "Cm8", "C+5", "Cm(maj6)"] {
            assert_eq!(Chord::parse(typed), None, "{typed}");
        }
    }

    #[test]
    fn every_note_is_spelled_as_the_name_says() {
        // For every set of notes on a root, the degree each note gets agrees with what the name shows.
        for bits in (1..4096u16).step_by(2) {
            let c = Chord::of(PitchClass::C, PcSet::from_bits(bits));
            let q = c.quality();
            for i in 0..12 {
                if c.has(i) {
                    assert_eq!(c.degree(i).interval(), i, "{} {i}", c.name());
                }
            }
            let says = |what: &str| q.contains(what);
            if says("#11") {
                assert_eq!(c.degree(6), Degree::SHARP_ELEVENTH, "{}", c.name());
            } else if says("b5") {
                assert_eq!(c.degree(6), Degree::FLAT_FIFTH, "{}", c.name());
            }
            if says("b13") {
                assert_eq!(c.degree(8), Degree::FLAT_THIRTEENTH, "{}", c.name());
            } else if says("#5") || q.starts_with('+') {
                assert_eq!(c.degree(8), Degree::SHARP_FIFTH, "{}", c.name());
            }
            if says("#9") {
                assert_eq!(c.degree(3), Degree::SHARP_NINTH, "{}", c.name());
            }
            if says("dim7") {
                assert_eq!(c.degree(9), Degree::DIMINISHED_SEVENTH, "{}", c.name());
            }
            if q.starts_with('6') || q.starts_with("m6") {
                assert_eq!(c.degree(9), Degree::SIXTH, "{}", c.name());
            }
            if q.starts_with("13") || q.starts_with("m13") || q.starts_with("maj13") {
                assert_eq!(c.degree(9), Degree::THIRTEENTH, "{}", c.name());
            }
        }
    }

    #[test]
    fn families_roles_and_guide_tones() {
        assert_eq!(chord("G13").family(), Family::Dominant);
        assert_eq!(chord("Bm7b5").family(), Family::HalfDiminished);
        assert_eq!(chord("Bdim7").family(), Family::Diminished);
        assert_eq!(chord("C+").family(), Family::Augmented);
        assert_eq!(chord("C7#5").family(), Family::Dominant);
        assert_eq!(chord("G9sus4").family(), Family::Sus);
        assert_eq!(chord("C5").family(), Family::Power);
        let cmaj7 = chord("Cmaj7");
        assert_eq!(
            [cmaj7.role(4), cmaj7.role(11), cmaj7.role(2), cmaj7.role(5)],
            [ToneRole::Third, ToneRole::Seventh, ToneRole::Tension, ToneRole::Avoid]
        );
        assert_eq!(chord("G13").guide_tones().to_string(), "F B");
        assert_eq!(chord("C6/9").core_tones().to_string(), "C E G A");
        assert_eq!(chord("Dm9").core_tones().to_string(), "C D F A");
    }

    #[test]
    fn which_chords_are_written_on_their_bass_only_changes_on_purpose() {
        // written_as_it_is reads the usual names' text (nothing added, nothing in brackets), so a change to how a
        // quality is written could move which chords over a bass are written on it, which readings are one, and what
        // the app keeps. Every set of notes on a root it says yes to is counted here: a change that moves one needs
        // the chord reader's version bumped (rule 7), then these numbers written again.
        let mut hash = crate::fnv::Fnv::new();
        let mut count = 0;
        for bits in (1..4096u16).step_by(2) {
            if written_as_it_is(PcSet::from_bits(bits)) {
                count += 1;
                hash.number(i128::from(bits));
            }
        }
        assert_eq!((count, hash.finish()), (174, 0xf38e_d7f6_11ba_db8f), "write these numbers here once it's meant");
    }

    #[test]
    fn two_chords_share_a_name_key_just_when_they_share_a_name() {
        // Every set of notes on every root, over the root: the key tells names apart just as the names do.
        let mut key_of: std::collections::HashMap<String, (PitchClass, u16)> = std::collections::HashMap::new();
        let mut name_of: std::collections::HashMap<(PitchClass, u16), String> = std::collections::HashMap::new();
        for root in (0..12).map(PitchClass::new) {
            for bits in 0..4096u16 {
                let c = Chord::new(root, PcSet::from_bits(bits).on(root), root);
                let (name, key) = (c.name(), c.name_key());
                assert_eq!(*key_of.entry(name.clone()).or_insert(key), key, "{name}");
                assert_eq!(*name_of.entry(key).or_insert_with(|| name.clone()), name, "{name}");
            }
        }
        // And over a bass that's only a bass, as written: D over C is D7, F over G is G9sus4.
        assert_eq!(chord("D/C").name_key(), chord("D7").name_key());
        assert_eq!(chord("F/G").name_key(), chord("G9sus4").name_key());
        assert_eq!(chord("C/E").name_key(), chord("C/G").name_key());
        assert_ne!(chord("C").name_key(), chord("Cm").name_key());
        assert_ne!(chord("C").name_key(), chord("Db").name_key());
    }

    #[test]
    fn a_chord_over_a_bass_keeps_it_and_is_named_from_all_its_notes() {
        let d_over_c = chord("D/C");
        assert_eq!(
            (d_over_c.root(), d_over_c.bass(), d_over_c.notes().to_string()),
            (PitchClass::D, PitchClass::C, "D F# A".into())
        );
        assert_eq!(d_over_c.sounding().to_string(), "C D F# A");
        // As written, the bass is one of the chord's notes, and stays in the bass.
        let written = d_over_c.as_written();
        assert_eq!(
            (written.root(), written.bass(), written.notes()),
            (PitchClass::D, PitchClass::C, d_over_c.sounding())
        );
        assert_eq!(written, chord("D7/C"));
        assert_eq!(chord("D7/C").as_written(), chord("D7/C"));
        assert_eq!(chord("C/E").as_written(), chord("C/E"));
        let named = |typed: &str| chord(typed).name();
        // Solomon's four (2026-11-03).
        assert_eq!([named("D/C"), named("F/G"), named("Am/G"), named("C/E")], ["D7", "G9sus4", "Am7", "C"]);
        // Written on the bass when the notes over it make a sus chord there, or a chord whose name players write as it
        // is.
        for (typed, name) in [
            ("Bb/C", "C9sus4"),
            ("Fm9/Bb", "Bb13sus4"),
            ("Am/B", "B7sus4b9"),
            ("E/C", "Cmaj7#5"),
            ("Am/F", "Fmaj7"),
            ("G/E", "Em7"),
            ("Eb/C", "Cm7"),
            ("Em7/C", "Cmaj9"),
            ("Fm/D", "Dm7b5"),
            ("Bdim7/G", "G7b9"),
            ("G+/C", "Cm(maj7)"),
            ("Ebmaj7#5/C", "Cm(maj9)"),
        ] {
            assert_eq!(named(typed), name, "{typed}");
        }
        // Else on its own root, the bass one more of its notes.
        for (typed, name) in [
            ("E/D", "E7"),
            ("C/B", "Cmaj7"),
            ("Ab/G", "Abmaj7"),
            ("G/C", "G(add11)"),
            ("C/F#", "Cadd#11"),
            ("Eb/E", "Ebaddb9"),
            ("B/C", "Baddb9"),
            ("D/F", "Dadd#9"),
        ] {
            assert_eq!(named(typed), name, "{typed}");
        }
        assert_eq!(d_over_c.transposed(2).name(), "E7");
        // As datasets write them, for the bench: the bass after a slash, and read back the same.
        for typed in ["D/C", "D7/C", "C/E", "F/G", "G9sus4", "Bbmaj7/D", "E7#9/G#", "C#dim7/E"] {
            assert_eq!(chord(typed).slash_name(), typed);
            assert_eq!(Chord::parse(&chord(typed).slash_name()), Some(chord(typed)), "{typed}");
        }
        assert_eq!(chord("C/E").root_position().name(), "C");
        assert_eq!(chord("C#m7").spellings(), ["Dbm7", "C#m7"]);
        assert_eq!(chord("Gb/Ab").spellings(), ["Ab9sus4", "G#9sus4"]);
        assert_eq!(Chord::parse("C/H"), None);
        assert_eq!(Chord::parse("C/Ebm"), None);
        assert_eq!(Chord::parse("Cfoo"), None);
        assert_eq!(Chord::parse(""), None);
    }
}
