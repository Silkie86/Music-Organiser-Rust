//! A save's shape (`mo_formats::shape`), read from the recipe saves (rule 4's one exception).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use mo_formats::shape::{self, Shape};

fn recipe(daw: &str, name: &str) -> Shape {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/recipe").join(daw).join(name);
    shape::read(&path).unwrap().unwrap()
}

#[test]
fn an_empty_fl_save_has_nothing() {
    let s = recipe("fl", "MO Recipe 01 empty.flp");
    assert_eq!(s, Shape::default());
}

#[test]
fn fl_counts_its_sounding_channels_automation_and_markers() {
    let s = recipe("fl", "MO Recipe 03 automation.flp");
    assert_eq!((s.tracks, s.automation, s.markers), (1, 1, 0));
    assert!((s.seconds - 1.875).abs() < 1e-9, "{}", s.seconds);
    // The sampler channel makes a third track; the marker is the song's one section mark.
    let s = recipe("fl", "MO Recipe 16 time signature and marker.flp");
    assert_eq!((s.tracks, s.automation, s.markers), (3, 1, 1));
}

#[test]
fn fl_finds_the_mixer_inserts_effects() {
    // Insert 2 holds Fruity Reeverb 2: a track with an effect, but no EQ or compressor, and the master has none.
    let s = recipe("fl", "MO Recipe 05 mixer.flp");
    assert_eq!((s.tracks, s.eq, s.compressed, s.sends), (2, 0, 0, 0));
    assert!(s.master.is_empty() && !s.master_limited());
}

#[test]
fn bitwig_counts_tracks_automation_lanes_sends_and_markers() {
    // Bitwig starts every project with one effect track.
    let s = recipe("bitwig", "MO Recipe 01 empty.bwproject");
    assert_eq!((s.tracks, s.sends, s.automation), (0, 1, 0));
    // Volume, pan and tempo lanes, each moving.
    let s = recipe("bitwig", "MO Recipe 03 automation.bwproject");
    assert_eq!((s.tracks, s.automation), (1, 3));
    // A group with no effects of its own isn't a mixer track, and nor is a deactivated track.
    let s = recipe("bitwig", "MO Recipe 05 drum machine.bwproject");
    assert_eq!(s.tracks, 3);
    let s = recipe("bitwig", "MO Recipe 06 group and off.bwproject");
    assert_eq!(s.tracks, 2);
    let s = recipe("bitwig", "MO Recipe 16 time signature and marker.bwproject");
    assert_eq!(s.markers, 1);
}

#[test]
fn a_damaged_bitwig_save_has_no_shape() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/recipe/bitwig/MO Recipe 06 group and off.bwproject");
    let mut raw = std::fs::read(&path).unwrap();
    // Its objects end short of where the header says.
    let at = raw.len() / 2;
    raw.splice(at..at, [0u8; 64]);
    assert_eq!(shape::read_bytes(&path, raw), None);
}

#[test]
fn a_studio_one_song_has_no_shape_yet() {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/recipe/studio-one/MO Recipe 06 mixer.song");
    assert_eq!(shape::read(&path).unwrap(), None);
    assert_eq!(shape::read_bytes(std::path::Path::new("x.flp"), &b"not a project"[..]), None);
}
