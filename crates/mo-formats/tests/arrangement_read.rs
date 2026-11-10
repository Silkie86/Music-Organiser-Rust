//! The arrangement preview of Bitwig and Studio One projects: their tracks and the clips on them, as each DAW shows
//! them. The C# app's `BitwigArrangement` and `StudioOneArrangement` on the same made-up files.

// Test files may unwrap: a test that stops there has failed, which is what it should do.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::Path;

use common::bitwig::*;
use common::studio_one::*;
use mo_formats::arrangement::{self, Arrangement, LaneKind, PreviewMarker, Stretch};
use mo_formats::velocity;

/// The one view a project has.
fn read(name: &str, bytes: &[u8]) -> Arrangement {
    let mut views = arrangement::read_bytes(Path::new(name), bytes);
    assert_eq!(views.len(), 1);
    views.pop().unwrap()
}

/// Every view a project has, in the order they're offered.
fn views(name: &str, bytes: &[u8]) -> Vec<Arrangement> {
    arrangement::read_bytes(Path::new(name), bytes)
}

/// Each lane as (name, kind, depth).
fn lanes(a: &Arrangement) -> Vec<(&str, LaneKind, u32)> {
    a.lanes.iter().map(|l| (l.name.as_str(), l.kind, l.depth)).collect()
}

/// A clip's notes as (start, length, key).
fn notes(a: &Arrangement, lane: usize, clip: usize) -> Vec<(f64, f64, u8)> {
    a.lanes[lane].clips[clip].notes.iter().map(|n| (n.start, n.length, n.key)).collect()
}

// Bitwig's objects and fields, as mapped from Solomon's projects.
const PROJECT_GROUP: u32 = 0x4dd;
const GROUP_TRACKS: u32 = 0x4de;
const TRACK_NAME: u32 = 0x15b;
const TRACK_COLOUR: u32 = 0x54c;
const TRACK_LANES: u32 = 0x2bf8;
const ARRANGER: u32 = 0x2d8e;
const LAUNCHER: u32 = 0x2d8f;
const SLOTS: u32 = 0x231;
const ITEMS: u32 = 0x21f;
const START: u32 = 0x2af;
const DURATION: u32 = 0x26;
const CLIP: u32 = 0x47;

/// A note clip `length` beats long at `start`, playing a 4-beat loop of a kick on beat 1 and a long one on beat 4.
fn note_clip(b: &mut BwWriter, start: f64, length: f64, looping: bool) {
    note_clip_looping(b, start, length, looping, 4.0);
}

#[test]
fn a_bitwig_clip_plays_from_its_start_marker() {
    // A 6-beat clip whose start marker is on beat 3 of its 4-beat loop: the long note on beat 4 first, then the loop
    // from its start (kick at 1, long note at 4 cut by the clip's end).
    let bytes = bitwig_project(|b| {
        track(
            b,
            "Beat",
            None,
            |b| {
                b.obj(ARRANGER);
                b.begin(0x511);
                b.list(ITEMS);
                note_clip_from(b, 0.0, 6.0, true, 4.0, Some(2.0));
                b.end_list();
                b.end();
            },
            |_| {},
        );
    });
    let a = read("Marker.bwproject", &bytes);
    assert_eq!(notes(&a, 0, 0), [(1.0, 2.0, 36), (2.0, 0.5, 36), (5.0, 1.0, 36)]);
}

#[test]
fn a_bitwig_clip_with_a_far_start_marker_is_read_at_once() {
    // Review B-1: a play start far after a short loop walked pass by pass. One far beyond what an arrangement holds
    // (a damaged file) is left out, so the clip plays from its loop's start.
    let clip = |play_start: f64, loop_beats: f64| {
        let bytes = bitwig_project(|b| {
            track(
                b,
                "Beat",
                None,
                |b| {
                    b.obj(ARRANGER);
                    b.begin(0x511);
                    b.list(ITEMS);
                    note_clip_from(b, 0.0, 4.0, true, loop_beats, Some(play_start));
                    b.end_list();
                    b.end();
                },
                |_| {},
            );
        });
        read("Far.bwproject", &bytes)
    };
    let started = std::time::Instant::now();
    assert_eq!(notes(&clip(1.7e100, 4.0), 0, 0), [(0.0, 0.5, 36), (3.0, 1.0, 36)]);
    // A quarter-beat loop a million beats before: the kick on every pass, found without walking there.
    assert_eq!(notes(&clip(1.0e6, 0.25), 0, 0).len(), 16);
    assert!(started.elapsed().as_secs() < 5);
}

#[test]
fn a_bitwig_loop_far_beyond_the_arrangement_is_not_moved_to_the_start() {
    // Review of #406: a loop start left out as damaged doesn't become a loop at 0 the file doesn't hold.
    let bytes = bitwig_project(|b| {
        track(
            b,
            "Beat",
            None,
            |b| {
                b.obj(ARRANGER);
                b.begin(0x511);
                b.list(ITEMS);
                note_clip_loop_at(b, 0.0, 8.0, true, 5.0e7, 4.0, None);
                b.end_list();
                b.end();
            },
            |_| {},
        );
    });
    assert_eq!(notes(&read("FarLoop.bwproject", &bytes), 0, 0), [(0.0, 0.5, 36), (3.0, 2.0, 36)]);
}

/// [`note_clip`] with a loop `loop_beats` long; returns the clip's number, for references.
fn note_clip_looping(b: &mut BwWriter, start: f64, length: f64, looping: bool, loop_beats: f64) -> u32 {
    note_clip_from(b, start, length, looping, loop_beats, None)
}

/// [`note_clip_looping`] that starts playing from `play_start` (its start marker).
fn note_clip_from(
    b: &mut BwWriter,
    start: f64,
    length: f64,
    looping: bool,
    loop_beats: f64,
    play_start: Option<f64>,
) -> u32 {
    note_clip_loop_at(b, start, length, looping, 0.0, loop_beats, play_start)
}

/// [`note_clip_from`] whose loop starts at `loop_at`.
fn note_clip_loop_at(
    b: &mut BwWriter,
    start: f64,
    length: f64,
    looping: bool,
    loop_at: f64,
    loop_beats: f64,
    play_start: Option<f64>,
) -> u32 {
    let clip = b.begin(CLIP);
    b.f64(START, start);
    b.f64(DURATION, length);
    b.obj(0x2ae9);
    b.begin(0x500);
    b.obj(0x10e9);
    b.begin(0x501);
    b.str(0x2d24, "Beat");
    b.obj(0x49c);
    b.begin(0x502);
    b.list(0x18cb);
    b.begin(0x42);
    b.f64(0xee, 36.0);
    b.list(ITEMS);
    for (at, long) in [(0.0, 0.5), (3.0, 2.0)] {
        b.begin(0x66);
        b.f64(START, at);
        b.f64(DURATION, long);
        b.f64(0xef, 1.0);
        b.end();
    }
    b.end_list();
    b.end();
    b.end_list();
    b.end();
    b.end();
    b.end();
    b.obj(0x2adb);
    b.begin(0x503);
    b.bool(0x991, looping);
    b.obj(0x992);
    b.begin(0x504);
    b.f64(START, loop_at);
    b.f64(DURATION, loop_beats);
    b.end();
    if let Some(at) = play_start {
        b.obj(0x98c);
        b.begin(0x505);
        b.f64(START, at);
        b.end();
    }
    b.end();
    b.end();
    clip
}

