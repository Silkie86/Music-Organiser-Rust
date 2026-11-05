//! The chords of a part, from `mo-harmony`'s chord reader, read from its notes when its card is shown (about a
//! millisecond for a loop, off the window's thread). Not kept in the index yet: searching by chords comes later.

use std::cell::RefCell;

use mo_formats::NoteEvent;
use mo_harmony::{ChordContext, ChordReader, ChordReading, Grid, Note, Part, Span};

use crate::PPQ;
use crate::keys::heard_as;
use crate::roles::Role;

thread_local! {
    /// Each thread's chord reader, which keeps its working space from part to part.
    static READER: RefCell<ChordReader> = RefCell::new(ChordReader::new());
}

/// Reads the chords of one part (notes at [`PPQ`]), heard as its kind: a whole part as a loop (its first chord comes
/// after its last), a piece `cut` from a longer stretch as a stretch of a song. None for drums, or with no pitched
/// notes.
pub fn read(notes: &[NoteEvent], role: Role, cut: bool) -> Option<ChordReading> {
    if role == Role::Drums {
        return None;
    }
    let pitched: Vec<Note> = notes
        .iter()
        .filter(|n| !n.muted)
        .map(|n| Note { start: n.start, length: n.length, key: n.key, velocity: n.velocity })
        .collect();
    if pitched.is_empty() {
        return None;
    }
    let parts = [Part { role: heard_as(role), notes: &pitched }];
    let span = if cut { Span::Window } else { Span::Loop };
    READER.with_borrow_mut(|r| r.read(&parts, Grid::four_four(PPQ as u32), &ChordContext::new(span)))
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use mo_formats::NoteAttribute;

    fn chord(bar: i64, keys: &[u8]) -> Vec<NoteEvent> {
        keys.iter()
            .map(|&key| NoteEvent {
                start: bar * 4 * PPQ,
                length: 4 * PPQ,
                key,
                velocity: 50_000,
                muted: false,
                step: false,
                attribute: NoteAttribute::default(),
            })
            .collect()
    }

    #[test]
    fn reads_a_loop_of_chords() {
        // Am7, Dm7, G7, Cmaj7, a bar each.
        let notes: Vec<NoteEvent> = [
            chord(0, &[45, 60, 64, 67]),
            chord(1, &[50, 60, 65, 69]),
            chord(2, &[43, 59, 62, 65]),
            chord(3, &[48, 59, 64, 67]),
        ]
        .concat();
        let reading = read(&notes, Role::Chords, false).unwrap();
        let names: Vec<String> =
            reading.names(None).into_iter().map(|n| n.map(|s| s.name()).unwrap_or_default()).collect();
        assert_eq!(names, ["Am7", "Dm7", "G7", "Cmaj7"]);
        assert!(read(&notes, Role::Drums, false).is_none());
        assert!(read(&[], Role::Chords, false).is_none());
    }
}
