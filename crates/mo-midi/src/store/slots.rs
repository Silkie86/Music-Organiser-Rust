//! Each set of notes' slots in the index (`slots`): how long it plays each pitch class in every two quarter notes
//! ([`crate::fit::slots_of`]). A search for parts that go with a song sorts every part by these first
//! (`mo_harmony`'s first look), and only reads the notes of the best of them.
//!
//! They're made from the notes alone, so they're kept per set of notes, with what made them
//! ([`SLOTS_VERSION`]): when that changes they're made again from the notes the index keeps, no file opened
//! (rule 7). A search loads them all into memory once ([`MidiStore::slots`], a [`SlotBook`]).

use std::collections::HashMap;

use mo_formats::NoteEvent;
use mo_harmony::Slot;
use rusqlite::params;

use super::{MidiStore, decode_notes};
use crate::fit::SLOTS_VERSION;
use crate::index::MidiError;

/// Slots as the index keeps them. For each slot: which pitch classes sound in it (two bytes, a bit each from C up,
/// the low byte first), then how long each of those does (a byte each, from C up). Most of a slot's twelve are
/// silent, so a tune's slot takes 3 or 4 bytes and a chord's 5 to 7, where all twelve would take 12.
pub(super) fn encode(slots: &[Slot]) -> Vec<u8> {
    let mut data = Vec::with_capacity(slots.len() * 6);
    for slot in slots {
        let sounding = slot.iter().enumerate().filter(|(_, time)| **time > 0);
        let mask = sounding.fold(0u16, |mask, (pc, _)| mask | 1 << pc);
        data.extend(mask.to_le_bytes());
        data.extend(slot.iter().filter(|time| **time > 0));
    }
    data
}

/// Puts the slots [`encode`] wrote at the end of `slots`. None for a damaged row (cut short, or naming a thirteenth
/// pitch class), with what it put there before it found out left for the caller to take off: the part is then left
/// out of a search rather than sorted by something made up.
fn decode_into(data: &[u8], slots: &mut Vec<Slot>) -> Option<()> {
    let mut rest = data;
    while let [low, high, after @ ..] = rest {
        let mask = u16::from_le_bytes([*low, *high]);
        if mask >> 12 != 0 {
            return None;
        }
        let (times, after) = after.split_at_checked(mask.count_ones() as usize)?;
        let mut times = times.iter();
        let mut slot = [0u8; 12];
        for (pc, time) in slot.iter_mut().enumerate() {
            if mask & (1 << pc) != 0 {
                *time = *times.next()?;
            }
        }
        slots.push(slot);
        rest = after;
    }
    rest.is_empty().then_some(())
}

/// Every part's slots in memory, by note hash (the same notes have the same slots, whatever file they're in): what
/// a search's first look goes through. About 200 bytes a part.
#[derive(Clone, Debug, Default)]
pub struct SlotBook {
    /// Where each note hash's slots are in `slots`: the first of them, and how many.
    places: HashMap<String, (u32, u16)>,
    slots: Vec<Slot>,
}

impl SlotBook {
    /// The slots of the notes with this hash; none when the index has none for them yet.
    pub fn of(&self, hash: &str) -> Option<&[Slot]> {
        let (first, count) = *self.places.get(hash)?;
        let first = usize::try_from(first).ok()?;
        self.slots.get(first..first.checked_add(usize::from(count))?)
    }

    /// How many sets of notes it has slots for.
    pub fn len(&self) -> usize {
        self.places.len()
    }

    pub fn is_empty(&self) -> bool {
        self.places.is_empty()
    }

    /// Keeps a set of notes' slots; the first kept for a hash stays.
    pub fn add(&mut self, hash: &str, slots: &[Slot]) {
        self.keep(hash, |all| {
            all.extend_from_slice(slots);
            Some(())
        });
    }

    /// Keeps the slots `put` puts at the end of all of them, for a hash that has none yet (the hash is looked up
    /// before anything is made for it). Nothing is kept when `put` gives up part of the way (a damaged row).
    fn keep(&mut self, hash: &str, put: impl FnOnce(&mut Vec<Slot>) -> Option<()>) {
        if self.places.contains_key(hash) {
            return;
        }
        let first = self.slots.len();
        let count = put(&mut self.slots).and_then(|()| u16::try_from(self.slots.len() - first).ok());
        match (u32::try_from(first), count) {
            (Ok(place), Some(count)) => {
                self.places.insert(hash.to_owned(), (place, count));
            }
            _ => self.slots.truncate(first),
        }
    }
}

impl MidiStore {
    /// How many sets of notes have no slots yet, or slots another [`SLOTS_VERSION`] made: two counts, each read
    /// from a small index, so it's quick when there are none. (That's what each row's version and its index are
    /// for: one version for the whole table would do to know when to make them again, but counting the rows would
    /// then walk every slot the table holds, each time the indexer runs.)
    pub fn without_slots_count(&self) -> Result<usize, MidiError> {
        let n: i64 = self.conn.query_row(
            "SELECT (SELECT COUNT(*) FROM notes) - (SELECT COUNT(*) FROM slots WHERE version = ?1)",
            [SLOTS_VERSION],
            |r| r.get(0),
        )?;
        Ok(n.max(0) as usize)
    }

