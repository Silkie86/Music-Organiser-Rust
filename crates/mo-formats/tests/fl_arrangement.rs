//! The arrangement preview of an FL project: its playlist as FL shows it, and its tracks as they open in Bitwig. The
//! C# app's `FlArrangement` on the same made-up files.

// Test files may unwrap: a test that stops there has failed, which is what it should do.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::*;
use mo_formats::arrangement::{Arrangement, LaneKind, PreviewMarker, PreviewPoint};
use mo_formats::fl::{arrangement, events};

fn read(bytes: &[u8]) -> [Arrangement; 2] {
    let views = arrangement::read(&events::load(bytes).expect("an FL project"));
    views.try_into().expect("two views")
}

/// Each lane as (name, kind, what it holds).
fn lanes(a: &Arrangement) -> Vec<(&str, LaneKind, &str)> {
    a.lanes.iter().map(|l| (l.name.as_str(), l.kind, l.detail.as_str())).collect()
}

/// Each clip on a lane as (name, start, length) in beats.
fn clips(a: &Arrangement, lane: usize) -> Vec<(&str, f64, f64)> {
    a.lanes[lane].clips.iter().map(|c| (c.name.as_str(), c.start, c.length)).collect()
}

#[test]
fn a_song_in_pattern_mode_shows_no_playlist_but_opens_in_bitwig_one_pattern_after_another() {
    let [playlist, bitwig] = read(&fl_studio_project());
    assert_eq!((playlist.view, bitwig.view), (arrangement::PLAYLIST_VIEW, arrangement::BITWIG_VIEW));
    assert!(playlist.lanes.is_empty());
    assert_eq!(playlist.notes.len(), 1, "says why the playlist is empty");
    assert_eq!((bitwig.tempo, bitwig.beats_per_bar), (128.0, 4));
    assert_eq!(
        lanes(&bitwig),
        [
            ("Lead", LaneKind::Instrument, "Serum"),
            ("Kick", LaneKind::Instrument, "Sampler · Kick 01.wav"),
            ("Bass", LaneKind::Instrument, "3x Osc"),
        ]
    );
    assert_eq!(clips(&bitwig, 0), [("Intro", 0.0, 4.0)]);
    assert_eq!(clips(&bitwig, 2), [("Drop", 4.0, 4.0)]);
    let lead = &bitwig.lanes[0].clips[0].notes;
    assert_eq!(lead.iter().map(|n| (n.start, n.length, n.key)).collect::<Vec<_>>(), [(0.0, 1.0, 60), (1.0, 1.0, 64)]);
    assert_eq!(bitwig.length(), 8.0);
    assert_eq!(bitwig.note_count(), 4);
}

#[test]
fn a_pattern_clip_starting_partway_shows_only_the_notes_from_there() {
    // Drop at the start; then the second half of Intro from beat 8.
    let playlist = [playlist_item(0, 2, 384, -1), playlist_item(768, 1, 192, 96)].concat();
    let [playlist, _] = read(&fl_studio_project_with(Some(&playlist)));
    assert!(playlist.notes.is_empty());
    assert_eq!(clips(&playlist, 0), [("Drop", 0.0, 4.0), ("Intro", 8.0, 2.0)]);
    assert_eq!(playlist.lanes[0].detail, "Drop + 1 more");
    let intro = &playlist.lanes[0].clips[1].notes;
    assert_eq!(intro.iter().map(|n| (n.start, n.length, n.key)).collect::<Vec<_>>(), [(0.0, 1.0, 64)]);
}

