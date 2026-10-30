//! Reading Standard MIDI Files: the C# app's `MidiBrowserTests` and `InstrumentTests`, on the same made-up files, and
//! more.

// Test files may unwrap: a test that stops there has failed, which is what it should do.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::midi::*;
use mo_formats::midi::smf::{self, read_bytes};
use mo_formats::{NoteEvent, velocity};

/// A note as a MIDI 1.0 file writes it.
fn note(start: i64, length: i64, key: u8, vel: u8) -> NoteEvent {
    NoteEvent { start, length, key, velocity: velocity::from_7(vel), muted: false, step: false, attribute: None }
}

#[test]
fn running_status_note_on_at_zero_velocity_and_drum_channel_are_understood() {
    let track = [
        0x00, 0x99, 36, 100, // kick on channel 10
        0x00, 38, 90, // snare, running status
        0x60, 36, 0, // kick off (velocity 0)
        0x00, 38, 0, // snare off
        0x00, 0x90, 60, 80, // C4 on channel 1
        0x83, 0x00, 0x80, 60, 0, // 384 ticks later: off
        0x00, 0xff, 0x2f, 0x00,
    ];
    let parts = read_bytes(&smf(96, &[&track]), "Loop").unwrap();
    assert_eq!(parts.len(), 2);
    let drums = parts.iter().find(|p| p.drums).unwrap();
    assert_eq!(drums.notes.iter().map(|n| n.key).collect::<Vec<_>>(), [36, 38]);
    assert!(drums.notes.iter().all(|n| n.length == 96));
    assert_eq!(parts.iter().find(|p| !p.drums).unwrap().notes, [note(96, 384, 60, 80)]);
    // One track split by channel: each part is named by its channel.
    assert_eq!(parts.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Ch 1", "Ch 10"]);
}

#[test]
fn running_status_goes_on_after_a_lyric() {
    let track = [
        0x00, 0x90, 60, 80, // C4 on
        0x00, 0xff, 0x05, 2, b'L', b'a', // a lyric
        0x60, 60, 0, // C4 off, running status
        0x00, 62, 80, // D4 on
        0x00, 0xf0, 1, 0xf7, // a sysex
        0x60, 62, 0, // D4 off
        0x00, 0xff, 0x2f, 0x00,
    ];
    let parts = read_bytes(&smf(96, &[&track]), "Song").unwrap();
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0].notes, [note(0, 96, 60, 80), note(96, 96, 62, 80)]);
}

#[test]
fn not_a_midi_file_says_so() {
    let e = read_bytes(b"RIFF0000WAVE", "x").unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::InvalidData);
    assert!(read_bytes(b"", "x").is_err());
    assert!(read_bytes(b"MThd\0\0\0\x06\0", "x").is_err());
}

#[test]
fn a_midi_files_sound_is_read_and_drums_are_channel_10() {
    let part = read_bytes(&smf(96, &[&track_with_program(0, Some(38), "")]), "x").unwrap();
    assert_eq!((part.len(), part[0].program, part[0].drums), (1, Some(38), false));
    let drums = read_bytes(&smf(96, &[&track_with_program(9, Some(0), "")]), "x").unwrap();
    assert_eq!((drums.len(), drums[0].program, drums[0].drums), (1, Some(0), true));
    let none = read_bytes(&smf(96, &[&track_with_program(0, None, "Keyscape")]), "Night drive").unwrap();
    assert_eq!(none[0].program, None);
    // A file's only part has no name of its own, but keeps its track's.
    assert_eq!((none[0].name.as_str(), none[0].track_name.as_str()), ("", "Keyscape"));
}

