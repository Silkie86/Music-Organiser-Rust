//! Every reader on broken copies of every test save: cut short at many places, and changed in a few bytes the way a
//! damaged disk, a half-copied file or a hand-made file changes them. A reader may say the file is damaged, but must
//! never panic (which would stop the app, or lose the file's line in a scan) or take minutes.
//!
//! The fuzzer (`fuzz/README.md`) does this far more deeply but needs nightly Rust and hours; this runs on every
//! change, with the same broken copies each time (a fixed random seed), so what it once caught can't come back
//! unseen. A file it trips on is printed as how it was made, to make again by hand.

// Test files may unwrap: a test that stops there has failed, which is what it should do.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod common;
// Not in `common`: other crates' tests take `common` by its path, and this needs mo-catch.
#[path = "common/broken.rs"]
mod broken;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use broken::*;
use common::ableton::*;
use common::bitwig::*;
use common::midi::*;
use common::studio_one::*;
use common::*;

/// Where the files are said to be: a folder that isn't there, so nothing is looked for on this PC.
fn nowhere(name: &str) -> PathBuf {
    Path::new("/mo-test-nowhere").join(name)
}

/// The longest one broken file may take, in a debug build on a slow CI machine: far more than any should.
const MOST: Duration = Duration::from_secs(20);

/// Runs `read` on each broken copy of each file; fails naming the file and the copy that made it panic or take too
/// long.
fn each_broken(kind: &str, files: &[Vec<u8>], cuts: usize, changes: usize, read: impl Fn(&[u8])) {
    let mut dice = Dice::named(kind);
    let mut slowest = (Duration::ZERO, String::new());
    for (n, file) in files.iter().enumerate() {
        for (how, bytes) in broken(file, cuts, changes, None, &mut dice) {
            let started = Instant::now();
            if let Some(said) = panic_in(|| read(&bytes)) {
                panic!("{kind} file {n}, {how}: the reader panicked: {said}");
            }
            let took = started.elapsed();
            assert!(took < MOST, "{kind} file {n}, {how}: it took {took:?}");
            if took > slowest.0 {
                slowest = (took, format!("file {n}, {how}"));
            }
        }
    }
    println!("{kind}: slowest {:?} ({})", slowest.0, slowest.1);
}

/// The recipe saves of one DAW (made by hand from a recipe, made-up tones only: CLAUDE.md rule 4's one exception).
fn recipe(folder: &str, ending: &str) -> Vec<Vec<u8>> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/recipe").join(folder);
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.to_string_lossy().to_lowercase().ends_with(ending))
        .collect();
    paths.sort();
    paths.iter().map(|p| std::fs::read(p).unwrap()).collect()
}

#[test]
fn fl_studio_readers_survive_broken_saves() {
    let mut files = vec![
        fl_studio_project(),
        fl_studio_9_project(b"VST2 chunk"),
        fl_studio_9_steps(),
        fl_studio_25_project(),
        fl_studio_25_automation(true, false),
        fl_studio_25_switched_and_mixer(true),
        fl_studio_25_audio_only(),
    ];
    files.extend(recipe("fl", ".flp"));
    let path = nowhere("broken.flp");
    each_broken("FL Studio", &files, 24, 60, |raw| {
        let _ = mo_formats::fl::read_bytes(raw);
        if let Ok(flp) = mo_formats::fl::events::load(raw) {
            let _ = mo_formats::fl::song::read_song_with(&flp, true);
            let _ = mo_formats::fl::song::extract_midi(&flp);
            let _ = mo_formats::fl::presets::extract_presets(&flp);
            let _ = mo_formats::fl::arrangement::read(&flp);
        }
        let _ = mo_formats::shape::read_bytes(&path, raw);
    });
}

#[test]
fn bitwig_readers_survive_broken_saves() {
    let mut files = vec![
        bitwig_project_bytes(),
        bitwig_scan_paths_bytes(),
        bitwig_collected_bytes(true),
        bitwig_template(Template::default()),
    ];
    files.extend(recipe("bitwig", ".bwproject"));
    let path = nowhere("broken.bwproject");
    each_broken("Bitwig", &files, 24, 40, |raw| {
        let _ = mo_formats::bitwig::read_from(raw, &path);
        if let Ok(project) = mo_formats::bitwig::Project::open(raw.to_vec()) {
            let _ = project.midi();
            let _ = project.presets();
            let _ = mo_formats::bitwig::arrangement::read(&project.doc);
            let _ = mo_formats::bitwig::automation::clips(&project.doc);
        }
        let _ = mo_formats::shape::read_bytes(&path, raw);
        let _ = mo_formats::bitwig::curve::read(raw.to_vec());
    });
}

/// A Studio One song's files, each XML broken on its own and zipped again (a broken zip mostly stops at the zip).
fn studio_one_read(raw: &[u8]) {
    if let Ok(mut song) = mo_formats::studio_one::Song::open(raw) {
        let _ = song.info();
        let _ = song.midi();
        let _ = song.presets();
        let _ = song.arrangement();
    }
    let _ = mo_formats::shape::read_bytes(&nowhere("broken.song"), raw);
}

