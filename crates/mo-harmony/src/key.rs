//! Keys: a tonic and major or minor, how they're written, and how near two keys are.

use std::fmt;

use crate::names::parse_note;
use crate::pitch::{PcSet, PitchClass};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Mode {
    Major,
    Minor,
}

/// A key: its home note (the tonic) and whether it's major or minor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Key {
    pub tonic: PitchClass,
    pub mode: Mode,
}

/// How a key's tonic is written: flat keys with flats (Eb major, Bb minor), as charts and pack names write them, and
/// the six-sharp or six-flat ones as Gb major and Eb minor.
const MAJOR_TONICS: [&str; 12] = ["C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B"];
const MINOR_TONICS: [&str; 12] = ["C", "C#", "D", "Eb", "E", "F", "F#", "G", "G#", "A", "Bb", "B"];

/// How one key stands to another, as key-finding contests score a wrong answer (see [`Key::mirex_score`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KeyRelation {
    Same,
    /// The same mode, a fifth higher: G major for C major.
    FifthAbove,
    /// The same mode, a fifth lower: F major for C major.
    FifthBelow,
    /// The other mode on the same notes: A minor for C major, Eb major for C minor.
    Relative,
    /// The other mode on the same tonic: C minor for C major.
    Parallel,
    Other,
}

impl Key {
    pub const fn major(tonic: PitchClass) -> Key {
        Key { tonic, mode: Mode::Major }
    }

    pub const fn minor(tonic: PitchClass) -> Key {
        Key { tonic, mode: Mode::Minor }
    }

    /// All 24 keys: the twelve major ones from C up, then the twelve minor ones.
    // The loop stays inside the array.
    #[allow(clippy::indexing_slicing)]
    pub const ALL: [Key; 24] = {
        let mut all = [Key::major(PitchClass::C); 24];
        let mut i = 0;
        while i < 24 {
            let tonic = PitchClass::new(i as i32 % 12);
            all[i] = if i < 12 { Key::major(tonic) } else { Key::minor(tonic) };
            i += 1;
        }
        all
    };

    /// Where the key is in [`Key::ALL`]: 0 to 11 major, 12 to 23 minor.
    pub const fn index(self) -> usize {
        self.tonic.index() + if matches!(self.mode, Mode::Minor) { 12 } else { 0 }
    }

    pub const fn is_minor(self) -> bool {
        matches!(self.mode, Mode::Minor)
    }

    /// The key on the same notes in the other mode: A minor for C major, Eb major for C minor.
    pub const fn relative(self) -> Key {
        match self.mode {
            Mode::Major => Key::minor(self.tonic.up(9)),
            Mode::Minor => Key::major(self.tonic.up(3)),
        }
    }

    /// The key on the same tonic in the other mode: C minor for C major.
    pub const fn parallel(self) -> Key {
        match self.mode {
            Mode::Major => Key::minor(self.tonic),
            Mode::Minor => Key::major(self.tonic),
        }
    }

    /// The notes of its scale: the major scale, or the natural minor one.
    pub const fn scale(self) -> PcSet {
        let steps = match self.mode {
            Mode::Major => PcSet::of(&[0, 2, 4, 5, 7, 9, 11]),
            Mode::Minor => PcSet::of(&[0, 2, 3, 5, 7, 8, 10]),
        };
        steps.on(self.tonic)
    }