/// A track named `name`; `lane` writes what's on its timeline or in its launcher; `inner` writes the tracks in it.
fn track(
    b: &mut BwWriter,
    name: &str,
    colour: Option<[f32; 4]>,
    lane: impl FnOnce(&mut BwWriter),
    inner: impl FnOnce(&mut BwWriter),
) {
    b.begin(0x285);
    b.str(TRACK_NAME, name);
    if let Some(c) = colour {
        b.floats(TRACK_COLOUR, &c);
    }
    b.obj(TRACK_LANES);
    b.begin(0x510);
    lane(b);
    b.end();
    b.list(GROUP_TRACKS);
    inner(b);
    b.end_list();
    b.end();
}

fn bitwig_project(tracks: impl FnOnce(&mut BwWriter)) -> Vec<u8> {
    BwWriter::file(
        |_| {},
        |b| {
            b.begin(0x520);
            b.obj(PROJECT_GROUP);
            b.begin(0x521);
            b.list(GROUP_TRACKS);
            tracks(b);
            b.end_list();
            b.end();
            b.end();
        },
        None,
    )
}

#[test]
fn bitwig_tracks_in_groups_with_looping_clips() {
    let bytes = bitwig_project(|b| {
        track(
            b,
            "Drums",
            Some([1.0, 0.5, 0.0, 1.0]),
            |_| {},
            |b| {
                track(
                    b,
                    "Beat",
                    None,
                    |b| {
                        b.obj(ARRANGER);
                        b.begin(0x511);
                        b.list(ITEMS);
                        note_clip(b, 8.0, 8.0, true);
                        note_clip(b, 16.0, 4.0, false);
                        b.end_list();
                        b.end();
                    },
                    |_| {},
                )
            },
        );
    });
    let a = read("Groups.bwproject", &bytes);
    assert_eq!(a.view, "Bitwig arranger");
    assert!(a.notes.is_empty(), "{:?}", a.notes);
    assert_eq!(lanes(&a), [("Drums", LaneKind::Group, 0), ("Beat", LaneKind::Instrument, 1)]);
    assert_eq!(a.lanes[0].colour, Some(0xFF_80_00));
    let beat = &a.lanes[1];
    assert_eq!(
        beat.clips.iter().map(|c| (c.name.as_str(), c.start, c.length)).collect::<Vec<_>>(),
        [("Beat", 8.0, 8.0), ("Beat", 16.0, 4.0)]
    );
    // The loop plays twice in the 8-beat clip; the second pass's long note is cut where the clip ends.
    assert_eq!(notes(&a, 1, 0), [(0.0, 0.5, 36), (3.0, 2.0, 36), (4.0, 0.5, 36), (7.0, 1.0, 36)]);
    assert_eq!(notes(&a, 1, 1), [(0.0, 0.5, 36), (3.0, 1.0, 36)]);
    assert_eq!(beat.clips[0].notes[0].velocity, velocity::from_unit(1.0));
    assert_eq!(a.length(), 20.0);
}

#[test]
fn a_bitwig_project_with_only_launcher_clips_shows_its_scenes_first() {
    // Each slot as (length, looping); a looping clip's loop is as long as the clip.
    let launcher = |slots: &'static [Option<(f64, bool)>]| {
        move |b: &mut BwWriter| {
            b.obj(LAUNCHER);
            b.begin(0x512);
            b.list(SLOTS);
            for slot in slots {
                b.begin(0x513);
                if let Some((length, looping)) = slot {
                    b.obj(0x240);
                    note_clip_looping(b, 0.0, *length, *looping, *length);
                }
                b.end();
            }
            b.end_list();
            b.end();
        }
    };
    let bytes = bitwig_project(|b| {
        track(b, "Keys", None, launcher(&[Some((8.0, true)), None, Some((4.0, false))]), |_| {});
        track(b, "Bass", None, launcher(&[Some((4.0, true)), Some((2.0, true))]), |_| {});
        // Bitwig 6 can define a clip at its first use outside the launcher (the track's 0x15f) and only refer to it
        // from the slot.
        track(
            b,
            "Pad",
            None,
            |b| {
                b.obj(0x15f);
                let clip = note_clip_looping(b, 0.0, 2.0, false, 2.0);
                b.obj(LAUNCHER);
                b.begin(0x512);
                b.list(SLOTS);
                b.begin(0x513);
                b.bare_ref(0x240, clip);
                b.end();
                b.end_list();
                b.end();
            },
            |_| {},
        );
    });
    let all = views("Jam.bwproject", &bytes);
    assert_eq!(all.iter().map(|v| v.view).collect::<Vec<_>>(), ["Bitwig scenes", "Bitwig arranger"]);
    let (a, arranger) = (&all[0], &all[1]);
    assert!(arranger.lanes.iter().all(|l| l.clips.is_empty()));
    assert_eq!(arranger.notes, ["Nothing is on the arrangement timeline; the clips are in the clip launcher."]);
    let starts = |lane: usize| a.lanes[lane].clips.iter().map(|c| (c.start, c.length)).collect::<Vec<_>>();
    // Scene 1 is as long as its longest clip (8 beats), and the looping bass fills it; scene 2 takes 2 beats and
    // scene 3 4. The pad doesn't loop, so it stops at its own end.
    assert_eq!(starts(0), [(0.0, 8.0), (10.0, 4.0)]);
    assert_eq!(starts(1), [(0.0, 8.0), (8.0, 2.0)]);
    assert_eq!(starts(2), [(0.0, 2.0)]);
    assert_eq!(notes(a, 1, 0), [(0.0, 0.5, 36), (3.0, 2.0, 36), (4.0, 0.5, 36), (7.0, 1.0, 36)]);
    assert_eq!(
        a.markers.iter().map(|m| (m.beat, m.name.as_str())).collect::<Vec<_>>(),
        [(0.0, "Scene 1"), (8.0, "Scene 2"), (10.0, "Scene 3")]
    );

    let empty = read("Empty.bwproject", &bitwig_project(|b| track(b, "Keys", None, |_| {}, |_| {})));
    assert_eq!(empty.notes, ["Nothing is on the arrangement timeline or in the clip launcher yet."]);
}

/// A note clip as Bitwig wrote it before version 6: its name on the clip, its content straight in 0x288, and the play
/// range inside the content. Same kick loop as [`note_clip`].
fn old_note_clip(b: &mut BwWriter, start: f64, length: f64) {
    b.begin(CLIP);
    b.f64(START, start);
    b.f64(DURATION, length);
    b.obj(0x288);
    b.begin(0x501);
    b.obj(0x98f);
    b.begin(0x503);
    b.bool(0x991, true);
    b.obj(0x992);
    b.begin(0x504);
    b.f64(START, 0.0);
    b.f64(DURATION, 4.0);
    b.end();
    b.end();
    b.obj(0x49c);
    b.begin(0x502);
    b.list(0x18cb);
    b.begin(0x42);
    b.f64(0xee, 36.0);
    b.list(ITEMS);
    for (at, long) in [(0.0, 0.5), (3.0, 2.0)] {
        b.begin(0x66);
        b.f64(START, at);
        b.f64(DURATION, long);
        b.end();
    }
    b.end_list();
    b.end();
    b.end_list();
    b.end();
    b.end();
    b.str(0x286, "Old beat");
    b.end();
}

