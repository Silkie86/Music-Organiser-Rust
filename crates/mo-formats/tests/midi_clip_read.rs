//! Reading MIDI Clip Files (SMF2): made-up clips, byte by byte, as the MIDI Clip File and Universal MIDI Packet
//! specifications lay them out. The C# app has no reader for them, so there's no twin to compare with.

// Test files may unwrap: a test that stops there has failed, which is what it should do.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::Path;

use common::midi::*;
use mo_formats::midi::{self, Controller, ControllerKind, clip};
use mo_formats::{NoteAttribute, NoteEvent, scale_up, velocity};

fn note(start: i64, length: i64, key: u8, velocity: u16) -> NoteEvent {
    NoteEvent { start, length, key, velocity, muted: false, step: false, attribute: NoteAttribute::NONE }
}

#[test]
fn a_clip_reads_its_ppq_tempo_name_and_each_channels_notes_at_midi_2_resolution() {
    let parts = clip::read_bytes(&clip_two_groups()).unwrap();
    assert_eq!(
        parts.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        ["Group 1 · Ch 1", "Group 1 · Ch 10", "Group 2 · Ch 2"]
    );
    assert!(parts.iter().all(|p| p.ppq == 480 && p.tempo == Some(92.5) && p.track_name == "Night Drive Verse Chords"));

    let chords = &parts[0];
    let pitched = NoteEvent { attribute: NoteAttribute { kind: 3, data: 64 << 9 }, ..note(0, 480, 64, 0x8000) };
    assert_eq!(chords.notes, [note(0, 480, 60, 0xc000), pitched]);
    assert_eq!(chords.program, Some(4));
    assert!(!chords.drums);

    assert_eq!(parts[1].notes, [note(0, 120, 36, 0xffff)]);
    assert!(parts[1].drums);
    // MIDI 1.0's velocity, scaled up by MIDI 2.0's rule, comes back as it was.
    assert_eq!(parts[2].notes, [note(0, 960, 36, velocity::from_7(100))]);
    assert_eq!(parts[2].notes[0].velocity7(), 100);
}

#[test]
fn one_channel_is_one_part_with_no_name_of_its_own() {
    let mut b = vec![dc(0)];
    b.extend(note_on2(0, 3, 48, 0x4000, (0, 0)));
    b.push(dc(96));
    b.extend(note_off2(0, 3, 48));
    let parts = clip::read_bytes(&clip(96, 120.0, "Bass", &b, 0)).unwrap();
    assert_eq!(parts.len(), 1);
    assert_eq!((parts[0].name.as_str(), parts[0].track_name.as_str()), ("", "Bass"));
    assert_eq!(parts[0].notes, [note(0, 96, 48, 0x4000)]);
}

#[test]
fn a_midi_2_note_on_at_velocity_0_is_a_note_but_a_midi_1_one_ends_it() {
    let mut b = vec![dc(0)];
    b.extend(note_on2(0, 0, 60, 0, (0, 0)));
    b.push(dc(0));
    b.push(midi1(0, 0x91, 62, 50));
    b.push(dc(48));
    b.extend(note_off2(0, 0, 60));
    b.push(dc(0));
    b.push(midi1(0, 0x91, 62, 0));
    let parts = clip::read_bytes(&clip(96, 120.0, "", &b, 0)).unwrap();
    assert_eq!(parts[0].notes, [note(0, 48, 60, 0)]);
    // As MIDI 1.0 it plays at its softest, never 0 (which MIDI 1.0 takes as the note ending).
    assert_eq!(parts[0].notes[0].velocity7(), 1);
    assert_eq!(parts[1].notes, [note(0, 48, 62, velocity::from_7(50))]);
}

