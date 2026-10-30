//! The song in an FL project: notes, the playlist, channels and audio clips. The C# reader tests for `ExtractMidi`,
//! `ExtractChannels` and `ReadSong`, on the same made-up files.

// Test files may unwrap: a test that stops there has failed, which is what it should do.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::BTreeMap;

use common::*;
use mo_formats::fl::{self, events, song};
use mo_formats::{MidiChannel, NoteEvent, SampleUse};

/// A note as read from FL, its velocity as FL writes it (0 to 128).
fn note(start: i64, length: i64, key: u8, fl_velocity: u8) -> NoteEvent {
    let velocity = mo_formats::velocity::from_unit(fl_velocity as f64 / 128.0);
    NoteEvent { start, length, key, velocity, muted: false, step: false }
}

fn channels(bytes: &[u8]) -> BTreeMap<String, MidiChannel> {
    let flp = events::load(bytes).expect("an FL project");
    song::extract_channels(&flp).into_iter().map(|c| (c.name.clone(), c)).collect()
}

fn read_song(bytes: &[u8]) -> song::FlSong {
    song::read_song(&events::load(bytes).expect("an FL project"))
}

#[test]
fn extracts_patterns_as_midi_clips() {
    let bytes = fl_studio_project();
    let clips = song::extract_midi(&events::load(&bytes).expect("an FL project"));
    assert_eq!(clips.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Intro", "Drop"]);
    let intro = &clips[0];
    assert_eq!(intro.ppq, 96);
    assert_eq!(intro.tracks.iter().map(|t| t.0.as_str()).collect::<Vec<_>>(), ["Lead", "Kick"]);
    assert_eq!(intro.tracks[0].1[1], note(96, 96, 64, 128));
    assert_eq!(clips[1].tracks.len(), 1);
    assert_eq!(clips[1].tracks[0].0, "Bass");
}

#[test]
fn extracts_each_channel_with_what_plays_it() {
    // No playlist: patterns one after another, each rounded up to a bar (4 × 96 ticks).
    let flp_bytes = fl_studio_project();
    let flp = events::load(&flp_bytes).expect("an FL project");
    let channels = song::extract_channels(&flp);
    assert_eq!(
        channels.iter().map(|c| (c.name.as_str(), c.source.as_str(), c.plugin.as_str())).collect::<Vec<_>>(),
        [("Lead", "Serum", "Serum"), ("Kick", "Kick 01.wav", ""), ("Bass", "3x Osc", "3x Osc")]
    );
    assert_eq!(channels[0].notes.iter().map(|n| n.start).collect::<Vec<_>>(), [0, 96]);
    assert_eq!(channels[2].notes.iter().map(|n| n.start).collect::<Vec<_>>(), [384]);
    assert_eq!(channels[1].sample_path, r"C:\Samples\Drums\Kick 01.wav");
    assert_eq!((channels[0].channel, channels[0].root_key), (Some(0), 60));
}

#[test]
fn places_patterns_where_the_playlist_puts_them() {
    // Drop at the start; then the second half of Intro from tick 768.
    let playlist = [playlist_item(0, 2, 384, -1), playlist_item(768, 1, 192, 96)].concat();
    let channels = channels(&fl_studio_project_with(Some(&playlist)));
    assert_eq!(channels["Bass"].notes.iter().map(|n| n.start).collect::<Vec<_>>(), [0]);
    assert_eq!(channels["Lead"].notes, [note(768, 96, 64, 128)]);
    assert!(!channels.contains_key("Kick"), "its note sits before the offset");
}

#[test]
fn reads_muted_playlist_tracks_and_switched_off_channels() {
    let bytes = fl_studio_25_muted_tracks();
    let song = read_song(&bytes);

    // Tracks 2 and 4 are muted in the first arrangement (the second's muted track 1 doesn't count), so what's on them
    // doesn't play: the Verse pattern in bar 2, the Acapella clips and the Pad. The clips themselves aren't muted.
    assert_eq!(
        song.muted_tracks,
        [song::FlTrack { number: 2, name: "Acapellas".into() }, song::FlTrack { number: 4, name: String::new() }]
    );
    let by_name = |name: &str| song.channels.iter().find(|c| c.name == name).expect("the channel");
    let snare = by_name("Snare");
    assert_eq!(
        snare.clips.iter().map(|c| (c.start, c.muted, c.on_muted_track, c.track)).collect::<Vec<_>>(),
        [(0, false, false, 1), (384, false, true, 2)]
    );
    assert_eq!(snare.notes.iter().map(|n| n.start).collect::<Vec<_>>(), [0]);
    assert!(by_name("Pad").notes.is_empty());
    let audio =
        |name: &str| by_name(name).audio_clips.iter().map(|c| (c.muted, c.on_muted_track, c.track)).collect::<Vec<_>>();
    assert_eq!(audio("Acapella"), [(false, true, 2), (false, true, 4)]);
    assert_eq!(audio("Vox"), [(false, false, 1), (false, false, 3)]);
    let midi = channels(&bytes);
    assert_eq!(midi["Snare"].notes.iter().map(|n| n.start).collect::<Vec<_>>(), [0]);
    assert!(!midi.contains_key("Pad"));

    // The hat and the vox are switched off in the Channel Rack (the project's own event 0 before them isn't a channel's).
    assert_eq!(
        song.channels.iter().map(|c| (c.name.as_str(), c.enabled)).collect::<Vec<_>>(),
        [("Snare", true), ("Hat", false), ("Acapella", true), ("Vox", false), ("Pad", true)]
    );
    assert_eq!(by_name("Acapella").stretch.mode, 1);
}

#[test]
fn says_which_samples_a_save_holds_without_playing() {
    let info = fl::read_bytes(&fl_studio_25_unplayed(false));
    assert_eq!(info.samples.len(), 3);
    assert_eq!(
        info.unplayed.iter().map(|(s, u)| (s.as_str(), *u)).collect::<Vec<_>>(),
        [(r"C:\Samples\Hat 01.wav", SampleUse::Muted), (r"C:\Samples\Kick 01.wav", SampleUse::NoNotes)]
    );
    let playing = fl::read_bytes(&fl_studio_25_unplayed(true));
    assert_eq!(playing.unplayed.values().copied().collect::<Vec<_>>(), [SampleUse::NoNotes]);

    // Switched off in the Channel Rack: the hat's sample doesn't play, though its clips aren't muted.
    let civil = fl::read_bytes(&fl_studio_25_muted_tracks());
    assert_eq!(civil.unplayed.get(r"C:\Samples\Hat 01.wav"), Some(&SampleUse::Muted));
    assert_eq!(civil.unplayed.get(r"C:\Samples\Pad.wav"), Some(&SampleUse::Muted), "on a muted track");
    assert_eq!(civil.unplayed.get(r"C:\Samples\Snare 01.wav"), None);
}

#[test]
fn reads_fl_9_step_sequencer_drums_and_playlist() {
    let song = read_song(&fl_studio_9_steps());
    let kick = &song.channels[0];
    // Placed at beat 8 (768 ticks): odd steps swung 3 ticks by shuffle 36, step 7 also shifted 12.
    assert_eq!(
        kick.notes.iter().map(|n| (n.start, n.key)).collect::<Vec<_>>(),
        [(768, 60), (768 + 123, 60), (768 + 183, 60)]
    );
    assert!(kick.notes[1].velocity < kick.notes[0].velocity);
    assert!(kick.notes.iter().all(|n| n.step));
}

#[test]
fn reads_fl_8_and_older_20_byte_notes_when_the_pattern_is_also_a_whole_number_of_24_byte_ones() {
    // Six of FL 7's 20-byte notes are 120 bytes, five 24-byte ones too. Read as 24, Solomon's "i dont know_8" had notes
    // at jumbled times lasting 2^31 ticks, its pan and velocity bytes landing in the length.
    let note20 = |pos: u32, key: u16| {
        let mut b = vec![0u8; 20];
        b[..4].copy_from_slice(&pos.to_le_bytes());
        b[4..6].copy_from_slice(&0x4000u16.to_le_bytes());
        b[8..12].copy_from_slice(&24u32.to_le_bytes());
        b[12..14].copy_from_slice(&key.to_le_bytes());
        // Fine pitch, pan, velocity, release, as FL 7 saves them.
        (b[14], b[16], b[17], b[18], b[19]) = (0x78, 0x40, 0x80, 0x80, 0x80);
        b
    };
    let notes: Vec<u8> = (0..6).flat_map(|i| note20(i * 24, 36 + i as u16)).collect();
    for version in [Some("7.0.0"), None] {
        let mut b = Flp::new();
        if let Some(v) = version {
            b = b.ascii(199, v);
        }
        let bytes = b.word(64, 0).byte(21, 0).ascii(192, "Kick").word(65, 1).data(224, &notes).build();
        let flp = events::load(&bytes).expect("an FL project");
        let clips = song::extract_midi(&flp);
        let kick = &clips[0].tracks[0].1;
        assert_eq!(kick.iter().map(|n| n.start).collect::<Vec<_>>(), [0, 24, 48, 72, 96, 120], "{version:?}");
        assert!(kick.iter().all(|n| (n.length, n.velocity7()) == (24, 127)), "{version:?}");
        assert_eq!(kick.iter().map(|n| n.key).collect::<Vec<_>>(), [36, 37, 38, 39, 40, 41]);
        assert_eq!(song::read_song(&flp).channels[0].notes.len(), 6);
        assert_eq!(fl::read_bytes(&bytes).details["Notes"], "6", "{version:?}");
    }
}

#[test]
fn reads_channel_pan_and_volume_on_the_scale_of_the_fl_version_that_saved_it() {
    // FL 4 and 5 count to 128, FL 6 on to 12800: Solomon's FL 4 grimey came into Bitwig panned hard left and silent.
    for (version, pan, volume, pan_knob, volume_knob) in [
        ("4.0.1", 64, 70, 0.5, 70.0 / 128.0),
        ("5.0.0", 20, 128, 20.0 / 128.0, 1.0),
        ("6.0.4", 6400, 10000, 0.5, 10000.0 / 12800.0),
    ] {
        let bytes = Flp::new()
            .ascii(199, version)
            .word(64, 0)
            .byte(21, 0)
            .ascii(192, "snr7")
            .data(219, &levels(pan, volume, 0))
            .build();
        let channel = &read_song(&bytes).channels[0];
        assert!((channel.pan - pan_knob).abs() < 1e-9, "{version}");
        assert!((channel.volume - volume_knob).abs() < 1e-9, "{version}");
    }
}

#[test]
fn a_channels_plugin_and_its_ids() {
    let song = read_song(&fl_studio_25_project());
    let plugin = |name: &str| {
        song.channels.iter().find(|c| c.name == name).and_then(|c| c.instrument.clone()).expect("a plugin")
    };
    let serum = plugin("Lead");
    assert_eq!((serum.name.as_str(), serum.format, serum.vendor.as_str()), ("Serum", "VST2", "Xfer Records"));
    assert_eq!(serum.device_id, i32::from_le_bytes(*b"XsfX").to_string());
    let arcade = plugin("Keys");
    assert_eq!(arcade.format, "VST3");
    assert_eq!(arcade.device_id, "56535441524344617263616465000000");
    assert!(!serum.nothing_saved);

    let cell = read_song(&fl_studio_25_nothing_saved()).channels[0].instrument.clone().expect("a plugin");
    assert!(cell.nothing_saved);
    let native = read_song(&fl_studio_project()).channels[2].instrument.clone().expect("a plugin");
    assert_eq!((native.name.as_str(), native.format, native.native()), ("3x Osc", "FL", true));
}

#[test]
fn class_ids_are_written_in_windows_guid_order() {
    let tuid = [0x41, 0x54, 0x53, 0x56, 0x43, 0x52, 0x61, 0x44, 0x72, 0x63, 0x61, 0x64, 0x65, 0, 0, 0];
    assert_eq!(song::class_id_string(&tuid), "56535441524344617263616465000000");
}

#[test]
fn an_old_playlist_with_more_patterns_than_tracks_still_reads() {
    // FL 9 rows go down to 1, so up to 999 patterns; newer FL has 500 tracks, so the last ones have no track.
    let items: Vec<u8> = (0..600u32)
        .flat_map(|n| {
            let mut item = vec![0u8; 32];
            item[..4].copy_from_slice(&(n * 384).to_le_bytes());
            item[6..8].copy_from_slice(&0x6000u16.to_le_bytes());
            item[8..12].copy_from_slice(&384u32.to_le_bytes());
            item[12..14].copy_from_slice(&(999 - n as u16).to_le_bytes());
            item
        })
        .collect();
    let bytes = Flp::new()
        .ascii(199, "9.0.0")
        .word(64, 0)
        .ascii(192, "Kick")
        .word(65, 599)
        .data(224, &common::note(0, 0, 96, 60, 100))
        .data(233, &items)
        .build();
    let song = read_song(&bytes);
    assert_eq!(song.channels[0].clips.len(), 1);
    assert_eq!(song.channels[0].clips[0].start, 599 * 384);
    assert_eq!(fl::read_bytes(&bytes).details["Notes"], "1");
}

#[test]
fn a_songs_samples_under_fls_user_data_folder_are_found() {
    let dir = test_dir("song-user-data");
    let projects = dir.join("FL Studio").join("Projects");
    std::fs::create_dir_all(&projects).expect("make the folders");
    let path = projects.join("Shuffle.flp");
    let bytes = fl_studio_25_project();
    let mut song = read_song(&bytes);
    song.resolve_samples(&path);
    assert_eq!(song.channels[0].sample, projects.join("Snare 01.wav").to_string_lossy());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_pattern_not_in_a_playlist_of_audio_clips_does_not_play() {
    // The playlist has an audio clip, so it isn't empty: patterns don't play one after another.
    let song = read_song(&fl_studio_25_audio_only());
    let snare = song.channels.iter().find(|c| c.name == "Snare").expect("the snare channel");
    assert!(snare.notes.is_empty() && snare.clips.is_empty());
    let info = fl::read_bytes(&fl_studio_25_audio_only());
    assert_eq!(info.unplayed.get(r"C:\Samples\Snare 01.wav"), Some(&SampleUse::NoNotes));
    // An insert name that's only its end isn't a name, so insert 2 isn't listed (the master is: the channels go to it).
    let names: Vec<(u32, &str)> = song.inserts.iter().map(|i| (i.index, i.name.as_str())).collect();
    assert_eq!(names, [(0, ""), (1, "Drums")]);
}
