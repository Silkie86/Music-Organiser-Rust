//! Automation clips and the mixer in FL projects: the C# `ReadSong` tests for them, on the same made-up files.

// Test files may unwrap: a test that stops there has failed, which is what it should do.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::*;
use mo_formats::fl::automation::FlControl;
use mo_formats::fl::{events, song};

fn read_song(bytes: &[u8]) -> song::FlSong {
    song::read_song(&events::load(bytes).expect("an FL project"))
}

fn points(song: &song::FlSong, name: &str) -> Vec<(f64, f64)> {
    let a = song.automation.iter().find(|a| a.name == name).expect("the automation clip");
    a.points.iter().map(|p| ((p.beat * 1e4).round() / 1e4, (p.value * 1e4).round() / 1e4)).collect()
}

fn channel<'a>(song: &'a song::FlSong, name: &str) -> &'a song::FlChannel {
    song.channels.iter().find(|c| c.name == name).expect("the channel")
}

#[test]
fn leaves_out_automation_clips_switched_off_in_the_channel_rack() {
    let names = |s: song::FlSong| s.automation.into_iter().map(|a| a.name).collect::<Vec<_>>();
    assert_eq!(names(read_song(&fl_studio_25_automation(false, false))), ["Snare - Channel volume", "Serum - Cutoff"]);
    assert_eq!(names(read_song(&fl_studio_25_automation(false, true))), ["Serum - Cutoff"]);
}

#[test]
fn reads_automation_clips_and_what_they_move() {
    let song = read_song(&fl_studio_25_automation(true, false));
    let volume = song.automation.iter().find(|a| a.name == "Snare - Channel volume").expect("the volume clip");
    assert_eq!((volume.channel, volume.control), (Some(0), FlControl::Volume));
    // Placed in bar 2 and again in bar 4: it holds its last value through the gap, then starts again.
    assert_eq!(
        points(&song, "Snare - Channel volume"),
        [(4.0, 0.0), (8.0, 1.0), (12.0, 1.0), (12.0, 0.0), (16.0, 1.0)]
    );
    let cutoff = song.automation.iter().find(|a| a.name == "Serum - Cutoff").expect("the cutoff clip");
    assert_eq!((cutoff.channel, cutoff.control, cutoff.parameter), (Some(2), FlControl::PluginParameter, 205));
    assert_eq!(points(&song, "Serum - Cutoff"), [(0.0, 0.2), (2.0, 1.0), (4.0, 0.5)]);
    assert_eq!(song.curves[&3].len(), 3);
    let snare = channel(&song, "Snare");
    assert_eq!((snare.root_key, snare.pitch_cents, snare.pan, snare.volume), (57, -200, 0.25, 1.0));
}

#[test]
fn automated_on_off_mutes_what_plays_while_the_channel_is_off() {
    let song = read_song(&fl_studio_25_switched_and_mixer(false));

    // The on/off holds each point's value until the next, and the points just past the clip's end (as FL saves its sums
    // of point gaps) count as at its end, jump and all: off 0-4, on 4-8, off 8-12, then on.
    let on_off = song.automation.iter().find(|a| a.name == "Bass - Mute / solo").expect("the on/off clip");
    assert_eq!((on_off.channel, on_off.control), (Some(0), FlControl::OnOff));
    assert_eq!(points(&song, "Bass - Mute / solo"), [(0.0, 0.0), (4.0, 1.0), (8.0, 0.0), (12.0, 0.0), (12.0, 0.97)]);

    // The bass is switched off in the Channel Rack, but that's only where the automation left it: it plays while it's
    // on. The clip at beats 0-4 is off all the way through, so it's muted; at 4-8 it plays; at 10-14 only its first
    // note (beat 10) starts while it's off.
    let bass = channel(&song, "Bass");
    assert!(bass.enabled);
    assert_eq!(bass.switched_by, ["Bass - Mute / solo"]);
    assert_eq!(
        bass.clips.iter().map(|c| (c.start, c.muted)).collect::<Vec<_>>(),
        [(0, true), (384, false), (960, false)]
    );
    assert_eq!(bass.clips[2].notes.iter().map(|n| n.muted).collect::<Vec<_>>(), [true, false]);
    assert_eq!(bass.notes.iter().map(|n| n.start).collect::<Vec<_>>(), [384, 576, 1152]);

    // The vox is off from beat 2: its clip at 0-4 plays part of the way, the one at 4-6 is muted.
    let vox = channel(&song, "Vox");
    assert_eq!(
        vox.audio_clips.iter().map(|c| (c.muted, c.partly_off)).collect::<Vec<_>>(),
        [(false, true), (true, false)]
    );
}

