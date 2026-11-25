//! A take saved as a WAV file (`mo_audio::record`).
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use mo_audio::record;

#[test]
fn a_take_is_saved_trimmed_and_never_over_another() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("record-save");
    let _ = std::fs::remove_dir_all(&dir);
    let (mut feed, take) = record::recording(1000, 1);
    let mut samples = vec![0.0f32; 300];
    samples[100..200].fill(0.5);
    feed.push(&[&samples]);
    let level = mo_audio::trim::level_of_db(-60);
    let first = record::save(take.finish(), Some(level), &dir, "Take: 1").unwrap();
    assert_eq!(first.file_name().unwrap(), "Take_ 1.wav");
    let read = mo_audio::read(&first, 60.0).unwrap();
    assert!(read.channels[0].len() < 300, "the silence is cut off");
    let (mut feed, take) = record::recording(1000, 1);
    feed.push(&[&samples]);
    let second = record::save(take.finish(), None, &dir, "Take: 1").unwrap();
    assert_eq!(second.file_name().unwrap(), "Take_ 1 (2).wav");
    // Silence all through isn't saved.
    let (mut feed, take) = record::recording(1000, 1);
    feed.push(&[&[0.0f32; 100]]);
    assert!(record::save(take.finish(), Some(level), &dir, "Quiet").is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn one_side_of_a_stereo_pair_records_as_a_mono_take() {
    let (left, right) = ([0.1f32, 0.2, 0.3], [0.7f32, 0.8, 0.9]);
    let (mut feed, take) = record::recording_of(1000, 2, Some(1));
    feed.push(&[&left, &right]);
    let audio = take.finish();
    assert_eq!(audio.channels, vec![right.to_vec()]);
    // A sound card's interleaved frames: the left side only.
    let (mut feed, take) = record::recording_of(1000, 2, Some(0));
    feed.push_interleaved(&[0.1, 0.7, 0.2, 0.8], 2);
    assert_eq!(take.finish().channels, vec![vec![0.1, 0.2]]);
}
