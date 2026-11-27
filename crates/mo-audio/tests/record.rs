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

/// The whole-app review of 26 Nov, XD-1: two takes saved in the same second, by two copies of the plugin, each get a
/// name of their own and keep their own sound; a part file already there (another save's, or one cut off) is never
/// written into or taken over.
#[test]
fn two_takes_saved_at_once_each_keep_their_own_name_and_sound() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("record-save-at-once");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // Another save's part file, being written.
    let busy = dir.join("Recording 10-00-00.wav.part");
    std::fs::write(&busy, b"another take").unwrap();
    let take = |level: f32| {
        let (mut feed, take) = record::recording(1000, 1);
        feed.push(&[&vec![level; 1_500]]);
        take.finish()
    };
    let saved: Vec<PathBuf> = std::thread::scope(|s| {
        let saves: Vec<_> = [0.25f32, 0.5, 0.75]
            .into_iter()
            .map(|level| {
                let take = take(level);
                let dir = &dir;
                s.spawn(move || record::save(take, None, dir, "Recording 10-00-00").unwrap())
            })
            .collect();
        saves.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let mut names: Vec<String> = saved.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, ["Recording 10-00-00 (2).wav", "Recording 10-00-00 (3).wav", "Recording 10-00-00 (4).wav"]);
    // Each file holds one take whole, with nothing of another's.
    let mut levels: Vec<f32> = saved
        .iter()
        .map(|p| {
            let read = mo_audio::read(p, 60.0).unwrap();
            assert_eq!(read.channels[0].len(), 1_500, "{}", p.display());
            let first = read.channels[0][0];
            assert!(read.channels[0].iter().all(|&v| (v - first).abs() < 1e-4), "{} is mixed", p.display());
            first
        })
        .collect();
    levels.sort_by(f32::total_cmp);
    assert!(levels.iter().zip([0.25, 0.5, 0.75]).all(|(a, b)| (a - b).abs() < 1e-4), "{levels:?}");
    assert_eq!(std::fs::read(&busy).unwrap(), b"another take", "the other save's part is left alone");
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

#[test]
fn a_take_keeps_its_shape_as_it_records() {
    // 20 stretches a second: at 1000 frames a second, one each 50 frames.
    let (mut feed, mut take) = record::recording(1000, 1);
    let mut samples = vec![0.1f32; 120];
    samples[60] = -0.8;
    feed.push(&[&samples]);
    take.gather();
    assert_eq!(take.shape(), &[0.1, 0.8]);
}
