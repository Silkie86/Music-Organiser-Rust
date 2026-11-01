//! The key of a part, from `mo-harmony`'s key finder. The finder's whole reading is kept as a [`KeyFact`] (8 bytes)
//! for each set of notes in `midi-index.db`, with [`VERSION`], so it's read again when the finder, or how this crate
//! asks it, gets better (rule 7). The C# app's own key finder and chord reader aren't ported: `mo-harmony` replaces
//! them.

use std::cell::RefCell;

use mo_formats::NoteEvent;
use mo_harmony::{Grid, KEY_FINDER_VERSION, KeyFact, KeyReader, Mode, Note, Part, Span};

use crate::PPQ;
use crate::analyser::{self, FEATURE_COUNT, Shape};
use crate::index::Key;
use crate::roles::Role;

/// How this crate asks the key finder (the kinds it hears parts as, loop or window, the grid): bump it when that
/// changes.
const ASKING: u32 = 1;

/// What a stored reading was made with: the key finder's version and how it was asked. Read again when the app's is
/// higher.
pub const VERSION: u32 = KEY_FINDER_VERSION * 100 + ASKING;

thread_local! {
    /// Each thread's key finder, which keeps its working space from part to part.
    static READER: RefCell<KeyReader> = RefCell::new(KeyReader::new());
}

/// Runs `f` with this thread's own key finder.
pub fn with_reader<T>(f: impl FnOnce(&mut KeyReader) -> T) -> T {
    READER.with_borrow_mut(f)
}

/// How the key finder hears a part of this kind.
fn heard_as(role: Role) -> mo_harmony::Role {
    match role {
        Role::Melody => mo_harmony::Role::Melody,
        Role::Chords | Role::Arp => mo_harmony::Role::Harmony,
        Role::Bass => mo_harmony::Role::Bass,
        Role::Drums => mo_harmony::Role::Drums,
        Role::Other => mo_harmony::Role::Unknown,
    }
}

/// Reads the key of one part (notes at [`PPQ`]): a whole part or section as a loop, a piece `cut` from a longer stretch
/// as a window (where its first bass note needn't be home). Gives the key the part's shape is counted from (the
/// answer, or the key it leans to) and the whole reading to keep; none for drums or without pitched notes.
pub fn read(reader: &mut KeyReader, notes: &[NoteEvent], role: Role, cut: bool) -> (Option<Key>, Option<KeyFact>) {
    if role == Role::Drums {
        return (None, None);
    }
    let heard: Vec<Note> = notes
        .iter()
        .filter(|n| !n.muted)
        .map(|n| Note { start: n.start, length: n.length, key: n.key, velocity: n.velocity })
        .collect();
    let parts = [Part { role: heard_as(role), notes: &heard }];
    let span = if cut { Span::Window } else { Span::Loop };
    let Some(reading) = reader.read(&parts, Grid::four_four(PPQ as u32), span) else { return (None, None) };
    let key = reading.answer.leaning_key().map(|k| Key {
        tonic: k.tonic.number(),
        minor: k.mode == Mode::Minor,
        confidence: reading.shown_chance,
    });
    (key, Some(reading.fact()))
}

/// The key a stored reading names for sure (one key, or the likelier of a pair), for the page to show and search by.
/// A home note, or no clear key, gives none.
pub fn sure_key(fact: &KeyFact) -> Option<Key> {
    let (tonic, mode) = (fact.tonic()?, fact.mode()?);
    Some(Key { tonic: tonic.number(), minor: mode == Mode::Minor, confidence: fact.shown_chance() })
}

/// The key and the "more like this" numbers of stored notes, read again (a newer key finder, or the part's kind
/// changed). `role` is the kind a file holding them gives them now.
pub fn reread(
    reader: &mut KeyReader,
    notes: &[NoteEvent],
    role: Role,
    cut: bool,
) -> (Option<KeyFact>, [f32; FEATURE_COUNT]) {
    let (key, fact) = read(reader, notes, role, cut);
    let chroma = analyser::chroma_of(notes);
    (fact, analyser::scale_and_features(&Shape::of(notes), notes, &chroma, key).1)
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;
    use mo_formats::NoteAttribute;

    fn note(start: i64, key: u8) -> NoteEvent {
        NoteEvent {
            start,
            length: PPQ,
            key,
            velocity: 50_000,
            muted: false,
            step: false,
            attribute: NoteAttribute::default(),
        }
    }

    /// A tune in A minor, over and over: A C E D C B A E, then A G# A.
    fn a_minor() -> Vec<NoteEvent> {
        let tune = [69, 72, 76, 74, 72, 71, 69, 64, 69, 68, 69, 76];
        (0..4)
            .flat_map(|bar| tune.iter().enumerate().map(move |(i, k)| note(bar * 12 * PPQ + i as i64 * PPQ, *k)))
            .collect()
    }

    #[test]
    fn reads_a_tune_and_keeps_the_reading() {
        let mut reader = KeyReader::new();
        let notes = a_minor();
        let (key, fact) = read(&mut reader, &notes, Role::Melody, false);
        let fact = fact.unwrap();
        let key = key.unwrap();
        assert_eq!((key.tonic, key.minor), (9, true), "{fact:?}");
        // The fact holds the whole answer.
        assert_eq!(fact.tonic().map(|t| t.number()), Some(9));
        assert_eq!(read(&mut reader, &notes, Role::Drums, false), (None, None), "drums aren't heard");
        assert_eq!(read(&mut reader, &[], Role::Melody, false), (None, None), "nothing pitched");
    }

    #[test]
    fn reading_again_gives_what_analysing_gave() {
        let mut reader = KeyReader::new();
        let notes = a_minor();
        for cut in [false, true] {
            let a = analyser::analyse_reading(&notes, "", false, cut, &mut reader);
            let (fact, features) = reread(&mut reader, &notes, a.role, cut);
            assert_eq!((fact, features), (a.key_fact, a.features));
        }
    }
}
