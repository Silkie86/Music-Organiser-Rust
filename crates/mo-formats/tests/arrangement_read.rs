//! The arrangement preview of Bitwig and Studio One projects: their tracks and the clips on them, as each DAW shows
//! them. The C# app's `BitwigArrangement` and `StudioOneArrangement` on the same made-up files.

// Test files may unwrap: a test that stops there has failed, which is what it should do.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::Path;

use common::bitwig::*;
use common::studio_one::*;
use mo_formats::arrangement::{self, Arrangement, LaneKind, PreviewMarker};
use mo_formats::velocity;

fn read(name: &str, bytes: &[u8]) -> Arrangement {
    let mut views = arrangement::read_bytes(Path::new(name), bytes);
    assert_eq!(views.len(), 1);
    views.pop().unwrap()
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
    b.begin(CLIP);
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
    b.f64(START, 0.0);
    b.f64(DURATION, 4.0);
    b.end();
    b.end();
    b.end();
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
fn a_bitwig_project_with_only_launcher_clips_shows_its_scenes_one_after_another() {
    let launcher = |lengths: &'static [Option<f64>]| {
        move |b: &mut BwWriter| {
            b.obj(LAUNCHER);
            b.begin(0x512);
            b.list(SLOTS);
            for length in lengths {
                b.begin(0x513);
                if let Some(l) = length {
                    b.obj(0x514);
                    note_clip(b, 0.0, *l, true);
                }
                b.end();
            }
            b.end_list();
            b.end();
        }
    };
    let bytes = bitwig_project(|b| {
        track(b, "Keys", None, launcher(&[Some(8.0), None, Some(4.0)]), |_| {});
        track(b, "Bass", None, launcher(&[Some(4.0), Some(2.0)]), |_| {});
    });
    let a = read("Jam.bwproject", &bytes);
    assert_eq!(a.notes, ["This project's clips are in the clip launcher, so its scenes are shown one after another."]);
    let starts = |lane: usize| a.lanes[lane].clips.iter().map(|c| (c.start, c.length)).collect::<Vec<_>>();
    // Scene 1 is as long as its longest clip (8 beats), scene 2 takes 2, scene 3 takes 4.
    assert_eq!(starts(0), [(0.0, 8.0), (10.0, 4.0)]);
    assert_eq!(starts(1), [(0.0, 4.0), (8.0, 2.0)]);

    let empty = read("Empty.bwproject", &bitwig_project(|b| track(b, "Keys", None, |_| {}, |_| {})));
    assert_eq!(empty.notes, ["Nothing is on the arrangement timeline or in the clip launcher yet."]);
}

#[test]
fn the_bitwig_template_shows_its_audio_and_note_clips_and_what_plays_each_track() {
    let a = read("template.bwproject", &bitwig_template(Template::default()));
    assert_eq!(
        a.lanes.iter().map(|l| (l.name.as_str(), l.kind, l.detail.as_str())).collect::<Vec<_>>(),
        [
            ("Kick", LaneKind::Instrument, "Sampler · Kick"),
            ("Drops", LaneKind::Audio, ""),
            ("Serum 2", LaneKind::Instrument, "Serum 2"),
            ("Drumazon", LaneKind::Instrument, "Drumazon"),
        ]
    );
    let drops = &a.lanes[1].clips[0];
    assert_eq!((drops.name.as_str(), drops.start, drops.length), ("Drops", 0.0, 64.0));
    let part = &drops.audio[0];
    assert_eq!((part.start, part.length, part.from, part.seconds_per_beat), (0.0, 64.0, 0.0, Some(0.5)));
    assert_eq!(a.lanes[2].clips[0].notes.len(), 3);

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
