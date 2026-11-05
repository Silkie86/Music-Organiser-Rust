//! Finding sounds whose audio is exactly the same. The sounds are made in the test build's own fixtures folder, under
//! a fixed name, never a random temp one.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use mo_sounds::copies::{self, Finding, SameAudio};
use mo_sounds::{Sound, SoundAnalysis};

fn folder(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A sound as the library lists it, analysed as `seconds` long at `rate` with `peak_db`: only these and the path
/// matter here.
fn sound(id: i64, path: &Path, seconds: f64, rate: i32, peak_db: f64) -> Sound {
    let mut s = Sound::new(id, &path.to_string_lossy());
    s.analysis =
        Some(SoundAnalysis { seconds, sample_rate: rate, channel_count: 2, peak_db, ..SoundAnalysis::default() });
    s
}

fn tone(frames: usize, step: f32) -> Vec<f32> {
    (0..frames).map(|i| 0.5 * (i as f32 * step).sin()).collect()
}

fn find(sounds: &[Sound], kept: &Path) -> (SameAudio, Vec<Finding>) {
    let mut said = Vec::new();
    let same = copies::find(sounds, kept, &AtomicBool::new(false), |f| said.push(f)).unwrap();
    (same, said)
}

#[test]
fn only_the_very_same_audio_counts_and_unchanged_files_are_not_read_again() {
    let dir = folder("mo-sounds-copies");
    let kept = dir.join(copies::FILE_NAME);
    let left = tone(4410, 0.05);
    let a = dir.join("Kick.wav");
    mo_audio::write_wav(&a, &[left.clone(), left.clone()], 44_100, 16).unwrap();
    // The very same file elsewhere, under another name.
    fs::create_dir_all(dir.join("Pack")).unwrap();
    let b = dir.join("Pack").join("Kick copy.wav");
    fs::copy(&a, &b).unwrap();
    // The same audio saved at 24 bits: the very same samples once decoded (16-bit values fit 24 bits exactly).
    let c = dir.join("Kick 24.wav");
    let decoded = mo_audio::read(&a, 60.0).unwrap();
    mo_audio::write_wav(&c, &decoded.channels, 44_100, 24).unwrap();
    // One sample different: not a copy, though it sounds the same.
    let mut nudged = decoded.channels.clone();
    nudged[1][2000] += 1.0 / 32768.0;
    let d = dir.join("Kick nudged.wav");
    mo_audio::write_wav(&d, &nudged, 44_100, 16).unwrap();
    // The same at another sample rate's header: not the same audio.
    let e = dir.join("Kick 48.wav");
    mo_audio::write_wav(&e, &decoded.channels, 48_000, 16).unwrap();
    // Another sound with nothing alike: never read.
    let f = dir.join("Snare.wav");
    mo_audio::write_wav(&f, &[tone(2000, 0.3), tone(2000, 0.3)], 44_100, 16).unwrap();

    let sounds = [
        sound(1, &a, 0.1, 44_100, -6.02),
        sound(2, &b, 0.1, 44_100, -6.02),
        sound(3, &c, 0.1, 44_100, -6.02),
        sound(4, &d, 0.1, 44_100, -6.02),
        sound(5, &e, 0.1, 44_100, -6.02),
        sound(6, &f, 0.045, 44_100, -6.02),
    ];
    assert_eq!(copies::candidates(&sounds), [0, 1, 2, 3, 4]);
    let (same, said) = find(&sounds, &kept);
    assert_eq!(same, SameAudio { groups: vec![vec![1, 2, 3]] });
    assert_eq!(same.copies(), 2);
    assert_eq!(said.first(), Some(&Finding::Known { same: SameAudio::default(), left: 5 }));
    assert_eq!(said.last(), Some(&Finding::Reading { done: 5, of: 5 }));

    // Nothing changed: all known at once, nothing read.
    let (again, said) = find(&sounds, &kept);
    assert_eq!(again, same);
    assert_eq!(said, [Finding::Known { same: same.clone(), left: 0 }]);

    // A changed file is read again: the nudged one made the same as the rest.
    fs::copy(&a, &d).unwrap();
    let (changed, said) = find(&sounds, &kept);
    assert_eq!(changed, SameAudio { groups: vec![vec![1, 2, 3, 4]] });
    assert!(matches!(said.first(), Some(Finding::Known { left: 1, .. })), "{said:?}");

    // A sound no longer a candidate is forgotten; one whose file is gone isn't a copy.
    fs::remove_file(&b).unwrap();
    let (gone, _) = find(&sounds[..3], &kept);
    assert_eq!(gone, SameAudio { groups: vec![vec![1, 3]] });
}

#[test]
fn a_damaged_or_foreign_file_where_the_prints_are_kept_is_made_afresh() {
    let dir = folder("mo-sounds-copies-damaged");
    let kept = dir.join(copies::FILE_NAME);
    fs::write(&kept, b"not a database at all, just some words that are long enough to be a page").unwrap();
    let a = dir.join("Hat.wav");
    mo_audio::write_wav(&a, &[tone(800, 0.7)], 22_050, 16).unwrap();
    let b = dir.join("Hat 2.wav");
    fs::copy(&a, &b).unwrap();
    let mut sounds = [sound(7, &a, 0.036, 22_050, -6.0), sound(8, &b, 0.036, 22_050, -6.0)];
    for s in &mut sounds {
        s.analysis.as_mut().unwrap().channel_count = 1;
    }
    let (same, _) = find(&sounds, &kept);
    assert_eq!(same, SameAudio { groups: vec![vec![7, 8]] });
    let (_, said) = find(&sounds, &kept);
    assert!(matches!(said.first(), Some(Finding::Known { left: 0, .. })), "{said:?}");
}

#[test]
fn files_the_app_cant_decode_are_copies_only_byte_for_byte() {
    let dir = folder("mo-sounds-copies-bytes");
    let a = dir.join("odd.mp3");
    fs::write(&a, b"not really an mp3").unwrap();
    let b = dir.join("odd copy.mp3");
    fs::copy(&a, &b).unwrap();
    let c = dir.join("odd other.mp3");
    fs::write(&c, b"not really an mp3!").unwrap();
    assert_eq!(copies::print_of(&a).unwrap(), copies::print_of(&b).unwrap());
    assert_ne!(copies::print_of(&a).unwrap(), copies::print_of(&c).unwrap());
    assert!(copies::print_of(&dir.join("not there.wav")).is_err());
}

/// A mono 32-bit whole-number PCM WAV holding `samples`.
fn wav32(path: &Path, samples: &[i32]) {
    let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&1u16.to_le_bytes()); // mono
    b.extend_from_slice(&44_100u32.to_le_bytes());
    b.extend_from_slice(&(44_100u32 * 4).to_le_bytes());
    b.extend_from_slice(&4u16.to_le_bytes());
    b.extend_from_slice(&32u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(data.len() as u32).to_le_bytes());
    b.extend_from_slice(&data);
    fs::write(path, b).unwrap();
}

