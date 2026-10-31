//! Note names: how the twelve pitch classes are written, and reading a written one back.
//!
//! Which spelling to use (C♯ or D♭) depends on the key and on what the note does; the tables here are the plain
//! choices, used where nothing says otherwise.

use crate::pitch::PitchClass;
use crate::spell::Tpc;

/// Flats for the black notes except F♯: how jazz charts write a chord's root when no key says otherwise.
pub const FLATS_BUT_F_SHARP: [&str; 12] = ["C", "Db", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];
/// Sharps for every black note.
pub const SHARPS: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
/// Flats for every black note.
pub const FLATS: [&str; 12] = ["C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B"];

/// The name of a pitch class from one of the tables above.
// A pitch class is always 0 to 11.
#[allow(clippy::indexing_slicing)]
pub fn name(table: &'static [&'static str; 12], pc: PitchClass) -> &'static str {
    table[pc.index()]
}

/// Reads a note name at the start of `text`: a letter A to G (either case) and up to two sharps or flats ("#", "b",
/// "♯", "♭"). Gives the pitch class and how many bytes of `text` the name took, or `None` when it doesn't start
/// with a note. A "b" after the letter always flattens: "Bb" is B flat, and "Bbm" is B flat minor. To keep the
/// spelling too, see [`crate::spell::Tpc::parse`].
pub fn parse_note(text: &str) -> Option<(PitchClass, usize)> {
    Tpc::parse(text).map(|(note, used)| (note.pitch_class(), used))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_names_are_read_with_their_sharps_and_flats() {
        let read = |s: &str| parse_note(s).map(|(pc, used)| (pc.number(), used));
        assert_eq!(read("C"), Some((0, 1)));
        assert_eq!(read("c#m7"), Some((1, 2)));
        assert_eq!(read("Bb"), Some((10, 2)));
        assert_eq!(read("Bbm"), Some((10, 2)));
        assert_eq!(read("Cb"), Some((11, 2)));
        assert_eq!(read("E#"), Some((5, 2)));
        assert_eq!(read("F##"), Some((7, 3)));
        assert_eq!(read("Abb7"), Some((7, 3)));
        assert_eq!(read("G♭maj7"), Some((6, 4)));
        assert_eq!(read("H"), None);
        assert_eq!(read(""), None);
        assert_eq!(read("7"), None);
    }

    #[test]
    fn every_table_names_every_pitch_class_back() {
        for table in [&FLATS_BUT_F_SHARP, &SHARPS, &FLATS] {
            for pc in PitchClass::ALL {
                let written = name(table, pc);
                assert_eq!(parse_note(written), Some((pc, written.len())));
            }
        }
    }
}