    /// The tonic's name as the key is written: "Eb" for Eb major, "C#" for C# minor.
    // A pitch class is always 0 to 11.
    #[allow(clippy::indexing_slicing)]
    pub fn tonic_name(self) -> &'static str {
        match self.mode {
            Mode::Major => MAJOR_TONICS[self.tonic.index()],
            Mode::Minor => MINOR_TONICS[self.tonic.index()],
        }
    }

    /// "Eb major", "C# minor".
    pub fn name(self) -> String {
        format!("{} {}", self.tonic_name(), if self.is_minor() { "minor" } else { "major" })
    }

    /// The short way: "Eb", "C#m".
    pub fn short(self) -> String {
        format!("{}{}", self.tonic_name(), if self.is_minor() { "m" } else { "" })
    }

    /// The key's place on the Camelot wheel DJs mix by: C major is 8B, A minor 8A, and a step round is a fifth.
    pub fn camelot(self) -> String {
        let major = match self.mode {
            Mode::Major => self.tonic,
            Mode::Minor => self.tonic.up(3),
        };
        let n = (8 + 7 * major.number() as u32) % 12;
        format!("{}{}", if n == 0 { 12 } else { n }, if self.is_minor() { 'A' } else { 'B' })
    }

    /// How this key, given as an answer, stands to `truth`.
    pub fn relation_to(self, truth: Key) -> KeyRelation {
        let up = self.tonic.above(truth.tonic);
        if self == truth {
            KeyRelation::Same
        } else if self.mode == truth.mode && up == 7 {
            KeyRelation::FifthAbove
        } else if self.mode == truth.mode && up == 5 {
            KeyRelation::FifthBelow
        } else if self == truth.relative() {
            KeyRelation::Relative
        } else if self == truth.parallel() {
            KeyRelation::Parallel
        } else {
            KeyRelation::Other
        }
    }

    /// The MIREX score for this key given as the answer when `truth` is right, as `mir_eval.key.weighted_score`
    /// gives it: 1 for the same key, 0.5 a fifth above in the same mode (a fifth below too when `both_fifths`),
    /// 0.3 for the relative key, 0.2 for the parallel one, and 0 for anything else.
    pub fn mirex_score(self, truth: Key, both_fifths: bool) -> f64 {
        match self.relation_to(truth) {
            KeyRelation::Same => 1.0,
            KeyRelation::FifthAbove => 0.5,
            KeyRelation::FifthBelow if both_fifths => 0.5,
            KeyRelation::Relative => 0.3,
            KeyRelation::Parallel => 0.2,
            KeyRelation::FifthBelow | KeyRelation::Other => 0.0,
        }
    }

    /// Reads a key as people and packs write it: "C", "F#m", "Bb minor", "A min", "Ebmin", "F#maj", "C Major",
    /// "Gb:maj", "c#:min". A tonic with nothing after it, or "maj"/"major", is major; "m", "min", "minor" or "-" is
    /// minor. `None` when it isn't a key.
    pub fn parse(text: &str) -> Option<Key> {
        let text = text.trim();
        let (tonic, used) = parse_note(text)?;
        let rest = text.get(used..)?.trim_start_matches([' ', ':', '_', '-']).to_ascii_lowercase();
        let minor = if rest.is_empty() || rest.starts_with("maj") {
            false
        } else if rest.starts_with('m') {
            // "m", "min", "minor", but not "major" (taken above) or "mixolydian".
            if rest.starts_with("mix") {
                return None;
            }
            true
        } else {
            return None;
        };
        // A minus straight after the tonic ("C-") is minor too.
        let minor = minor || text.get(used..).is_some_and(|r| r.starts_with('-'));
        Some(if minor { Key::minor(tonic) } else { Key::major(tonic) })
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(text: &str) -> Key {
        Key::parse(text).unwrap()
    }

    #[test]
    fn keys_are_named_as_charts_write_them() {
        let names: Vec<String> = Key::ALL.iter().map(|k| k.short()).collect();
        assert_eq!(names.join(" "), "C Db D Eb E F Gb G Ab A Bb B Cm C#m Dm Ebm Em Fm F#m Gm G#m Am Bbm Bm");
        assert_eq!(key("eb").name(), "Eb major");
        assert_eq!(key("D#m").name(), "Eb minor");
        for k in Key::ALL {
            assert_eq!(Key::parse(&k.name()), Some(k));
            assert_eq!(Key::parse(&k.short()), Some(k));
            assert_eq!(Key::ALL[k.index()], k);
        }
    }

    #[test]
    fn keys_are_read_as_people_and_packs_type_them() {
        for (text, name) in [
            ("C", "C major"),
            ("F#m", "F# minor"),
            ("Bb minor", "Bb minor"),
            ("A min", "A minor"),
            ("Ebmin", "Eb minor"),
            ("F#maj", "Gb major"),
            ("C Major", "C major"),
            ("Gb:maj", "Gb major"),
            ("c#:min", "C# minor"),
            ("C-", "C minor"),
            (" g minor ", "G minor"),
        ] {
            assert_eq!(Key::parse(text).map(|k| k.name()).as_deref(), Some(name), "{text}");
        }
        for text in ["", "H", "minor", "C dorian", "D mixolydian", "7"] {
            assert_eq!(Key::parse(text), None, "{text}");
        }
    }

    #[test]
    fn relatives_parallels_and_scales() {
        assert_eq!(key("C").relative(), key("Am"));
        assert_eq!(key("Cm").relative(), key("Eb"));
        assert_eq!(key("C").parallel(), key("Cm"));
        assert_eq!(key("A").scale().to_string(), "Db D E F# Ab A B");
        assert_eq!(key("Am").scale(), key("C").scale());
        for k in Key::ALL {
            assert_eq!(k.relative().relative(), k);
            assert_eq!(k.relative().scale(), k.scale());
        }
    }

    #[test]
    fn camelot_codes() {
        let codes = ["C", "Am", "G", "Em", "F", "Db", "B", "F#", "E", "Ebm"].map(|k| key(k).camelot());
        assert_eq!(codes.join(" "), "8B 8A 9B 9A 7B 3B 1B 2B 12B 2A");
    }

    #[test]
    fn wrong_keys_score_as_mirex_scores_them() {
        let c = key("C");
        let score = |answer: &str| key(answer).mirex_score(c, false);
        assert_eq!(
            [score("C"), score("G"), score("F"), score("Am"), score("Cm"), score("D")],
            [1.0, 0.5, 0.0, 0.3, 0.2, 0.0]
        );
        assert_eq!(key("F").mirex_score(c, true), 0.5);
        // From a minor key: its relative is the major a minor third up.
        assert_eq!(key("Eb").mirex_score(key("Cm"), false), 0.3);
        assert_eq!(key("Gm").mirex_score(key("Cm"), false), 0.5);
        assert_eq!(key("Em").relation_to(c), KeyRelation::Other);
    }
}
