//! Writing Standard MIDI Files: the C# app's `MidiWriterTests` (the no-tempo test the spec keeps), round trips through
//! the reader, and what the writer does with notes a MIDI 1.0 file can't hold.

// Test files may unwrap: a test that stops there has failed, which is what it should do.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use mo_formats::midi::smf::read_bytes;
use mo_formats::midi::write::{
    SmfFormat, SmfSong, SmfTrack, Tidy, TimeSignature, WriteError, WriteOptions, write, write_clip,
};
use mo_formats::midi::{Controller, ControllerKind, MidiPart};
use mo_formats::{MidiClip, NoteAttribute, NoteEvent, scale_up, velocity};

/// A note at a 16-bit velocity.
fn note(start: i64, length: i64, key: u8, velocity: u16) -> NoteEvent {
    NoteEvent { start, length, key, velocity, muted: false, step: false, attribute: NoteAttribute::NONE }
}

/// A note at a MIDI 1.0 velocity, as the reader gives it back.
fn note7(start: i64, length: i64, key: u8, vel: u8) -> NoteEvent {
    note(start, length, key, velocity::from_7(vel))
}

fn track(name: &str, channel: u8, notes: Vec<NoteEvent>) -> SmfTrack {
    SmfTrack { name: name.into(), channel, notes, ..SmfTrack::default() }
}

fn song(tracks: Vec<SmfTrack>) -> SmfSong {
    SmfSong { name: "Song".into(), ppq: 96, tracks, ..SmfSong::default() }
}

/// A variable-length number at `i`, moving `i` past it.
fn vlq(body: &[u8], i: &mut usize) -> i64 {
    let mut value = 0i64;
    loop {
        let b = body[*i];
        *i += 1;
        value = (value << 7) | i64::from(b & 0x7F);
        if b & 0x80 == 0 {
            return value;
        }
    }
}

/// Every track's events, as (tick, bytes), read straight from the file's bytes (a reader of its own, so the tests
/// don't only check the writer against the app's reader). Running status isn't followed: the writer never uses it.
fn events(file: &[u8]) -> Vec<Vec<(i64, Vec<u8>)>> {
    let mut tracks = Vec::new();
    let mut at = 14;
    while at + 8 <= file.len() {
        assert_eq!(&file[at..at + 4], b"MTrk");
        let len = u32::from_be_bytes(file[at + 4..at + 8].try_into().unwrap()) as usize;
        let body = &file[at + 8..at + 8 + len];
        let (mut i, mut tick, mut list) = (0, 0i64, Vec::new());
        while i < body.len() {
            tick += vlq(body, &mut i);
            let status = body[i];
            assert!(status >= 0x80, "running status at byte {i}");
            let size = match status {
                // A meta event's length is a variable-length number too: FF, its kind, the length, the bytes.
                0xFF => {
                    let mut at = i + 2;
                    let len = vlq(body, &mut at) as usize;
                    at - i + len
                }
                s if s & 0xF0 == 0xC0 || s & 0xF0 == 0xD0 => 2,
                _ => 3,
            };
            list.push((tick, body[i..i + size].to_vec()));
            i += size;
        }
        tracks.push(list);
        at += 8 + len;
    }
    tracks
}

// ---- The C# app's MidiWriterTests ----