#[test]
fn a_long_wait_is_delta_clockstamps_with_a_no_op_between_and_a_second_in_a_row_counts() {
    let mut b = vec![dc(0)];
    b.extend(note_on2(0, 0, 60, 0x8000, (0, 0)));
    // 0xfffff, then a No Op, then 10 more: the note ends 1,048,585 ticks later.
    b.extend([dc(0xf_ffff), NOOP, dc(10)]);
    b.extend(note_off2(0, 0, 60));
    // Two Delta Clockstamps in a row: the second says how long.
    b.extend([dc(500), dc(20)]);
    b.extend(note_on2(0, 0, 62, 0x8000, (0, 0)));
    b.push(dc(5));
    b.extend(note_off2(0, 0, 62));
    let parts = clip::read_bytes(&clip(96, 120.0, "", &b, 0)).unwrap();
    assert_eq!(parts[0].notes, [note(0, 1_048_585, 60, 0x8000), note(1_048_605, 5, 62, 0x8000)]);
}

#[test]
fn notes_still_held_end_with_the_clip_and_nothing_after_it_counts() {
    let mut words = vec![dc(0), dctpq(96), dc(0)];
    words.extend(START_OF_CLIP);
    words.push(dc(0));
    words.extend(note_on2(0, 0, 60, 0x8000, (0, 0)));
    words.push(dc(384));
    words.extend(END_OF_CLIP);
    words.push(dc(0));
    words.extend(note_on2(0, 0, 72, 0x8000, (0, 0)));
    let parts = clip::read_bytes(&clip_file(&words)).unwrap();
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0].notes, [note(0, 384, 60, 0x8000)]);
    // No tempo or name in the header: none.
    assert_eq!((parts[0].tempo, parts[0].track_name.as_str()), (None, ""));
}

#[test]
fn a_clip_cut_short_reads_up_to_where_it_stops() {
    let file = clip_two_groups();
    for n in 0..file.len() {
        let _ = clip::read_bytes(&file[..n]);
    }
    // Cut just before the bass's MIDI 1.0 note off: the bass, never released, ends where the clip stops.
    let cut = file.len() - 4 * (4 + 1 + 1);
    let parts = clip::read_bytes(&file[..cut]).unwrap();
    assert_eq!(parts[2].notes, [note(0, 480, 36, velocity::from_7(100))]);
}

#[test]
fn a_clip_without_its_ticks_a_quarter_note_gets_96() {
    let mut words = START_OF_CLIP.to_vec();
    words.push(dc(0));
    words.extend(note_on2(0, 0, 60, 0x8000, (0, 0)));
    let parts = clip::read_bytes(&clip_file(&words)).unwrap();
    assert_eq!(parts[0].ppq, clip::DEFAULT_PPQ);
}

#[test]
fn bytes_that_arent_a_clip_file_say_so() {
    let e = clip::read_bytes(b"MThd\0\0\0\x06").unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::InvalidData);
    assert!(clip::read_bytes(b"SMF2CLI").is_err());
    assert_eq!(clip::read_bytes(b"SMF2CLIP").unwrap(), []);
}

#[test]
fn the_midi_library_reads_either_kind_by_how_it_starts() {
    assert!(midi::is_midi_path("Idea.MIDI2") && midi::is_midi_path("Idea.mid") && !midi::is_midi_path("Idea.wav"));
    let named = Path::new("Night drive.mid");
    let clip_parts = midi::read_from(&clip_two_groups(), named).unwrap();
    assert_eq!(clip_parts.len(), 3);
    let smf_parts = midi::read_from(&two_tracks(), named).unwrap();
    assert_eq!(smf_parts.len(), 2);
    let dir = common::test_dir("midi-clip-read");
    let path = dir.join("Verse.midi2");
    std::fs::write(&path, clip_two_groups()).unwrap();
    assert_eq!(midi::read(&path).unwrap(), clip_parts);
}

#[test]
fn the_clips_time_starts_at_start_of_clip() {
    // A header whose Delta Clockstamps aren't 0 (the spec writes 0 there): the clip's notes still start at 0.
    let mut words = vec![dc(0), dctpq(96), dc(200)];
    words.extend(START_OF_CLIP);
    words.push(dc(0));
    words.extend(note_on2(0, 0, 60, 0x8000, (0, 0)));
    words.push(dc(96));
    words.extend(note_off2(0, 0, 60));
    let parts = clip::read_bytes(&clip_file(&words)).unwrap();
    assert_eq!(parts[0].notes, [note(0, 96, 60, 0x8000)]);
}

