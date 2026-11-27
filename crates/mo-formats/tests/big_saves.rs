//! Saves far bigger than most: hundreds of tracks and clips, a few hundred thousand notes, read by every reader. A
//! reader that looks at every clip again for each clip (steps that grow with the square of the file) takes minutes on
//! these, so each must finish well inside a time a beta tester would wait for. The saves are made here from the
//! recipe saves, copied many times over, and written with the app's own FL Studio and Studio One writers.

// Test files may unwrap: a test that stops there has failed, which is what it should do.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use mo_formats::arrangement::{Arrangement, LaneKind};

/// The recipe save's arrangement, its note lanes copied `lanes` times and each lane's clips `clips` times over.
fn big_song(lanes: usize, clips: usize) -> Arrangement {
    let raw = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/recipe/studio-one/MO Recipe 02 notes.song"),
    )
    .unwrap();
    let mut song = mo_formats::studio_one::Song::open(&raw).unwrap().arrangement();
    let lane = song
        .lanes
        .iter()
        .find(|l| l.kind == LaneKind::Instrument && l.clips.iter().any(|c| !c.notes.is_empty()))
        .unwrap()
        .clone();
    let clip = lane.clips.iter().find(|c| !c.notes.is_empty()).unwrap().clone();
    let length = clip.length.max(1.0);
    song.lanes.clear();
    for l in 0..lanes {
        let mut lane = lane.clone();
        lane.name = format!("Track {l}");
        lane.clips = (0..clips)
            .map(|c| {
                let mut clip = clip.clone();
                clip.start = c as f64 * length;
                clip.name = format!("Clip {l}.{c}");
                clip
            })
            .collect();
        song.lanes.push(lane);
    }
    song
}

fn timed(what: &str, most: Duration, read: impl FnOnce()) {
    let started = Instant::now();
    read();
    let took = started.elapsed();
    println!("{what}: {took:?}");
    assert!(took < most, "{what} took {took:?}");
}

/// How long a debug build may take on a slow CI machine (a release build is ten or more times quicker).
const MOST: Duration = Duration::from_secs(20);

#[test]
fn big_fl_studio_and_studio_one_saves_read_in_good_time() {
    let song = big_song(100, 100);
    let notes: usize = song.lanes.iter().flat_map(|l| &l.clips).map(|c| c.notes.len()).sum();
    println!(
        "{} lanes, {} clips, {notes} notes",
        song.lanes.len(),
        song.lanes.iter().map(|l| l.clips.len()).sum::<usize>()
    );

    let flp = mo_formats::fl::write::write(&song, "Big", 0.0).bytes;
    println!("FL file {} bytes", flp.len());
    let path = Path::new("/mo-test-nowhere/big.flp");
    timed("FL list", MOST, || drop(mo_formats::fl::read_bytes(&flp)));
    timed("FL song, notes, presets, arrangement", MOST, || {
        let f = mo_formats::fl::events::load(&flp).unwrap();
        let _ = mo_formats::fl::song::read_song_with(&f, true);
        let _ = mo_formats::fl::song::extract_midi(&f);
        let _ = mo_formats::fl::presets::extract_presets(&f);
        let _ = mo_formats::fl::arrangement::read(&f);
    });
    timed("FL shape", MOST, || drop(mo_formats::shape::read_bytes(path, flp.as_slice())));

    let unknown = |_: &str| None;
    let song_file = mo_formats::studio_one::write::write(&song, "Big", &unknown).bytes;
    println!("Studio One file {} bytes", song_file.len());
    timed("Studio One list, notes, presets, arrangement", MOST, || {
        let mut s = mo_formats::studio_one::Song::open(&song_file).unwrap();
        let _ = s.info();
        let _ = s.midi();
        let _ = s.presets();
        let _ = s.arrangement();
    });
}

/// A made-up Live set: `tracks` MIDI tracks, each with `clips` arrangement clips of `notes` notes and a plugin.
fn big_set(tracks: usize, clips: usize, notes: usize) -> String {
    let mut xml =
        String::from(r#"<?xml version="1.0" encoding="UTF-8"?><Ableton Creator="Ableton Live 12.1"><LiveSet><Tracks>"#);
    let note_events: String = (0..notes)
        .map(|n| format!(r#"<MidiNoteEvent Time="{}" Duration="0.25" Velocity="100" NoteId="{n}" />"#, n as f64 * 0.25))
        .collect();
    for t in 0..tracks {
        xml.push_str(&format!(
            r#"<MidiTrack Id="{t}"><Name><EffectiveName Value="Track {t}" /></Name><TrackGroupId Value="-1" /><DeviceChain><MainSequencer><ClipTimeable><ArrangerAutomation><Events>"#
        ));
        for c in 0..clips {
            let start = c * 16;
            xml.push_str(&format!(
                r#"<MidiClip Id="{c}" Time="{start}"><CurrentStart Value="{start}" /><CurrentEnd Value="{}" /><Name Value="Clip {c}" /><Loop><LoopStart Value="0" /><LoopEnd Value="16" /><LoopOn Value="false" /></Loop><Notes><KeyTracks><KeyTrack Id="0"><Notes>{note_events}</Notes><MidiKey Value="60" /></KeyTrack></KeyTracks></Notes></MidiClip>"#,
                start + 16
            ));
        }
        xml.push_str(r#"</Events></ArrangerAutomation></ClipTimeable></MainSequencer><DeviceChain><Devices><PluginDevice Id="0"><PluginDesc><Vst3PluginInfo Id="0"><Name Value="Diva" /></Vst3PluginInfo></PluginDesc></PluginDevice></Devices></DeviceChain></DeviceChain></MidiTrack>"#);
    }
    xml.push_str(r#"</Tracks><MasterTrack><DeviceChain><Mixer><Tempo><Manual Value="128" /></Tempo></Mixer></DeviceChain></MasterTrack></LiveSet></Ableton>"#);
    xml
}

#[test]
fn a_big_ableton_set_reads_in_good_time() {
    let xml = big_set(100, 50, 32);
    println!("Live set {} bytes", xml.len());
    let set = mo_formats::ableton::LiveSet::open(xml.as_bytes()).unwrap();
    timed("Ableton list", MOST, || drop(set.info()));
    timed("Ableton notes", MOST, || drop(set.midi()));
    timed("Ableton arrangement", MOST, || drop(set.arrangements()));
    timed("Ableton shape", MOST, || {
        drop(mo_formats::shape::read_bytes(Path::new("/mo-test-nowhere/big.als"), xml.as_bytes()))
    });
}

/// A Standard MIDI File with `tracks` tracks of `notes` notes each.
fn big_midi(tracks: usize, notes: usize) -> Vec<u8> {
    let track: Vec<u8> = (0..notes)
        .flat_map(|n| {
            let key = 36 + (n % 48) as u8;
            [0x00, 0x90, key, 100, 0x30, 0x80, key, 0]
        })
        .chain([0x00, 0xff, 0x2f, 0x00])
        .collect();
    let tracks: Vec<&[u8]> = (0..tracks).map(|_| track.as_slice()).collect();
    common::midi::smf(480, &tracks)
}

#[test]
fn a_big_midi_file_reads_in_good_time() {
    let raw = big_midi(32, 20_000);
    println!("MIDI file {} bytes", raw.len());
    timed("MIDI", MOST, || drop(mo_formats::midi::read_from(&raw, Path::new("/mo-test-nowhere/big.mid"))));
}