/// The spec's "no-tempo MIDI writer test" (C# `WritesAValidType1File`): a clip with one track is a valid format 1
/// file of two tracks, with no tempo even when the song has one, and a repeated note ends before it starts again.
#[test]
fn writes_a_valid_type_1_file_with_no_tempo() {
    let mut clip = MidiClip { name: "Test".into(), ppq: 96, ..MidiClip::default() };
    clip.track("Lead").push(note7(0, 96, 60, 100));
    clip.track("Lead").push(note7(96, 96, 60, 100));
    // The C# test passes a tempo of 120: here the song carries one, and the default options still leave it out.
    let mut with_tempo = SmfSong::from_clip(&clip);
    with_tempo.tempo = Some(120.0);
    let bytes = write(&with_tempo, &WriteOptions::default()).unwrap().bytes;
    assert_eq!(bytes, write_clip(&clip).unwrap().bytes);

    assert_eq!(&bytes[0..4], b"MThd");
    assert_eq!(bytes[9], 1); // format 1
    assert_eq!(bytes[11], 2); // the clip's name + 1 instrument track
    assert_eq!(bytes[13], 96); // PPQ
    assert_eq!(bytes.windows(4).filter(|w| w == b"MTrk").count(), 2);
    // No tempo, though one was given, so a DAW doesn't ask to take it for the project.
    assert!(!bytes.windows(2).any(|w| w == [0xFF, 0x51]));
    // At tick 96 the first note's off must come before the second note's on.
    let off = bytes.windows(3).position(|w| w == [0x80, 0x3C, 0x00]).unwrap();
    let last_on = bytes.windows(3).rposition(|w| w == [0x90, 0x3C, 0x64]).unwrap();
    assert!(off < last_on);

    // And the reader finds both notes, whole, with the track's name.
    let parts = read_bytes(&bytes, "Test").unwrap();
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0].notes, [note7(0, 96, 60, 100), note7(96, 96, 60, 100)]);
    assert_eq!((parts[0].track_name.as_str(), parts[0].tempo), ("Lead", None));
}

#[test]
fn the_first_track_holds_the_song_name_and_ends_at_once() {
    let mut clip = MidiClip { name: "Test".into(), ppq: 96, end: Some(384), ..MidiClip::default() };
    clip.track("Lead").push(note7(0, 96, 60, 100));
    let tracks = events(&write_clip(&clip).unwrap().bytes);
    assert_eq!(tracks[0], [(0, b"\xFF\x03\x04Test".to_vec()), (0, vec![0xFF, 0x2F, 0x00])]);
    // The instrument track ends at the clip's end, past its last note, so a DAW makes the clip that long.
    assert_eq!(tracks[1].last().unwrap(), &(384, vec![0xFF, 0x2F, 0x00]));
}

// ---- Tempo, time signature and format ----

#[test]
fn tempo_and_time_signature_are_written_only_when_asked() {
    let mut s = song(vec![track("Keys", 0, vec![note7(0, 96, 60, 90)])]);
    s.tempo = Some(120.0);
    s.time_signature = Some(TimeSignature { numerator: 6, denominator: 8 });
    let bytes = write(&s, &WriteOptions { tempo: true, ..WriteOptions::default() }).unwrap().bytes;
    let first = &events(&bytes)[0];
    assert_eq!(first[1], (0, vec![0xFF, 0x51, 0x03, 0x07, 0xA1, 0x20])); // 500,000 microseconds a beat
    assert_eq!(first[2], (0, vec![0xFF, 0x58, 0x04, 6, 3, 24, 8]));
    assert_eq!(read_bytes(&bytes, "x").unwrap()[0].tempo, Some(120.0));
    // The reader's two decimals come back: 120.5 is 497,925 microseconds.
    s.tempo = Some(120.5);
    let bytes = write(&s, &WriteOptions { tempo: true, ..WriteOptions::default() }).unwrap().bytes;
    assert_eq!(read_bytes(&bytes, "x").unwrap()[0].tempo, Some(120.5));
}

#[test]
fn format_0_holds_a_single_track() {
    let mut s = song(vec![track("Bass", 2, vec![note7(0, 48, 36, 127), note7(48, 48, 43, 1)])]);
    s.tempo = Some(90.0);
    for format in [SmfFormat::Zero, SmfFormat::Auto] {
        let bytes = write(&s, &WriteOptions { format, tempo: true, ..WriteOptions::default() }).unwrap().bytes;
        assert_eq!((bytes[9], bytes[11]), (0, 1));
        let tracks = events(&bytes);
        assert_eq!(tracks[0][0], (0, b"\xFF\x03\x04Bass".to_vec()));
        let parts = read_bytes(&bytes, "x").unwrap();
        assert_eq!(parts[0].notes, [note7(0, 48, 36, 127), note7(48, 48, 43, 1)]);
        assert_eq!(parts[0].tempo, Some(90.0));
    }
    // Two tracks: Auto writes format 1, Zero is an error.
    s.tracks.push(track("Lead", 0, vec![note7(0, 48, 72, 64)]));
    let auto = write(&s, &WriteOptions { format: SmfFormat::Auto, ..WriteOptions::default() }).unwrap().bytes;
    assert_eq!((auto[9], auto[11]), (1, 3));
    let zero = WriteOptions { format: SmfFormat::Zero, ..WriteOptions::default() };
    assert_eq!(write(&s, &zero), Err(WriteError::TooManyTracksForFormat0(2)));
}