/// A track as Bitwig wrote it before version 6: its timeline in 0x15d → 0x238, its launcher in 0x15e.
fn old_track(b: &mut BwWriter, name: &str, timeline: &[(f64, f64)], launcher: &[f64]) {
    b.begin(0x285);
    b.str(TRACK_NAME, name);
    b.obj(0x15d);
    b.begin(0x510);
    b.obj(0x238);
    b.begin(0x511);
    b.list(ITEMS);
    for &(start, length) in timeline {
        old_note_clip(b, start, length);
    }
    b.end_list();
    b.end();
    b.end();
    b.obj(0x15e);
    b.begin(0x512);
    b.list(SLOTS);
    for &length in launcher {
        b.begin(0x513);
        b.obj(0x240);
        old_note_clip(b, 0.0, length);
        b.end();
    }
    b.end_list();
    b.end();
    b.list(GROUP_TRACKS);
    b.end_list();
    b.end();
}

#[test]
fn bitwig_projects_from_before_version_6_show_their_clips() {
    // Solomon's saves from Bitwig 2 to 5.2 all hold their clips this way (they used to show as empty).
    let a = read("Old.bwproject", &bitwig_project(|b| old_track(b, "Beat", &[(8.0, 8.0)], &[])));
    assert!(a.notes.is_empty(), "{:?}", a.notes);
    assert_eq!(
        a.lanes[0].clips.iter().map(|c| (c.name.as_str(), c.start, c.length)).collect::<Vec<_>>(),
        [("Old beat", 8.0, 8.0)]
    );
    // The loop plays twice in the 8-beat clip.
    assert_eq!(notes(&a, 0, 0), [(0.0, 0.5, 36), (3.0, 2.0, 36), (4.0, 0.5, 36), (7.0, 1.0, 36)]);

    // Each launcher clip loops 4 beats, so each scene is 4 beats long.
    let jam = &views("Old jam.bwproject", &bitwig_project(|b| old_track(b, "Keys", &[], &[8.0, 4.0])))[0];
    assert_eq!(jam.view, "Bitwig scenes");
    assert_eq!(jam.lanes[0].clips.iter().map(|c| (c.start, c.length)).collect::<Vec<_>>(), [(0.0, 4.0), (4.0, 4.0)]);
}

/// An automation lane as Bitwig 6.1 saves it: what it moves, and the points drawn on it (a value, or yes/no).
fn automation_lane(b: &mut BwWriter, device: &str, setting: &str, points: &[(f64, f64)], on_off: bool) {
    b.begin(0x23d);
    b.obj(0x2a3);
    b.begin(0xf1);
    b.str(0x133b, device);
    b.str(0x133a, setting);
    b.end();
    b.obj(0x2d8c);
    b.begin(0xd91);
    b.list(ITEMS);
    b.end_list();
    b.obj(0x35ac);
    b.begin(0x6e);
    b.list(ITEMS);
    for &(beat, value) in points {
        b.begin(if on_off { 0x7d } else { 0x108 });
        b.f64(0x2af, beat);
        if on_off {
            b.bool(0x12e, value != 0.0);
        } else {
            b.f64(0x28f, value);
        }
        b.end();
    }
    b.end_list();
    b.end();
    b.end();
    b.end();
}

/// A track with nothing on it but its on switch, its mute and its automation lanes.
fn switched_track(b: &mut BwWriter, name: &str, active: bool, muted: bool, lanes: impl FnOnce(&mut BwWriter)) {
    b.begin(0x285);
    b.str(TRACK_NAME, name);
    b.bool(0x1380, active);
    b.obj(0x165);
    b.begin(0xab);
    b.obj(0x1a6);
    b.begin(0x4ce);
    b.bool(0xd2, muted);
    b.end();
    b.end();
    b.obj(0x2dc9);
    b.begin(0xfa2);
    b.list(0x4ba);
    lanes(b);
    b.end_list();
    b.end();
    b.end();
}

#[test]
fn bitwig_deactivated_tracks_time_signature_markers_and_automation() {
    // The shapes Solomon's recipe saves showed (Bitwig 6.1.7, 10 Nov 2026).
    let bytes = BwWriter::file(
        |_| {},
        |b| {
            b.begin(0x520);
            // The transport: 3/4 (12 sixteenths to the bar), two cue markers and the tempo's curve.
            b.obj(0x190);
            b.begin(0xd7);
            b.obj(0x212);
            b.begin(0x7f4);
            b.i32(0x1cbe, 0x0C00_0024);
            b.end();
            b.obj(0x2adc);
            b.begin(0x26a);
            b.list(ITEMS);
            for (beat, name) in [(8.0, "Drop"), (2.0, ""), (f64::NAN, "Lost")] {
                b.begin(0x266);
                b.f64(0x2af, beat);
                b.str(0x98b, name);
                b.end();
            }
            b.end_list();
            b.end();
            b.obj(0x2dc9);
            b.begin(0xfa3);
            b.list(0x4ba);
            automation_lane(b, "Transport", "Tempo", &[(0.0, 20.0), (4.0, 343.0), (8.0, 666.0)], false);
            b.end_list();
            b.end();
            b.end();
            b.obj(PROJECT_GROUP);
            b.begin(0x521);
            b.list(GROUP_TRACKS);
            switched_track(b, "Off", false, false, |b| {
                automation_lane(b, "Mixer", "Volume", &[(0.0, 2f64.cbrt()), (4.0, 0.0)], false);
                automation_lane(b, "Mixer", "Pan", &[(0.0, -1.0), (4.0, 0.0), (8.0, 1.0)], false);
            });
            switched_track(b, "Muted", true, true, |b| {
                // Written out of order: an on/off setting holds each answer until the next in time.
                automation_lane(b, "Filter", "On/Off", &[(4.0, 1.0), (0.0, 0.0)], true);
            });
            switched_track(b, "Synth", true, false, |b| {
                // A setting that isn't 0 to 1 is spread over what its lane holds.
                automation_lane(b, "Synth", "Cutoff", &[(0.0, 10.0), (2.0, 30.0), (4.0, 20.0)], false);
                // A setting that never moves has no lane.
                automation_lane(b, "Synth", "Resonance", &[(0.0, 0.5)], false);
            });
            b.end_list();
            b.end();
            b.end();
        },
        None,
    );
    let a = read("Song.bwproject", &bytes);
    assert_eq!(a.beats_per_bar, 3);
    let markers: Vec<_> = a.markers.iter().map(|m| (m.name.as_str(), m.beat)).collect();
    assert_eq!(markers, [("Marker", 2.0), ("Drop", 8.0)]);
    let shown: Vec<_> =
        a.lanes.iter().map(|l| (l.name.as_str(), l.detail.as_str(), l.depth, l.muted, l.disabled)).collect();
    assert_eq!(
        shown,
        [
            ("Tempo", "Song tempo", 0, false, false),
            // A deactivated track is muted too, and so are its curves.
            ("Off", "", 0, true, true),
            ("Volume", "Moves Off", 1, true, false),
            ("Pan", "Moves Off", 1, true, false),
            ("Muted", "", 0, true, false),
            ("On/Off", "Moves Filter on Muted", 1, true, false),
            ("Synth", "", 0, false, false),
            ("Cutoff", "Moves Synth", 1, false, false),
        ]
    );
    let points = |lane: usize| -> Vec<(f64, f64)> {
        let clip = &a.lanes[lane].clips[0];
        clip.points.iter().map(|p| (clip.start + p.beat, p.value)).collect()
    };
    // The tempo over Bitwig's 20 to 666 BPM; the volume up to the fader's top; the pan from left to right.
    assert_eq!(points(0), [(0.0, 0.0), (4.0, 0.5), (8.0, 1.0)]);
    assert_eq!(points(2), [(0.0, 1.0), (4.0, 0.0)]);
    assert_eq!(points(3), [(0.0, 0.0), (4.0, 0.5), (8.0, 1.0)]);
    assert_eq!(points(5), [(0.0, 0.0), (4.0, 0.0), (4.0, 1.0)]);
    assert_eq!(points(7), [(0.0, 0.0), (2.0, 1.0), (4.0, 0.5)]);
    // The curves aren't tracks: a project with nothing else on its timeline still says so.
    assert_eq!(a.notes, ["Nothing is on the arrangement timeline or in the clip launcher yet."]);
}

