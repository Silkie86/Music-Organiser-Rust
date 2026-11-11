//! Favourites from Bitwig multisamples: finding a multisample's samples in the sound library. Made-up sounds and
//! multisamples, in the test build's own fixtures folder under fixed names, never a random temp one (rule 11).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use mo_app::bitwig_favourites::{self, Multisample, is_temp_file, look, missing_lines, multisamples, summary};
use mo_sounds::{Sound, SoundAnalysis};

fn folder(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-app-bitwig-favourites").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A made-up 16-bit mono sound, `seed` making each one different.
fn tone(seed: u32, frames: usize) -> Vec<f32> {
    (0..frames).map(|i| (((i as u32).wrapping_mul(seed) % 200) as f32 / 100.0 - 1.0) * 0.5).collect()
}

fn wav(path: &Path, samples: &[f32]) {
    mo_audio::write_wav(path, &[samples.to_vec()], 44100, 16).unwrap();
}

/// A library sound at `path`, analysed as the file is.
fn sound(id: i64, path: &Path, frames: usize) -> Sound {
    let mut s = Sound::new(id, path.to_str().unwrap());
    s.analysis = Some(SoundAnalysis {
        seconds: frames as f64 / 44100.0,
        sample_rate: 44100,
        channel_count: 1,
        ..SoundAnalysis::default()
    });
    s
}

/// A multisample at `path` holding `samples` (their names and bytes), as Bitwig saves one.
fn multisample(path: &Path, samples: &[(&str, Vec<u8>)]) -> Multisample {
    let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
    let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("multisample.xml", stored).unwrap();
    zip.write_all(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><multisample name=\"Test\"/>").unwrap();
    for (name, bytes) in samples {
        zip.start_file(*name, stored).unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap();
    Multisample { path: path.to_owned(), name: path.file_stem().unwrap().to_string_lossy().into_owned() }
}

#[test]
fn samples_are_found_by_their_file_or_their_audio_only() {
    let dir = folder("found");
    let samples = dir.join("samples");
    let temp = dir.join("temp");
    fs::create_dir_all(&samples).unwrap();
    fs::create_dir_all(&temp).unwrap();

    // The library: a snare, the same snare copied to another folder, a kick under a new name, a hat, and a sound
    // that only sounds like the clap (one sample differs).
    let snare = samples.join("Snare 1.wav");
    wav(&snare, &tone(7, 3000));
    fs::create_dir_all(samples.join("copy")).unwrap();
    let snare_copy = samples.join("copy").join("snare 1.WAV");
    fs::copy(&snare, &snare_copy).unwrap();
    let kick = samples.join("Big Kick renamed.wav");
    wav(&kick, &tone(11, 4000));
    let hat = samples.join("Hat.wav");
    wav(&hat, &tone(13, 2000));
    let mut clap_alike = tone(17, 2500);
    clap_alike[100] = 0.25;
    let alike = samples.join("Clap.wav");
    wav(&alike, &clap_alike);
    let library = vec![
        sound(1, &snare, 3000),
        sound(2, &snare_copy, 3000),
        sound(3, &kick, 4000),
        sound(4, &hat, 2000),
        sound(5, &alike, 2500),
    ];

    // The hat saved again with a note added after its audio: other bytes, the same audio.
    let mut hat_noted = fs::read(&hat).unwrap();
    hat_noted.extend_from_slice(b"LIST\x04\x00\x00\x00INFO");
    let riff = (hat_noted.len() - 8) as u32;
    hat_noted[4..8].copy_from_slice(&riff.to_le_bytes());
    let clap_path = dir.join("clap.wav");
    wav(&clap_path, &tone(17, 2500));
    let m = multisample(
        &dir.join("Drums Select.multisample"),
        &[
            ("Snare 1.wav", fs::read(&snare).unwrap()),
            ("Kick.wav", fs::read(&kick).unwrap()),
            ("Hat.wav", hat_noted),
            ("Clap.wav", fs::read(&clap_path).unwrap()),
        ],
    );
    let broken = dir.join("Broken.multisample");
    fs::write(&broken, b"not a zip").unwrap();
    let broken = Multisample { path: broken, name: "Broken".into() };

    let found = look(&[m, broken], &library, &temp, &AtomicBool::new(false), |_, _| {});
    // The snare (both copies by its own name), the kick by its bytes, the hat by its audio; never the look-alike clap.
    assert_eq!(found.sounds, vec![1, 2, 3, 4]);
    assert_eq!((found.multisamples, found.samples, found.samples_found), (1, 4, 3));
    assert_eq!(found.missing.len(), 1);
    assert_eq!(found.missing[0].sample, "Clap.wav");
    assert_eq!(found.unreadable.len(), 1);
    assert_eq!(missing_lines(&found)[0], "Drums Select: Clap.wav");
    assert!(
        summary(&found, 3)
            .starts_with("Found 3 of 4 samples in your sound library: 4 sounds now favourites (1 already were).")
    );
    // Every sample written to Temp went again.
    assert_eq!(fs::read_dir(&temp).unwrap().count(), 0);
    // Nothing in the multisample or the samples changed.
    assert_eq!(fs::read(&snare).unwrap(), fs::read(&snare_copy).unwrap());
}

#[test]
fn a_stopped_look_says_so() {
    let dir = folder("stopped");
    let m = multisample(&dir.join("A.multisample"), &[("a.wav", vec![0; 10])]);
    let found = look(&[m], &[], &dir, &AtomicBool::new(true), |_, _| {});
    assert!(found.stopped);
    assert_eq!(found.samples, 0);
}

#[test]
fn multisamples_are_listed_from_each_library_once() {
    let dir = folder("list");
    let one = dir.join("One").join("Library");
    let ms = one.join("Multisamples");
    fs::create_dir_all(ms.join("Drums")).unwrap();
    for name in ["b Snare.multisample", "Drums/A Kick.multisample", "Kick.multisample.backup", ".backup"] {
        fs::write(ms.join(name), b"").unwrap();
    }
    let found = multisamples(&[one.clone(), one, dir.join("gone")]);
    let names: Vec<&str> = found.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["A Kick", "b Snare"]);
    assert!(!is_temp_file("A Kick.multisample"));
    assert_eq!(bitwig_favourites::MULTISAMPLES, "Multisamples");
}