// ---- Round trips ----

/// A small random number maker, so the round trips cover many notes the same way every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self, below: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % below
    }
}

/// Model -> bytes -> model gives the same notes at MIDI 1.0's precision, and writing what was read gives the same
/// notes again (so velocities don't drift on each save). A part doesn't keep its channel, so the channels are handed
/// out again (drums on 10, the rest from 1): the tracks here start on other channels to show it.
#[test]
fn notes_round_trip_through_the_reader() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut tracks = Vec::new();
    for (name, channel, program) in [("Keys", 5, Some(4)), ("Drums", 9, None), ("Pad", 12, Some(88))] {
        // Notes on a grid with gaps, so none overlaps another on its key; any 16-bit velocity.
        let mut notes = Vec::new();
        let mut at = 0;
        for _ in 0..200 {
            let length = 1 + rng.next(200) as i64;
            notes.push(note(at, length, rng.next(128) as u8, rng.next(65536) as u16));
            if rng.next(3) == 0 {
                at += length + rng.next(50) as i64;
            }
        }
        // Different keys may overlap; the same key may not (that's tidied, tested below).
        let mut ends = [0i64; 128];
        notes.retain(|n| {
            let free = ends[usize::from(n.key)] <= n.start;
            if free {
                ends[usize::from(n.key)] = n.start + n.length;
            }
            free
        });
        tracks.push(SmfTrack { program, ..track(name, channel, notes) });
    }
    let original = song(tracks);
    let bytes = write(&original, &WriteOptions::default()).unwrap().bytes;

    let parts = read_bytes(&bytes, "Song").unwrap();
    assert_eq!(parts.len(), 3);
    for (part, written) in parts.iter().zip(&original.tracks) {
        assert_eq!(part.track_name, written.name);
        assert_eq!(part.drums, written.channel == 9);
        assert_eq!(part.program, written.program);
        assert_eq!(part.ppq, 96);
        // MIDI 1.0's precision: the velocity the reader gives for the MIDI 1.0 velocity written.
        let mut expected: Vec<NoteEvent> = written
            .notes
            .iter()
            .map(|n| NoteEvent { velocity: velocity::from_7(velocity::to_7(n.velocity)), ..*n })
            .collect();
        expected.sort_by_key(|n| (n.start, n.key));
        assert_eq!(part.notes, expected, "{}", written.name);
    }

    // Read, then written again: the same notes, sounds and names, on channels handed out again.
    let again = SmfSong::from_parts("Song", &parts).unwrap();
    assert_eq!(again.tracks.iter().map(|t| t.channel).collect::<Vec<_>>(), [0, 9, 1]);
    let second = write(&again, &WriteOptions::default()).unwrap().bytes;
    assert_ne!(second, bytes);
    let parts_again = read_bytes(&second, "Song").unwrap();
    assert_eq!(parts_again, parts);
    // From then on, saving what was read gives the same bytes.
    let third = write(&SmfSong::from_parts("Song", &parts_again).unwrap(), &WriteOptions::default()).unwrap().bytes;
    assert_eq!(third, second);
}