#[test]
fn follows_every_clip_that_switches_a_channel_and_keeps_automated_effects_on() {
    let song = read_song(&fl_studio_25_switched_and_mixer(true));

    // The first clip leaves the bass on from beat 12; the second switches it off at 16 and on again at 20.
    let bass = channel(&song, "Bass");
    assert_eq!(bass.switched_by, ["Bass - Mute / solo", "Bass - Mute / solo #2"]);
    assert_eq!(
        bass.clips.iter().map(|c| (c.start, c.muted)).collect::<Vec<_>>(),
        [(0, true), (384, false), (960, false), (1536, true), (1920, false)]
    );

    // WOW2 was off when the song was saved, but automation switches it, so it stays on.
    let insert = song.inserts.iter().find(|i| i.index == 1).expect("insert 1");
    assert_eq!(insert.effects.iter().map(|e| e.enabled).collect::<Vec<_>>(), [true, true]);

    // A rise that jumps back down at the clip's end rises each time the clip plays.
    assert_eq!(points(&song, "Bass - Volume"), [(0.0, 0.0), (4.0, 1.0), (4.0, 0.0), (8.0, 1.0), (8.0, 0.0)]);
}

#[test]
fn where_clips_switching_a_channel_meet_the_one_starting_there_decides() {
    for on_linked_first in [false, true] {
        let song = read_song(&fl_studio_25_back_to_back(on_linked_first));

        // "Kick off" in bars 1 and 3, "Kick on" in bar 2, whichever is linked first in the file.
        let kick = channel(&song, "Kick");
        let mut switched = kick.switched_by.clone();
        switched.sort();
        assert_eq!(switched, ["Kick off", "Kick on"]);
        assert_eq!(
            kick.clips.iter().map(|c| (c.start, c.muted)).collect::<Vec<_>>(),
            [(0, true), (384, false), (768, true)],
            "{on_linked_first}"
        );

        // The volume clip placed again over the second half of itself takes over where it starts: halfway up, it starts
        // again.
        assert_eq!(points(&song, "Kick - Volume"), [(0.0, 0.0), (4.0, 0.5), (4.0, 0.0), (12.0, 1.0)]);
        // Placed again inside itself, it plays that stretch, then the longer placement carries on.
        assert_eq!(
            points(&song, "Kick - Pan"),
            [(0.0, 0.0), (2.0, 0.25), (2.0, 0.0), (4.0, 0.25), (4.0, 0.5), (8.0, 1.0)]
        );
        assert_eq!(points(&song, "Kick - Pitch"), [(0.0, 0.0), (4.0, 0.5), (8.0, 1.0)]);
    }
}

#[test]
fn reads_mixer_insert_settings_effect_slots_and_what_automation_moves_on_them() {
    let song = read_song(&fl_studio_25_switched_and_mixer(false));

    // Insert 1's fader is moved and its second effect, in slot 1, is switched off; the master is as FL leaves it.
    let insert = song.inserts.iter().find(|i| i.index == 1).expect("insert 1");
    assert_eq!(
        insert.effects.iter().map(|e| (e.name.as_str(), e.slot, e.enabled)).collect::<Vec<_>>(),
        [("Pro-Q 3", Some(0), true), ("WOW2", Some(1), false)]
    );
    assert_eq!((insert.fader, insert.pan, insert.moved()), (Some(9600), Some(0), true));
    assert_eq!(
        song.inserts.iter().map(|i| (i.index, i.fader, i.moved())).collect::<Vec<_>>(),
        [(0, Some(12800), false), (1, Some(9600), true)]
    );
    // Pattern 1 holds edit events, which are found but not read yet.
    assert_eq!(song.edit_events, [1]);
    // Where each insert goes: insert 1's slots come before its routes, which still belong to it.
    assert_eq!(song.routes.into_iter().collect::<Vec<_>>(), [(0, vec![]), (1, vec![0, 2]), (2, vec![0])]);

    // Automation of a setting of the effect in insert 1's slot 0, of the tempo and of insert 1's fader.
    let moved: Vec<_> = song
        .automation
        .iter()
        .filter(|a| a.control != FlControl::OnOff)
        .map(|a| (a.name.as_str(), a.control, a.channel, a.insert, a.slot, a.parameter))
        .collect();
    assert_eq!(
        moved,
        [
            ("Pro-Q 3 - Gain", FlControl::InsertEffect, None, Some(1), Some(0), 3),
            ("Tempo", FlControl::Tempo, None, None, None, 5),
            ("Insert 1 - Volume", FlControl::InsertVolume, None, Some(1), Some(0), 0x1fc0),
        ]
    );
}

#[test]
fn a_sample_on_a_channel_automation_switches_on_is_played() {
    // The bass has no sample, but the vox's channel is switched on by its light and only automated off part of the way.
    let info = mo_formats::fl::read_bytes(&fl_studio_25_switched_and_mixer(false));
    assert!(info.unplayed.is_empty(), "{:?}", info.unplayed);
}
