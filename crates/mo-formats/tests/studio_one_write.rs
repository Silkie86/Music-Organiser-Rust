//! Writing Studio One songs: built on Studio One's own empty song (the recipe save) and read back with the app's own
//! Studio One reader.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::io::Read;
use std::path::{Path, PathBuf};

use mo_formats::arrangement::{
    self, Arrangement, AudioPart, LaneKind, PreviewClip, PreviewLane, PreviewMarker, PreviewNote, TimeSignature,
};
use mo_formats::studio_one::write;

fn save(daw: &str, name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/recipe").join(daw).join(name)
}

fn view_of(bytes: &[u8]) -> Arrangement {
    arrangement::read_bytes(Path::new("x.song"), bytes).into_iter().next().unwrap()
}

fn unknown(_: &str) -> Option<write::SampleFormat> {
    None
}

/// One file of the song, as text.
fn file_of(bytes: &[u8], name: &str) -> String {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut text = String::new();
    zip.by_name(name).unwrap().read_to_string(&mut text).unwrap();
    text
}

#[test]
fn an_empty_song_has_its_tempo_time_signature_and_title_and_nothing_of_whoever_saved_the_empty_one() {
    let song = Arrangement {
        view: "test",
        tempo: 128.0,
        signature: TimeSignature::new(3, 4).unwrap(),
        lanes: Vec::new(),
        markers: Vec::new(),
        notes: Vec::new(),
    };
    let written = write::write(&song, "Night Drive", &unknown);
    assert!(written.left_out.is_empty(), "{:?}", written.left_out);
    let back = view_of(&written.bytes);
    assert!((back.tempo - 128.0).abs() < 1e-9);
    assert_eq!(back.signature, TimeSignature::new(3, 4).unwrap());
    let meta = file_of(&written.bytes, "metainfo.xml");
    assert!(meta.contains("<Attribute id=\"Document:Title\" value=\"Night Drive\"/>\r\n"), "{meta}");
    assert!(!meta.contains("motst"));
    assert!(!file_of(&written.bytes, "Song/mediapool.xml").contains("J:/"));
    // The tempo track's point is the tempo too, as Studio One keeps it: (BPM - 10) / 390.
    let xml = file_of(&written.bytes, "Song/song.xml");
    assert!(xml.contains(&format!("<EnvelopePoint value=\"{}\"", 118.0 / 390.0)), "{xml}");
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
    let mut lead = PreviewLane::new("Lead & \"Hook\"", LaneKind::Instrument);
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
            gain_db: -6.0,
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
    let format = |_: &str| Some(write::SampleFormat { rate: 44_100, channels: 2, seconds: 4.0 });
    let written = write::write(&made_up_song(), "Made up", &format);
    let back = view_of(&written.bytes);
    assert!(back.notes.is_empty(), "{:?}", back.notes);
    assert!((back.tempo - 100.0).abs() < 1e-9);
    let markers: Vec<(f64, &str)> = back.markers.iter().map(|m| (m.beat, m.name.as_str())).collect();
    assert!(markers.contains(&(0.0, "Intro")) && markers.contains(&(16.0, "Drop")), "{markers:?}");
    let lanes: Vec<(&str, bool, usize)> = back
        .lanes
        .iter()
        .filter(|l| l.kind != LaneKind::Automation)
        .map(|l| (l.name.as_str(), l.muted, l.clips.len()))
        .collect();
    assert_eq!(lanes, [("Pad", true, 2), ("Lead & \"Hook\"", false, 1), ("Drums", false, 1)]);
    let pad = back.lanes.iter().find(|l| l.name == "Pad").unwrap();
    assert_eq!((pad.clips[0].start, pad.clips[0].length, pad.clips[1].start), (0.0, 8.0, 16.0));
    let notes: Vec<(f64, f64, u8, bool)> =
        pad.clips[0].notes.iter().map(|n| (n.start, n.length, n.key, n.muted)).collect();
    assert_eq!(notes, [(0.0, 4.0, 60, false), (4.0, 4.0, 65, true)]);
    assert!((i32::from(pad.clips[0].notes[0].velocity) - 52000).abs() < 2);
    let lead = &back.lanes.iter().find(|l| l.name.starts_with("Lead")).unwrap().clips[0];
    assert!(lead.muted);
    assert_eq!((lead.start, lead.length), (8.0, 6.0));

    let drums = &back.lanes.iter().find(|l| l.name == "Drums").unwrap().clips[0];
    assert!((drums.start - 4.0).abs() < 1e-9 && (drums.length - 2.0).abs() < 1e-9, "{drums:?}");
    let part = &drums.audio[0];
    assert_eq!(part.path, r"C:\Samples\Loop.wav");
    assert!((part.from - 0.3).abs() < 1e-9, "{}", part.from);
    assert!((part.seconds_per_beat.unwrap() - 0.6).abs() < 1e-9);
    assert!((part.gain_db + 6.0).abs() < 1e-9);
    assert!((drums.fade_in - 0.5).abs() < 1e-9, "{}", drums.fade_in);
    // The same clip twice is one file of notes.
    let pool = file_of(&written.bytes, "Song/mediapool.xml");
    assert_eq!(pool.matches("<MusicClip ").count(), 2);
    assert!(pool.contains("frameCount=\"176400\""));
    assert_eq!(written.left_out.len(), 2, "{:?}", written.left_out);
}

