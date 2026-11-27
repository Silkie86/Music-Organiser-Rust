//! Writes the starting files for the fuzzer (see `fuzz/README.md`): the made-up projects the other tests build, one
//! folder per fuzz target, in `fuzz/corpus/<target>/`. None of Solomon's files are used (CLAUDE.md rule 4), and the
//! files are made fresh each time rather than kept in the repository.
//!
//! It only runs when asked, as it writes into the repository's folder:
//! `cargo test -p mo-formats --test fuzz_seeds -- --ignored`

// Test files may unwrap: a test that stops there has failed, which is what it should do.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::{Path, PathBuf};

use common::ableton::*;
use common::bitwig::*;
use common::midi::*;
use common::studio_one::*;
use common::*;

/// Where the starting files go: `fuzz/corpus` at the top of the repository, or the folder `MO_FUZZ_CORPUS` names.
fn corpus() -> PathBuf {
    std::env::var_os("MO_FUZZ_CORPUS")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus"))
}

/// Writes each file into the target's folder, named `seed-1`, `seed-2` and so on.
fn write(target: &str, files: &[Vec<u8>]) {
    write_named(target, "seed", files);
}

/// Writes each file into the target's folder, named `<name>-1`, `<name>-2` and so on.
fn write_named(target: &str, name: &str, files: &[Vec<u8>]) {
    let dir = corpus().join(target);
    std::fs::create_dir_all(&dir).expect("make the corpus folder");
    for (i, bytes) in files.iter().enumerate() {
        std::fs::write(dir.join(format!("{name}-{}", i + 1)), bytes).expect("write a seed");
    }
}

/// The files for a target whose first byte picks the file's ending (see the target): `pick` put before each.
fn picked(pick: u8, files: &[Vec<u8>]) -> Vec<Vec<u8>> {
    files.iter().map(|f| std::iter::once(pick).chain(f.iter().copied()).collect()).collect()
}

/// The files with this ending in a folder of the repository (the recipe saves, the sound tests' files).
fn files_in(folder: &str, ending: &str) -> Vec<Vec<u8>> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(folder);
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("read a fixtures folder")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.to_string_lossy().to_lowercase().ends_with(ending))
        .collect();
    paths.sort();
    paths.iter().map(|p| std::fs::read(p).expect("read a fixture")).collect()
}