#[test]
fn studio_one_readers_survive_broken_saves() {
    let mut files = vec![studio_one_song_bytes(), studio_one_inserts_bytes()];
    files.extend(recipe("studio-one", ".song"));
    each_broken("Studio One", &files, 8, 16, studio_one_read);

    // Inside the zip: each XML file of a song broken, the rest kept.
    let mut dice = Dice::named("Studio One inside");
    for (n, file) in files.iter().enumerate() {
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(file.as_slice())).unwrap();
        let mut parts = Vec::new();
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i).unwrap();
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut bytes).unwrap();
            parts.push((entry.name().unwrap().into_owned(), bytes));
        }
        for i in 0..parts.len() {
            if !parts[i].0.ends_with(".xml") {
                continue;
            }
            for (how, bytes) in broken(&parts[i].1, 4, 6, None, &mut dice) {
                let mut changed = parts.clone();
                changed[i].1 = bytes;
                let named: Vec<(&str, &[u8])> = changed.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
                if let Some(said) = panic_in(|| studio_one_read(&stored_zip(&named))) {
                    panic!("Studio One file {n}, {} {how}: the reader panicked: {said}", parts[i].0);
                }
            }
        }
    }
}

#[test]
fn ableton_readers_survive_broken_sets() {
    let clip = r#"<MidiClip Id="0" Time="0"><CurrentStart Value="0" /><CurrentEnd Value="4" /><Name Value="Hook" /><Notes><KeyTracks><KeyTrack Id="0"><Notes><MidiNoteEvent Time="0" Duration="1" Velocity="100" /></Notes><MidiKey Value="60" /></KeyTrack></KeyTracks></Notes></MidiClip>"#;
    let texts = [set_xml(), small_set(clip, 124.0, true), small_set(clip, 90.0, false)];
    // The plain XML broken (Live reads it too), and the gzipped file broken.
    let mut files: Vec<Vec<u8>> = texts.iter().map(|t| t.as_bytes().to_vec()).collect();
    files.extend(texts.iter().map(|t| gzipped(t)));
    let path = nowhere("broken.als");
    each_broken("Ableton", &files, 24, 120, |raw| {
        if let Ok(set) = mo_formats::ableton::LiveSet::open(raw) {
            let set = set.in_folder(path.parent());
            let _ = set.info();
            let _ = set.midi();
            let _ = set.arrangements();
        }
        let _ = mo_formats::arrangement::read_bytes(&path, raw);
    });
}

#[test]
fn midi_readers_survive_broken_files() {
    let files = vec![
        two_tracks(),
        one_track_two_channels("Lead"),
        stacked_and_unreleased(),
        clip_two_groups(),
        clip(480, 128.0, "Bass", &[midi1(0, 0x90, 60, 100)], 96),
    ];
    each_broken("MIDI", &files, 16, 200, |raw| {
        let _ = mo_formats::midi::read_from(raw, &nowhere("broken.mid"));
        let _ = mo_formats::midi::smf::read_from(raw, &nowhere("broken.mid"));
        let _ = mo_formats::midi::clip::read_bytes(raw);
    });
}

#[test]
fn shape_readers_survive_broken_files() {
    let vital = br#"{"num_points":3,"points":[0.0,1.0,1.0,0.0,1.0,1.0],"powers":[0.0,2.0,0.0],"smooth":false}"#;
    let duck = br#"<?xml version="1.0" encoding="UTF-8"?>
<MultipointEnvelopeData thumbnail="">
  <node x="0.0" value="0.0" curve="0.5" curveAux="0.5" shape="0"/>
  <node x="0.5" value="1.0" curve="0.2" curveAux="0.5" shape="1"/>
  <node x="1.0" value="0.0" curve="0.5" curveAux="0.5" shape="0"/>
  <lfoSettings start="0.0" end="1.0" offset="0.0"/>
</MultipointEnvelopeData>"#;
    let curve = mo_formats::bitwig::curve::write(&mo_formats::bitwig::curve::Curve {
        name: "Broken".into(),
        creator: "Music Organiser".into(),
        comment: String::new(),
        category: "Sequence".into(),
        tags: String::new(),
        beat_length: Some(4.0),
        points: vec![
            mo_formats::bitwig::curve::CurvePoint { x: 0.0, y: 0.0, bend: 0.5 },
            mo_formats::bitwig::curve::CurvePoint { x: 1.0, y: 1.0, bend: 0.0 },
        ],
    })
    .unwrap();
    let files = vec![vital.to_vec(), duck.to_vec(), curve];
    each_broken("shapes", &files, 16, 200, |raw| {
        for name in ["a.XferShape", "a.shp", "a.fxp", "a.vitallfo", "a.vital", "a.dmlfo"] {
            let _ = mo_formats::serum::read(name, raw);
            let _ = mo_formats::vital::read(name, raw);
            let _ = mo_formats::duck::read(name, raw);
        }
        let _ = mo_formats::bitwig::curve::read(raw.to_vec());
    });
}
