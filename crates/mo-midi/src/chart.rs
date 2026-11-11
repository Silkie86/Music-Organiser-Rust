//! The chords of a part, from `mo-harmony`'s chord reader, read from its notes when its card is shown (about a
//! millisecond for a loop, off the window's thread). Not kept in the index yet: searching by chords comes later.

use std::cell::RefCell;

use mo_formats::NoteEvent;
use mo_harmony::{Backing, ChordContext, ChordReader, ChordReading, Grid, Note, Part, Span};

use crate::keys::{heard_as, heard_notes};
use crate::roles::Role;
use crate::{BAR, PPQ};

thread_local! {
    /// Each thread's chord reader, which keeps its working space from part to part (as the key finder's does).
    static READER: RefCell<ChordReader> = RefCell::new(ChordReader::new());
}

/// The most bars a chart is read for: what the card shows. A longer part is read for its first ones only, as a stretch
/// of music, so a long song piece costs no more than a loop.
pub const MOST_BARS: usize = 16;

/// A part's chords as the card's chart shows them.
#[derive(Clone, Debug)]
pub struct Read {
    pub reading: ChordReading,
    /// The part goes on past the [`MOST_BARS`] read.
    pub goes_on: bool,
}

/// Runs `f` with this thread's own chord reader.
pub(crate) fn with_reader<T>(f: impl FnOnce(&mut ChordReader) -> T) -> T {
    READER.with_borrow_mut(f)
}

/// Reads the chords of one part (notes at [`PPQ`], bars of 4/4), heard as its kind: a whole part as a loop (its first
/// chord comes after its last), a piece `cut` from a longer stretch, or the first [`MOST_BARS`] of a longer part, as a
/// stretch of a song. None for drums, or with no pitched notes.
pub fn read(notes: &[NoteEvent], role: Role, cut: bool) -> Option<Read> {
    let (heard, goes_on) = heard(notes, role)?;
    let reading = reading_of(&heard, role, cut || goes_on)?;
    Some(Read { reading, goes_on })
}

/// The same part's chords kept to fit tunes over ([`mo_harmony::sit`]): read as [`read`] reads them, its first
/// [`MOST_BARS`] bars at most.
pub fn backing(notes: &[NoteEvent], role: Role, cut: bool) -> Option<Backing> {
    let (heard, goes_on) = heard(notes, role)?;
    let reading = reading_of(&heard, role, cut || goes_on)?;
    Some(Backing::of(&reading, &[Part { role: heard_as(role), notes: &heard }]))
}

/// The notes [`read`] hears: the pitched ones of the first [`MOST_BARS`] bars, and whether the part goes on past them.
fn heard(notes: &[NoteEvent], role: Role) -> Option<(Vec<Note>, bool)> {
    if role == Role::Drums {
        return None;
    }
    let mut heard = heard_notes(notes);
    let first = heard.iter().map(|n| n.start).min()?;
    let until = first.div_euclid(BAR) * BAR + MOST_BARS as i64 * BAR;
    let goes_on = heard.iter().any(|n| n.start >= until);
    if goes_on {
        heard.retain(|n| n.start < until);
        for n in &mut heard {
            n.length = n.length.min(until - n.start);
        }
    }
    Some((heard, goes_on))
}

fn reading_of(heard: &[Note], role: Role, window: bool) -> Option<ChordReading> {
    let parts = [Part { role: heard_as(role), notes: heard }];
    let span = if window { Span::Window } else { Span::Loop };
    with_reader(|r| r.read(&parts, Grid::four_four(PPQ as u32), &ChordContext::new(span)))
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
        let read_ = read(&notes, Role::Chords, false).unwrap();
        assert!(!read_.goes_on);
        let names: Vec<String> =
            read_.reading.names(None).into_iter().map(|n| n.map(|s| s.name()).unwrap_or_default()).collect();
        assert_eq!(names, ["Am7", "Dm7", "G7", "Cmaj7"]);
        assert!(read(&notes, Role::Drums, false).is_none());
        assert!(read(&[], Role::Chords, false).is_none());
        // A long part: its first bars only.
        let long: Vec<NoteEvent> = (0..40).flat_map(|bar| chord(bar, &[45, 60, 64, 67])).collect();
        let first = read(&long, Role::Chords, false).unwrap();
        assert!(first.goes_on);
        assert!(first.reading.end <= MOST_BARS as i64 * BAR);
    }
}