#[test]
#[ignore = "writes the fuzzer's starting files into fuzz/corpus; run by hand"]
fn writes_the_fuzz_seeds() {
    let fl = vec![
        fl_studio_project(),
        fl_studio_9_project(b"VST2 chunk"),
        fl_studio_9_steps(),
        fl_studio_25_nothing_saved(),
        fl_studio_25_project(),
        fl_studio_25_unplayed(true),
        fl_studio_25_muted_tracks(),
        fl_studio_25_automation(true, false),
        fl_studio_25_switched_and_mixer(true),
        fl_studio_25_back_to_back(true),
        fl_studio_25_one_sample_two_ways(r"C:\Samples\Kick.wav"),
        fl_studio_25_audio_only(),
    ];
    write("fl", &fl);

    let bitwig = vec![
        bitwig_project_bytes(),
        bitwig_undecodable_bytes("Pad"),
        bitwig_scan_paths_bytes(),
        bitwig_collected_bytes(true),
        bitwig_collected_bytes(false),
        bitwig_template(Template::default()),
        bitwig_template(Template {
            automation_lane: false,
            audio_track: false,
            device_power: false,
            master_plugin: true,
            as_6_1_saves: false,
        }),
    ];
    write("bitwig_decode", &bitwig);
    write("bitwig_read", &bitwig);

    write("studio_one", &[studio_one_song_bytes(), studio_one_inserts_bytes()]);

    write("midi_smf", &[two_tracks(), one_track_two_channels("Lead"), stacked_and_unreleased()]);

    write("midi_clip", &[clip_two_groups(), clip(480, 128.0, "Bass", &[midi1(0, 0x90, 60, 100)], 96)]);

    // The recipe saves (made by hand from a recipe, made-up tones only: CLAUDE.md rule 4's one exception).
    let recipe = "crates/mo-formats/tests/fixtures/recipe";
    let fl_saves = files_in(&format!("{recipe}/fl"), ".flp");
    let bitwig_saves = files_in(&format!("{recipe}/bitwig"), ".bwproject");
    let studio_one_saves = files_in(&format!("{recipe}/studio-one"), ".song");
    write_named("fl", "recipe", &fl_saves);
    write_named("bitwig_decode", "recipe", &bitwig_saves);
    write_named("bitwig_read", "recipe", &bitwig_saves);
    write_named("studio_one", "recipe", &studio_one_saves);

    // Ableton sets, plain and gzipped as Live saves them.
    let set_texts = [
        set_xml(),
        small_set(
            r#"<MidiClip Id="0" Time="0"><CurrentStart Value="0" /><CurrentEnd Value="4" /><Name Value="Hook" /><Notes><KeyTracks><KeyTrack Id="0"><Notes><MidiNoteEvent Time="0" Duration="1" Velocity="100" /></Notes><MidiKey Value="60" /></KeyTrack></KeyTracks></Notes></MidiClip>"#,
            124.0,
            true,
        ),
    ];
    let mut sets: Vec<Vec<u8>> = set_texts.iter().map(|t| gzipped(t)).collect();
    sets.extend(set_texts.iter().map(|t| t.as_bytes().to_vec()));
    write("ableton", &sets);

    // The song shape and the MIDI index's look pick the ending by the first byte (see each target).
    let fl_all: Vec<Vec<u8>> = fl.iter().chain(&fl_saves).cloned().collect();
    let bitwig_all: Vec<Vec<u8>> = bitwig.iter().chain(&bitwig_saves).cloned().collect();
    write_named("song_shape", "fl", &picked(0, &fl_all));
    write_named("song_shape", "bitwig", &picked(1, &bitwig_all));
    write_named("song_shape", "ableton", &picked(2, &sets));
    write_named("song_shape", "studio-one", &picked(3, &studio_one_saves));
    let smf = [two_tracks(), one_track_two_channels("Lead"), stacked_and_unreleased()];
    write_named("midi_find", "smf", &picked(0, &smf));
    write_named("midi_find", "clip", &picked(1, &[clip_two_groups()]));
    write_named("midi_find", "fl", &picked(2, &fl_all));
    write_named("midi_find", "bitwig", &picked(3, &bitwig_all));
    write_named("midi_find", "ableton", &picked(4, &sets));
    write_named("midi_find", "studio-one", &picked(5, &studio_one_saves));

    // Shapes and curves (the endings are listed in the shapes target).
    let vital =
        br#"{"num_points":3,"points":[0.0,1.0,1.0,0.0,1.0,1.0],"powers":[0.0,2.0,0.0],"smooth":false}"#.to_vec();
    let vital_preset = format!(
        r#"{{"author":"x","settings":{{"lfos":[{}]}}}}"#,
        r#"{"name":"Mine","num_points":2,"points":[0.0,1.0,0.25,0.0],"powers":[1.0,0.0],"smooth":false}"#
    )
    .into_bytes();
    let duck = br#"<?xml version="1.0" encoding="UTF-8"?>
<MultipointEnvelopeData thumbnail="">
  <node x="0.0" value="0.0" curve="0.5" curveAux="0.5" shape="0"/>
  <node x="0.5" value="1.0" curve="0.2" curveAux="0.5" shape="1"/>
  <node x="1.0" value="0.0" curve="0.5" curveAux="0.5" shape="0"/>
  <lfoSettings start="0.0" end="1.0" offset="0.0"/>
</MultipointEnvelopeData>"#
        .to_vec();
    let curve = mo_formats::bitwig::curve::write(&mo_formats::bitwig::curve::Curve {
        name: "Fuzz".into(),
        creator: "Music Organiser".into(),
        comment: String::new(),
        category: "Sequence".into(),
        tags: String::new(),
        beat_length: Some(4.0),
        points: vec![
            mo_formats::bitwig::curve::CurvePoint { x: 0.0, y: 0.0, bend: 0.5 },
            mo_formats::bitwig::curve::CurvePoint { x: 0.5, y: 1.0, bend: 0.0 },
            mo_formats::bitwig::curve::CurvePoint { x: 1.0, y: 0.0, bend: 0.0 },
        ],
    })
    .expect("write a curve");
    write_named("shapes", "vital", &picked(3, &[vital]));
    write_named("shapes", "vital-preset", &picked(4, &[vital_preset]));
    write_named("shapes", "duck", &picked(5, &[duck]));
    write_named("shapes", "curve", &picked(7, &[curve]));
    write_named("shapes", "fl", &picked(8, &fl_all));
    write_named("shapes", "bitwig", &picked(9, &bitwig_all));

    // Presets: an .fxp around a chunk and one of values, an .fxb, a .vstpreset and a Bitwig preset.
    use mo_formats::presets::{fxp, vstpreset};
    let chunk = fxp::wrap_chunk(b"a plugin's own settings", "Fuzz", 0x5866_6572);
    let params = fxp::wrap_params(&[0.0, 0.5, 1.0], "Fuzz", 0x5866_6572);
    write_named("presets", "fxp", &picked(0, &[chunk.clone(), params]));
    write_named("presets", "fxb", &picked(1, &[mo_formats::presets::fxp_to_bank(&chunk, 0)]));
    write_named(
        "presets",
        "vstpreset",
        &picked(2, &[vstpreset::build("56535458565364736572756d00000000", b"component state", b"controller")]),
    );
    write_named("presets", "bwpreset", &picked(3, &files_in(&format!("{recipe}/bitwig"), ".bwpreset")));

    // Sounds: the sound tests' files and the recipe's tone, by their endings (see the audio target).
    let audio = "crates/mo-audio/tests/fixtures";
    let mut wavs = files_in(audio, ".wav");
    wavs.extend(files_in(&format!("{recipe}/studio-one/Media"), ".wav"));
    write_named("audio", "wav", &picked(0, &wavs));
    write_named("audio", "mp3", &picked(3, &files_in(audio, ".mp3")));
    write_named("audio", "flac", &picked(4, &files_in(audio, ".flac")));
    write_named("audio", "ogg", &picked(5, &files_in(audio, ".ogg")));
    write_named("audio", "m4a", &picked(6, &files_in(audio, ".m4a")));
}