#[test]
fn the_bitwig_template_shows_its_audio_and_note_clips_and_what_plays_each_track() {
    let a = read("template.bwproject", &bitwig_template(Template::default()));
    assert_eq!(
        a.lanes.iter().map(|l| (l.name.as_str(), l.kind, l.detail.as_str())).collect::<Vec<_>>(),
        [
            ("Kick", LaneKind::Instrument, "Sampler · Kick"),
            // The template's automation: each curve under its track.
            ("Volume", LaneKind::Automation, "Moves Kick"),
            ("Drops", LaneKind::Audio, ""),
            ("Serum 2", LaneKind::Instrument, "Serum 2"),
            ("Drumazon", LaneKind::Instrument, "Drumazon"),
            ("Bass Drum Level", LaneKind::Automation, "Moves Drumazon"),
        ]
    );
    let drops = &a.lanes[2].clips[0];
    assert_eq!((drops.name.as_str(), drops.start, drops.length), ("Drops", 0.0, 64.0));
    let part = &drops.audio[0];
    assert_eq!((part.start, part.length, part.from, part.seconds_per_beat), (0.0, 64.0, 0.0, Some(0.5)));
    assert_eq!(a.lanes[3].clips[0].notes.len(), 3);
    // Each is an automation clip with one point.
    let curve = &a.lanes[1].clips[0];
    assert_eq!((curve.start, curve.length, curve.points.len()), (0.0, 4.0, 1));

    for bad in [bitwig_undecodable_bytes("Undecodable"), b"not bitwig".to_vec()] {
        let bad: Vec<u8> = bad;
        assert_eq!(read("x.bwproject", &bad).notes, ["This project couldn't be read in enough detail to draw."]);
    }
}

/// UBJSON as Studio One writes a part's notes: {"timeFormat": 1, "events": [{start, length, pitch, velocity}...]}.
fn musicx(notes: &[(f64, f64, i8, f64)]) -> Vec<u8> {
    let key = |b: &mut Vec<u8>, k: &str| {
        b.extend([b'i', k.len() as u8]);
        b.extend(k.as_bytes());
    };
    let mut b = vec![b'{'];
    key(&mut b, "timeFormat");
    b.extend([b'i', 1]);
    key(&mut b, "events");
    b.push(b'[');
    for (start, length, pitch, vel) in notes {
        b.push(b'{');
        for (k, v) in [("start", start), ("length", length), ("velocity", vel)] {
            key(&mut b, k);
            b.push(b'D');
            b.extend(v.to_be_bytes());
        }
        key(&mut b, "pitch");
        b.extend([b'i', *pitch as u8]);
        b.push(b'}');
    }
    b.extend(b"]}");
    b
}

#[test]
fn studio_one_tracks_folders_parts_patterns_audio_and_markers() {
    let song = r#"<?xml version="1.0" encoding="UTF-8"?>
<Song>
  <Attributes x:id="tempoMap"><TempoMapSegment tempo="0.5"/></Attributes>
  <TimeSignatureMapSegment numerator="6" denominator="8"/>
  <List x:id="Tracks">
    <FolderTrack name="Beats" color="FF102030">
      <List x:id="Tracks">
        <MediaTrack name="Drums" mediaType="Music" mute="1">
          <List x:id="Events">
            <MusicPatternPart name="" start="4" length="2" clipID="{P1}"/>
          </List>
        </MediaTrack>
      </List>
    </FolderTrack>
    <MediaTrack name="Keys" mediaType="Music">
      <List x:id="Events">
        <MusicPart name="Chords" start="8" length="2" offset="1" transpose="12" clipID="{M1}"/>
        <MusicPart start="0" length="1">
          <NoteEvent start="0" length="0.5" pitch="60" velocity="100" mute="1"/>
        </MusicPart>
      </List>
    </MediaTrack>
    <MediaTrack name="" mediaType="Audio">
      <List x:id="Events">
        <AudioEvent name="" start="2" length="0" offset="1" speed="2" clipID="{A1}"/>
        <AudioEvent name="Shout" start="12" length="3" offset="0.25" timeFormat="0" clipID="{a1}"/>
      </List>
    </MediaTrack>
    <BusTrack name="Not a track"/>
  </List>
  <List x:id="Markers">
    <MarkerEvent name="Start" start="0" markerType="2"/>
    <MarkerEvent name="Drop" start="16"/>
    <MarkerEvent name="Intro" start="1"/>
  </List>
</Song>"#;
    let pool = r#"<?xml version="1.0" encoding="UTF-8"?>
<MediaPool>
  <AudioClip mediaID="{A1}"><Url url="file:///C:/Samples/Vox%20Chop.wav"/><Attributes x:id="format" frameCount="88200" sampleRate="44100"/></AudioClip>
  <MusicClip mediaID="{M1}"><Url url="media:///Performances/Chords%201.musicx"/></MusicClip>
  <MusicClip mediaID="{P1}"><Url url="media:///Performances/Beat.pattern"/></MusicClip>
</MediaPool>"#;
    let pattern = r#"<?xml version="1.0" encoding="UTF-8"?>
<Pattern><PatternPerformance stepLength="0.5" stepCount="2"><PatternStep start="0" pitch="36" velocity="1"/></PatternPerformance></Pattern>"#;
    let chords = musicx(&[(0.0, 1.0, 60, 0.5), (1.0, 4.0, 64, 1.0), (2.5, 1.0, 67, 1.0), (3.0, 1.0, 71, 1.0)]);
    let bytes = stored_zip(&[
        ("Song/song.xml", song.as_bytes()),
        ("Song/mediapool.xml", pool.as_bytes()),
        ("Performances/Chords 1.musicx", &chords),
        ("Performances/Beat.pattern", pattern.as_bytes()),
    ]);
    let a = read("Song.song", &bytes);
    assert_eq!((a.view, a.tempo, a.beats_per_bar), ("Studio One arrangement", 120.0, 3));
    assert!(a.notes.is_empty(), "{:?}", a.notes);
    assert_eq!(
        lanes(&a),
        [
            ("Beats", LaneKind::Group, 0),
            ("Drums", LaneKind::Instrument, 1),
            ("Keys", LaneKind::Instrument, 0),
            ("Audio", LaneKind::Audio, 0)
        ]
    );
    assert_eq!(a.lanes[0].colour, Some(0x10_2030));
    assert!(a.lanes[1].muted);
    // The pattern's one step repeats every beat for the part's two beats.
    assert_eq!(notes(&a, 1, 0), [(0.0, 0.5, 36), (1.0, 0.5, 36)]);
    // Clips in time order; the part starts a beat into its notes, an octave up, cut at its end.
    let keys = &a.lanes[2];
    assert_eq!(
        keys.clips.iter().map(|c| (c.name.as_str(), c.start)).collect::<Vec<_>>(),
        [("Keys", 0.0), ("Chords", 8.0)]
    );
    assert_eq!(notes(&a, 2, 1), [(0.0, 2.0, 76), (1.5, 0.5, 79)]);
    assert!(keys.clips[0].notes[0].muted);
    assert_eq!(velocity::to_7(keys.clips[0].notes[0].velocity), 100);
    // No length saved: the file's 2 seconds at double speed is half a beat... at 120, a second is 2 beats.
    let audio = &a.lanes[3].clips;
    assert_eq!((audio[0].name.as_str(), audio[0].start, audio[0].length), ("Vox Chop", 2.0, 2.0));
    assert_eq!(audio[0].audio[0].path, r"C:\Samples\Vox Chop.wav");
    assert_eq!((audio[0].audio[0].from, audio[0].audio[0].seconds_per_beat), (1.0, Some(1.0)));
    // Timed in seconds: 3 seconds is 6 beats, and it starts a quarter second into the file.
    assert_eq!((audio[1].name.as_str(), audio[1].length, audio[1].audio[0].from), ("Shout", 6.0, 0.25));
    assert_eq!(
        a.markers,
        [PreviewMarker { beat: 1.0, name: "Intro".into() }, PreviewMarker { beat: 16.0, name: "Drop".into() }]
    );

    let groove = read("Groove.song", &studio_one_song_bytes());
    assert_eq!(
        (groove.tempo, groove.notes.as_slice()),
        (92.0, ["Nothing is on this song's timeline yet.".to_owned()].as_slice())
    );
    assert_eq!(read("x.song", b"not a zip").notes, ["This song couldn't be read in enough detail to draw."]);
}

