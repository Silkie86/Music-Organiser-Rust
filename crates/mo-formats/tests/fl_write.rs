//! Writing FL Studio projects: built on FL's own empty project, checked against Solomon's recipe saves (what FL
//! really writes) and read back with the app's own FL reader.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};

use mo_formats::arrangement::{
    self, Arrangement, AudioPart, LaneKind, PreviewClip, PreviewLane, PreviewMarker, PreviewNote, TimeSignature,
};
use mo_formats::fl::{self, events, write};

fn save(daw: &str, name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/recipe").join(daw).join(name)
}

fn view_of(bytes: &[u8]) -> Arrangement {
    arrangement::read_bytes(Path::new("x.flp"), bytes).into_iter().next().unwrap()
}

/// Each event as (id, data), the way two projects are compared.
fn events_of(raw: &[u8]) -> Vec<(u8, Vec<u8>)> {
    events::load(raw).unwrap().events.iter().map(|e| (e.id, e.data.to_vec())).collect()
}

#[test]
fn nothing_of_whoever_saved_the_empty_project_goes_into_a_new_one() {
    let song = view_of(&std::fs::read(save("fl", "MO Recipe 01 empty.flp")).unwrap());
    let written = write::write(&song, "New", 46_000.25);
    let ours = events_of(&written.bytes);
    // No registration text and no project folder; the one date is the new project's.
    assert!(!ours.iter().any(|(id, _)| [200, 202].contains(id)));
    let dates: Vec<_> = ours.iter().filter(|(id, _)| *id == 237).collect();
    assert_eq!(dates.len(), 1);
    assert_eq!(dates[0].1[..8], 46_000.25f64.to_le_bytes());
    assert_eq!(written.bytes[..14], std::fs::read(save("fl", "MO Recipe 01 empty.flp")).unwrap()[..14]);
}

#[test]
fn an_empty_song_writes_fls_empty_project_with_its_tempo_and_title() {
    let song = Arrangement {
        view: "test",
        tempo: 128.0,
        signature: TimeSignature::new(3, 4).unwrap(),
        lanes: Vec::new(),
        markers: Vec::new(),
        notes: Vec::new(),
    };
    let written = write::write(&song, "Night Drive", 46_000.5);
    let recipe = std::fs::read(save("fl", "MO Recipe 01 empty.flp")).unwrap();
    let ours = events_of(&written.bytes);
    let fls: Vec<_> = events_of(&recipe).into_iter().filter(|(id, _)| ![200, 202].contains(id)).collect();
    assert_eq!(ours.len(), fls.len());
    let differ: Vec<u8> = ours.iter().zip(&fls).filter(|(a, b)| a != b).map(|(a, _)| a.0).collect();
    // Tempo, time signature top, title and the date it was made; nothing else.
    assert_eq!(differ, [156, 17, 194, 237]);
    let info = fl::read_bytes(&written.bytes);
    assert_eq!(info.tempo, Some(128.0));
    assert_eq!(info.title, "Night Drive");
    assert!(written.left_out.is_empty());
}

fn note(start: f64, length: f64, key: u8, velocity: u16, muted: bool) -> PreviewNote {
    PreviewNote { start, length, key, velocity, muted, step: false, part: 0 }
}

/// A made-up song: a muted pad track with the same clip twice and a lead with one, a drum loop on an audio track, two
/// markers, at 100 BPM.
fn made_up_song() -> Arrangement {
    let chords = PreviewClip {
        name: "Chords".into(),
        start: 0.0,
        length: 8.0,
        notes: vec![note(0.0, 4.0, 60, 52000, false), note(4.0, 4.0, 65, 30000, true)],
        ..PreviewClip::default()
    };
    let mut pad = PreviewLane::new("Pad", LaneKind::Instrument);
    pad.muted = true;
    pad.clips = vec![chords.clone(), PreviewClip { start: 16.0, ..chords }];
    let mut lead = PreviewLane::new("Lead", LaneKind::Instrument);
    lead.clips = vec![PreviewClip {
        start: 8.0,
        length: 6.0,
        notes: vec![note(0.5, 0.25, 72, 65535, false)],
        muted: true,
        ..PreviewClip::default()
    }];
    let mut drums = PreviewLane::new("Drums", LaneKind::Audio);
    drums.clips = vec![PreviewClip {
        start: 4.0,
        length: 2.0,
        audio: vec![AudioPart {
            path: r"C:\Samples\Loop.wav".into(),
            start: 0.0,
            length: 2.0,
            from: 0.3,
            seconds_per_beat: Some(0.6),
            ..AudioPart::default()
        }],
        fade_in: 0.5,
        ..PreviewClip::default()
    }];
    Arrangement {
        view: "test",
        tempo: 100.0,
        signature: TimeSignature::FOUR_FOUR,
        lanes: vec![pad, lead, drums],
        markers: vec![
            PreviewMarker { beat: 0.0, name: "Intro".into() },
            PreviewMarker { beat: 16.0, name: "Drop".into() },
        ],
        notes: Vec::new(),
    }
}