    /// Notes with no slots of this version after the set of notes `after` (an id; 0 for the first), in id order, up
    /// to `limit`: each with its notes. The caller goes on from the last one given, so each row is looked at once.
    pub fn without_slots(&self, after: i64, limit: usize) -> Result<Vec<(i64, Vec<NoteEvent>)>, MidiError> {
        let mut st = self.conn.prepare_cached(
            "SELECT d.notes, d.data FROM note_data d
             WHERE d.notes > ?1
               AND NOT EXISTS (SELECT 1 FROM slots s WHERE s.notes = d.notes AND s.version = ?3)
             ORDER BY d.notes LIMIT ?2",
        )?;
        let rows = st.query_map(params![after, limit as i64, SLOTS_VERSION], |r| {
            Ok((r.get(0)?, decode_notes(&r.get::<_, Vec<u8>>(1)?)))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Keeps the slots worked out for [`MidiStore::without_slots`] notes, in place of any another version made.
    pub fn save_slots(&mut self, slots: &[(i64, Vec<Slot>)]) -> Result<(), MidiError> {
        let tx = self.conn.transaction()?;
        {
            let mut st = tx.prepare_cached(
                "INSERT INTO slots (notes, version, data) VALUES (?1, ?2, ?3)
                 ON CONFLICT(notes) DO UPDATE SET version = ?2, data = ?3",
            )?;
            for (notes, slots) in slots {
                st.execute(params![notes, SLOTS_VERSION, encode(slots)])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Every set of notes' slots, for a search: read once and kept in memory. Only this version's: a part whose
    /// slots aren't made yet (the indexer makes them after its next read) isn't in it, and neither is anything in an
    /// index from before slots were kept.
    pub fn slots(&self) -> Result<SlotBook, MidiError> {
        let mut book = SlotBook::default();
        // Asked each time, not once when the index was opened: the page keeps its index open, and one from before
        // slots were kept gets the table when the indexer next opens it.
        let kept: bool = self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'slots')",
            [],
            |r| r.get(0),
        )?;
        if !kept {
            return Ok(book);
        }
        // The whole table in its own order (the `+` keeps SQLite off the version index, which would jump about in
        // it), each row's note hash looked up by id.
        let mut st = self
            .conn
            .prepare("SELECT n.hash, s.data FROM slots s JOIN notes n ON n.id = s.notes WHERE +s.version = ?1")?;
        let mut rows = st.query([SLOTS_VERSION])?;
        while let Some(r) = rows.next()? {
            // A row that isn't what was written (not bytes, or damaged) is left out: never taken for a part with no
            // notes in it. Each row's slots go straight to their place in the book.
            if let (Ok(hash), Ok(data)) = (r.get_ref(0)?.as_str(), r.get_ref(1)?.as_blob()) {
                book.keep(hash, |slots| decode_into(data, slots));
            }
        }
        Ok(book)
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn decode(data: &[u8]) -> Option<Vec<Slot>> {
        let mut slots = Vec::new();
        decode_into(data, &mut slots).map(|()| slots)
    }

    #[test]
    fn slots_are_kept_small_and_come_back_the_same() {
        let mut tune = [0u8; 12];
        tune[4] = 64;
        let mut chord = [0u8; 12];
        (chord[0], chord[4], chord[7], chord[11]) = (64, 64, 32, 255);
        let slots = vec![tune, [0; 12], chord];
        let data = encode(&slots);
        // Two bytes a slot for which pitch classes sound, and one for each that does.
        assert_eq!(data.len(), 3 + 2 + 6);
        assert_eq!(decode(&data), Some(slots));
        assert_eq!(decode(&[]), Some(Vec::new()));
        // Damaged: cut short, a byte left over, a thirteenth pitch class.
        assert_eq!(decode(&data[..data.len() - 1]), None);
        assert_eq!(decode(&[data.as_slice(), &[1]].concat()), None);
        assert_eq!(decode(&[0, 0x10]), None);
    }

    #[test]
    fn a_book_gives_each_hashs_slots_and_keeps_the_first() {
        let mut book = SlotBook::default();
        assert!(book.is_empty() && book.of("A").is_none());
        book.add("A", &[[1; 12], [2; 12]]);
        book.add("B", &[]);
        book.add("C", &[[3; 12]]);
        book.add("A", &[[9; 12]]);
        // A damaged row keeps nothing, and what came before it and comes after it is where it was.
        let damaged = encode(&[[4; 12], [5; 12]]);
        book.keep("E", |slots| decode_into(&damaged[..damaged.len() - 1], slots));
        book.keep("F", |slots| decode_into(&encode(&[[6; 12]]), slots));
        assert!(book.of("E").is_none());
        assert_eq!(book.of("F"), Some([[6; 12]].as_slice()));
        assert_eq!(book.len(), 4);
        assert_eq!(book.of("A"), Some([[1; 12], [2; 12]].as_slice()));
        assert_eq!(book.of("B"), Some([].as_slice()));
        assert_eq!(book.of("C"), Some([[3; 12]].as_slice()));
        assert!(book.of("D").is_none());
    }
}
