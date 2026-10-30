//! Pattern notes (event 224): records of u32 position, u16 flags, u16 channel, u32 length, u16 key, and more.
//! FL 9 onwards writes 24 bytes a note, with the velocity at 21; FL 4 to 8 20 bytes, with the velocity at 17 (checked
//! on Solomon's saves from FL 4 to FL 26).

use std::collections::HashSet;

use super::events::{byte_at, u16_at, u32_at};
use crate::model::{NoteEvent, velocity};

/// The size of the note records in an event, by FL's version (rule 9): 24 bytes from FL 9, 20 bytes in FL 4 to 8.
/// Notes the reader made from old step sequencer dots are always 24. Without a version, or in an FL 4 to 8 save whose
/// notes don't divide into 20 bytes, the size is told from the notes themselves; `channels` are the song's channels,
/// when known, which a right reading only ever names.
pub fn note_size(data: &[u8], major: u32, made_here: bool, channels: Option<&HashSet<u32>>) -> usize {
    match major {
        _ if made_here => 24,
        9.. => 24,
        1.. if data.len().is_multiple_of(20) => 20,
        _ => size_from_notes(data, channels),
    }
}

/// The record size that reads the notes in time order (as FL keeps them) on known channels, else the one that divides
/// them (on known channels), 24 when both or neither do. Read as 24-byte notes, Solomon's FL 7 "i dont know_8" got
/// notes at jumbled times lasting 2^31 ticks: its patterns were 120 and 240 bytes long, a whole number of either size.
fn size_from_notes(data: &[u8], channels: Option<&HashSet<u32>>) -> usize {
    let known = |size: usize| {
        data.len().is_multiple_of(size)
            && channels.is_none_or(|known| data.chunks_exact(size).all(|r| known.contains(&(u16_at(r, 6) as u32))))
    };
    let in_order =
        |size: usize| !data.is_empty() && known(size) && data.chunks_exact(size).map(|r| u32_at(r, 0)).is_sorted();
    if in_order(24) {
        24
    } else if in_order(20) || (known(20) && !known(24)) {
        20
    } else {
        24
    }
}

/// The notes in an event, each with its channel. A note muted in the piano roll (24-byte records: bit 0x20 of byte 19)
/// is marked muted; a length of 0 is a step from the step sequencer, kept 1 tick long and marked.
pub fn decode(data: &[u8], size: usize) -> Vec<(u32, NoteEvent)> {
    let velocity_at = size - 3;
    data.chunks_exact(size)
        .map(|r| {
            let length = u32_at(r, 8);
            // FL's velocity is 0 to 128 (100 by default), kept in MIDI 2.0's 16 bits.
            let velocity = velocity::from_unit(byte_at(r, velocity_at) as f64 / 128.0);
            let note = NoteEvent {
                start: u32_at(r, 0) as i64,
                length: length.max(1) as i64,
                key: (u16_at(r, 12) & 0x7F) as u8,
                velocity,
                muted: size == 24 && byte_at(r, 19) & 0x20 != 0,
                step: length == 0,
            };
            (u16_at(r, 6) as u32, note)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(pos: u32, channel: u16, length: u32, key: u16, velocity: u8, size: usize) -> Vec<u8> {
        let mut r = vec![0u8; size];
        r[..4].copy_from_slice(&pos.to_le_bytes());
        r[6..8].copy_from_slice(&channel.to_le_bytes());
        r[8..12].copy_from_slice(&length.to_le_bytes());
        r[12..14].copy_from_slice(&key.to_le_bytes());
        r[size - 3] = velocity;
        r
    }

    #[test]
    fn fl_4_to_8_notes_are_20_bytes_even_when_they_also_divide_into_24() {
        // Six 20-byte notes are 120 bytes: five 24-byte ones too.
        let data: Vec<u8> = (0..6).flat_map(|i| note(i * 96, 0, 96, 60 + i as u16, 100, 20)).collect();
        assert_eq!(note_size(&data, 7, false, None), 20);
        let notes = decode(&data, 20);
        assert_eq!(
            notes.iter().map(|(_, n)| (n.start, n.key)).collect::<Vec<_>>(),
            [(0, 60), (96, 61), (192, 62), (288, 63), (384, 64), (480, 65)]
        );
        // Without a version, time order tells: read as 24 bytes these would jump about.
        assert_eq!(note_size(&data, 0, false, Some(&HashSet::from([0]))), 20);
    }

    #[test]
    fn from_fl_9_notes_are_always_24_bytes() {
        let mut data = note(0, 0, 96, 60, 100, 24);
        data.extend([0; 4]);
        assert_eq!(note_size(&data, 21, false, None), 24, "a damaged event isn't read as 20-byte notes");
        assert_eq!(decode(&data, 24).len(), 1);
    }

    #[test]
    fn velocity_steps_and_muted_notes() {
        let mut data = note(0, 2, 0, 0x80 | 64, 128, 24);
        data[19] = 0x20;
        let (channel, n) = decode(&data, 24)[0];
        assert_eq!((channel, n.key, n.velocity7(), n.length, n.step, n.muted), (2, 64, 127, 1, true, true));
        assert_eq!(decode(&note(0, 0, 10, 60, 64, 24), 24)[0].1.velocity, 32768, "half of 128 is half of 16 bits");
        assert_eq!(decode(&note(0, 0, 10, 60, 64, 24), 24)[0].1.velocity7(), 64, "63.5 rounds to the even 64");
        assert_eq!(decode(&note(0, 0, 10, 60, 0, 24), 24)[0].1.velocity7(), 1);
    }
}
