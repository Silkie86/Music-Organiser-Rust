//! Saves nested far deeper than any DAW writes them (racks in racks, objects in objects, tags in tags): a reader may
//! say the file is damaged, but must never run its thread out of stack (that ends the app at once, with no warning)
//! or take minutes. Each is read on a thread with a 1 MiB stack, Windows' own for a program's first thread and less
//! than the 2 MiB the app's other threads get, in a debug build, whose steps take the most stack.

// Test files may unwrap: a test that stops there has failed, which is what it should do.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use common::bitwig::*;

/// Runs `read` on a thread with a 1 MiB stack, and says how long it took. A thread that runs out of stack ends the
/// whole test program, which fails the test too.
fn on_small_stack(what: &str, read: impl FnOnce() + Send + 'static) -> Duration {
    let started = Instant::now();
    std::thread::Builder::new()
        .name(what.to_owned())
        .stack_size(1024 * 1024)
        .spawn(read)
        .expect("start the thread")
        .join()
        .unwrap_or_else(|_| panic!("{what}: the reader panicked"));
    started.elapsed()
}

/// Checks it took less than `most`: a deep file read in steps that go round and round (each level looking at all
/// the levels below it) takes minutes.
fn quick(what: &str, took: Duration, most: Duration) {
    assert!(took < most, "{what} took {took:?}");
}

/// An Ableton set whose Instrument Racks hold racks `levels` deep, a plugin at the bottom.
fn ableton_racks(levels: usize) -> String {
    let open = r#"<InstrumentGroupDevice Id="0"><Branches><InstrumentBranch Id="0"><DeviceChain><MidiToAudioDeviceChain><Devices>"#;
    let close =
        r#"</Devices></MidiToAudioDeviceChain></DeviceChain></InstrumentBranch></Branches></InstrumentGroupDevice>"#;
    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton Creator="Ableton Live 12.1"><LiveSet><Tracks><MidiTrack Id="1"><Name><EffectiveName Value="Deep" /></Name>
<TrackGroupId Value="-1" /><DeviceChain><DeviceChain><Devices>"#,
    );
    xml.push_str(&open.repeat(levels));
    xml.push_str(r#"<PluginDevice Id="0"><PluginDesc><Vst3PluginInfo Id="0"><Name Value="Diva" /></Vst3PluginInfo></PluginDesc></PluginDevice>"#);
    xml.push_str(&close.repeat(levels));
    xml.push_str("</Devices></DeviceChain></DeviceChain></MidiTrack></Tracks></LiveSet></Ableton>");
    xml
}

#[test]
fn ableton_racks_in_racks_never_run_out_of_stack() {
    for levels in [8, 200, 5_000] {
        let xml = ableton_racks(levels);
        let what = format!("racks {levels} deep");
        let took = on_small_stack(&what, move || {
            let set = mo_formats::ableton::LiveSet::open(xml.as_bytes()).unwrap();
            let info = set.info();
            // A real set's racks (a handful deep) still give the plugin at the bottom.
            if levels <= 8 {
                assert!(info.plugins.iter().any(|p| p.name == "Diva"), "{:?}", info.plugins);
            }
            let _ = set.midi();
            let _ = set.arrangements();
        });
        quick(&what, took, Duration::from_secs(20));
    }
}

/// Any XML, `levels` tags deep.
fn deep_tags(root: &str, inner: &str, levels: usize) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><{root}>{}{}</{root}>"#,
        format!("<{inner}>").repeat(levels),
        format!("</{inner}>").repeat(levels)
    )
}

#[test]
fn deep_xml_never_runs_out_of_stack() {
    let levels = 50_000;
    let what = "deep xml";
    let took = on_small_stack(what, move || {
        // Ableton: tags in tags anywhere in the set.
        for inner in ["Devices", "Tracks", "MidiTrack", "DeviceChain", "Branches", "Events", "MidiClip", "KeyTrack"] {
            let xml = deep_tags("Ableton", inner, levels);
            if let Ok(set) = mo_formats::ableton::LiveSet::open(xml.as_bytes()) {
                let _ = set.info();
                let _ = set.midi();
                let _ = set.arrangements();
            }
        }
        // Studio One: each of the song's files nested deep.
        for inner in ["Attributes", "Tracks", "MusicTrack", "Events", "MusicPart", "List"] {
            let song = deep_tags("Song", inner, levels);
            let meta = deep_tags("MetaInformation", inner, levels);
            let pool = deep_tags("MediaPool", inner, levels);
            let synths = deep_tags("SynthFolder", inner, levels);
            let mixer = deep_tags("AudioMixer", inner, levels);
            let zip = stored_zip(&[
                ("metainfo.xml", meta.as_bytes()),
                ("Song/song.xml", song.as_bytes()),
                ("Song/mediapool.xml", pool.as_bytes()),
                ("Devices/audiosynthfolder.xml", synths.as_bytes()),
                ("Devices/audiomixer.xml", mixer.as_bytes()),
            ]);
            if let Ok(mut s) = mo_formats::studio_one::Song::open(&zip) {
                let _ = s.info();
                let _ = s.midi();
                let _ = s.presets();
                let _ = s.arrangement();
            }
            let path = Path::new("/mo-test-nowhere/deep.song");
            let _ = mo_formats::shape::read_bytes(path, zip.as_slice());
        }
        // Duck's shapes.
        let duck = deep_tags("MultipointEnvelopeData", "MultipointEnvelopeData", levels);
        let _ = mo_formats::duck::read("deep.dmlfo", duck.as_bytes());
    });
    quick(what, took, Duration::from_secs(60));
}

/// A Bitwig project whose objects hold objects `levels` deep, in the field a device chain uses.
fn bitwig_nested(levels: usize) -> Vec<u8> {
    BwWriter::file(
        |m| {
            m.meta_entry("application_version_name", 8);
            m.str_val("5.2");
        },
        |b| {
            for _ in 0..levels {
                b.begin(41);
                b.obj(5541);
            }
            b.begin(41);
            b.str(5542, "Bottom");
            b.end();
            for _ in 0..levels {
                b.end();
            }
        },
        None,
    )
}

#[test]
fn bitwig_objects_in_objects_never_run_out_of_stack() {
    for levels in [100, 511, 600, 20_000] {
        let raw = bitwig_nested(levels);
        let what = format!("bitwig objects {levels} deep");
        let took = on_small_stack(&what, move || {
            let path = Path::new("/mo-test-nowhere/deep.bwproject");
            let _ = mo_formats::bitwig::read_from(&raw, path);
            if let Ok(project) = mo_formats::bitwig::Project::open(raw.clone()) {
                let _ = project.midi();
                let _ = project.presets();
                let _ = mo_formats::bitwig::arrangement::read(&project.doc);
                let _ = mo_formats::bitwig::automation::clips(&project.doc);
            }
            let _ = mo_formats::shape::read_bytes(path, raw.as_slice());
            let _ = mo_formats::bitwig::curve::read(raw);
        });
        quick(&what, took, Duration::from_secs(20));
    }
}