#[test]
fn every_velocity_comes_back_the_same_after_one_save() {
    // All 65,536 velocities, a note each.
    let notes: Vec<_> = (0..=u16::MAX).map(|v| note(i64::from(v) * 2, 1, 60, v)).collect();
    let bytes = write(&song(vec![track("All", 0, notes.clone())]), &WriteOptions::default()).unwrap().bytes;
    let read = read_bytes(&bytes, "x").unwrap().remove(0);
    for (written, back) in notes.iter().zip(&read.notes) {
        assert_eq!(back.velocity7(), written.velocity7(), "{}", written.velocity);
        assert!(back.velocity7() >= 1);
    }
    // Every MIDI 1.0 velocity is written as itself.
    for v in 1..=127u8 {
        let bytes =
            write(&song(vec![track("One", 0, vec![note7(0, 1, 60, v)])]), &WriteOptions::default()).unwrap().bytes;
        assert!(bytes.windows(3).any(|w| w == [0x90, 60, v]), "{v}");
    }
}

#[test]
fn names_keep_their_letters() {
    let s = song(vec![track("Basse é ♪", 0, vec![note7(0, 1, 60, 100)])]);
    let parts = read_bytes(&write(&s, &WriteOptions::default()).unwrap().bytes, "x").unwrap();
    assert_eq!(parts[0].track_name, "Basse é ♪");
}

// ---- Controllers (rule 22) ----

#[test]
fn controllers_are_cut_to_midi_1_bits_only_in_the_file() {
    let c = |tick, kind, value| Controller { tick, kind, value };
    let mut t = track("Lead", 3, vec![note7(0, 96, 60, 100)]);
    t.controllers = vec![
        c(0, ControllerKind::Control(7), scale_up(100, 7, 32)),
        c(0, ControllerKind::PitchBend, 0x8000_0000),
        c(10, ControllerKind::PitchBend, scale_up(0x3FFF, 14, 32)),
        c(20, ControllerKind::ChannelPressure, scale_up(64, 7, 32)),
        c(30, ControllerKind::KeyPressure(60), scale_up(1, 7, 32)),
        c(40, ControllerKind::Registered(0, 0), scale_up(12 << 7, 14, 32)),
        c(50, ControllerKind::Assignable(1, 2), scale_up(0x1234, 14, 32)),
        // MIDI 2.0's own: no MIDI 1.0 message, left out.
        c(60, ControllerKind::NotePitchBend(60), 0),
        c(60, ControllerKind::RelativeRegistered(0, 0), 1),
    ];
    let written = write(&song(vec![t]), &WriteOptions::default()).unwrap();
    // The two MIDI 2.0 ones are counted, so the caller can say they were left out.
    assert_eq!(written.dropped_controllers, 2);
    let bytes = written.bytes;
    let tracks = events(&bytes);
    let got: Vec<_> = tracks[1].iter().filter(|(_, e)| e[0] != 0xFF && e[0] & 0xE0 != 0x80).cloned().collect();
    assert_eq!(
        got,
        [
            (0, vec![0xB3, 7, 100]),
            (0, vec![0xE3, 0x00, 0x40]), // 8192: no bend
            (10, vec![0xE3, 0x7F, 0x7F]),
            (20, vec![0xD3, 64]),
            (30, vec![0xA3, 60, 1]),
            (40, vec![0xB3, 101, 0]),
            (40, vec![0xB3, 100, 0]),
            (40, vec![0xB3, 6, 12]),
            (40, vec![0xB3, 38, 0]),
            (50, vec![0xB3, 99, 1]),
            (50, vec![0xB3, 98, 2]),
            (50, vec![0xB3, 6, 0x24]),
            (50, vec![0xB3, 38, 0x34]),
        ]
    );
    // At tick 0 the controllers come before the note starts.
    let at_zero: Vec<u8> = tracks[1].iter().filter(|(t, e)| *t == 0 && e[0] != 0xFF).map(|(_, e)| e[0]).collect();
    assert_eq!(at_zero, [0xB3, 0xE3, 0x93]);
}

