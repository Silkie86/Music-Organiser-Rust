//! Spelling: which letter a note is written with. C♯ and D♭ are one key on a keyboard but not one note in a chord:
//! A7's third is C♯ (A, C♯, E, G), so A7 over its third is A7/C♯, never A7/D♭.
//!
//! A spelled note is a place on the line of fifths ([`Tpc`]): … E♭ −3, B♭ −2, F −1, C 0, G 1, D 2, A 3, E 4, B 5,
//! F♯ 6, C♯ 7 … Each step up is a fifth up, so spelling is adding: a chord's notes are its root plus a fixed number of
//! steps for each degree ([`Degree`]). A major third is always four steps up (C to E, A to C♯, D♭ to F), a minor
//! seventh always two down (A to G, D♭ to C♭).
//!
//! So only a chord's root needs choosing. With no key it's written as charts do; in a key it's written by what the
//! chord does there ([`root_in_key`], [`spell_in_key`]): the chord after it decides a secondary dominant
//! (F♯7 to B), a leading-tone diminished chord (C♯°7 to Dm) or a tritone substitute (D♭7 to C), and chords borrowed
//! from the minor key take flats (A♭ and E♭ in C major, never G♯ and D♯).

use std::fmt;

use crate::chord::{Chord, Family};
use crate::key::Key;
use crate::pitch::PitchClass;

/// A spelled note: its place on the line of fifths. C is 0, each fifth up adds one (G 1, D 2 … B 5, F♯ 6, C♯ 7) and
/// each fifth down takes one away (F −1, B♭ −2, E♭ −3). Seven steps make the same letter a sharp higher (C 0 to C♯ 7)
/// and twelve make the same key on the keyboard (C♯ 7 and D♭ −5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Tpc(i8);

impl Tpc {
    pub const C: Tpc = Tpc(0);
    pub const D: Tpc = Tpc(2);
    pub const E: Tpc = Tpc(4);
    pub const F: Tpc = Tpc(-1);
    pub const G: Tpc = Tpc(1);
    pub const A: Tpc = Tpc(3);
    pub const B: Tpc = Tpc(5);

    /// The furthest a spelling goes either way: five sharps or flats, far past anything a chart writes.
    const LIMIT: i32 = 35;

    /// The note this many fifths above C (below when negative).
    pub const fn new(fifths: i32) -> Tpc {
        let f = if fifths > Tpc::LIMIT {
            Tpc::LIMIT
        } else if fifths < -Tpc::LIMIT {
            -Tpc::LIMIT
        } else {
            fifths
        };
        Tpc(f as i8)
    }

    /// How many fifths above C it is (below when negative).
    pub const fn fifths(self) -> i32 {
        self.0 as i32
    }

    /// The key it's played on, with the octave left aside: C♯ and D♭ are both 1.
    pub const fn pitch_class(self) -> PitchClass {
        PitchClass::new(7 * self.0 as i32)
    }

    /// How many sharps (above 0) or flats (below 0) it's written with: 1 for F♯, −2 for B𝄫.
    pub const fn accidentals(self) -> i32 {
        (self.0 as i32 + 1).div_euclid(7)
    }

    /// Its letter, A to G.
    pub const fn letter(self) -> char {
        match (self.0 as i32 + 1).rem_euclid(7) {
            0 => 'F',
            1 => 'C',
            2 => 'G',
            3 => 'D',
            4 => 'A',
            5 => 'E',
            _ => 'B',
        }
    }

    /// The spelling of `pc` among the twelve on the line of fifths from `lowest` up (each pitch class has exactly one
    /// there): `Tpc::in_window(pc, -5)` writes the black notes D♭ E♭ F♯ A♭ B♭, `in_window(pc, -1)` writes them all
    /// sharp.
    pub const fn in_window(pc: PitchClass, lowest: i32) -> Tpc {
        // 7 × 7 = 49, one more than four octaves, so 7 × pc is always a spelling of pc; twelve steps keep the key.
        let any = 7 * pc.number() as i32;
        Tpc::new(lowest + (any - lowest).rem_euclid(12))
    }

