//! MIDI files from the MIDI library (loop packs, sketches): read only, never changed. Standard MIDI Files are read
//! here as the C# app's `MidiFileReader` reads them; each track (or each channel of a track holding several) becomes
//! a part.

pub mod smf;

use crate::model::NoteEvent;

/// One part found in a MIDI file: a track, or one MIDI channel of a track that holds several, with its notes.
#[derive(Clone, Debug, PartialEq)]
pub struct MidiPart {
    /// What the MIDI page calls the part: the track's name (unless it's only the file's name again), with its
    /// channel when the track was split; "" for a file's only part.
    pub name: String,
    pub ppq: u16,
    /// In order of start, then key.
    pub notes: Vec<NoteEvent>,
    /// On MIDI channel 10, which General MIDI keeps for drums.
    pub drums: bool,
    /// The file's first tempo, in beats a minute, if it gives one.
    pub tempo: Option<f64>,
    /// The General MIDI program (sound, 0 to 127) the part's channel is first set to, if the file sets one.
    pub program: Option<u8>,
    /// The track's own name, kept even where `name` leaves it out.
    pub track_name: String,
}