#[test]
fn playlist_tracks_keep_their_names_mutes_and_what_they_hold() {
    let [playlist, bitwig] = read(&fl_studio_25_muted_tracks());
    assert_eq!(
        lanes(&playlist),
        [
            ("Track 1", LaneKind::Instrument, "Verse + 1 more"),
            ("Acapellas", LaneKind::Instrument, "Acapella + 1 more"),
            ("Track 3", LaneKind::Audio, "Vox"),
            ("Track 4", LaneKind::Instrument, "Acapella + 1 more"),
        ]
    );
    // Only the first arrangement's mutes count; FL's default grey isn't worth showing.
    assert_eq!(playlist.lanes.iter().map(|l| l.muted).collect::<Vec<_>>(), [false, true, false, true]);
    assert!(playlist.lanes.iter().all(|l| l.colour.is_none()));
    assert_eq!(clips(&playlist, 2), [("Vox", 2.0, 4.0)]);
    let vox = &playlist.lanes[2].clips[0].audio[0];
    assert_eq!((vox.path.as_str(), vox.length, vox.from), (r"C:\Samples\Vox.wav", 4.0, 0.0));
    // At 120 beats a minute, unstretched, a beat takes half a second of the file.
    assert_eq!((vox.seconds_per_beat, vox.whole_file_beats), (Some(0.5), None));

    assert_eq!(
        lanes(&bitwig).iter().map(|(name, kind, _)| (*name, *kind)).collect::<Vec<_>>(),
        [
            ("Snare", LaneKind::Instrument),
            ("Hat", LaneKind::Instrument),
            ("Acapella", LaneKind::Audio),
            ("Vox", LaneKind::Audio),
            ("Pad", LaneKind::Instrument),
        ]
    );
    assert_eq!(clips(&bitwig, 3), [("Vox", 0.0, 4.0), ("Vox", 2.0, 4.0)]);
}

#[test]
fn automation_clips_show_their_curve_from_where_they_start() {
    let [playlist, bitwig] = read(&fl_studio_25_automation(true, false));
    assert_eq!(
        lanes(&playlist),
        [
            ("Track 1", LaneKind::Instrument, "Pattern 1"),
            ("Track 2", LaneKind::Automation, "Snare - Channel volume"),
            ("Track 3", LaneKind::Automation, "Serum - Cutoff"),
        ]
    );
    let point = |beat, value| PreviewPoint { beat, value };
    assert_eq!(clips(&playlist, 1), [("Snare - Channel volume", 4.0, 4.0), ("Snare - Channel volume", 12.0, 4.0)]);
    assert_eq!(playlist.lanes[1].clips[1].points, [point(0.0, 0.0), point(4.0, 1.0)]);
    assert_eq!(playlist.lanes[2].clips[0].points, [point(0.0, 0.2), point(2.0, 1.0), point(4.0, 0.5)]);
    // In Bitwig each becomes one lane across the song, moving its channel.
    let auto: Vec<_> = bitwig.lanes.iter().filter(|l| l.kind == LaneKind::Automation).collect();
    assert_eq!(
        auto.iter().map(|l| (l.detail.as_str(), l.clips[0].start, l.clips[0].length)).collect::<Vec<_>>(),
        [("Moves Snare", 4.0, 12.0), ("Moves Serum", 0.0, 4.0)]
    );
}

#[test]
fn markers_colours_and_the_time_signature() {
    let bytes = Flp::new()
        .ascii(199, "24.1.1.4234")
        .dword(156, 90_000)
        .byte(17, 3)
        .word(64, 0)
        .byte(21, 2)
        .text(201, "3x Osc")
        .text(203, "Bass")
        .data(213, &[9])
        .dword(128, 0x0000_00FF)
        .word(65, 1)
        .text(193, "Riff")
        .dword(150, 0x00FF_0000)
        .data(224, &note(0, 0, 96, 48, 100))
        .word(99, 0)
        .data(233, &playlist_item_25_on(288, 20481, 288, 499, false))
        .data(238, &playlist_track(1, false))
        .text(239, "Bass line")
        .dword(148, 288)
        .text(205, "Drop")
        .build();
    let [playlist, bitwig] = read(&bytes);
    assert_eq!((playlist.tempo, playlist.beats_per_bar), (90.0, 3));
    assert_eq!(playlist.markers, [PreviewMarker { beat: 3.0, name: "Drop".into() }]);
    assert_eq!(lanes(&playlist), [("Bass line", LaneKind::Instrument, "Riff")]);
    // FL saves colours blue first; they come out red first. The pattern's colour is the clip's.
    assert_eq!(playlist.lanes[0].clips[0].colour, Some(0x00_00FF));
    assert_eq!(bitwig.lanes[0].colour, Some(0xFF_0000));
    assert_eq!(playlist.length(), 6.0);
    assert_eq!(playlist.bars(), 2.0);
    assert_eq!(playlist.seconds(), 4.0);
}