    /// The note a degree above this one: C up a major third is E, A up a minor third is C.
    pub const fn up(self, degree: Degree) -> Tpc {
        Tpc::new(self.fifths() + degree.fifths())
    }

    /// The same key with no double sharp or flat: F𝄪 as G, B𝄫 as A, C𝄪 as D. Single sharps and flats stay.
    pub const fn without_doubles(self) -> Tpc {
        let mut f = self.0 as i32;
        // Single accidentals run from F♭ (−8) to B♯ (12).
        while f > 12 {
            f -= 12;
        }
        while f < -8 {
            f += 12;
        }
        Tpc::new(f)
    }

    /// The same key written the easy way: no double sharps or flats, and no E♯, B♯, F♭ or C♭ (F, C, E and B).
    pub const fn readable(self) -> Tpc {
        let mut f = self.0 as i32;
        // From G♭ (−6) to A♯ (10), every note is a white key or a black key with one sharp or flat.
        while f > 10 {
            f -= 12;
        }
        while f < -6 {
            f += 12;
        }
        Tpc::new(f)
    }

    /// Whether it's written with letters a chart reader expects: one sharp or flat at most, and not E♯, B♯, F♭ or C♭.
    pub const fn is_readable(self) -> bool {
        self.0 >= -6 && self.0 <= 10
    }

    /// "C", "F#", "Bb", "E#", "Fbb": the letter and its sharps or flats, as plain text.
    pub fn name(self) -> String {
        let mut name = String::from(self.letter());
        let a = self.accidentals();
        for _ in 0..a.unsigned_abs() {
            name.push(if a > 0 { '#' } else { 'b' });
        }
        name
    }

    /// Reads a spelled note at the start of `text`: a letter A to G (either case) and up to two sharps or flats
    /// ("#", "b", "♯", "♭"). Gives the note and how many bytes of `text` it took, or `None` when it doesn't start with
    /// a note. A "b" after the letter always flattens: "Bb" is B flat.
    pub fn parse(text: &str) -> Option<(Tpc, usize)> {
        let mut chars = text.char_indices();
        let (_, letter) = chars.next()?;
        let mut fifths = match letter.to_ascii_uppercase() {
            'F' => -1,
            'C' => 0,
            'G' => 1,
            'D' => 2,
            'A' => 3,
            'E' => 4,
            'B' => 5,
            _ => return None,
        };
        let mut used = letter.len_utf8();
        let mut accidentals = 0;
        for (at, c) in chars {
            fifths += match c {
                '#' | '♯' => 7,
                'b' | '♭' => -7,
                _ => break,
            };
            used = at + c.len_utf8();
            accidentals += 1;
            if accidentals == 2 {
                break;
            }
        }
        Some((Tpc::new(fifths), used))
    }
}

impl fmt::Display for Tpc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
    }
}

/// A note of a chord by its place above the root, as chord symbols and Harte labels write it: 1, ♭3, 3, ♭5, 5, ♯5,
/// 6, 𝄫7, ♭7, 7, the tensions ♭9, 9, ♯9, 11, ♯11, ♭13, 13, and 2 and 4 in sus chords. ♯9 and ♭3 sound the same but are
/// spelled apart (C7♯9 has a D♯, Cm7 an E♭), and so are ♯11 and ♭5, ♭13 and ♯5.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Degree {
    /// 1 to 13: the step up the scale from the root, counting the root as 1.
    step: u8,
    /// Sharps (above 0) or flats (below 0) on the step, from the major scale's: −1 makes the 3 a ♭3.
    alter: i8,
}