#[test]
fn key_pressure_comes_after_the_note_it_presses() {
    let mut t = track("Lead", 0, vec![note7(0, 96, 60, 100)]);
    t.controllers = vec![
        Controller { tick: 0, kind: ControllerKind::KeyPressure(60), value: scale_up(50, 7, 32) },
        Controller { tick: 0, kind: ControllerKind::Control(1), value: scale_up(20, 7, 32) },
    ];
    let bytes = write(&song(vec![t]), &WriteOptions::default()).unwrap().bytes;
    let at_zero: Vec<u8> =
        events(&bytes)[1].iter().filter(|(t, e)| *t == 0 && e[0] != 0xFF).map(|(_, e)| e[0]).collect();
    // The controller before the note, the key's pressure after it.
    assert_eq!(at_zero, [0xB0, 0x90, 0xA0]);
}

// ---- Notes a MIDI 1.0 file can't hold as they are ----

#[test]
fn a_note_of_no_length_gets_one_tick_or_is_refused() {
    let s = song(vec![track("Hats", 0, vec![note7(10, 0, 42, 100)])]);
    let bytes = write(&s, &WriteOptions::default()).unwrap().bytes;
    // Its start comes before its end, so it doesn't hang on to the end of the track.
    let notes: Vec<_> = events(&bytes)[1].iter().filter(|(_, e)| e[0] != 0xFF).cloned().collect();
    assert_eq!(notes, [(10, vec![0x90, 42, 100]), (11, vec![0x80, 42, 0])]);
    assert_eq!(read_bytes(&bytes, "x").unwrap()[0].notes, [note7(10, 1, 42, 100)]);

    let refuse = WriteOptions { tidy: Tidy::Refuse, ..WriteOptions::default() };
    assert_eq!(write(&s, &refuse), Err(WriteError::ZeroLength { track: 0, start: 10, key: 42 }));
}

#[test]
fn a_note_over_another_on_its_key_ends_where_the_key_starts_again() {
    let s = song(vec![track(
        "Bass",
        0,
        vec![
            note7(0, 100, 36, 90), // still sounding when the next 36 starts
            note7(50, 30, 36, 80),
            note7(200, 50, 36, 40), // two struck together: one note, the longer and louder
            note7(200, 80, 36, 70),
            note7(0, 300, 48, 60), // another key overlapping is fine
        ],
    )]);
    let bytes = write(&s, &WriteOptions::default()).unwrap().bytes;
    let notes = &read_bytes(&bytes, "x").unwrap()[0].notes;
    assert_eq!(notes, &[note7(0, 50, 36, 90), note7(0, 300, 48, 60), note7(50, 30, 36, 80), note7(200, 80, 36, 70)]);
    // What was fixed stays fixed: read and written again, the same bytes.
    let parts = read_bytes(&bytes, "Song").unwrap();
    assert_eq!(write(&SmfSong::from_parts("Song", &parts).unwrap(), &WriteOptions::default()).unwrap().bytes, bytes);

    let refuse = WriteOptions { tidy: Tidy::Refuse, ..WriteOptions::default() };
    assert_eq!(write(&s, &refuse), Err(WriteError::Overlap { track: 0, start: 50, key: 36 }));
    // Notes that only touch (one ends as the next starts) are fine either way.
    let touching = song(vec![track("Bass", 0, vec![note7(0, 50, 36, 90), note7(50, 50, 36, 90)])]);
    assert!(write(&touching, &refuse).is_ok());
}

