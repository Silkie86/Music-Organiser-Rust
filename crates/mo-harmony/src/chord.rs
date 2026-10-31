//! Chords as jazz and gospel players write them: a root, every note in the chord, and the bass under it, so
//! "G13(b9)", "Bbmaj9/D", "F#m7b5", "C#dim7" and "Ab7alt" all come out right. A chord is made from a typed name
//! ([`Chord::parse`]) or by the chord reader from notes.
//!
//! The names follow the C# app's, which Solomon has used and checked (its tests are carried over below), with
//! bracketed tensions after the chord's own name. A chord here is its pitch classes; how its notes are spelled (C♯ or
//! D♭) is [`crate::spell`]'s job: each note is written by its degree above the root ([`Chord::degree`]), so the root
//! is the only note whose spelling is chosen. With no key, roots are written as the C# app wrote them (flats for black
//! notes except F♯, sharps for diminished chords), and the bass by its degree: A7/C#, where the C# app wrote A7/Db.

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

/// Whether a dominant chord is an altered one (7alt): a major third and flat seventh, no perfect fifth, no natural 9,
/// 11 or 13 and no major seventh (none of them is in the altered scale), a ♭9 or ♯9, and a ♯11 or ♭13. With a
/// perfect fifth it's written out instead: C7(b9,b13).
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
/// diminished chord or a dominant with no fifth (C7(b5)), else a ♯11; eight a ♯5 in an augmented chord or a dominant
/// with no fifth (C7(#5)), else a ♭13; nine a 𝄫7 in a diminished seventh, a 13 over a seventh, else a 6. In an altered
/// chord they're its ♯11 and ♭13. In a sus chord, two and five semitones are its 2 and 4.
pub const fn degree(intervals: PcSet, family: Family, interval: i32) -> Degree {
    let no_fifth_dominant = matches!(family, Family::Dominant) && !intervals.has(7) && !is_altered(intervals);
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
            if matches!(family, Family::Diminished | Family::HalfDiminished) || no_fifth_dominant {
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

/// How a bass that isn't one of the chord's own notes is written above the root: within the octave, on the plain
/// side (the C under D/C is its ♭7, the G under F/G its 2, the F♯ under C/F♯ its ♯4).
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
            // b5 and #5 stand in for the fifth.
            6 | 8 if !p5 => ToneRole::Fifth,
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

/// A chord: its root, its own notes, and the bass under it. When the bass is the root, the chord is in root position
/// and its name has no slash.
///
/// The bass may be one of the chord's own notes (C/E, G7/F) or only a bass (the C of D/C, the Lydian sound): D/C and
/// D7/C sound the same four notes, but a player writes them differently, so the chord keeps which it is.
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

    /// The bass's degree above the root: its own degree when it's one of the chord's notes (the 3 of A7/C#), else a
    /// plain one within the octave (the ♭7 of D/C).
    pub const fn bass_degree(self) -> Degree {
        let interval = self.bass.above(self.root);
        if self.notes.contains(self.bass) { self.degree(interval as i32) } else { bass_only_degree(interval) }
    }

    /// The quality written after the root: "maj9", "m7b5", "13(#11)", "7alt", "9sus4".
    pub fn quality(self) -> String {
        quality_of(self.intervals(), self.family())
    }

    /// The root as [`Chord::name`] writes it: sharp for a diminished chord, else flat except F#.
    pub fn root_name(self) -> String {
        Spelled::plain(self).root().name()
    }

    /// The bass under a slash ("" when the root is in the bass), spelled by its degree.
    pub fn bass_name(self) -> String {
        if self.bass == self.root { String::new() } else { Spelled::plain(self).bass().without_doubles().name() }
    }

    /// "Dm9", "G13(b9)", "Cmaj7/E", "A7/C#": written as charts write it when no key says otherwise. In a key, see
    /// [`crate::spell::spell_in_key`].
    pub fn name(self) -> String {
        Spelled::plain(self).name()
    }

    /// Every way the name is written, for searching: "C#m7" and "Dbm7".
    pub fn spellings(self) -> Vec<String> {
        let mut all = vec![self.name()];
        // All sharps (F up to A#), then all flats (Gb up to B).
        for lowest in [-1, -6] {
            all.push(Spelled::new(self, Tpc::in_window(self.root, lowest)).name());
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

    /// A chord from its name as typed: "Dm9", "G7alt", "C6/9", "F#m7b5", "Bbmaj7#11", "Ebdim7", "Ab13sus", "C/E",
    /// "Dø", "C°7", "CΔ9", "C-7". `None` when it can't be read.
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

/// The quality as written after the root, for a chord of this family with these intervals above its root.
///
/// Every note the chord has is in its name: a note the usual name doesn't cover is written after it, as its degree
/// ("C(addb13)", "C7(maj7)", "Cdim7(9)"). The name may still stand for notes the chord leaves out, as charts do: the
/// fifth, the 9 under an 11 or 13, the 11 of a minor 13, and the alterations of a 7alt.
pub fn quality_of(intervals: PcSet, family: Family) -> String {
    let mut q = usual_quality(intervals, family);
    let left = intervals.difference(intervals_of(&q).unwrap_or_default());
    // Added notes take "add" when the chord has no seventh, as in "C(add#11)" beside "Cmaj7(#11)".
    let add = !(intervals.has(10) || intervals.has(11) || (matches!(family, Family::Diminished) && intervals.has(9)));
    let extra: Vec<&str> =
        left.iter().filter_map(|pc| extra_tone(intervals, family, pc.number().into(), add)).collect();
    if !extra.is_empty() {
        q.push('(');
        q.push_str(&extra.join(","));
        q.push(')');
    }
    q
}

/// How a note the usual name leaves out is written after it, so that reading the name back gives the note: its degree
/// ([`degree`]), with "add" in front of a tension when `add`. `None` for the root, the thirds and the fifth, which
/// every usual name covers.
fn extra_tone(intervals: PcSet, family: Family, interval: i32, add: bool) -> Option<&'static str> {
    let tension = |with_add: &'static str, plain: &'static str| Some(if add { with_add } else { plain });
    match interval {
        1 => tension("addb9", "b9"),
        2 => tension("add9", "9"),
        // A minor third that isn't the chord's third is a #9.
        3 => tension("add#9", "#9"),
        5 => tension("add11", "11"),
        6 if degree(intervals, family, 6).alter() < 0 => Some("b5"),
        6 => tension("add#11", "#11"),
        8 if degree(intervals, family, 8).alter() > 0 => Some("#5"),
        8 => tension("addb13", "b13"),
        9 => tension("add6", "13"),
        10 => Some("b7"),
        11 => Some("maj7"),
        _ => None,
    }
}

/// The usual name of the quality, from the notes that decide it; [`quality_of`] adds what it leaves out.
fn usual_quality(intervals: PcSet, family: Family) -> String {
    let has = |i| intervals.has(i);
    let (b9, n9, s9, n11, s11, b13, n13) = (has(1), has(2), has(3), has(5), has(6), has(8), has(9));
    let (p5, b7, maj7) = (has(7), has(10), has(11));
    match family {
        Family::Major => {
            if !p5 && b13 {
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
            if s11 {
                q.push_str(if maj7 || n13 { "(#11)" } else { "(add#11)" });
            }
            if n11 {
                q.push_str("(add11)");
            }
            if b9 {
                q.push_str("(b9)");
            }
            if s9 && !maj7 {
                q.push_str("(add#9)");
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
        Family::Minor => {
            let mut q = String::from(if maj7 {
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
            });
            if b13 {
                // A seventh of either kind makes the b13 a tension, not an added note.
                q.push_str(if b7 || maj7 { "(b13)" } else { "(addb13)" });
            }
            if b9 {
                q.push_str("(b9)");
            }
            if s11 && p5 {
                q.push_str("(#11)");
            }
            q
        }
        Family::Dominant => {
            // An altered dominant (no fifth, no natural 9 or 13) is 7alt; otherwise the alterations are written out.
            let mut alterations = Vec::new();
            if b9 {
                alterations.push("b9");
            }
            if s9 {
                alterations.push("#9");
            }
            if s11 {
                alterations.push(if p5 { "#11" } else { "b5" });
            }
            if b13 {
                alterations.push(if p5 { "b13" } else { "#5" });
            }
            if is_altered(intervals) {
                return "7alt".into();
            }
            let q = if n13 {
                "13"
            } else if n9 {
                "9"
            } else {
                "7"
            };
            if n11 {
                alterations.push("11");
            }
            if alterations.is_empty() { q.into() } else { format!("{q}({})", alterations.join(",")) }
        }
        Family::HalfDiminished => {
            let q = if n11 {
                "m11b5"
            } else if n9 {
                "m9b5"
            } else {
                "m7b5"
            };
            if b13 { format!("{q}(b13)") } else { q.into() }
        }
        Family::Diminished => match (has(9), maj7) {
            (true, true) => "dim7(maj7)",
            (true, false) => "dim7",
            (false, true) => "dim(maj7)",
            (false, false) => "dim",
        }
        .into(),
        Family::Sus => {
            let sus = if n11 { "sus4" } else { "sus2" };
            let mut q = if b7 {
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
            };
            if b9 {
                q.push_str("(b9)");
            }
            q
        }
        // A seventh over a bare fifth: "C7(no3)", as rock charts write it.
        Family::Power if b7 => "7(no3)".into(),
        Family::Power if maj7 => "maj7(no3)".into(),
        Family::Power => "5".into(),
    }
}

/// The intervals a typed quality means: "", "m", "7", "maj9", "m7b5", "dim7", "13(#11)", "7alt", "9sus4", "6/9",
/// "add9", "+", "ø", "°7", "Δ", "-7". `None` when it can't be read.
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

/// The quality the way a lead sheet writes it when room is short: Δ for maj, ° for dim, ø for m7b5, sus for sus4,
/// and no bracketed tensions. "maj7" is "Δ7", "13(b9)" is "13", "dim7" is "°7". A bracketed major seventh says what
/// the chord is, so it stays, as Δ: "m(maj7)" is "mΔ7".
pub fn short_quality(quality: &str) -> String {
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

/// The least of a quality that still says what kind of chord it is, for when even the short form won't fit: "m" for
/// a minor chord, ° for diminished, ø for half-diminished, + for augmented, and nothing for major and dominant ones.
pub fn least_quality(quality: &str) -> String {
    let q = short_quality(quality);
    match q.chars().next() {
        Some('m') => "m".into(),
        Some(c @ ('°' | 'ø' | '+')) => c.into(),
        _ => String::new(),
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
            "A7alt",
            "Bm7b5",
            "C#dim7",
            "C6/9",
            "F#m11",
            "Ab13(#11)",
            "G9sus4",
            "Bbmaj7/D",
            "Cm(maj7)",
            "E7(b9)",
            "Db7(#9)",
            "Adim(maj7)",
            "C#dim7(maj7)/E",
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
            ("F#7b9", "F#7(b9)"),
            ("Bb7#9", "Bb7(#9)"),
            ("E♭maj7", "Ebmaj7"),
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
            ("G7(b9)", "7", ""),
            ("G7sus4", "7sus", ""),
            ("Cmaj9", "Δ9", ""),
            ("Cm(maj7)", "mΔ7", "m"),
            ("Cm(maj9)", "mΔ9", "m"),
            ("C+", "+", "+"),
        ] {
            let c = chord(name);
            assert_eq!(short_quality(&c.quality()), shorter, "{name}");
            assert_eq!(least_quality(&c.quality()), least, "{name}");
            assert_eq!(
                format!(
                    "{}{}{}{}",
                    c.root_name(),
                    c.quality(),
                    if c.bass_name().is_empty() { "" } else { "/" },
                    c.bass_name()
                ),
                name
            );
        }
    }

    #[test]
    fn an_altered_dominant_has_no_fifth() {
        let c = |iv: &[i32]| Chord::of(PitchClass::C, PcSet::of(iv)).name();
        assert_eq!(c(&[0, 4, 10, 1, 8]), "C7alt");
        assert_eq!(c(&[0, 4, 10, 3, 6]), "C7alt");
        // With the fifth sounding, the alterations are written out.
        assert_eq!(c(&[0, 4, 7, 10, 1, 8]), "C7(b9,b13)");
        assert_eq!(c(&[0, 4, 7, 10, 3, 6]), "C7(#9,#11)");
        // A natural 11 isn't in the altered scale, so it's written out too.
        assert_eq!(c(&[0, 4, 10, 1, 8, 5]), "C7(b9,#5,11)");
        // Typed, 7alt stands for every alteration.
        assert_eq!(chord("C7alt").notes(), PcSet::of(&[0, 4, 10, 1, 3, 6, 8]));
    }

    #[test]
    fn every_note_is_in_the_name() {
        // For every set of notes on a root, reading its name back gives every note in it. The name may stand for a
        // few notes the chord leaves out, as charts do: the fifth, the 9 under an 11 or 13, the 11 of a minor 13, and
        // the alterations of a 7alt; never a third or a seventh the chord doesn't have.
        for bits in (1..4096u16).step_by(2) {
            let c = Chord::of(PitchClass::C, PcSet::from_bits(bits));
            let name = c.name();
            let back = chord(&name);
            assert!(c.notes().difference(back.notes()).is_empty(), "{name} leaves out some of {}", c.notes());
            let may_add = if name.contains("alt") { PcSet::of(&[2, 7, 1, 3, 6, 8]) } else { PcSet::of(&[2, 5, 7]) };
            let added = back.notes().difference(c.notes());
            assert!(added.difference(may_add).is_empty(), "{name} adds {added} to {}", c.notes());
        }
        let c = |iv: &[i32]| Chord::of(PitchClass::C, PcSet::of(iv)).name();
        assert_eq!(c(&[0, 4, 7, 8]), "C(addb13)");
        assert_eq!(c(&[0, 4, 7, 11, 8]), "Cmaj7(b13)");
        assert_eq!(c(&[0, 4, 10, 11]), "C7(maj7)");
        assert_eq!(c(&[0, 3, 8]), "Cm(addb13)");
        assert_eq!(c(&[0, 3, 7, 10, 11]), "Cm(maj7)(b7)");
        assert_eq!(c(&[0, 3, 5, 9]), "Cm6(add11)");
        assert_eq!(c(&[0, 3, 10, 2, 5, 9]), "Cm13");
        assert_eq!(c(&[0, 3, 6, 9, 2]), "Cdim7(9)");
        assert_eq!(c(&[0, 3, 6, 10, 1]), "Cm7b5(b9)");
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
            if says("alt") || (says("#11") && !says("b5")) {
                assert_eq!(c.degree(6), Degree::SHARP_ELEVENTH, "{}", c.name());
            } else if says("b5") {
                assert_eq!(c.degree(6), Degree::FLAT_FIFTH, "{}", c.name());
            }
            if says("alt") || (says("b13") && !says("#5")) {
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
    fn slash_chords_keep_their_bass() {
        let d_over_c = chord("D/C");
        assert_eq!(
            (d_over_c.root(), d_over_c.bass(), d_over_c.notes().to_string()),
            (PitchClass::D, PitchClass::C, "D F# A".into())
        );
        assert_eq!(d_over_c.sounding().to_string(), "C D F# A");
        assert_eq!(d_over_c.name(), "D/C");
        // The same four notes with the C the chord's own seventh.
        assert_eq!(chord("D7/C").name(), "D7/C");
        assert_eq!(chord("D7/C").sounding(), d_over_c.sounding());
        assert_eq!(chord("C/E").notes(), chord("C").notes());
        assert_eq!(d_over_c.transposed(2).name(), "E/D");
        assert_eq!(chord("C/E").root_position().name(), "C");
        assert_eq!(chord("Am7/G").name(), "Am7/G");
        assert_eq!(chord("C#m7").spellings(), ["Dbm7", "C#m7"]);
        assert_eq!(Chord::parse("C/H"), None);
        assert_eq!(Chord::parse("C/Ebm"), None);
        assert_eq!(Chord::parse("Cfoo"), None);
        assert_eq!(Chord::parse(""), None);
    }
}