impl Degree {
    pub const ROOT: Degree = Degree::new(1, 0);
    pub const FLAT_NINTH: Degree = Degree::new(9, -1);
    /// The 2 of a sus2 chord.
    pub const SECOND: Degree = Degree::new(2, 0);
    pub const NINTH: Degree = Degree::new(9, 0);
    pub const SHARP_NINTH: Degree = Degree::new(9, 1);
    pub const MINOR_THIRD: Degree = Degree::new(3, -1);
    pub const MAJOR_THIRD: Degree = Degree::new(3, 0);
    /// The 4 of a sus4 chord.
    pub const FOURTH: Degree = Degree::new(4, 0);
    pub const ELEVENTH: Degree = Degree::new(11, 0);
    pub const SHARP_ELEVENTH: Degree = Degree::new(11, 1);
    pub const FLAT_FIFTH: Degree = Degree::new(5, -1);
    pub const FIFTH: Degree = Degree::new(5, 0);
    pub const SHARP_FIFTH: Degree = Degree::new(5, 1);
    pub const FLAT_THIRTEENTH: Degree = Degree::new(13, -1);
    /// The 6 of a sixth chord (no seventh).
    pub const SIXTH: Degree = Degree::new(6, 0);
    pub const THIRTEENTH: Degree = Degree::new(13, 0);
    /// The 𝄫7 of a diminished seventh chord.
    pub const DIMINISHED_SEVENTH: Degree = Degree::new(7, -2);
    pub const MINOR_SEVENTH: Degree = Degree::new(7, -1);
    pub const MAJOR_SEVENTH: Degree = Degree::new(7, 0);

    /// Step `step` (1 to 13) with `alter` sharps (flats when negative).
    pub const fn new(step: u8, alter: i8) -> Degree {
        let step = if step < 1 {
            1
        } else if step > 13 {
            13
        } else {
            step
        };
        Degree { step, alter }
    }

    /// 1 to 13: 3 for ♭3 and 3, 9 for ♭9.
    pub const fn step(self) -> u8 {
        self.step
    }

    /// Sharps (above 0) or flats (below 0) on the step.
    pub const fn alter(self) -> i8 {
        self.alter
    }

    /// The step's place in the major scale above the root, 0 (the root) to 6 (the seventh): 9 is a 2, 13 a 6.
    const fn scale_index(self) -> u8 {
        (self.step - 1) % 7
    }

    /// Semitones above the root, within the octave: 4 for a 3, 3 for a ♭3 or a ♯9, 2 for a 9.
    pub const fn interval(self) -> i32 {
        let natural = match self.scale_index() {
            0 => 0,
            1 => 2,
            2 => 4,
            3 => 5,
            4 => 7,
            5 => 9,
            _ => 11,
        };
        (natural + self.alter as i32).rem_euclid(12)
    }

    /// How many fifths up the line of fifths it is from the root: 4 for a major third, −3 for a minor one, 9 for a ♯9.
    pub const fn fifths(self) -> i32 {
        let natural = match self.scale_index() {
            0 => 0,
            1 => 2,
            2 => 4,
            3 => -1,
            4 => 1,
            5 => 3,
            _ => 5,
        };
        natural + 7 * self.alter as i32
    }

    /// The same degree within the octave: 9 as 2, ♭13 as ♭6, ♯11 as ♯4.
    pub const fn simple(self) -> Degree {
        Degree::new(if self.step > 7 { self.step - 7 } else { self.step }, self.alter)
    }
}

impl fmt::Display for Degree {
    /// As Harte labels write it: "b3", "#11", "bb7", "13".
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for _ in 0..self.alter.unsigned_abs() {
            f.write_str(if self.alter > 0 { "#" } else { "b" })?;
        }
        write!(f, "{}", self.step)
    }
}

/// A chord with its root spelled. Every other note follows from the root by its degree, so A7 over its third is
/// A7/C♯, D♭7's seventh is C♭, and Cdim7's is B𝄫 (shown as A).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Spelled {
    chord: Chord,
    root: Tpc,
}

impl Spelled {
    /// The chord with its root written as `root`. A spelling of another note is moved to the chord's root, the letter
    /// nearest it on the line of fifths.
    pub const fn new(chord: Chord, root: Tpc) -> Spelled {
        let root = if root.pitch_class().number() == chord.root().number() {
            root
        } else {
            Tpc::in_window(chord.root(), root.fifths() - 5)
        };
        Spelled { chord, root }
    }

    /// Written as charts write a chord when no key says otherwise, as the C# app did: flats for black-note roots but
    /// F♯ (D♭, E♭, F♯, A♭, B♭), and sharps for diminished chords, which usually climb to the next chord (C♯dim7, D♯dim7).
    pub const fn plain(chord: Chord) -> Spelled {
        let lowest = if matches!(chord.family(), Family::Diminished) { -1 } else { -5 };
        Spelled { chord, root: Tpc::in_window(chord.root(), lowest) }
    }