#[test]
fn fl_and_bitwig_saves_keep_their_notes_audio_and_markers_in_studio_one() {
    for (daw, name) in [
        ("fl", "MO Recipe 02 notes.flp"),
        ("fl", "MO Recipe 04 audio clip.flp"),
        ("fl", "MO Recipe 16 time signature and marker.flp"),
        ("bitwig", "MO Recipe 02 notes.bwproject"),
        ("bitwig", "MO Recipe 04 audio clip.bwproject"),
        ("studio-one", "MO Recipe 12 time signature and marker.song"),
    ] {
        let original = arrangement::read(&save(daw, name)).unwrap().into_iter().next().unwrap();
        let back = view_of(&write::write(&original, "Copy", &unknown).bytes);
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
        assert_eq!(back.signature, original.signature, "{name}");
        let beats = |a: &Arrangement| a.markers.iter().map(|m| (m.beat * 96.0).round() as i64).collect::<Vec<_>>();
        assert_eq!(beats(&back), beats(&original), "{name}");
    }
}

#[test]
fn a_track_with_notes_and_audio_becomes_two_and_an_empty_one_keeps_its_kind() {
    let mut song = made_up_song();
    let drums = song.lanes.remove(2);
    let mut mixed = PreviewLane::new("Mixed", LaneKind::Instrument);
    mixed.clips = vec![song.lanes[1].clips[0].clone(), drums.clips[0].clone()];
    song.lanes = vec![mixed, PreviewLane::new("Synth", LaneKind::Instrument)];
    let original = song.clone();
    let back = view_of(&write::write(&song, "Mixed", &unknown).bytes);
    let lanes: Vec<(&str, LaneKind)> =
        back.lanes.iter().filter(|l| l.kind != LaneKind::Automation).map(|l| (l.name.as_str(), l.kind)).collect();
    assert_eq!(
        lanes,
        [("Mixed", LaneKind::Instrument), ("Mixed audio", LaneKind::Audio), ("Synth", LaneKind::Instrument)]
    );
    assert_eq!(back.note_count(), original.note_count());
}

#[test]
fn an_fl_pattern_with_several_channels_is_a_track_for_each_and_its_steps_get_a_length() {
    let mut song = made_up_song();
    let mut steps = note(0.0, 0.0, 36, 40000, false);
    steps.step = true;
    let mut snare = note(1.0, 0.0, 38, 40000, false);
    snare.step = true;
    snare.part = 1;
    let mut lane = PreviewLane::new("Track 1", LaneKind::Instrument);
    lane.clips = vec![PreviewClip {
        start: 0.0,
        length: 4.0,
        notes: vec![steps, snare],
        parts: vec!["Kick".into(), "Snare".into()],
        ..PreviewClip::default()
    }];
    song.lanes = vec![lane];
    let back = view_of(&write::write(&song, "Steps", &unknown).bytes);
    let lanes: Vec<(&str, f64)> = back
        .lanes
        .iter()
        .filter(|l| l.kind == LaneKind::Instrument)
        .map(|l| (l.name.as_str(), l.clips[0].notes[0].length))
        .collect();
    assert_eq!(lanes, [("Kick", 0.25), ("Snare", 0.25)]);
}

#[test]
fn audio_fitted_to_its_beats_plays_at_the_speed_that_fits_and_a_percent_in_its_path_reads_back() {
    let mut song = made_up_song();
    let part = &mut song.lanes[2].clips[0].audio[0];
    part.path = r"C:\Loops\Rise 50%40bpm.wav".into();
    part.seconds_per_beat = None;
    // A 4.8-second file over 4 beats at 100 BPM (0.6 s a beat): 1.2 seconds a beat, twice as slow.
    part.whole_file_beats = Some(4.0);
    let format = |_: &str| Some(write::SampleFormat { rate: 48_000, channels: 2, seconds: 4.8 });
    let written = write::write(&song, "Fit", &format);
    let back = view_of(&written.bytes);
    let part = &back.lanes.iter().find(|l| l.name == "Drums").unwrap().clips[0].audio[0];
    assert_eq!(part.path, r"C:\Loops\Rise 50%40bpm.wav");
    assert!((part.seconds_per_beat.unwrap() - 1.2).abs() < 1e-9, "{:?}", part.seconds_per_beat);
    assert!(!file_of(&written.bytes, "Devices/hardwarestoragemanager.xml").contains("48v"));
}