#[test]
fn what_midi_1_cannot_hold_is_an_error_not_a_panic() {
    let one = |t: SmfTrack| write(&song(vec![t]), &WriteOptions::default());
    assert_eq!(one(track("x", 0, vec![note7(0, 1, 128, 1)])), Err(WriteError::KeyOutOfRange { track: 0, key: 128 }));
    assert_eq!(one(track("x", 0, vec![note7(0, 1, 255, 1)])), Err(WriteError::KeyOutOfRange { track: 0, key: 255 }));
    assert_eq!(one(track("x", 16, vec![])), Err(WriteError::ChannelOutOfRange { track: 0, channel: 16 }));
    assert_eq!(one(track("x", 200, vec![])), Err(WriteError::ChannelOutOfRange { track: 0, channel: 200 }));
    assert_eq!(
        one(SmfTrack { program: Some(128), ..track("x", 0, vec![]) }),
        Err(WriteError::ProgramOutOfRange { track: 0, program: 128 })
    );
    assert_eq!(one(track("x", 0, vec![note7(-1, 1, 60, 1)])), Err(WriteError::NegativeTime { track: 0, tick: -1 }));
    assert_eq!(
        one(track("x", 0, vec![note7(5, -1, 60, 1)])),
        Err(WriteError::NegativeLength { track: 0, start: 5, length: -1 })
    );
    assert!(matches!(one(track("x", 0, vec![note7(i64::MAX, 2, 60, 1)])), Err(WriteError::TooFar { .. })));
    assert!(matches!(one(track("x", 0, vec![note7(0x1000_0000, 1, 60, 1)])), Err(WriteError::TooFar { .. })));
    let cc = |kind| SmfTrack { controllers: vec![Controller { tick: 0, kind, value: 0 }], ..track("x", 0, vec![]) };
    for kind in [ControllerKind::Control(128), ControllerKind::KeyPressure(128), ControllerKind::Registered(128, 0)] {
        assert_eq!(one(cc(kind)), Err(WriteError::ControllerOutOfRange { track: 0, kind }));
    }
    let late = SmfTrack {
        controllers: vec![Controller { tick: -5, kind: ControllerKind::PitchBend, value: 0 }],
        ..track("x", 0, vec![])
    };
    assert_eq!(one(late), Err(WriteError::NegativeTime { track: 0, tick: -5 }));

    for ppq in [0, 0x8000, u16::MAX] {
        let s = SmfSong { ppq, ..song(vec![]) };
        assert_eq!(write(&s, &WriteOptions::default()), Err(WriteError::BadPpq(ppq)));
    }
    let tempo = WriteOptions { tempo: true, ..WriteOptions::default() };
    for bpm in [0.0, -120.0, f64::NAN, f64::INFINITY, 1.0] {
        // 1 beat a minute is 60,000,000 microseconds, more than three bytes hold.
        let s = SmfSong { tempo: Some(bpm), ..song(vec![]) };
        assert!(matches!(write(&s, &tempo), Err(WriteError::BadTempo(_))), "{bpm}");
    }
    for (numerator, denominator) in [(0, 4), (3, 3), (4, 0)] {
        let sig = TimeSignature { numerator, denominator };
        let s = SmfSong { time_signature: Some(sig), ..song(vec![]) };
        assert_eq!(write(&s, &WriteOptions::default()), Err(WriteError::BadTimeSignature(sig)));
    }
}

#[test]
fn parts_at_different_ticks_a_beat_cant_share_a_file() {
    let part = |ppq| MidiPart {
        name: String::new(),
        ppq,
        notes: vec![],
        drums: false,
        tempo: None,
        program: None,
        track_name: String::new(),
        controllers: vec![],
    };
    assert_eq!(SmfSong::from_parts("x", &[part(96), part(480)]), Err(WriteError::MixedPpq { first: 96, other: 480 }));
}

#[test]
fn clip_tracks_take_channels_in_order_without_the_drum_channel() {
    let mut clip = MidiClip { name: "Many".into(), ppq: 96, ..MidiClip::default() };
    for i in 0..17 {
        clip.track(&format!("T{i}")).push(note7(0, 1, 60, 100));
    }
    let channels: Vec<u8> = SmfSong::from_clip(&clip).tracks.iter().map(|t| t.channel).collect();
    assert_eq!(channels, [0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12, 13, 14, 15, 0, 1]);
}

#[test]
fn errors_name_the_songs_track_in_either_format() {
    let bad = || track("Bad", 0, vec![note7(0, 1, 128, 1)]);
    for format in [SmfFormat::Zero, SmfFormat::One, SmfFormat::Auto] {
        let options = WriteOptions { format, ..WriteOptions::default() };
        assert_eq!(write(&song(vec![bad()]), &options), Err(WriteError::KeyOutOfRange { track: 0, key: 128 }));
    }
    // The second of two tracks is track 1, though the file's first track (the song's name) comes before it.
    let two = song(vec![track("Good", 0, vec![note7(0, 1, 60, 1)]), bad()]);
    assert_eq!(write(&two, &WriteOptions::default()), Err(WriteError::KeyOutOfRange { track: 1, key: 128 }));
}