    pub const fn chord(self) -> Chord {
        self.chord
    }

    pub const fn root(self) -> Tpc {
        self.root
    }

    /// The bass, spelled by its degree above the root: the third of A7 is C♯, the C under D/C its ♭7.
    pub const fn bass(self) -> Tpc {
        self.root.up(self.chord.bass_degree())
    }

    /// The chord's own notes from the root up, each with its degree and spelling: Cdim7 is C E♭ G♭ B𝄫.
    pub fn notes(self) -> Vec<(Degree, Tpc)> {
        self.chord.degrees().into_iter().map(|d| (d, self.root.up(d))).collect()
    }

    /// "A7/C#", "Dbmaj7", "C#dim7": the root as spelled, the quality, and the bass by its degree (a double sharp or flat
    /// in the bass shown as the plain note: E7(#9)/G, not E7(#9)/F##).
    pub fn name(self) -> String {
        let mut name = self.root.name();
        name.push_str(&self.chord.quality());
        if self.chord.bass() != self.chord.root() {
            name.push('/');
            name.push_str(&self.bass().without_doubles().name());
        }
        name
    }

    /// The chord as a Harte label, the way chord datasets and JAMS files write chords: "A:(1,3,5,b7)/3",
    /// "Db:(1,3,5,b7,9,13)". Every degree is listed, so nothing depends on which notes a shorthand like "13" implies.
    /// A root that doesn't sound at all is marked left out ("*1").
    pub fn harte(self) -> String {
        let mut parts: Vec<String> = self.chord.degrees().iter().map(Degree::to_string).collect();
        if !self.chord.notes().contains(self.chord.root()) {
            // A rootless voicing: over its root in the bass the root still sounds; with no root anywhere it's left out.
            parts.insert(0, if self.chord.bass() == self.chord.root() { "1" } else { "*1" }.to_owned());
        }
        let mut label = format!("{}:({})", self.root.name(), parts.join(","));
        if self.chord.bass() != self.chord.root() {
            label.push('/');
            label.push_str(&self.chord.bass_degree().to_string());
        }
        label
    }

    /// Reads a typed chord and keeps its root as typed: "C#m7" stays C♯m7 and "Dbm7" stays D♭m7. The bass is spelled
    /// by its degree, so a typed "A7/Db" comes back as A7/C#.
    pub fn parse(text: &str) -> Option<Spelled> {
        let chord = Chord::parse(text)?;
        let (root, _) = Tpc::parse(text.trim())?;
        Some(Spelled::new(chord, root))
    }
}

impl fmt::Display for Spelled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
    }
}

/// Whether a chord of this family is a diminished one (dim, dim7, m7♭5), whose root is usually a leading tone: the
/// note a semitone under the chord it goes to.
const fn is_diminished(family: Family) -> bool {
    matches!(family, Family::Diminished | Family::HalfDiminished)
}

