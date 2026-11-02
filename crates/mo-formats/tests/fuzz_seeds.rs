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
    let dir = corpus().join(target);
    std::fs::create_dir_all(&dir).expect("make the corpus folder");
    for (i, bytes) in files.iter().enumerate() {
        std::fs::write(dir.join(format!("seed-{}", i + 1)), bytes).expect("write a seed");
    }
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
        }),
    ];
    write("bitwig_decode", &bitwig);
    write("bitwig_read", &bitwig);

    write("studio_one", &[studio_one_song_bytes(), studio_one_inserts_bytes()]);

    write("midi_smf", &[two_tracks(), one_track_two_channels("Lead"), stacked_and_unreleased()]);

    write("midi_clip", &[clip_two_groups(), clip(480, 128.0, "Bass", &[midi1(0, 0x90, 60, 100)], 96)]);
}