#[test]
fn a_song_reads_back_as_it_went_in() {
    let written = write::write(&made_up_song(), "Made up", 46_000.0);
    let back = view_of(&written.bytes);
    assert!(back.notes.is_empty(), "{:?}", back.notes);
    assert_eq!(back.tempo, 100.0);
    assert_eq!(
        back.markers.iter().map(|m| (m.beat, m.name.as_str())).collect::<Vec<_>>(),
        [(0.0, "Intro"), (16.0, "Drop")]
    );
    let lanes: Vec<(&str, bool, usize)> =
        back.lanes.iter().map(|l| (l.name.as_str(), l.muted, l.clips.len())).collect();
    assert_eq!(&lanes[..3], [("Pad", true, 2), ("Lead", false, 1), ("Drums", false, 1)]);

    let pad = &back.lanes[0];
    assert_eq!((pad.clips[0].start, pad.clips[0].length, pad.clips[1].start), (0.0, 8.0, 16.0));
    let notes: Vec<(f64, f64, u8, bool)> =
        pad.clips[0].notes.iter().map(|n| (n.start, n.length, n.key, n.muted)).collect();
    assert_eq!(notes, [(0.0, 4.0, 60, false), (4.0, 4.0, 65, true)]);
    // FL keeps velocity 0 to 128.
    assert_eq!(pad.clips[0].notes[0].velocity, mo_formats::velocity::from_unit(102.0 / 128.0));

    let lead = &back.lanes[1].clips[0];
    assert!(lead.muted);
    // Its pattern is one bar: the clip stops there, so FL doesn't play the pattern again in its last two beats.
    assert_eq!((lead.start, lead.length), (8.0, 4.0));

    let drums = &back.lanes[2].clips[0];
    assert_eq!((drums.start, drums.length), (4.0, 2.0));
    assert_eq!(drums.audio[0].path, r"C:\Samples\Loop.wav");
    assert!((drums.audio[0].from - 0.3).abs() < 1e-4);
    assert!((drums.fade_in - 0.5).abs() < 1e-3, "{}", drums.fade_in);
    assert_eq!(written.left_out.len(), 2, "{:?}", written.left_out);
    let info = fl::read_bytes(&written.bytes);
    assert_eq!(info.samples, [r"C:\Samples\Loop.wav"]);
}

#[test]
fn the_same_clip_twice_is_one_pattern() {
    let written = write::write(&made_up_song(), "Made up", 46_000.0);
    let patterns = events_of(&written.bytes).iter().filter(|(id, _)| *id == events::EV_PATTERN_NEW).count();
    assert_eq!(patterns, 2);
}

#[test]
fn fls_own_save_goes_through_and_back() {
    for name in ["MO Recipe 02 notes.flp", "MO Recipe 16 time signature and marker.flp"] {
        let original = arrangement::read(&save("fl", name)).unwrap().into_iter().next().unwrap();
        let back = view_of(&write::write(&original, "Copy", 46_000.0).bytes);
        let notes = |a: &Arrangement| -> Vec<(String, f64, u8)> {
            let mut out: Vec<_> = a
                .lanes
                .iter()
                .flat_map(|l| {
                    l.clips.iter().flat_map(|c| c.notes.iter().map(|n| (l.name.clone(), c.start + n.start, n.key)))
                })
                .collect();
            out.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.2.cmp(&b.2)));
            out
        };
        assert_eq!(notes(&back).len(), notes(&original).len(), "{name}");
        assert_eq!(back.tempo, original.tempo, "{name}");
        assert_eq!(back.markers, original.markers, "{name}");
    }
}

#[test]
fn bitwig_and_studio_one_saves_keep_their_notes_audio_and_markers_in_fl() {
    for (daw, name) in [
        ("bitwig", "MO Recipe 02 notes.bwproject"),
        ("bitwig", "MO Recipe 04 audio clip.bwproject"),
        ("studio-one", "MO Recipe 02 notes.song"),
        ("studio-one", "MO Recipe 04 audio clip.song"),
        ("studio-one", "MO Recipe 12 time signature and marker.song"),
    ] {
        let original = arrangement::read(&save(daw, name)).unwrap().into_iter().next().unwrap();
        let back = view_of(&write::write(&original, "Copy", 46_000.0).bytes);
        assert!(back.notes.is_empty(), "{name}: {:?}", back.notes);
        assert_eq!(back.note_count(), original.note_count(), "{name}");
        let parts = |a: &Arrangement| -> Vec<(String, i64)> {
            let mut out: Vec<_> = a
                .lanes
                .iter()
                .flat_map(|l| {
                    l.clips.iter().flat_map(|c| {
                        c.audio.iter().map(|p| (p.path.clone(), ((c.start + p.start) * 96.0).round() as i64))
                    })
                })
                .collect();
            out.sort();
            out
        };
        assert_eq!(parts(&back), parts(&original), "{name}");
        assert!((back.tempo - original.tempo).abs() < 1e-3, "{name}");
        let beats = |a: &Arrangement| a.markers.iter().map(|m| (m.beat * 96.0).round() as i64).collect::<Vec<_>>();
        assert_eq!(beats(&back), beats(&original), "{name}");
    }
}