#[test]
fn audio_read_as_floats_that_lose_bits_is_a_copy_only_byte_for_byte() {
    let dir = folder("mo-sounds-copies-32bit");
    let samples: Vec<i32> = (0..1000).map(|i| (i * 1_000_003) << 8).collect();
    let mut nudged = samples.clone();
    // A difference in the last bit: lost when read as 32-bit floats, so the two read the same.
    nudged[500] += 1;
    let (a, b) = (dir.join("deep.wav"), dir.join("deep nudged.wav"));
    wav32(&a, &samples);
    wav32(&b, &nudged);
    assert_eq!(mo_audio::read(&a, 60.0).unwrap(), mo_audio::read(&b, 60.0).unwrap());
    assert!(!mo_audio::reads_exactly(&a));
    assert_ne!(copies::print_of(&a).unwrap(), copies::print_of(&b).unwrap());
    let c = dir.join("deep copy.wav");
    fs::copy(&a, &c).unwrap();
    assert_eq!(copies::print_of(&a).unwrap(), copies::print_of(&c).unwrap());
    // A 16-bit file reads exactly.
    let d = dir.join("plain.wav");
    mo_audio::write_wav(&d, &[tone(100, 0.1)], 44_100, 16).unwrap();
    assert!(mo_audio::reads_exactly(&d));
}

#[test]
fn a_long_wav_is_printed_whole_a_part_at_a_time() {
    let dir = folder("mo-sounds-copies-long");
    // 70 seconds at 8 kHz: three parts. Two files that differ only in their last second.
    let frames = 70 * 8000;
    let one = tone(frames, 0.01);
    let mut two = one.clone();
    two[frames - 100] = 0.0;
    let (a, b) = (dir.join("long.wav"), dir.join("long other end.wav"));
    mo_audio::write_wav(&a, &[one], 8000, 16).unwrap();
    mo_audio::write_wav(&b, &[two], 8000, 16).unwrap();
    assert_ne!(copies::print_of(&a).unwrap(), copies::print_of(&b).unwrap());
}

#[test]
fn another_apps_database_where_the_prints_are_kept_is_left_alone() {
    let dir = folder("mo-sounds-copies-foreign");
    let kept = dir.join(copies::FILE_NAME);
    {
        let other = rusqlite::Connection::open(&kept).unwrap();
        other.execute_batch("PRAGMA application_id = 1234; CREATE TABLE theirs (x);").unwrap();
    }
    let a = dir.join("Hat.wav");
    mo_audio::write_wav(&a, &[tone(800, 0.7)], 22_050, 16).unwrap();
    let sounds = [sound(1, &a, 0.036, 22_050, -6.0), sound(2, &a, 0.036, 22_050, -6.0)];
    assert!(copies::find(&sounds, &kept, &AtomicBool::new(false), |_| {}).is_err());
    let other = rusqlite::Connection::open(&kept).unwrap();
    let n: i64 = other.query_row("SELECT COUNT(*) FROM theirs", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0);
}
