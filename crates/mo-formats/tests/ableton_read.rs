//! Reading Ableton Live sets, on made-up sets laid out as Live 11 and 12 save them (none of Solomon's are in the repo).

// Test files may unwrap: a test that stops there has failed, which is what it should do.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod common;

use common::ableton::*;
use common::test_dir;
use mo_formats::ableton::{self, LiveSet};
use mo_formats::arrangement::{LaneKind, Stretch, TimeSignature};
use mo_formats::{PluginKind, velocity};

#[test]
fn reads_tracks_plugins_samples_and_tempo() {
    let dir = test_dir("als-read");
    let path = dir.join("Night Drive.als");
    std::fs::write(&path, gzipped(&set_xml())).unwrap();
    let info = ableton::read(&path).expect("read");
    assert_eq!(info.daw, "Ableton Live");
    assert_eq!(info.version, "Ableton Live 12.1.5");
    assert_eq!(info.tempo, Some(128.0));
    assert_eq!(info.tracks, ["Music", "1-Serum", "Drums", "A-Reverb"]);
    let plugins: Vec<(&str, PluginKind, &str, &str)> =
        info.plugins.iter().map(|p| (p.name.as_str(), p.kind, p.format.as_str(), p.id.as_str())).collect();
    assert_eq!(
        plugins,
        [
            ("Serum_x64", PluginKind::Instrument, "VST2", "XfsX"),
            ("EQ Eight", PluginKind::Effect, "Native", ""),
            ("Pro-Q 3", PluginKind::Effect, "VST3", "72726271FFFFFFFF0000000000000001"),
            ("Simpler", PluginKind::Instrument, "Native", ""),
            ("Reverb", PluginKind::Effect, "Native", ""),
            ("Glue Compressor", PluginKind::Effect, "Native", ""),
        ]
    );
    assert_eq!(info.plugins[0].path, r"C:\VstPlugins\Serum_x64.dll");
    // The frozen track's audio is Live's own, so it isn't listed.
    assert_eq!(info.samples, [r"C:\Samples\Break.wav", r"C:\Drums\Kick.aif"]);
    assert_eq!(info.details.get("Time signature").map(String::as_str), Some("6/8"));
    assert_eq!(info.details.get("Clips").map(String::as_str), Some("3"));
    assert_eq!(info.details.get("Notes").map(String::as_str), Some("4"));
    assert_eq!(info.details.get("Scenes").map(String::as_str), Some("2"));
}

#[test]
fn a_plain_xml_set_reads_too_and_other_files_dont() {
    let set = LiveSet::open(set_xml().as_bytes()).expect("plain XML");
    assert_eq!(set.info().tempo, Some(128.0));
    assert!(LiveSet::open(b"PK\x03\x04 not a set").is_err());
    assert!(LiveSet::open(&gzipped("<?xml version=\"1.0\"?><Bitwig/>")).is_err());
    // XML that won't parse is listed with a warning, not dropped.
    let broken = LiveSet::open(b"<?xml version=\"1.0\"?><Ableton><LiveSet>").unwrap().info();
    assert_eq!(broken.warnings.len(), 1);
}

