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
    assert_eq!((bitwig.tempo, bitwig.signature.beats()), (128.0, 4.0));
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
    assert_eq!((playlist.tempo, playlist.signature.beats()), (90.0, 3.0));
    assert_eq!(playlist.markers, [PreviewMarker { beat: 3.0, name: "Drop".into() }]);
    // A convert makes them the Bitwig project's cue markers, so the Bitwig view has them too.
    assert_eq!(bitwig.markers, playlist.markers);
    // The same two facts alone, for a reader that needs no more of the playlist.
    let flp = events::load(&bytes).expect("an FL project");
    assert_eq!(arrangement::bars_and_markers(&flp), (playlist.signature, playlist.markers.clone()));
    assert_eq!(
        arrangement::bars_and_markers(&events::load(&fl_studio_project()).expect("an FL project")),
        (Default::default(), vec![])
    );
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

#[test]
fn the_time_signature_keeps_both_its_numbers() {
    // FL 20 onwards says "Set as time signature" in event 35; event 18 is then the bottom number.
    let read = |events: &[(u8, u8)]| {
        let mut b = Flp::new().ascii(199, "25.2.5.5319");
        for &(id, value) in events {
            b = b.byte(id, value);
        }
        let bytes = b.build();
        let (signature, _) = arrangement::bars_and_markers(&events::load(&bytes).expect("an FL project"));
        (signature.top(), signature.bottom(), signature.beats())
    };
    // Tempo counts quarter notes whatever the signature, so a 6/8 bar is 3 of them.
    assert_eq!(read(&[(17, 6), (18, 8), (35, 1)]), (6, 8, 3.0));
    assert_eq!(read(&[(17, 12), (18, 8), (35, 1)]), (12, 8, 6.0));
    assert_eq!(read(&[(17, 2), (18, 2), (35, 1)]), (2, 2, 4.0));
    assert_eq!(read(&[(17, 7), (18, 8), (35, 1)]), (7, 8, 3.5));
    assert_eq!(read(&[(17, 3), (18, 4), (35, 1)]), (3, 4, 3.0));
    // Before FL 20 (no event 35), or in time division mode, event 18 is the steps in a beat: quarter notes.
    assert_eq!(read(&[(17, 6), (18, 4)]), (6, 4, 6.0));
    assert_eq!(read(&[(17, 4), (18, 6)]), (4, 4, 4.0));
    assert_eq!(read(&[(17, 6), (18, 8), (35, 0)]), (6, 4, 6.0));
    // A bottom number no time signature has, or none at all: quarter notes again, and 4/4 with nothing said.
    assert_eq!(read(&[(17, 6), (18, 5), (35, 1)]), (6, 4, 6.0));
    assert_eq!(read(&[(35, 1)]), (4, 4, 4.0));
    // The first of each is the song's: a later one is a pattern's own.
    assert_eq!(read(&[(17, 6), (18, 8), (35, 1), (17, 4), (18, 4)]), (6, 8, 3.0));
    // A first top number that can't be one doesn't let a pattern's stand in, and a bottom number alone says nothing.
    assert_eq!(read(&[(17, 0), (18, 8), (35, 1), (17, 3)]), (4, 4, 4.0));
    assert_eq!(read(&[(18, 8), (35, 1)]), (4, 4, 4.0));
}

#[test]
fn a_patterns_own_markers_stay_off_the_songs_ruler_and_a_marker_keeps_its_place_whatever_its_kind() {
    let kind = |kind: u32, ticks: u32| kind << 24 | ticks;
    let bytes = Flp::new()
        .ascii(199, "25.2.5.5319")
        .byte(17, 4)
        .byte(18, 4)
        .byte(35, 1)
        // A pattern with a Piano roll marker (a scale, kind 11) and its own time signature: neither is the song's.
        .word(65, 1)
        .dword(148, kind(11, 96))
        .byte(33, 3)
        .byte(34, 4)
        .text(205, "C Major")
        .byte(17, 3)
        .word(64, 0)
        .text(203, "Bass")
        .word(99, 0)
        // The song's: a loop marker (kind 4) on beat 3, and a time signature change that FL named itself.
        .dword(148, kind(4, 192))
        .text(205, "Loop")
        .dword(148, kind(8, 384))
        .byte(33, 6)
        .byte(34, 8)
        .text(205, "")
        .build();
    let (signature, markers) = arrangement::bars_and_markers(&events::load(&bytes).expect("an FL project"));
    assert_eq!(
        markers,
        [PreviewMarker { beat: 2.0, name: "Loop".into() }, PreviewMarker { beat: 4.0, name: "6/8".into() }]
    );
    // A change after the start isn't followed, and whoever carries the song on can say so.
    assert!(arrangement::changes_time_signature(&events::load(&bytes).expect("an FL project")));
    assert!(!arrangement::changes_time_signature(&events::load(&fl_studio_project()).expect("an FL project")));
    // A change further on isn't followed: the song is read by the signature it starts with.
    assert_eq!((signature.top(), signature.bottom()), (4, 4));

    // A time signature marker at the very start of the playlist says the song's over the project's own.
    let bytes = Flp::new()
        .ascii(199, "25.2.5.5319")
        .byte(17, 4)
        .byte(18, 4)
        .byte(35, 1)
        .word(99, 0)
        .dword(148, kind(8, 0))
        .byte(33, 12)
        .byte(34, 8)
        .text(205, "Shuffle")
        .build();
    let (signature, markers) = arrangement::bars_and_markers(&events::load(&bytes).expect("an FL project"));
    assert_eq!((signature.top(), signature.bottom(), signature.beats()), (12, 8, 6.0));
    assert_eq!(markers, [PreviewMarker { beat: 0.0, name: "Shuffle".into() }]);
}

#[test]
fn a_save_from_before_arrangements_keeps_every_marker_as_the_songs() {
    // FL 12 and older: no event 99, and a marker can follow a pattern's events with no playlist in between.
    let bytes = Flp::new()
        .ascii(199, "12.5.1.165")
        .word(64, 0)
        .text(203, "Bass")
        .word(65, 1)
        .dword(148, 4 << 24 | 192)
        .text(205, "Drop")
        .build();
    let (_, markers) = arrangement::bars_and_markers(&events::load(&bytes).expect("an FL project"));
    assert_eq!(markers, [PreviewMarker { beat: 2.0, name: "Drop".into() }]);
}

/// Whole-app review CN-7 (19 Nov 2026): with nothing in the playlist, patterns follow each other in the song's own
/// bars, not bars of 4 beats.
#[test]
fn patterns_with_no_playlist_follow_each_other_in_the_songs_own_bars() {
    let song = |top: u8| {
        let bytes = Flp::new()
            .ascii(199, "24.1.1.4234")
            .byte(17, top)
            .word(64, 0)
            .byte(21, 2)
            .text(201, "3x Osc")
            .text(203, "Bass")
            .word(65, 1)
            .data(224, &note(0, 0, 96, 48, 100))
            .word(65, 2)
            .data(224, &note(0, 0, 96, 50, 100))
            .build();
        let song = mo_formats::fl::song::read_song(&events::load(&bytes).expect("an FL project"));
        let clips: Vec<(i64, i64)> = song.channels[0].clips.iter().map(|c| (c.start, c.length)).collect();
        (song.signature.top(), clips)
    };
    assert_eq!(song(4), (4, vec![(0, 384), (384, 384)]));
    assert_eq!(song(3), (3, vec![(0, 288), (288, 288)]));
}