#[test]
fn controllers_pitch_bend_and_pressure_are_kept_at_32_bits_with_the_channels_notes() {
    let mut b = vec![dc(0)];
    b.extend(note_on2(0, 0, 60, 0x8000, (0, 0)));
    // MIDI 2.0: a control change, per-note pitch bend and a per-note controller, each at 32 bits.
    b.push(dc(10));
    b.extend([0x40b0_0000 | 74 << 8, 0x1234_5678]);
    b.push(dc(10));
    b.extend([0x4060_0000 | 60 << 8, 0x9000_0000]);
    b.push(dc(0));
    b.extend([0x4000_0000 | 60 << 8 | 3, 0xffff_ffff]);
    // MIDI 2.0's registered and assignable controllers: a pitch bend range of 12 (bank 0, index 0), an assignable
    // one, and a step down of 1 to each.
    b.push(dc(0));
    b.extend([0x4020_0000, 12 << 25]);
    b.push(dc(0));
    b.extend([0x4030_0000 | 5 << 8 | 9, 0x4000_0000]);
    b.push(dc(0));
    b.extend([0x4040_0000 | 1 << 8 | 2, u32::MAX]);
    b.push(dc(0));
    b.extend([0x4050_0000 | 3 << 8 | 4, u32::MAX]);
    // MIDI 1.0: pitch bend all the way up, a mod wheel at 64 and channel pressure, scaled up by MIDI 2.0's rule.
    b.push(dc(10));
    b.push(midi1(0, 0xe0, 0x7f, 0x7f));
    b.push(dc(0));
    b.push(midi1(0, 0xb0, 1, 64));
    b.push(dc(0));
    b.push(midi1(0, 0xd0, 100, 0));
    b.push(dc(10));
    b.extend(note_off2(0, 0, 60));
    // A controller on a channel with no notes makes no part.
    b.push(dc(0));
    b.push(midi1(0, 0xb3, 7, 100));
    let parts = clip::read_bytes(&clip(96, 120.0, "", &b, 0)).unwrap();
    assert_eq!(parts.len(), 1);
    let c = |tick, kind, value| Controller { tick, kind, value };
    assert_eq!(
        parts[0].controllers,
        [
            c(10, ControllerKind::Control(74), 0x1234_5678),
            c(20, ControllerKind::NotePitchBend(60), 0x9000_0000),
            c(20, ControllerKind::NoteRegistered(60, 3), 0xffff_ffff),
            c(20, ControllerKind::Registered(0, 0), 12 << 25),
            c(20, ControllerKind::Assignable(5, 9), 0x4000_0000),
            c(20, ControllerKind::RelativeRegistered(1, 2), u32::MAX),
            c(20, ControllerKind::RelativeAssignable(3, 4), u32::MAX),
            c(30, ControllerKind::PitchBend, u32::MAX),
            c(30, ControllerKind::Control(1), 0x8000_0000),
            c(30, ControllerKind::ChannelPressure, scale_up(100, 7, 32)),
        ]
    );
}

#[test]
fn midi_2_scaling_keeps_the_bottom_middle_and_top_and_gives_each_value_back() {
    for (bits, to) in [(7, 16), (7, 32), (14, 32), (16, 32)] {
        assert_eq!(scale_up(0, bits, to), 0);
        assert_eq!(scale_up(1 << (bits - 1), bits, to), 1 << (to - 1));
        assert_eq!(u64::from(scale_up((1 << bits) - 1, bits, to)), (1u64 << to) - 1);
        // Every value, both sides of the middle, where the bits are repeated.
        for v in 0..1u32 << bits {
            assert_eq!(scale_up(v, bits, to) >> (to - bits), v);
        }
    }
    // The same as the velocities' own rule.
    for v in 0..=127u8 {
        assert_eq!(scale_up(v.into(), 7, 16), u32::from(velocity::from_7(v)));
    }
}