#[test]
fn midi_clips_come_out_by_track_from_where_they_start_playing() {
    let set = LiveSet::open(&gzipped(&set_xml())).unwrap();
    let clips = set.midi().unwrap();
    assert_eq!(clips.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Hook", "Session Hook"]);
    let hook = &clips[0];
    assert_eq!(hook.ppq, 960);
    assert_eq!(hook.tracks[0].0, "1-Serum");
    // The whole loop: the note at 0, before the start marker, plays from the second pass on.
    let notes: Vec<(i64, i64, u8, bool)> =
        hook.tracks[0].1.iter().map(|n| (n.start, n.length, n.key, n.muted)).collect();
    assert_eq!(notes, [(0, 480, 60, false), (1440, 240, 64, false), (1920, 960, 60, true)]);
    assert_eq!(hook.tracks[0].1[1].velocity, velocity::from_unit(80.0 / 127.0));
    assert_eq!(hook.end, Some(3840));
}

#[test]
fn the_arrangement_unrolls_loops_and_times_audio_by_its_warp_markers() {
    let set = LiveSet::open(&gzipped(&set_xml())).unwrap();
    let views = set.arrangements();
    assert_eq!(views.len(), 1, "the arrangement has clips, so the session isn't shown");
    let view = &views[0];
    assert_eq!(view.view, "Ableton arrangement");
    assert_eq!((view.tempo, view.signature), (128.0, TimeSignature::new(6, 8).unwrap()));
    assert_eq!(
        view.markers.iter().map(|m| (m.beat, m.name.as_str())).collect::<Vec<_>>(),
        [(0.0, "Intro"), (32.0, "Drop")]
    );
    let lanes: Vec<(&str, LaneKind, u32, bool)> =
        view.lanes.iter().map(|l| (l.name.as_str(), l.kind, l.depth, l.muted)).collect();
    assert_eq!(
        lanes,
        [
            ("Music", LaneKind::Group, 0, false),
            ("1-Serum", LaneKind::Instrument, 1, false),
            ("Drums", LaneKind::Audio, 0, true),
        ]
    );
    assert_eq!(view.lanes[1].detail, "Serum_x64");
    let hook = &view.lanes[1].clips[0];
    assert_eq!((hook.start, hook.length, hook.name.as_str()), (16.0, 10.0, "Hook"));
    // First pass from beat 1 of the loop (3 beats), then the 4-beat loop again from 0, twice (the second cut short).
    let notes: Vec<(f64, u8)> = hook.notes.iter().map(|n| (n.start, n.key)).collect();
    assert_eq!(notes, [(0.5, 64), (1.0, 60), (3.0, 60), (4.5, 64), (5.0, 60), (7.0, 60), (8.5, 64), (9.0, 60)]);

    let brk = &view.lanes[2].clips[0];
    assert!(brk.muted);
    assert_eq!((brk.start, brk.length, brk.fade_in, brk.fade_out), (4.0, 8.0, 0.5, 1.0));
    let part = &brk.audio[0];
    assert_eq!(part.path, r"C:\Samples\Break.wav");
    assert_eq!((part.start, part.length, part.from, part.seconds_per_beat), (0.0, 8.0, 0.0, Some(0.5)));
    assert_eq!((part.stretch, part.semitones), (Some(Stretch::ElastiquePro), 2.5));
    assert!((part.gain_db + 6.0206).abs() < 1e-3);
}

#[test]
fn an_empty_arrangement_shows_the_session_scenes_one_after_another() {
    // The arrangement's clips taken out: only the session clip in scene 2 is left.
    let xml = set_xml();
    let cut = |s: &str, from: &str, to: &str| {
        let a = s.find(from).unwrap();
        let b = s[a..].find(to).unwrap() + a + to.len();
        format!("{}{}", &s[..a], &s[b..])
    };
    let xml = cut(&xml, "<ClipTimeable>", "</ClipTimeable>");
    let xml = cut(&xml, "<MainSequencer><Sample>", "</Sample></MainSequencer>");
    let views = LiveSet::open(xml.as_bytes()).unwrap().arrangements();
    assert_eq!(views.iter().map(|v| v.view).collect::<Vec<_>>(), ["Ableton session", "Ableton arrangement"]);
    let session = &views[0];
    // Scene 1 is empty (a 6/8 bar, 3 beats); scene 2 starts after it.
    assert_eq!(
        session.markers.iter().map(|m| (m.beat, m.name.as_str())).collect::<Vec<_>>(),
        [(0.0, "Scene 1"), (3.0, "Hook")]
    );
    let clip = &session.lanes[1].clips[0];
    assert_eq!((clip.start, clip.length, clip.notes.len()), (3.0, 8.0, 1));
}

#[test]
fn a_damaged_sets_midi_says_so() {
    let set = LiveSet::open(&gzipped("<?xml version=\"1.0\"?><Ableton><LiveSet><Tracks>")).unwrap();
    assert!(set.midi().is_err());
    assert!(set.try_arrangements().is_err());
    assert_eq!(set.arrangements()[0].clip_count(), 0);
}

#[test]
fn a_plugin_in_an_instrument_rack_is_the_instrument_and_old_scenes_are_read() {
    let clip = r#"<MidiClip Id="0" Time="0"><CurrentStart Value="0" /><CurrentEnd Value="4" />
        <Loop><LoopStart Value="0" /><LoopEnd Value="4" /><LoopOn Value="true" /></Loop>
        <Notes><KeyTracks><KeyTrack Id="0"><Notes><MidiNoteEvent Time="1" Duration="1" Velocity="90" /></Notes>
        <MidiKey Value="48" /></KeyTrack></KeyTracks></Notes></MidiClip>"#;
    let set = LiveSet::open(small_set(clip, 120.0, false).as_bytes()).unwrap();
    let info = set.info();
    let plugins: Vec<(&str, PluginKind)> = info.plugins.iter().map(|p| (p.name.as_str(), p.kind)).collect();
    assert_eq!(
        plugins,
        [
            ("Instrument Rack", PluginKind::Instrument),
            ("Diva", PluginKind::Instrument),
            ("Drum Rack", PluginKind::Instrument),
            ("ValhallaRoom", PluginKind::Effect),
        ]
    );
    assert_eq!(info.details.get("Scenes").map(String::as_str), Some("1"));
    let views = set.arrangements();
    assert_eq!(views[0].view, "Ableton session");
    assert_eq!(views[0].lanes[0].detail, "Diva");
    assert_eq!(views[0].markers[0].name, "Verse");
    assert_eq!(views[0].lanes[0].clips[0].notes.len(), 1);
}

#[test]
fn a_start_marker_before_the_loop_plays_its_lead_in_once() {
    let clip = r#"<MidiClip Id="0" Time="0"><CurrentStart Value="0" /><CurrentEnd Value="12" />
        <Loop><LoopStart Value="4" /><LoopEnd Value="8" /><StartRelative Value="-4" /><LoopOn Value="true" /></Loop>
        <Notes><KeyTracks><KeyTrack Id="0"><Notes>
          <MidiNoteEvent Time="0" Duration="1" Velocity="90" /><MidiNoteEvent Time="4" Duration="1" Velocity="90" />
        </Notes><MidiKey Value="48" /></KeyTrack></KeyTracks></Notes></MidiClip>"#;
    let set = LiveSet::open(small_set(clip, 120.0, true).as_bytes()).unwrap();
    let starts: Vec<f64> = set.arrangements()[0].lanes[0].clips[0].notes.iter().map(|n| n.start).collect();
    assert_eq!(starts, [0.0, 4.0, 8.0]);
    let midi = set.midi().unwrap();
    assert_eq!(midi[0].tracks[0].1.iter().map(|n| n.start).collect::<Vec<_>>(), [0, 3840]);
}

#[test]
fn an_unwarped_clip_plays_its_seconds_at_any_tempo() {
    // 30 BPM: two seconds a beat. Ten seconds of the file, placed for five beats.
    let clip = r#"<AudioClip Id="0" Time="0"><CurrentStart Value="0" /><CurrentEnd Value="5" />
        <Loop><LoopStart Value="0" /><LoopEnd Value="10" /><LoopOn Value="false" /></Loop>
        <SampleRef><FileRef><Path Value="C:/Samples/Pad.wav" /></FileRef></SampleRef><IsWarped Value="false" />
        </AudioClip>"#;
    let set = LiveSet::open(small_set(clip, 30.0, true).as_bytes()).unwrap();
    let part = &set.arrangements()[0].lanes[0].clips[0].audio[0];
    assert_eq!((part.start, part.length, part.from, part.seconds_per_beat), (0.0, 5.0, 0.0, Some(2.0)));
    assert_eq!(part.stretch, Some(Stretch::Raw));
}
