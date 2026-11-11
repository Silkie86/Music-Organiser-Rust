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
    // The same two facts alone, for a reader that needs no more of the playlist.
    let flp = events::load(&bytes).expect("an FL project");
    assert_eq!(arrangement::bars_and_markers(&flp), (3, playlist.markers.clone()));
    assert_eq!(arrangement::bars_and_markers(&events::load(&fl_studio_project()).expect("an FL project")), (4, vec![]));
    assert_eq!(lanes(&playlist), [("Bass line", LaneKind::Instrument, "Riff")]);
    // FL saves colours blue first; they come out red first. The pattern's colour is the clip's.
    assert_eq!(playlist.lanes[0].clips[0].colour, Some(0x00_00FF));
    assert_eq!(bitwig.lanes[0].colour, Some(0xFF_0000));
    assert_eq!(playlist.length(), 6.0);
    assert_eq!(playlist.bars(), 2.0);
    assert_eq!(playlist.seconds(), 4.0);
}

#[test]
fn a_track_name_after_settings_too_short_to_read_isnt_put_on_the_track_before() {
    let bytes = Flp::new()
        .ascii(199, "24.1.1.4234")
        .word(65, 1)
        .data(224, &note(0, 0, 96, 48, 100))
        .word(64, 0)
        .byte(21, 2)
        .text(201, "3x Osc")
        .text(203, "Bass")
        .data(213, &[9])
        .word(99, 0)
        .data(
            233,
            &[playlist_item_25_on(0, 20481, 96, 499, false), playlist_item_25_on(0, 20481, 96, 498, false)].concat(),
        )
        .data(238, &playlist_track(1, false))
        .text(239, "Bass line")
        .data(238, &[2, 0, 0])
        .text(239, "Lost")
        .build();
    let [playlist, bitwig] = read(&bytes);
    assert_eq!(playlist.lanes.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["Bass line", "Track 2"]);
    // Both views agree on what plays.
    assert_eq!(playlist.note_count(), 2);
    assert_eq!(bitwig.note_count(), 2);
}

#[test]
fn clips_on_a_muted_track_dont_play_as_it_opens_in_bitwig_either() {
    let [playlist, bitwig] = read(&fl_studio_25_muted_tracks());
    // The Pad's only clip is on muted track 4; the Snare's second is on muted track 2.
    assert_eq!(playlist.note_count(), bitwig.note_count());
    let pad = bitwig.lanes.iter().find(|l| l.name == "Pad").unwrap();
    assert!(pad.clips.iter().all(|c| c.muted));
    let acapella = bitwig.lanes.iter().find(|l| l.name == "Acapella").unwrap();
    assert!(acapella.clips.iter().all(|c| c.muted), "both its clips are on muted tracks");
}

#[test]
fn a_pattern_clip_names_the_channels_it_plays_as_its_parts() {
    // "Intro" plays Lead's notes and Kick's: one part each, in the order they first play.
    let [playlist, _] = read(&fl_studio_project_with(Some(&playlist_item(0, 1, 384, -1))));
    let intro = &playlist.lanes[0].clips[0];
    assert_eq!(intro.parts, ["Lead", "Kick"]);
    let parts: Vec<_> = intro.notes.iter().map(|n| (n.key, n.part, n.step)).collect();
    assert_eq!(parts, [(60, 0, false), (36, 1, false), (64, 0, false)]);
    // Two channels both called "Perc": two parts all the same, so a drag out of the app makes two MIDI tracks.
    let notes = [note(0, 0, 24, 36, 100), note(24, 1, 24, 38, 100), note(48, 0, 24, 36, 100)].concat();
    let flp = Flp::new()
        .ascii(199, "21.2.3.4004")
        .dword(156, 128_000)
        .word(64, 0)
        .byte(21, 0)
        .text(203, "Perc")
        .word(64, 1)
        .byte(21, 0)
        .text(203, "Perc")
        .word(65, 1)
        .text(193, "Beat")
        .data(224, &notes)
        .data(233, &playlist_item(0, 1, 384, -1))
        .build();
    let [playlist, _] = read(&flp);
    let beat = &playlist.lanes[0].clips[0];
    assert_eq!(beat.parts, ["Perc", "Perc 2"]);
    assert_eq!(beat.notes.iter().map(|n| n.part).collect::<Vec<_>>(), [0, 1, 0]);
}

#[test]
fn step_sequencer_notes_are_marked_as_steps() {
    let [playlist, _] = read(&fl_studio_9_steps());
    let clip = &playlist.lanes[0].clips[0];
    assert_eq!(clip.parts, ["Kick"]);
    assert_eq!(clip.notes.len(), 3);
    assert!(clip.notes.iter().all(|n| n.step && n.part == 0));
}

#[test]
fn a_channel_switched_off_in_the_channel_rack_is_muted_in_both_views() {
    // The Hat and the Vox have their green lights off: FL plays neither.
    let [playlist, bitwig] = read(&fl_studio_25_muted_tracks());
    for name in ["Hat", "Vox"] {
        let lanes: Vec<_> = bitwig.lanes.iter().filter(|l| l.name == name).collect();
        assert!(!lanes.is_empty(), "{name} has a lane");
        assert!(lanes.iter().all(|l| l.muted), "{name} is muted, as the converter mutes its track");
    }
    assert!(bitwig.lanes.iter().filter(|l| !["Hat", "Vox"].contains(&l.name.as_str())).all(|l| !l.muted));
    // In the playlist the Vox's clip is muted on a track that plays, and so is every note of the Hat's.
    assert!(!playlist.lanes[2].muted);
    assert!(playlist.lanes[2].clips.iter().all(|c| c.name == "Vox" && c.muted));
    let mut hat = 0;
    for clip in playlist.lanes.iter().flat_map(|l| &l.clips) {
        for n in &clip.notes {
            let on_hat = clip.parts.get(n.part as usize).is_some_and(|p| p == "Hat");
            assert_eq!(n.muted, on_hat, "only the Hat's notes are muted ({})", clip.name);
            hat += usize::from(on_hat);
        }
    }
    assert!(hat > 0, "the Hat's notes are still in the picture");
}