#[test]
fn names_of_any_length_read_back() {
    let names = [
        "a".repeat(127),
        "a".repeat(128),
        "a".repeat(255),
        "é".repeat(64),                  // 128 bytes
        format!("{}a", "é".repeat(127)), // 255 bytes
        "€".repeat(85),                  // 255 bytes, three a letter
    ];
    for name in &names {
        // A track's name, in format 1: every note still found after it.
        let s = song(vec![track(name, 0, vec![note7(0, 96, 60, 100)])]);
        let parts = read_bytes(&write(&s, &WriteOptions::default()).unwrap().bytes, "x").unwrap();
        assert_eq!((parts.len(), parts[0].notes.len()), (1, 1), "{} bytes", name.len());
        assert_eq!(&parts[0].track_name, name);
        // The song's name: format 1's first track (read straight from the bytes), and format 0's only track when its
        // own has no name.
        let named = SmfSong { name: name.clone(), ..song(vec![track("Keys", 0, vec![note7(0, 96, 60, 100)])]) };
        let bytes = write(&named, &WriteOptions::default()).unwrap().bytes;
        let first = &events(&bytes)[0][0].1;
        assert_eq!(&first[first.len() - name.len()..], name.as_bytes());
        assert_eq!(read_bytes(&bytes, "x").unwrap()[0].notes.len(), 1);
        let unnamed = SmfSong { name: name.clone(), ..song(vec![track("", 0, vec![note7(0, 96, 60, 100)])]) };
        let zero = WriteOptions { format: SmfFormat::Zero, ..WriteOptions::default() };
        let parts = read_bytes(&write(&unnamed, &zero).unwrap().bytes, "x").unwrap();
        assert_eq!((&parts[0].track_name, parts[0].notes.len()), (name, 1));
    }
    // Longer than 255 bytes: cut at the last whole letter.
    let long = "é".repeat(150);
    let s = song(vec![track(&long, 0, vec![note7(0, 96, 60, 100)])]);
    let parts = read_bytes(&write(&s, &WriteOptions::default()).unwrap().bytes, "x").unwrap();
    assert_eq!(parts[0].track_name, "é".repeat(127));
}

/// A reader's part, as `read_bytes` gives it.
fn part(name: &str, program: Option<u8>, drums: bool) -> MidiPart {
    MidiPart {
        name: String::new(),
        ppq: 96,
        notes: vec![note7(0, 96, 60, 100)],
        drums,
        tempo: None,
        program,
        track_name: name.into(),
        controllers: vec![],
    }
}

