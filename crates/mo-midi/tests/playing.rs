//! A saved song read as it plays (`mo_midi::playing`), on the made-up FL Studio projects the format tests use.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

#[allow(unused_imports)]
#[path = "../../mo-formats/tests/common/mod.rs"]
mod common;

use std::path::Path;

use common::*;
use mo_formats::fl;
use mo_midi::Role;
use mo_midi::playing::{self, ClipPlace, FL_VIEW};

fn names(song: &playing::Song) -> Vec<&str> {
    song.parts.iter().map(|p| p.name.as_str()).collect()
}

#[test]
fn a_channel_switched_off_in_the_channel_rack_plays_nothing() {
    // The Verse pattern holds the Snare and the Hat: it plays in bar 1, and again in bar 2 on a muted track. The
    // Hat's green light is off. The Pad's pattern is only on a muted track.
    let bytes = fl_studio_25_muted_tracks();
    let song = playing::from_bytes(Path::new("song.flp"), &bytes).unwrap();
    assert_eq!(song.view, FL_VIEW);
    assert_eq!(names(&song), ["Snare"]);
    let snare = &song.parts[0];
    assert_eq!(snare.notes.iter().map(|n| (n.start, n.key)).collect::<Vec<_>>(), [(0, 60)]);
    assert_eq!(snare.clips, [ClipPlace { name: "Verse".into(), bars: 0..1 }]);
    // It plays a sample, whose own pitch the project doesn't say: not one of the parts fitted to unless picked.
    assert!(snare.sample);
    assert!(song.harmony_parts().is_empty() && song.melody_parts().is_empty());

    // Read from the arrangement's lane-a-channel layout instead, the Snare plays the same. (Whether the Hat is left
    // out there too is that layout's to say, by marking its lane muted.)
    let flp = fl::events::load(&bytes).unwrap();
    let views = fl::arrangement::read(&flp);
    let by_lanes = playing::of(&views[1]);
    assert_eq!(by_lanes.parts[0].name, "Snare");
    assert_eq!(by_lanes.parts[0].notes.iter().map(|n| (n.start, n.key)).collect::<Vec<_>>(), [(0, 60)]);
}

#[test]
fn each_channel_plays_where_its_patterns_are_placed() {
    // Drop at the start; then the second half of Intro from tick 768 (bar 3).
    let playlist = [playlist_item(0, 2, 384, -1), playlist_item(768, 1, 192, 96)].concat();
    let song = playing::from_bytes(Path::new("song.flp"), &fl_studio_project_with(Some(&playlist))).unwrap();
    // The Kick's note sits before where the Intro is played from.
    assert_eq!(names(&song), ["Lead", "Bass"]);
    let (lead, bass) = (&song.parts[0], &song.parts[1]);
    assert_eq!(lead.notes.iter().map(|n| (n.start, n.length, n.key)).collect::<Vec<_>>(), [(768, 96, 64)]);
    assert_eq!((lead.plugin.as_str(), lead.sample, lead.bars.clone()), ("Serum", false, 2..3));
    assert_eq!(lead.clips, [ClipPlace { name: "Intro".into(), bars: 2..3 }]);
    assert_eq!(bass.notes.iter().map(|n| (n.start, n.key)).collect::<Vec<_>>(), [(0, 41)]);
    assert_eq!((bass.plugin.as_str(), bass.role, bass.bars.clone()), ("3x Osc", Role::Bass, 0..1));
    assert_eq!((song.tempo, song.beats_per_bar, song.bars()), (128.0, 4, 3));

    // With nothing in the playlist FL plays the patterns in pattern mode: here they come one after another.
    let song = playing::from_bytes(Path::new("song.flp"), &fl_studio_project()).unwrap();
    assert_eq!(names(&song), ["Lead", "Kick", "Bass"]);
    assert_eq!(song.parts[2].bars, 1..2);
}

#[test]
fn a_patterns_name_is_not_its_channels() {
    // Single notes around middle C on a channel called Synth, in a pattern called "Bass": the pattern's name is
    // every channel's in it, so it doesn't make this one a bassline.
    let tune: Vec<u8> =
        [(0, 60), (96, 62), (192, 64), (288, 62)].iter().flat_map(|&(at, key)| note(at, 0, 96, key, 100)).collect();
    let bytes = Flp::new()
        .ascii(199, "24.1.1.4234")
        .dword(156, 120_000)
        .word(64, 0)
        .byte(21, 2)
        .text(201, "3x Osc")
        .text(203, "Synth")
        .data(213, &[9])
        .word(65, 1)
        .text(193, "Bass")
        .data(224, &tune)
        .build();
    let song = playing::from_bytes(Path::new("song.flp"), &bytes).unwrap();
    assert_eq!(names(&song), ["Synth"]);
    assert_eq!(song.parts[0].role, Role::Melody, "{song:?}");
    assert_eq!(song.melody_parts(), [0]);
}
