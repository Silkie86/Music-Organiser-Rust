//! The key of a part, from `mo-harmony`'s key finder. The finder's whole reading is kept as a [`KeyFact`] (8 bytes)
//! for each set of notes in `midi-index.db`, with [`VERSION`], so it's read again when the finder, or how this crate
//! asks it, gets better (rule 7). The C# app's own key finder and chord reader aren't ported: `mo-harmony` replaces
//! them.

use std::cell::RefCell;

use mo_formats::NoteEvent;
use mo_harmony::keyfind::{self, SHIPPED};
use mo_harmony::{Grid, Heard, KEY_FINDER_VERSION, KeyFact, KeyReader, Mode, Note, Part, Span};

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
    let Some((heard, span)) = hear(reader, notes, role, cut) else { return (None, None) };
    let reading = keyfind::read_heard(&heard, span, &SHIPPED);
    let key = reading.answer.leaning_key().map(|k| Key {
        tonic: k.tonic.number(),
        minor: k.mode == Mode::Minor,
        confidence: reading.shown_chance,
    });
    (key, Some(reading.fact()))
}

/// What the key finder hears in one part, as [`read`] hears it, and how that is read: as a loop, or as a window for a
/// piece cut from a longer stretch. `keyfind::read_heard` reads it with any finder's numbers ([`SHIPPED`] for the one
/// built in; `theorybench keycheck` reads it with an older one's too, to compare the two). None for drums or without
/// pitched notes.
pub fn hear(reader: &mut KeyReader, notes: &[NoteEvent], role: Role, cut: bool) -> Option<(Heard, Span)> {
    if role == Role::Drums {
        return None;
    }
    let pitched: Vec<Note> = notes
        .iter()
        .filter(|n| !n.muted)
        .map(|n| Note { start: n.start, length: n.length, key: n.key, velocity: n.velocity })
        .collect();
    let parts = [Part { role: heard_as(role), notes: &pitched }];
    let span = if cut { Span::Window } else { Span::Loop };
    Some((reader.hear(&parts, Grid::four_four(PPQ as u32))?, span))
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
    use mo_harmony::KeyAnswer;

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
        // The fact holds the whole answer, the key it leans to with it: A minor, whether or not the finder is sure
        // enough to give it as the key (a tune alone seldom is).
        let leaning = fact.answer().and_then(KeyAnswer::leaning_key);
        assert_eq!(leaning.map(|k| (k.tonic.number(), k.mode)), Some((9, Mode::Minor)), "{fact:?}");
        assert_eq!(read(&mut reader, &notes, Role::Drums, false), (None, None), "drums aren't heard");
        assert_eq!(read(&mut reader, &[], Role::Melody, false), (None, None), "nothing pitched");
    }

    /// Chords held a bar each, the progression played twice.
    fn chords(progression: &[&[u8]]) -> Vec<NoteEvent> {
        let bar = 4 * PPQ;
        (0..2)
            .flat_map(|round| {
                progression.iter().enumerate().map(move |(i, chord)| (round * progression.len() + i, *chord))
            })
            .flat_map(|(at, chord)| chord.iter().map(move |&k| NoteEvent { length: bar, ..note(at as i64 * bar, k) }))
            .collect()
    }

    #[test]
    fn a_part_the_finder_is_sure_of_gives_the_key_to_show() {
        let mut reader = KeyReader::new();
        // Am Dm E Am: A minor for sure, the key the page shows and searches by.
        let notes = chords(&[&[69, 72, 76], &[62, 65, 69], &[64, 68, 71], &[69, 72, 76]]);
        let (_, fact) = read(&mut reader, &notes, Role::Chords, false);
        let fact = fact.unwrap();
        assert!(matches!(fact.answer(), Some(KeyAnswer::Key(_))), "{fact:?}");
        let shown = sure_key(&fact).expect("a key to show");
        assert_eq!((shown.tonic, shown.minor), (9, true), "{fact:?}");
    }

    #[test]
    fn what_a_part_is_heard_as_is_read_with_the_numbers_given() {
        let mut reader = KeyReader::new();
        let notes = a_minor();
        let (heard, span) = hear(&mut reader, &notes, Role::Melody, false).unwrap();
        assert_eq!(span, Span::Loop);
        // No weight for anything: every key as likely as the next, so no clear key and none to lean to.
        let flat = [0.0; keyfind::FEATURES];
        let none = keyfind::read_heard(&heard, span, &keyfind::Model { weights: &flat, ..SHIPPED });
        assert!(none.chances.iter().all(|&p| (p - 1.0 / 24.0).abs() < 1e-12), "{none:?}");
        assert!(matches!(none.answer, KeyAnswer::Unclear { leaning: None, .. }), "{none:?}");
        // With the numbers built in, the reading is the one `read` keeps.
        let built_in = keyfind::read_heard(&heard, span, &SHIPPED);
        assert_eq!(Some(built_in.fact()), read(&mut reader, &notes, Role::Melody, false).1);
        // A piece cut from a longer stretch is read as a window; drums aren't heard.
        assert_eq!(hear(&mut reader, &notes, Role::Bass, true).map(|(_, span)| span), Some(Span::Window));
        assert!(hear(&mut reader, &notes, Role::Drums, false).is_none(), "drums aren't heard");
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
