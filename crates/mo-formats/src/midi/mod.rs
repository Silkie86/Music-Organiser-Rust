//! MIDI files from the MIDI library (loop packs, sketches): read only, never changed. Standard MIDI Files are read
//! here as the C# app's `MidiFileReader` reads them; each track (or each channel of a track holding several) becomes
//! a part. MIDI 2.0's Clip Files are read too ([`clip`]), a part for each channel with notes.

pub mod clip;
pub mod smf;

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::io;
use std::path::Path;

use crate::model::NoteEvent;

/// Whether the MIDI library reads a path: a Standard MIDI File or a MIDI Clip File, by its ending.
pub fn is_midi_path(path: &str) -> bool {
    smf::is_midi_path(path) || clip::is_clip_path(path)
}

/// Reads a MIDI file's bytes, already read from `path`: a MIDI Clip File when it starts as one (whatever its
/// ending), a Standard MIDI File otherwise.
pub fn read_from(raw: &[u8], path: &Path) -> io::Result<Vec<MidiPart>> {
    if raw.starts_with(clip::MAGIC) { clip::read_bytes(raw) } else { smf::read_from(raw, path) }
}

/// Reads a MIDI file from disk, as [`read_from`].
pub fn read(path: &Path) -> io::Result<Vec<MidiPart>> {
    read_from(&std::fs::read(path)?, path)
}

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

/// Notes still held, by key (a channel and a note), with what each was struck with; in the order the keys were first
/// struck, so notes never released end in the same order every time (and as in the C# app).
pub(crate) struct Held<K, V> {
    index: HashMap<K, usize>,
    keys: Vec<(K, VecDeque<V>)>,
}

impl<K, V> Default for Held<K, V> {
    fn default() -> Self {
        Held { index: HashMap::new(), keys: Vec::new() }
    }
}

impl<K: Hash + Eq + Copy, V> Held<K, V> {
    /// A key struck. Struck again before it's released, it stacks: each release ends the earliest.
    pub(crate) fn strike(&mut self, key: K, struck: V) {
        let i = *self.index.entry(key).or_insert_with(|| {
            self.keys.push((key, VecDeque::new()));
            self.keys.len() - 1
        });
        if let Some((_, queue)) = self.keys.get_mut(i) {
            queue.push_back(struck);
        }
    }

    /// A key released: the earliest of its notes still held, if any (a release of a key never struck starts nothing).
    pub(crate) fn release(&mut self, key: K) -> Option<V> {
        let i = *self.index.get(&key)?;
        self.keys.get_mut(i)?.1.pop_front()
    }

    /// Every note still held, key by key in the order the keys were first struck.
    pub(crate) fn into_held(self) -> impl Iterator<Item = (K, V)> {
        self.keys.into_iter().flat_map(|(k, q)| q.into_iter().map(move |v| (k, v)))
    }
}