#[test]
fn more_than_15_parts_share_channels_by_sound() {
    // 20 melodic parts with 5 sounds, and drums: the first 15 get a channel each, the other 5 go on the channel of
    // the first part with their sound, so every part keeps its sound.
    let mut parts: Vec<MidiPart> = (0..20).map(|i| part(&format!("P{i}"), Some(i % 5 * 10), false)).collect();
    parts.insert(3, part("Drums", None, true));
    let s = SmfSong::from_parts("Song", &parts).unwrap();
    let channels: Vec<u8> = s.tracks.iter().map(|t| t.channel).collect();
    assert_eq!(channels, [0, 1, 2, 9, 3, 4, 5, 6, 7, 8, 10, 11, 12, 13, 14, 15, 0, 1, 2, 3, 4]);
    let written = write(&s, &WriteOptions::default()).unwrap();
    assert!(written.shared_channels.is_empty());
    let back = read_bytes(&written.bytes, "Song").unwrap();
    assert_eq!(back.iter().map(|p| p.program).collect::<Vec<_>>(), parts.iter().map(|p| p.program).collect::<Vec<_>>());

    // 16 different sounds: the 16th has to share channel 1 with the first, and loses its sound; the file is still
    // written, and names the channel and both its tracks.
    let parts: Vec<MidiPart> = (0..16).map(|i| part(&format!("P{i}"), Some(i), false)).collect();
    let s = SmfSong::from_parts("Song", &parts).unwrap();
    assert_eq!(s.tracks[15].channel, 0);
    let written = write(&s, &WriteOptions::default()).unwrap();
    assert_eq!(written.shared_channels, [(0, vec![0, 15])]);
    assert_eq!(read_bytes(&written.bytes, "Song").unwrap()[15].program, Some(0));

    // 16 parts that set no sound: 15 channels, then the 16th shares with the first (the same sound: none).
    let parts: Vec<MidiPart> = (0..16).map(|i| part(&format!("P{i}"), None, false)).collect();
    let s = SmfSong::from_parts("Song", &parts).unwrap();
    let channels: Vec<u8> = s.tracks.iter().map(|t| t.channel).collect();
    assert_eq!(channels, [0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12, 13, 14, 15, 0]);
    assert!(write(&s, &WriteOptions::default()).unwrap().shared_channels.is_empty());

    // 15 parts: a channel each, whatever their sounds.
    let parts: Vec<MidiPart> = (0..15).map(|i| part(&format!("P{i}"), Some(1), false)).collect();
    let channels: Vec<u8> = SmfSong::from_parts("Song", &parts).unwrap().tracks.iter().map(|t| t.channel).collect();
    assert_eq!(channels, [0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12, 13, 14, 15]);
}

#[test]
fn a_shared_channel_names_every_track_on_it() {
    // A part that sets no sound first: the 16th part's sound change takes over its channel, so the first part changes
    // too. Both are named, not only the later one.
    let mut parts = vec![part("Plain", None, false)];
    parts.extend((0..15).map(|i| part(&format!("P{i}"), Some(i), false)));
    let s = SmfSong::from_parts("Song", &parts).unwrap();
    let written = write(&s, &WriteOptions::default()).unwrap();
    assert_eq!(written.shared_channels, [(0, vec![0, 15])]);
    // The reader gives the first part the 16th part's sound: it did change.
    assert_eq!(read_bytes(&written.bytes, "Song").unwrap()[0].program, Some(14));

    // Two drum kits on channel 10, the first setting no sound: both named.
    let parts = [part("Kit A", None, true), part("Kit B", Some(25), true), part("Bass", Some(33), false)];
    let written = write(&SmfSong::from_parts("Song", &parts).unwrap(), &WriteOptions::default()).unwrap();
    assert_eq!(written.shared_channels, [(9, vec![0, 1])]);
}

/// C# `FlStepSequencerStepsAreWrittenAsSixteenthsNotOneTick`: FL's step-sequencer steps have no length (1 tick, which
/// a sampler barely sounds); each is written as a sixteenth, or up to the next step on its key if that's sooner.
#[test]
fn fl_step_sequencer_steps_are_written_as_sixteenths_not_one_tick() {
    // Kicks on each beat and two hats a 32nd apart, as FL's step sequencer stores them (no length), and a played note.
    let step = |start, key| NoteEvent { step: true, ..note7(start, 1, key, 100) };
    let mut clip = MidiClip { name: "Drums".into(), ppq: 96, ..MidiClip::default() };
    for at in [0, 96, 192, 288] {
        clip.track("Kick").push(step(at, 36));
    }
    clip.track("Kick").push(step(0, 42));
    clip.track("Kick").push(step(12, 42));
    clip.track("Kick").push(note7(0, 192, 60, 100));
    let parts = read_bytes(&write_clip(&clip).unwrap().bytes, "Drums").unwrap();
    let notes: Vec<_> = parts.iter().flat_map(|p| &p.notes).collect();
    assert!(notes.iter().filter(|n| n.key == 36).all(|n| n.length == 24));
    // Never over the next hat.
    assert_eq!(notes.iter().filter(|n| n.key == 42).map(|n| n.length).collect::<Vec<_>>(), [12, 24]);
    assert_eq!(notes.iter().find(|n| n.key == 60).unwrap().length, 192);
}