/// How a chord's root is spelled in `key`, given the chord after it (already spelled) when there is one.
///
/// In order:
/// 1. A root in the key's scale is spelled as the scale has it (in a minor key, its raised sixth and seventh too:
///    F♯ and G♯ in A minor).
/// 2. Then what the chord does next: falling a fifth (a secondary dominant, F♯7 to B, or ii to V), it's a fifth
///    above the next root; a diminished chord a semitone under the next root is its leading tone (C♯°7 to Dm);
///    falling a semitone it takes the flat side (D♭7 to C, E♭°7 to Dm, E7 to E♭ written E, not F♭); on the next
///    chord's root (a C°7 between two Cs), it's spelled as that root.
/// 3. A chord borrowed from the parallel key takes flats: ♭II, ♭III, ♭VI and ♭VII (D♭, E♭, A♭, B♭ in C major).
/// 4. A diminished chord takes sharps, as leading tones do (D♯°7, G♯°7 in C major).
/// 5. Anything else is spelled within the key's reach: from three flats beyond the key to three sharps beyond it.
///
/// A root written with a double sharp or flat, or as E♯, B♯, F♭ or C♭, is written the easy way (E, not F♭) unless
/// the key's own scale has it (C♭ in G♭ major, E♯ in F♯ minor).
pub fn root_in_key(chord: Chord, key: Key, next: Option<Spelled>) -> Tpc {
    let pc = chord.root();
    let diminished = is_diminished(chord.family());
    // The major key on the same notes: C major for C major and for A minor.
    let t = key.major_tonic().fifths();

    let diatonic = Tpc::in_window(pc, t - 1);
    if key.has(diatonic) {
        return guarded(diatonic, key);
    }
    if let Some(next) = next {
        let to = next.root().fifths();
        match next.chord().root().above(pc) {
            0 => return guarded(next.root(), key),
            5 => return guarded(Tpc::new(to + 1), key),
            1 if diminished => return guarded(Tpc::new(to + 5), key),
            11 => return guarded(Tpc::new(to - 5), key),
            _ => {}
        }
    }
    if !diminished {
        let tonic = key.spelled_tonic().fifths();
        for borrowed in [-5, -4, -3, -2] {
            let b = Tpc::new(tonic + borrowed);
            if b.pitch_class().number() == pc.number() {
                return guarded(b, key);
            }
        }
    }
    let lowest = if diminished { t - 1 } else { t - 3 };
    guarded(Tpc::in_window(pc, lowest), key)
}

/// A root spelled the easy way unless the key's scale has it as it is.
fn guarded(root: Tpc, key: Key) -> Tpc {
    if root.is_readable() || (key.has(root) && root.accidentals().abs() < 2) { root } else { root.readable() }
}