#[test]
fn tracks_are_parts_with_the_first_tempo_and_each_channels_first_sound() {
    let parts = read_bytes(&two_tracks(), "Idea").unwrap();
    assert_eq!(parts.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Keys", "Drums"]);
    assert!(parts.iter().all(|p| p.ppq == 192 && p.tempo == Some(120.5)));
    assert_eq!((parts[0].program, parts[1].program), (Some(4), None));
    // In order of start, then key; velocities come back as they were written.
    assert_eq!(parts[0].notes, [note(0, 192, 60, 70), note(0, 192, 64, 90)]);
    assert_eq!(parts[0].notes.iter().map(NoteEvent::velocity7).collect::<Vec<_>>(), [70, 90]);
    assert_eq!(parts[1].notes, [note(0, 48, 36, 127)]);
    assert!(parts[1].drums);
}

#[test]
fn a_track_named_after_its_file_is_named_by_its_channels_only() {
    let parts = read_bytes(&one_track_two_channels("loop one"), "Loop One").unwrap();
    assert_eq!(parts.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Ch 1", "Ch 2"]);
    let parts = read_bytes(&one_track_two_channels("Pads"), "Loop One").unwrap();
    assert_eq!(parts.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Pads · Ch 1", "Pads · Ch 2"]);
    assert!(parts.iter().all(|p| p.track_name == "Pads"));
}

#[test]
fn a_key_struck_twice_releases_the_earliest_first_and_held_notes_end_with_the_track() {
    let parts = read_bytes(&stacked_and_unreleased(), "x").unwrap();
    assert_eq!(parts[0].notes, [note(0, 32, 60, 100), note(16, 48, 60, 50), note(48, 48, 62, 30)]);
}

#[test]
fn an_rmi_file_holds_a_midi_file() {
    let inner = two_tracks();
    let mut rmi = b"RIFF".to_vec();
    rmi.extend(((inner.len() + 12) as u32).to_le_bytes());
    rmi.extend(b"RMIDdata");
    rmi.extend((inner.len() as u32).to_le_bytes());
    rmi.extend(&inner);
    assert_eq!(read_bytes(&rmi, "Idea").unwrap(), read_bytes(&inner, "Idea").unwrap());
}

#[test]
fn a_chunk_that_isnt_a_track_is_skipped_and_not_counted() {
    let keys = track_with_program(0, Some(1), "Keys");
    let bass = track_with_program(1, Some(33), "Bass");
    let mut file = smf(96, &[&keys, &bass]);
    // Another kind of chunk between the header and the first track, which the header doesn't count.
    let at = 14;
    file.splice(at..at, chunk(b"XFIH", b"made by something"));
    let parts = read_bytes(&file, "x").unwrap();
    assert_eq!(parts.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Keys", "Bass"]);
}

#[test]
fn smpte_timing_is_read_as_ticks_a_beat_at_120() {
    // 25 frames a second, 40 ticks a frame: 1,000 ticks a second, so 500 a beat at 120.
    let division = i16::from_be_bytes([(-25i8) as u8, 40]);
    let parts = read_bytes(&smf(division, &[&track_with_program(0, None, "")]), "x").unwrap();
    assert_eq!(parts[0].ppq, 500);
    // Too few ticks a beat to be real: at least 24.
    let division = i16::from_be_bytes([(-24i8) as u8, 1]);
    assert_eq!(read_bytes(&smf(division, &[&track_with_program(0, None, "")]), "x").unwrap()[0].ppq, 24);
}

#[test]
fn a_file_cut_short_is_read_up_to_where_it_stops() {
    let file = two_tracks();
    // Into the drums track, just after its hit: the keys are whole, and the hit, never released, ends where the
    // file does.
    let parts = read_bytes(&file[..file.len() - 8], "x").unwrap();
    assert_eq!(parts.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Keys", "Drums"]);
    assert_eq!(parts[1].notes, [note(0, 1, 36, 127)]);
    // Every length of the file reads without a panic.
    for n in 0..file.len() {
        let _ = read_bytes(&file[..n], "x");
    }
}

#[test]
fn a_name_that_isnt_utf8_is_read_as_latin_1() {
    let mut t = vec![0x00, 0xff, 0x03, 5, b'C', 0xe9, b'l', b'l', b'o'];
    t.extend([0x00, 0x90, 60, 80, 0x60, 0x80, 60, 0]);
    t.extend(END);
    let other = track_with_program(1, None, "Bass");
    let parts = read_bytes(&smf(96, &[&t, &other]), "x").unwrap();
    assert_eq!(parts[0].name, "Céllo");
}

#[test]
fn midi_files_are_known_by_their_endings() {
    assert!(smf::is_midi_path("Loops/Idea.MID"));
    assert!(smf::is_midi_path("a.kar"));
    assert!(!smf::is_midi_path("Idea.wav"));
    let dir = common::test_dir("midi-read");
    let path = dir.join("Idea.mid");
    std::fs::write(&path, two_tracks()).unwrap();
    assert_eq!(smf::read(&path).unwrap().len(), 2);
}