/// UBJSON as Studio One writes an automation curve: {"stepcount": 2, "events": [{start, value}...]} (a point at the
/// song's start has no start; only a curve that jumps has a stepcount).
fn envelopex(steps: bool, points: &[(f64, f64)]) -> Vec<u8> {
    let key = |b: &mut Vec<u8>, k: &str| {
        b.extend([b'i', k.len() as u8]);
        b.extend(k.as_bytes());
    };
    let mut b = vec![b'{'];
    if steps {
        key(&mut b, "stepcount");
        b.extend([b'i', 2]);
    }
    key(&mut b, "events");
    b.push(b'[');
    for (start, value) in points {
        b.push(b'{');
        for (k, v) in [("start", start), ("value", value)] {
            if k == "start" && *v == 0.0 {
                continue;
            }
            key(&mut b, k);
            b.push(b'D');
            b.extend(v.to_be_bytes());
        }
        b.push(b'}');
    }
    b.extend(b"]}");
    b
}

#[test]
fn studio_one_stretch_layers_audio_parts_automation_and_channels() {
    // The shapes Solomon's own songs showed (10 Nov 2026), at 120 BPM: half a second to a beat.
    let song = r#"<?xml version="1.0" encoding="UTF-8"?>
<Song>
  <TempoMap x:id="tempoMap">
    <Attributes x:id="envelope"><EnvelopePoint value="0.25"/><EnvelopePoint start="8" value="0.5"/></Attributes>
    <TempoMapSegment tempo="0.5"/>
  </TempoMap>
  <List x:id="Tracks">
    <MediaTrack name="Loop" mediaType="Audio" tempoFollow="2" materialMode="1" timeFormat="2">
      <UID x:id="channelID" uid="{CH-LOOP}"/>
      <Attributes x:id="AutomationRegionList">
        <AutomationRegion title="Volume" color="FF112233"><Url x:id="points" url="media:///Envelopes/Loop/Volume.envelopex"/></AutomationRegion>
        <AutomationRegion title="Mute" mute="1"><Url x:id="points" url="media:///Envelopes/Loop/Mute.envelopex"/></AutomationRegion>
        <AutomationRegion title="Pan"><Url x:id="points" url="media:///Envelopes/Loop/Pan.envelopex"/></AutomationRegion>
        <AutomationRegion title="Gone"><Url x:id="points" url="media:///Envelopes/Loop/Gone.envelopex"/></AutomationRegion>
      </Attributes>
      <List x:id="Events">
        <AudioEvent name="Stretched" start="4" length="8" offset="2" timeFormat="2" speed="1" transpose="-2" tune="50" clipID="{LOOP}">
          <Attributes x:id="volumeCurve" fadeIn.length="0.25" fadeOut.length="100" level="0.5"/>
        </AudioEvent>
        <AudioEvent name="No tempo" start="16" length="4" timeFormat="2" clipID="{HIT}">
          <Attributes x:id="volumeCurve" level="0"/>
        </AudioEvent>
      </List>
    </MediaTrack>
    <MediaTrack name="Tape" mediaType="Audio" tempoFollow="2" timeFormat="2">
      <UID x:id="stretchMethod" uid="{b442aefc-7b3c-497b-a22e-630e00b1b908}"/>
      <UID x:id="channelID" uid="{ch-tape}"/>
      <List x:id="Events"><AudioEvent start="0" length="1" timeFormat="2" clipID="{LOOP}"/></List>
    </MediaTrack>
    <MediaTrack name="Mix" mediaType="Audio" tempoFollow="0" timeFormat="0" activeLayer="1">
      <List x:id="Layers">
        <Attributes id="0"><List x:id="Events"><AudioEvent name="Old take" start="0" length="1" timeFormat="0" clipID="{HIT}"/></List></Attributes>
        <Attributes id="1">
          <List x:id="Events">
            <AudioEvent name="Set" start="10" length="30" offset="5" timeFormat="0" clipID="{HIT}"/>
            <AudioPartEvent name="Edits" start="60" length="4" offset="1" timeFormat="0" clipID="{PART}"/>
            <AudioPartEvent name="Lost" start="90" length="4" timeFormat="0" clipID="{NOWHERE}"/>
          </List>
        </Attributes>
      </List>
    </MediaTrack>
    <MediaTrack name="Keys" mediaType="Music" timeFormat="2"><UID x:id="channelID" uid="{CH-KEYS}"/></MediaTrack>
    <MediaTrack name="Kontakt 5" mediaType="Music" timeFormat="2"><UID x:id="channelID" uid="{CH-KONTAKT}"/></MediaTrack>
    <AutomationTrack name="Drum bus" timeFormat="2">
      <Attributes x:id="AutomationRegionList">
        <AutomationRegion title="Volume"><Url x:id="points" url="media:///Envelopes/Drum%20bus/Volume.envelope"/></AutomationRegion>
      </Attributes>
    </AutomationTrack>
    <MediaTrack name="Bass" mediaType="Music" timeFormat="2"><UID x:id="channelID" uid="{CH-BASS}"/></MediaTrack>
  </List>
</Song>"#;
    let pool = r#"<?xml version="1.0" encoding="UTF-8"?>
<MediaPool>
  <AudioClip mediaID="{loop}"><Url url="file:///C:/Samples/Loop%2090.wav"/>
    <AudioTempoMap x:id="tempoMap"><AudioTempoMapSegment offset="0" tempo="0.75"/></AudioTempoMap>
    <Attributes x:id="format" frameCount="264600" sampleRate="44100"/></AudioClip>
  <AudioClip mediaID="{HIT}"><Url url="file:///C:/Samples/Hit.wav"/></AudioClip>
  <AudioPartClip mediaID="{PART}" eventFormat="2">
    <Attributes x:id="Events" timeFormat="0" length="100">
      <AudioEvent start="0" length="1.5" offset="7" timeFormat="0" clipID="{HIT}"/>
      <AudioEvent start="2" length="8" offset="3" timeFormat="2" clipID="{LOOP}"/>
      <AudioEvent start="1.5" length="0.25" timeFormat="0" mute="1" clipID="{HIT}"/>
      <AudioEvent start="40" length="1" timeFormat="0" clipID="{HIT}"/>
    </Attributes>
  </AudioPartClip>
</MediaPool>"#;
    let mixer = r#"<?xml version="1.0" encoding="UTF-8"?>
<AudioMixer>
  <AudioOutputChannel label="Main"><UID x:id="uniqueID" uid="{MAIN}"/></AudioOutputChannel>
  <AudioTrackChannel label="Loop" disabled="1"><UID x:id="uniqueID" uid="{ch-loop}"/>
    <Connection x:id="destination" objectID="{BUS}/Input" friendlyName="Drum bus"/>
    <Attributes x:id="Inserts"><Attributes name="FX01" disabled="1"/></Attributes></AudioTrackChannel>
  <AudioTrackChannel label="Tape"><UID x:id="uniqueID" uid="{CH-TAPE}"/>
    <Connection x:id="destination" objectID="{main}/Input" friendlyName="Main"/></AudioTrackChannel>
  <AudioSynthChannel label="Kt. st. 1"><UID x:id="uniqueID" uid="{SYNTH-OUT}"/>
    <Connection x:id="destination" objectID="{BUS}/Input" friendlyName="Drum bus"/></AudioSynthChannel>
  <AudioGroupChannel label="Drum bus"><UID x:id="uniqueID" uid="{BUS}"/>
    <Connection x:id="destination" objectID="{MAIN}/Input" friendlyName="Main"/></AudioGroupChannel>
</AudioMixer>"#;
    let tracks = r#"<?xml version="1.0" encoding="UTF-8"?>
<MusicTrackDevice>
  <MusicTrackChannel label="Keys"><UID x:id="uniqueID" uid="{CH-KEYS}"/>
    <Connection x:id="instrumentOut" objectID="{synth-out}/Input" friendlyName="Kt. st. 1"/>
    <Connection x:id="destination" objectID="{K}/MusicInput00" friendlyName="2 - Kontakt 5/CH 1"/></MusicTrackChannel>
  <MusicTrackChannel label="Kontakt 5"><UID x:id="uniqueID" uid="{CH-KONTAKT}"/>
    <Connection x:id="destination" objectID="{K}/MusicInput00" friendlyName="Kontakt 5/Synth Input"/></MusicTrackChannel>
  <MusicTrackChannel label="Bass"><UID x:id="uniqueID" uid="{CH-BASS}"/>
    <Connection x:id="destination" objectID="{GONE}/MusicInput00" friendlyName="3 - 808 - Sub/CH 1"/></MusicTrackChannel>
</MusicTrackDevice>"#;
    let synths = r#"<?xml version="1.0" encoding="UTF-8"?>
<AudioSynthFolder>
  <Attributes><UID x:id="uniqueID" uid="{OUTER}"/>
    <Attributes x:id="deviceData" name="Kontakt 5"><UID x:id="uniqueID" uid="{k}"/></Attributes></Attributes>
</AudioSynthFolder>"#;
    let old_envelope = r#"<?xml version="1.0" encoding="UTF-8"?>
<Envelope><EnvelopePoint start="16" value="0.25"/><EnvelopePoint start="8" value="2"/><EnvelopePoint start="9" value="NaN"/>
<EnvelopeAnchor start="2400000" value="0"/></Envelope>"#;
    let bytes = stored_zip(&[
        ("Song/song.xml", song.as_bytes()),
        ("Song/mediapool.xml", pool.as_bytes()),
        ("Devices/audiomixer.xml", mixer.as_bytes()),
        ("Devices/musictrackdevice.xml", tracks.as_bytes()),
        ("Devices/audiosynthfolder.xml", synths.as_bytes()),
        ("Envelopes/Loop/Volume.envelopex", &envelopex(false, &[(0.0, 0.3), (4.0, 0.6)])),
        ("Envelopes/Loop/Mute.envelopex", &envelopex(true, &[(2.0, 0.0), (6.0, 1.0), (7.0, 0.0)])),
        ("Envelopes/Loop/Pan.envelopex", &envelopex(false, &[(0.0, 0.5)])),
        ("Envelopes/Drum bus/Volume.envelope", old_envelope.as_bytes()),
    ]);
    let a = read("Song.song", &bytes);
    assert!(a.notes.is_empty(), "{:?}", a.notes);
    let shown: Vec<_> = a.lanes.iter().map(|l| (l.name.as_str(), l.kind, l.detail.as_str(), l.depth)).collect();
    assert_eq!(
        shown,
        [
            ("Tempo", LaneKind::Automation, "Song tempo", 0),
            ("Loop", LaneKind::Audio, "To Drum bus", 0),
            ("Volume", LaneKind::Automation, "Moves Loop", 1),
            ("Mute", LaneKind::Automation, "Moves Loop", 1),
            // A pan that never moves, and a curve whose file is gone, have no lane.
            ("Tape", LaneKind::Audio, "", 0),
            ("Mix", LaneKind::Audio, "", 0),
            // The instrument's own name, found by its id (its input is called "2 - Kontakt 5/CH 1").
            ("Keys", LaneKind::Instrument, "Kontakt 5, to Drum bus", 0),
            // Named after its instrument, so that isn't said twice.
            ("Kontakt 5", LaneKind::Instrument, "", 0),
            ("Volume", LaneKind::Automation, "Moves Drum bus", 1),
            // An instrument whose name isn't with its id goes by its input's name, without Studio One's number.
            ("Bass", LaneKind::Instrument, "808 - Sub", 0),
        ]
    );
    let points = |lane: usize| -> Vec<(f64, f64)> {
        let clip = &a.lanes[lane].clips[0];
        clip.points.iter().map(|p| (clip.start + p.beat, p.value)).collect()
    };
    assert_eq!(points(0), [(0.0, 0.25), (8.0, 0.5)]);
    assert_eq!(points(2), [(0.0, 0.3), (4.0, 0.6)]);
    assert_eq!(a.lanes[2].colour, Some(0x11_2233));
    // A curve that jumps holds each value until the next; a muted curve is a muted lane.
    assert_eq!(points(3), [(2.0, 0.0), (6.0, 0.0), (6.0, 1.0), (7.0, 1.0), (7.0, 0.0)]);
    assert!(a.lanes[3].muted);
    // The old XML curve: in time order, values kept to 0..1, a point that isn't a number and the anchor left out.
    assert_eq!(points(8), [(8.0, 1.0), (16.0, 0.25)]);

    // The disabled track is muted too (its disabled insert is not the track's own switch).
    assert!(a.lanes[1].disabled && a.lanes[1].muted);
    assert!(a.lanes.iter().skip(2).all(|l| !l.disabled));
    // And so are its curves: they move nothing.
    assert!(a.lanes[2].muted);

    // Stretched: the file's own beat is 0.75 s, so each of the song's beats plays 0.75 s of it. Two beats in is 1.5 s.
    let loops = &a.lanes[1].clips;
    let part = &loops[0].audio[0];
    assert_eq!((loops[0].start, loops[0].length, part.from, part.seconds_per_beat), (4.0, 8.0, 1.5, Some(0.75)));
    assert_eq!(part.stretch, Some(Stretch::ElastiqueSolo));
    // Down two semitones and up 50 cents; half as loud is -6 dB.
    assert!((part.semitones + 1.5).abs() < 1e-9 && (part.gain_db + 6.0206).abs() < 1e-3);
    // The fades are in seconds: a quarter second is half a beat, and the fade out can't be longer than what's left.
    assert_eq!((loops[0].fade_in, loops[0].fade_out), (0.5, 7.5));
    // A file whose tempo Studio One doesn't know plays at the song's; a gain of nothing is silence.
    let part = &loops[1].audio[0];
    assert_eq!((part.seconds_per_beat, part.gain_db), (Some(0.5), f64::NEG_INFINITY));
    assert_eq!(a.lanes[4].clips[0].audio[0].stretch, Some(Stretch::Repitch));

    // A track that doesn't follow the tempo counts in seconds: 10 s is beat 20. Its active layer is the second.
    let mix = &a.lanes[5].clips;
    assert_eq!(mix.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Set", "Edits"]);
    assert_eq!((mix[0].start, mix[0].length, mix[0].audio[0].from), (20.0, 60.0, 5.0));
    assert_eq!(mix[0].audio[0].stretch, Some(Stretch::Raw));
    // The audio part shows its events from a second (2 beats) in, for 4 seconds (8 beats).
    assert_eq!((mix[1].start, mix[1].length), (120.0, 8.0));
    let parts: Vec<_> = mix[1].audio.iter().map(|p| (p.path.as_str(), p.start, p.length, p.from)).collect();
    assert_eq!(
        parts,
        [
            // Starts before what's shown: its first second is cut off, so it plays from 8 s into the file.
            (r"C:\Samples\Hit.wav", 0.0, 1.0, 8.0),
            // Starts at 2 s (a second into what's shown) and is cut at the part's end: 3 beats in at 0.75 s a beat.
            (r"C:\Samples\Loop 90.wav", 2.0, 6.0, 2.25),
        ]
    );
}

#[test]
fn studio_one_seconds_follow_the_tempo_changes() {
    // 120 BPM for 8 beats (4 seconds), then 60 BPM: what's counted in seconds lands by the tempo at that place.
    let song = r#"<?xml version="1.0" encoding="UTF-8"?>
<Song>
  <TempoMap x:id="tempoMap"><TempoMapSegment start="8" tempo="1"/><TempoMapSegment start="0" tempo="0.5"/></TempoMap>
  <List x:id="Tracks">
    <MediaTrack name="Mix" mediaType="Audio" tempoFollow="0" timeFormat="0" activeLayer="1" mute="1">
      <Attributes x:id="AutomationRegionList">
        <AutomationRegion title="Mute"><Url x:id="points" url="media:///Envelopes/Mix/Mute.envelopex"/></AutomationRegion>
      </Attributes>
      <List x:id="Events"/>
      <List x:id="Layers">
        <Attributes id="1">
          <List x:id="Events">
            <AudioEvent name="Across" start="3" length="2" offset="1.5" timeFormat="0" clipID="{HIT}">
              <Attributes x:id="volumeCurve" fadeIn.length="0.5" fadeOut.length="0.5"/>
            </AudioEvent>
            <AudioEvent name="After" start="6" length="3" timeFormat="0" clipID="{HIT}"/>
            <AudioPartEvent name="Edits" start="10" timeFormat="0" clipID="{PART}"/>
          </List>
        </Attributes>
      </List>
    </MediaTrack>
    <AutomationTrack timeFormat="2">
      <Attributes x:id="AutomationRegionList">
        <AutomationRegion title="Mute"><Url x:id="points" url="media:///Envelopes/Mix/Mute.envelopex"/></AutomationRegion>
      </Attributes>
    </AutomationTrack>
  </List>
  <MarkerEvent name="Drop" start="5" timeFormat="0"/>
</Song>"#;
    let pool = r#"<?xml version="1.0" encoding="UTF-8"?>
<MediaPool>
  <AudioClip mediaID="{HIT}"><Url url="file:///C:/Samples/Hit.wav"/></AudioClip>
  <AudioPartClip mediaID="{PART}">
    <Attributes x:id="Events" timeFormat="0">
      <AudioEvent start="0" length="1" timeFormat="0" clipID="{HIT}"/>
      <AudioEvent start="1" length="2" timeFormat="0" clipID="{HIT}"/>
    </Attributes>
  </AudioPartClip>
</MediaPool>"#;
    let bytes = stored_zip(&[
        ("Song/song.xml", song.as_bytes()),
        ("Song/mediapool.xml", pool.as_bytes()),
        // Written out of order: a curve that jumps still holds each value until the next in time.
        ("Envelopes/Mix/Mute.envelopex", &envelopex(true, &[(6.0, 1.0), (2.0, 0.0)])),
    ]);
    let a = read("Song.song", &bytes);
    assert!(a.notes.is_empty(), "{:?}", a.notes);
    assert_eq!(a.tempo, 120.0, "the tempo at the song's start, wherever it's written");
    let shown: Vec<_> = a.lanes.iter().map(|l| (l.name.as_str(), l.detail.as_str(), l.muted)).collect();
    // A muted track's curve is muted with it; an automation track with no name still has one to show.
    assert_eq!(shown, [("Mix", "", true), ("Mute", "Moves Mix", true), ("Mute", "Moves Automation", false)]);

    // The takes are found though the track has an (empty) list of its own.
    let mix = &a.lanes[0].clips;
    assert_eq!(mix.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Across", "After", "Edits"]);
    // 3 s is beat 6; its 2 s are two beats at 120 and one at 60. It plays the file at the tempo it starts in.
    assert_eq!((mix[0].start, mix[0].length), (6.0, 3.0));
    assert_eq!((mix[0].audio[0].from, mix[0].audio[0].seconds_per_beat), (1.5, Some(0.5)));
    // Half a second fades in over a beat at 120, and out over half a beat at 60.
    assert_eq!((mix[0].fade_in, mix[0].fade_out), (1.0, 0.5));
    // 6 s is two seconds past the change: beat 10, and 3 s there are 3 beats.
    assert_eq!((mix[1].start, mix[1].length, mix[1].audio[0].seconds_per_beat), (10.0, 3.0, Some(1.0)));
    // An audio part with no length runs to the end of its last event: 3 s after 10 s.
    assert_eq!((mix[2].start, mix[2].length), (14.0, 3.0));
    let inside: Vec<_> = mix[2].audio.iter().map(|p| (p.start, p.length)).collect();
    assert_eq!(inside, [(0.0, 1.0), (1.0, 2.0)]);
    assert_eq!((a.markers[0].name.as_str(), a.markers[0].beat), ("Drop", 9.0));

    // The curve on the seconds track is in seconds (2 s is beat 4, 6 s beat 10); the other is in beats. Both came
    // from one file.
    let points = |lane: usize| -> Vec<(f64, f64)> {
        let clip = &a.lanes[lane].clips[0];
        clip.points.iter().map(|p| (clip.start + p.beat, p.value)).collect()
    };
    assert_eq!(points(1), [(4.0, 0.0), (10.0, 0.0), (10.0, 1.0)]);
    assert_eq!(points(2), [(2.0, 0.0), (6.0, 0.0), (6.0, 1.0)]);
}

#[test]
fn a_damaged_song_cant_make_its_audio_parts_look_at_one_clip_for_ever() {
    // 1,500 audio parts, each on the same clip of 1,500 events: 2.25 million looks, past the 2 million the preview makes.
    let events: String =
        (0..1500).map(|i| format!(r#"<AudioEvent start="{i}" length="1" clipID="{{HIT}}"/>"#)).collect();
    let parts: String = (0..1500)
        .map(|i| format!(r#"<AudioPartEvent start="{i}" length="1" offset="{i}" clipID="{{PART}}"/>"#))
        .collect();
    let song = format!(
        r#"<Song><List x:id="Tracks"><MediaTrack name="Mix" mediaType="Audio"><List x:id="Events">{parts}</List></MediaTrack></List></Song>"#
    );
    let pool = format!(
        r#"<MediaPool><AudioClip mediaID="{{HIT}}"><Url url="file:///C:/Hit.wav"/></AudioClip>
<AudioPartClip mediaID="{{PART}}"><Attributes x:id="Events" timeFormat="2">{events}</Attributes></AudioPartClip></MediaPool>"#
    );
    let a =
        read("Parts.song", &stored_zip(&[("Song/song.xml", song.as_bytes()), ("Song/mediapool.xml", pool.as_bytes())]));
    assert_eq!(
        a.notes,
        [
            "This song's audio parts hold more events than the preview looks at (2 million), so the last parts are left out."
        ]
    );
    // 1,333 parts fit; each shows the one event that plays in its beat.
    let clips = &a.lanes[0].clips;
    assert_eq!(clips.len(), 1333);
    assert!(clips.iter().all(|c| c.audio.len() == 1 && c.audio[0].start == 0.0 && c.audio[0].length == 1.0));
}

#[test]
fn a_damaged_project_cant_loop_a_clip_for_ever() {
    // A clip that goes on for ever isn't drawn; a tiny loop in a long clip stops at the notes the preview holds.
    let bytes = bitwig_project(|b| {
        track(
            b,
            "Beat",
            None,
            |b| {
                b.obj(ARRANGER);
                b.begin(0x511);
                b.list(ITEMS);
                note_clip(b, 0.0, f64::INFINITY, true);
                note_clip_looping(b, 0.0, 1e12, true, 1e-6);
                b.end_list();
                b.end();
            },
            |_| {},
        )
    });
    let a = read("Damaged.bwproject", &bytes);
    assert_eq!(a.clip_count(), 1);
    assert_eq!(a.lanes[0].clips[0].notes.len(), arrangement::MOST_NOTES);
    assert_eq!(a.notes, [arrangement::TOO_MANY_NOTES]);

    let song = r#"<Song><List x:id="Tracks"><MediaTrack name="Drums" mediaType="Music"><List x:id="Events">
        <MusicPatternPart start="0" length="1e300" clipID="{P1}"/>
        <MusicPatternPart start="0" length="1e400" clipID="{P1}"/>
    </List></MediaTrack></List></Song>"#;
    let pool = r#"<MediaPool><MusicClip mediaID="{P1}"><Url url="media:///Beat.pattern"/></MusicClip></MediaPool>"#;
    let pattern = r#"<Pattern><PatternPerformance stepLength="0.25" stepCount="1"><PatternStep start="0" pitch="36"/></PatternPerformance></Pattern>"#;
    let bytes = stored_zip(&[
        ("Song/song.xml", song.as_bytes()),
        ("Song/mediapool.xml", pool.as_bytes()),
        ("Beat.pattern", pattern.as_bytes()),
    ]);
    let a = read("Damaged.song", &bytes);
    assert_eq!(a.lanes[0].clips.iter().map(|c| c.notes.len()).collect::<Vec<_>>(), [arrangement::MOST_NOTES, 0]);
    assert!(a.notes.contains(&arrangement::TOO_MANY_NOTES.to_owned()));
}

#[test]
fn a_damaged_songs_huge_transpose_or_steps_past_the_part_end_quickly() {
    // "1e10" saturated to i32::MAX and overflowed on the add; steps past the part with a tiny loop never ended.
    let song = r#"<Song><List x:id="Tracks"><MediaTrack name="Keys" mediaType="Music"><List x:id="Events">
        <MusicPart start="0" length="4" transpose="1e10"><NoteEvent start="0" length="1" pitch="60"/></MusicPart>
        <MusicPart start="4" length="4" transpose="-Infinity"><NoteEvent start="0" length="1" pitch="60"/></MusicPart>
        <MusicPatternPart start="8" length="16" clipID="{P1}"/>
    </List></MediaTrack></List></Song>"#;
    let pool = r#"<MediaPool><MusicClip mediaID="{P1}"><Url url="media:///Beat.pattern"/></MusicClip></MediaPool>"#;
    let pattern = r#"<Pattern><PatternPerformance stepLength="0.25" stepCount="1e-12"><PatternStep start="64" pitch="36"/></PatternPerformance></Pattern>"#;
    let bytes = stored_zip(&[
        ("Song/song.xml", song.as_bytes()),
        ("Song/mediapool.xml", pool.as_bytes()),
        ("Beat.pattern", pattern.as_bytes()),
    ]);
    let started = std::time::Instant::now();
    let a = read("Damaged.song", &bytes);
    assert!(started.elapsed().as_secs() < 5, "{:?}", started.elapsed());
    let keys: Vec<Vec<u8>> = a.lanes[0].clips.iter().map(|c| c.notes.iter().map(|n| n.key).collect()).collect();
    assert_eq!(keys, [vec![127], vec![0], vec![]]);
}

#[test]
fn notes_on_muted_tracks_dont_count_as_playing() {
    let song = r#"<Song><List x:id="Tracks">
        <MediaTrack name="On" mediaType="Music"><List x:id="Events"><MusicPart start="0" length="4"><NoteEvent start="0" pitch="60"/></MusicPart></List></MediaTrack>
        <MediaTrack name="Off" mediaType="Music" mute="1"><List x:id="Events"><MusicPart start="0" length="4"><NoteEvent start="0" pitch="60"/><NoteEvent start="1" pitch="62"/></MusicPart></List></MediaTrack>
    </List></Song>"#;
    let a = read("Muted.song", &stored_zip(&[("Song/song.xml", song.as_bytes())]));
    assert_eq!(a.lanes.iter().map(|l| l.muted).collect::<Vec<_>>(), [false, true]);
    assert_eq!(a.note_count(), 1);
}