/// Spells a run of chords in `key` (or as charts do with no key), each root by what it does next: the last chord is
/// spelled first, so each knows how the one after it is written.
pub fn spell_in_key(chords: &[Chord], key: Option<Key>) -> Vec<Spelled> {
    let mut spelled = Vec::with_capacity(chords.len());
    let mut next: Option<Spelled> = None;
    for &chord in chords.iter().rev() {
        let s = match key {
            Some(k) => Spelled::new(chord, root_in_key(chord, k, next)),
            None => Spelled::plain(chord),
        };
        spelled.push(s);
        next = Some(s);
    }
    spelled.reverse();
    spelled
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pitch::PcSet;

    fn chord(name: &str) -> Chord {
        Chord::parse(name).unwrap_or_else(|| panic!("{name} doesn't read"))
    }

    fn key(name: &str) -> Key {
        Key::parse(name).unwrap_or_else(|| panic!("{name} isn't a key"))
    }

    /// The chords' names when spelled in a run in this key, the C# app's way when there's no key.
    fn names_in(key_name: Option<&str>, chords: &[&str]) -> Vec<String> {
        let chords: Vec<Chord> = chords.iter().map(|c| chord(c)).collect();
        spell_in_key(&chords, key_name.map(key)).into_iter().map(Spelled::name).collect()
    }

    #[test]
    fn notes_sit_on_the_line_of_fifths() {
        let names: Vec<String> = (-15..=19).map(|f| Tpc::new(f).name()).collect();
        assert_eq!(
            names.join(" "),
            "Fbb Cbb Gbb Dbb Abb Ebb Bbb Fb Cb Gb Db Ab Eb Bb F C G D A E B F# C# G# D# A# E# B# F## C## G## D## A## \
             E## B##"
        );
        for f in -15..=19 {
            let t = Tpc::new(f);
            assert_eq!(Tpc::parse(&t.name()), Some((t, t.name().len())), "{t}");
            // Each step is a fifth: seven semitones.
            assert_eq!(t.pitch_class(), PitchClass::new(7 * f));
        }
        assert_eq!(Tpc::parse("C♯m7"), Some((Tpc::new(7), "C♯".len())));
        assert_eq!(Tpc::parse("Bbm"), Some((Tpc::new(-2), 2)));
        assert_eq!(Tpc::parse("H"), None);
    }

    #[test]
    fn every_window_of_twelve_spells_each_key_once() {
        for lowest in -12..=12 {
            let mut seen = PcSet::EMPTY;
            for pc in PitchClass::ALL {
                let t = Tpc::in_window(pc, lowest);
                assert_eq!(t.pitch_class(), pc);
                assert!((lowest..lowest + 12).contains(&t.fifths()));
                seen = seen.with(pc);
            }
            assert_eq!(seen, PcSet::ALL);
        }
        let flats_but_f_sharp: Vec<String> = PitchClass::ALL.iter().map(|&pc| Tpc::in_window(pc, -5).name()).collect();
        assert_eq!(flats_but_f_sharp.join(" "), "C Db D Eb E F F# G Ab A Bb B");
    }

    #[test]
    fn hard_spellings_are_made_easy() {
        let easy = |name: &str| Tpc::parse(name).map(|(t, _)| (t.without_doubles().name(), t.readable().name()));
        assert_eq!(easy("F##"), Some(("G".into(), "G".into())));
        assert_eq!(easy("Bbb"), Some(("A".into(), "A".into())));
        assert_eq!(easy("Cbb"), Some(("Bb".into(), "Bb".into())));
        assert_eq!(easy("E#"), Some(("E#".into(), "F".into())));
        assert_eq!(easy("B#"), Some(("B#".into(), "C".into())));
        assert_eq!(easy("Fb"), Some(("Fb".into(), "E".into())));
        assert_eq!(easy("Cb"), Some(("Cb".into(), "B".into())));
        assert_eq!(easy("Gb"), Some(("Gb".into(), "Gb".into())));
        assert_eq!(easy("A#"), Some(("A#".into(), "A#".into())));
    }

    #[test]
    fn degrees_are_fixed_steps_on_the_line_of_fifths() {
        // From the research's table: M3 +4, m3 −3, P5 +1, b7 −2, M7 +5, 9 +2, b9 −5, #9 +9, 11 −1, #11 +6, b5 −6,
        // #5 +8, b13 −4, 13 and 6 +3, bb7 −9.
        for (degree, fifths, semitones) in [
            (Degree::MAJOR_THIRD, 4, 4),
            (Degree::MINOR_THIRD, -3, 3),
            (Degree::FIFTH, 1, 7),
            (Degree::MINOR_SEVENTH, -2, 10),
            (Degree::MAJOR_SEVENTH, 5, 11),
            (Degree::NINTH, 2, 2),
            (Degree::FLAT_NINTH, -5, 1),
            (Degree::SHARP_NINTH, 9, 3),
            (Degree::ELEVENTH, -1, 5),
            (Degree::SHARP_ELEVENTH, 6, 6),
            (Degree::FLAT_FIFTH, -6, 6),
            (Degree::SHARP_FIFTH, 8, 8),
            (Degree::FLAT_THIRTEENTH, -4, 8),
            (Degree::THIRTEENTH, 3, 9),
            (Degree::SIXTH, 3, 9),
            (Degree::DIMINISHED_SEVENTH, -9, 9),
        ] {
            assert_eq!((degree.fifths(), degree.interval()), (fifths, semitones), "{degree}");
        }
        assert_eq!(
            [Degree::FLAT_NINTH, Degree::SHARP_ELEVENTH, Degree::DIMINISHED_SEVENTH, Degree::THIRTEENTH]
                .map(|d| d.to_string()),
            ["b9", "#11", "bb7", "13"]
        );
        assert_eq!(Degree::FLAT_THIRTEENTH.simple().to_string(), "b6");
    }

    #[test]
    fn chord_notes_are_spelled_by_their_degrees() {
        let notes = |root: &str, name: &str| {
            let (r, _) = Tpc::parse(root).unwrap();
            Spelled::new(chord(name), r).notes().iter().map(|(_, t)| t.name()).collect::<Vec<_>>().join(" ")
        };
        assert_eq!(notes("C", "Cdim7"), "C Eb Gb Bbb");
        assert_eq!(notes("F#", "F#m7b5"), "F# A C E");
        assert_eq!(notes("C", "C13(#11)"), "C E G Bb D F# A");
        assert_eq!(notes("Db", "Db7"), "Db F Ab Cb");
        assert_eq!(notes("E#", "Fdim7"), "E# G# B D");
        assert_eq!(notes("C", "C7(#9)"), "C E G Bb D#");
        assert_eq!(notes("C", "C7(b5)"), "C E Gb Bb");
        assert_eq!(notes("C", "C7(#11)"), "C E G Bb F#");
        assert_eq!(notes("C", "C7(#5)"), "C E G# Bb");
        assert_eq!(notes("C", "C7(b13)"), "C E G Bb Ab");
        assert_eq!(notes("D#", "D#m9"), "D# F# A# C# E#");
        assert_eq!(notes("G", "G+"), "G B D#");
        assert_eq!(notes("C#", "C#7"), "C# E# G# B");
        // The full altered dominant: b9, #9, #11 and b13 over the third and seventh.
        let alt = Chord::of(PitchClass::C, PcSet::of(&[0, 4, 10, 1, 3, 6, 8]));
        assert_eq!(alt.name(), "C7alt");
        assert_eq!(
            Spelled::new(alt, Tpc::C).notes().iter().map(|(_, t)| t.name()).collect::<Vec<_>>().join(" "),
            "C E Bb Db D# F# Ab"
        );
    }

    #[test]
    fn a_slash_bass_is_spelled_by_its_degree() {
        for (typed, name) in [
            ("A7/C#", "A7/C#"),
            ("A7/Db", "A7/C#"),
            ("E7/G#", "E7/G#"),
            ("F#7/A#", "F#7/A#"),
            ("B/D#", "B/D#"),
            ("D/C", "D/C"),
            ("F/G", "F/G"),
            ("Bb/C", "Bb/C"),
            ("Ab/Bb", "Ab/Bb"),
            ("C/F#", "C/F#"),
            ("E/C", "E/C"),
            ("Bbmaj7/D", "Bbmaj7/D"),
            ("C#dim7/E", "C#dim7/E"),
        ] {
            assert_eq!(chord(typed).name(), name, "{typed}");
        }
        // A double sharp in the bass is shown as the plain note.
        let e7_sharp9 = Spelled::new(Chord::new(PitchClass::E, chord("E7(#9)").notes(), PitchClass::G), Tpc::E);
        assert_eq!(e7_sharp9.bass().name(), "F##");
        assert_eq!(e7_sharp9.name(), "E7(#9)/G");
    }

    #[test]
    fn typed_roots_keep_their_spelling() {
        let spelled = |t: &str| Spelled::parse(t).map(Spelled::name);
        assert_eq!(spelled("C#m7").as_deref(), Some("C#m7"));
        assert_eq!(spelled("Dbm7").as_deref(), Some("Dbm7"));
        assert_eq!(spelled("Gb7/Bb").as_deref(), Some("Gb7/Bb"));
        assert_eq!(spelled("F#7/Bb").as_deref(), Some("F#7/A#"));
        assert_eq!(spelled("Cfoo"), None);
        // With no key, the C# app's way.
        assert_eq!(chord("C#m7").name(), "Dbm7");
        assert_eq!(chord("Dbdim7").name(), "C#dim7");
    }

    #[test]
    fn harte_labels_list_every_degree() {
        let harte = |t: &str| Spelled::parse(t).map(Spelled::harte);
        assert_eq!(harte("A7/C#").as_deref(), Some("A:(1,3,5,b7)/3"));
        assert_eq!(harte("D#m9/A#").as_deref(), Some("D#:(1,b3,5,b7,9)/5"));
        assert_eq!(harte("Db13").as_deref(), Some("Db:(1,3,5,b7,9,13)"));
        assert_eq!(harte("C9sus4").as_deref(), Some("C:(1,4,5,b7,9)"));
        assert_eq!(harte("Csus2").as_deref(), Some("C:(1,2,5)"));
        assert_eq!(harte("D/C").as_deref(), Some("D:(1,3,5)/b7"));
        assert_eq!(harte("Cdim7").as_deref(), Some("C:(1,b3,b5,bb7)"));
        assert_eq!(harte("C6/9").as_deref(), Some("C:(1,3,5,6,9)"));
        assert_eq!(harte("C7(#9)").as_deref(), Some("C:(1,3,5,b7,#9)"));
        // A rootless voicing with no bass under it.
        let rootless = Spelled::new(Chord::new(PitchClass::C, PcSet::of(&[4, 10, 2, 9]), PitchClass::E), Tpc::C);
        assert_eq!(rootless.harte(), "C:(*1,3,b7,9,13)/3");
    }

    #[test]
    fn roots_are_spelled_by_what_they_do_in_the_key() {
        let cases: &[(&str, &[&str], &[&str])] = &[
            // A leading-tone diminished chord: C#, the note under D.
            ("C", &["C#dim7", "Dm7"], &["C#dim7", "Dm7"]),
            // Falling, a passing diminished chord takes the flat side.
            ("C", &["Em7", "Ebdim7", "Dm7"], &["Em7", "Ebdim7", "Dm7"]),
            // A common-tone diminished chord is spelled as the chord it decorates.
            ("C", &["C", "Cdim7", "C"], &["C", "Cdim7", "C"]),
            // V+ to I: the root is G, its raised fifth D#.
            ("C", &["G+", "C"], &["G+", "C"]),
            // One set of notes, two jobs: a tritone substitute in C, a secondary dominant in E.
            ("C", &["Db7", "Cmaj7"], &["Db7", "Cmaj7"]),
            ("E", &["C#7", "F#m7"], &["C#7", "F#m7"]),
            // A tritone substitute that would be Fb7 is written E7.
            ("Bb", &["E7", "Ebmaj7"], &["E7", "Ebmaj7"]),
            // Borrowed from the minor key: flats.
            ("E", &["C"], &["C"]),
            ("C", &["Ab", "Bb", "C"], &["Ab", "Bb", "C"]),
            ("G", &["Eb"], &["Eb"]),
            ("D", &["Bb"], &["Bb"]),
            // The Neapolitan, and the leading-tone chord, in minor keys.
            ("Dm", &["Eb", "C#dim7", "Dm"], &["Eb", "C#dim7", "Dm"]),
            ("Cm", &["F#dim7", "G"], &["F#dim7", "G"]),
            ("Cm", &["Bdim7", "Cm"], &["Bdim7", "Cm"]),
            // Secondary dominants and ii-V chains spell each root a fifth above the next.
            ("C", &["F#m7b5", "B7", "Em7"], &["F#m7b5", "B7", "Em7"]),
            ("C", &["Gb7", "Fmaj7"], &["Gb7", "Fmaj7"]),
            ("Eb", &["G7", "Cm7", "F7", "Bb7", "Eb"], &["G7", "Cm7", "F7", "Bb7", "Eb"]),
            // Sharp keys keep their sharps, the bass by its degree.
            ("A", &["F#m", "C#7/E#", "F#m"], &["F#m", "C#7/E#", "F#m"]),
            ("B", &["D#m7", "G#7", "C#m7", "F#7"], &["D#m7", "G#7", "C#m7", "F#7"]),
            // A diminished chord with nothing after it takes sharps.
            ("C", &["D#dim7"], &["D#dim7"]),
            ("C", &["Ebdim7"], &["D#dim7"]),
        ];
        for (k, chords, want) in cases {
            assert_eq!(names_in(Some(k), chords), *want, "in {k}: {chords:?}");
        }
        // With no key, the C# app's way.
        assert_eq!(names_in(None, &["C#7", "F#m7"]), ["Db7", "F#m7"]);
    }

    #[test]
    fn hard_roots_stay_only_when_the_key_has_them() {
        // Cb is IV in Gb major, E# the leading tone in F# minor.
        assert_eq!(names_in(Some("Gb"), &["Cb", "Gb"]), ["Cb", "Gb"]);
        assert_eq!(names_in(Some("F#m"), &["E#dim7", "F#m"]), ["E#dim7", "F#m"]);
        // In E major the leading tone of F#m is E#, which E major doesn't have: F.
        assert_eq!(names_in(Some("E"), &["Fdim7", "F#m7"]), ["Fdim7", "F#m7"]);
        // In G# minor the leading tone is F double sharp: written G.
        assert_eq!(names_in(Some("G#m"), &["Gdim7", "G#m"]), ["Gdim7", "G#m"]);
    }
}