#[test]
fn a_pattern_is_whole_bars_of_the_songs_time_signature() {
    let mut song = made_up_song();
    song.signature = TimeSignature::new(3, 4).unwrap();
    let mut lead = PreviewLane::new("Lead", LaneKind::Instrument);
    lead.clips = vec![PreviewClip {
        start: 0.0,
        length: 6.0,
        notes: vec![note(0.0, 1.0, 60, 40000, false)],
        ..PreviewClip::default()
    }];
    song.lanes = vec![lead];
    let back = view_of(&write::write(&song, "Waltz", 46_000.0).bytes);
    let clip = &back.lanes[0].clips[0];
    assert_eq!((clip.start, clip.length), (0.0, 3.0));
}

#[test]
fn a_note_on_a_clips_last_hair_stays_in_the_clip() {
    // Seen on Solomon's songs (real-song test, 27 Nov 2026): a Bitwig note 0.00003 of a beat before its clip's end,
    // and a Studio One one a hair before its clip's start. Rounded to FL's ticks, the first landed on the end, where
    // FL doesn't play it, so the copy lost it and refused to save.
    let clip = PreviewClip {
        name: "Bass".into(),
        start: 128.0,
        length: 64.0,
        notes: vec![
            note(-2.8e-14, 0.5, 39, 40000, false),
            note(32.0, 1.0, 41, 40000, false),
            note(63.999_965_667_724_61, 0.000_034_332_275_390_625, 46, 40000, false),
        ],
        ..PreviewClip::default()
    };
    let mut bass = PreviewLane::new("Bass", LaneKind::Instrument);
    bass.clips = vec![clip];
    let song = Arrangement {
        view: "test",
        tempo: 140.0,
        signature: TimeSignature::FOUR_FOUR,
        lanes: vec![bass],
        markers: Vec::new(),
        notes: Vec::new(),
    };
    let back = view_of(&write::write(&song, "Edge", 46_000.0).bytes);
    assert_eq!(back.note_count(), 3);
    let clip = &back.lanes[0].clips[0];
    let starts: Vec<f64> = clip.notes.iter().map(|n| n.start).collect();
    // On the clip's first tick, in its middle, and on its last tick (64 beats is 6,144 ticks at 96 a beat).
    assert_eq!(starts, [0.0, 32.0, 6143.0 / 96.0]);
    assert!(clip.notes.iter().all(|n| n.start + n.length <= clip.length + 1e-9));
}

#[test]
fn a_clip_that_starts_before_the_song_starts_with_it() {
    // A Studio One part a 20th of a beat before the song's start (seen on Solomon's songs): FL's playlist has nothing
    // before its start, so the whole clip, and every note in it, was left out and the copy refused to save.
    let clip = PreviewClip {
        name: "Diva".into(),
        start: -0.0459,
        length: 8.0459,
        notes: vec![note(0.0, 1.0, 44, 40000, false), note(2.0459, 1.0, 46, 40000, false)],
        ..PreviewClip::default()
    };
    let mut keys = PreviewLane::new("Diva", LaneKind::Instrument);
    keys.clips = vec![clip];
    let mut drums = PreviewLane::new("Loop", LaneKind::Audio);
    drums.clips = vec![PreviewClip {
        start: -1.0,
        length: 5.0,
        audio: vec![AudioPart {
            path: r"C:\Loops\Loop.wav".into(),
            start: 0.0,
            length: 5.0,
            from: 0.5,
            seconds_per_beat: Some(0.5),
            ..AudioPart::default()
        }],
        ..PreviewClip::default()
    }];
    let song = Arrangement {
        view: "test",
        tempo: 120.0,
        signature: TimeSignature::FOUR_FOUR,
        lanes: vec![keys, drums],
        markers: Vec::new(),
        notes: Vec::new(),
    };
    let written = write::write(&song, "Early", 46_000.0);
    assert!(
        written.left_out.iter().any(|l| l.contains("a note before the song's start plays")),
        "{:?}",
        written.left_out
    );
    let back = view_of(&written.bytes);
    assert_eq!(back.note_count(), 2);
    let starts: Vec<f64> =
        back.lanes[0].clips.iter().flat_map(|c| c.notes.iter().map(move |n| c.start + n.start)).collect();
    assert_eq!(starts, [0.0, 2.0]);
    // The audio is cut at the song's start: a beat (half a second) later into the file, a beat shorter.
    let part = &back.lanes[1].clips[0].audio[0];
    assert_eq!(back.lanes[1].clips[0].start, 0.0);
    assert!((part.from - 1.0).abs() < 1e-3 && (part.length - 4.0).abs() < 1e-3, "{part:?}");
}
