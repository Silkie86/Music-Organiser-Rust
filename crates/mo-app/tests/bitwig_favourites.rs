//! Favourites from Bitwig multisamples: finding a multisample's samples in the sound library. Made-up sounds and
//! multisamples, in the test build's own fixtures folder under fixed names, never a random temp one (rule 11).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use mo_app::bitwig_favourites::{
    self, Marked, Multisample, Why, is_temp_file, look, missing_lines, multisamples, save_missing, sounds_at, summary,
};
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
    // A sample Bitwig recorded from an instrument: only inside the multisample.
    wav(&dir.join("recorded.wav"), &tone(19, 2200));
    let m = multisample(
        &dir.join("Drums Select.multisample"),
        &[
            ("Snare 1.wav", fs::read(&snare).unwrap()),
            ("Kick.wav", fs::read(&kick).unwrap()),
            ("Hat.wav", hat_noted),
            ("Clap.wav", fs::read(&clap_path).unwrap()),
            ("Snare 036 - c1.wav", fs::read(dir.join("recorded.wav")).unwrap()),
        ],
    );
    let broken = dir.join("Broken.multisample");
    fs::write(&broken, b"not a zip").unwrap();
    let broken = Multisample { path: broken, name: "Broken".into() };

    let found = look(&[m, broken], &library, &temp, &AtomicBool::new(false), |_, _| {});
    // The snare (both copies by its own name), the kick by its bytes, the hat by its audio; never the look-alike clap.
    assert_eq!(found.sounds, vec![1, 2, 3, 4]);
    // "Drums Select" names no type, so none is given.
    assert_eq!(found.types, vec![]);
    assert_eq!((found.multisamples, found.samples, found.samples_found), (1, 5, 3));
    assert_eq!(found.missing.len(), 2);
    assert_eq!(found.missing[0].sample, "Clap.wav");
    assert_eq!(found.unreadable.len(), 1);
    // A sound named Clap.wav is in the library, but its audio is different.
    assert_eq!(found.missing[0].why, Why::OtherAudio);
    assert_eq!(missing_lines(&found)[0], "Drums Select: Clap.wav (same name, different audio)");
    // No sound has the recorded one's name, nor its audio.
    assert_eq!((found.missing[1].sample.as_str(), found.missing[1].why), ("Snare 036 - c1.wav", Why::NoSuchName));
    assert!(
        summary(&found, Marked { newly: 3, already: 1, typed: 0 })
            .starts_with("Found 3 of 5 samples in your sound library: 4 sounds now favourites (1 already were).")
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

#[test]
fn samples_not_found_are_saved_into_from_bitwig_and_never_over_a_file() {
    let dir = folder("saved");
    let samples = dir.join("samples");
    let temp = dir.join("temp");
    fs::create_dir_all(&samples).unwrap();
    fs::create_dir_all(&temp).unwrap();
    // The library has a Clap.wav, but not this one's audio; nothing has the recorded snare's name or audio.
    let clap = samples.join("Clap.wav");
    wav(&clap, &tone(17, 2500));
    let library = vec![sound(1, &clap, 2500)];
    let other_clap = dir.join("other clap.wav");
    wav(&other_clap, &tone(23, 2500));
    let recorded = dir.join("recorded.wav");
    wav(&recorded, &tone(19, 2200));
    let m = multisample(
        &dir.join("Snare 2..multisample"),
        &[("Clap.wav", fs::read(&other_clap).unwrap()), ("Snare 036 - c1.wav", fs::read(&recorded).unwrap())],
    );
    let zip_before = fs::read(&m.path).unwrap();
    let found = look(std::slice::from_ref(&m), &library, &temp, &AtomicBool::new(false), |_, _| {});
    assert_eq!(found.missing.len(), 2);

    let saved = save_missing(&found.missing, &samples, &AtomicBool::new(false));
    // In the sample folder › From Bitwig › the multisample's name (its dot at the end left out, as Windows does).
    let into = samples.join(bitwig_favourites::FROM_BITWIG).join("Snare 2");
    assert_eq!(saved.paths, vec![into.join("Clap.wav"), into.join("Snare 036 - c1.wav")]);
    assert_eq!((saved.written, saved.there, saved.failed.len()), (2, 0, 0));
    assert_eq!(saved.types, vec![Some("Snare"), Some("Snare")]);
    assert_eq!(fs::read(&saved.paths[0]).unwrap(), fs::read(&other_clap).unwrap());
    assert_eq!(fs::read(&saved.paths[1]).unwrap(), fs::read(&recorded).unwrap());
    // Nothing half-written is left, and the multisample is as it was.
    assert_eq!(fs::read_dir(&into).unwrap().count(), 2);
    assert_eq!(fs::read(&m.path).unwrap(), zip_before);
    assert!(summary_of(&saved, &samples).starts_with("Saved 2 samples into "));

    // Run again: the same files are there, nothing is written.
    let again = save_missing(&found.missing, &samples, &AtomicBool::new(false));
    assert_eq!((again.written, again.there, again.paths.clone()), (0, 2, saved.paths.clone()));

    // A file of its name with other bytes is never written over: the sample gets " (2)".
    fs::write(into.join("Clap.wav"), b"mine").unwrap();
    let beside = save_missing(&found.missing[..1], &samples, &AtomicBool::new(false));
    assert_eq!(beside.paths, vec![into.join("Clap (2).wav")]);
    assert_eq!(fs::read(into.join("Clap.wav")).unwrap(), b"mine");

    // Once scanned, the saved files are found among the sounds by their paths, either slash and letter case aside.
    let scanned = vec![
        sound(7, &into.join("clap (2).WAV"), 2500),
        sound(8, &into.join("Snare 036 - c1.wav"), 2200),
        sound(9, &clap, 2500),
    ];
    let gone = into.join("Not scanned yet.wav");
    assert_eq!(
        sounds_at(&[into.join("Clap (2).wav"), gone, saved.paths[1].clone()], &scanned),
        [Some(7), None, Some(8)]
    );
}

fn summary_of(saved: &bitwig_favourites::Saved, into: &Path) -> String {
    bitwig_favourites::saved_summary(saved, into)
}

#[test]
fn a_sample_bitwig_padded_with_silence_is_found_under_its_own_name() {
    let dir = folder("padded");
    let samples = dir.join("samples");
    let temp = dir.join("temp");
    fs::create_dir_all(&samples).unwrap();
    fs::create_dir_all(&temp).unwrap();
    // The library's snare and tom, as in his sample folder.
    let snare = samples.join("Snare 036 - c1.wav");
    wav(&snare, &tone(29, 1500));
    let tom = samples.join("Tom.wav");
    wav(&tom, &tone(31, 1500));
    let library = vec![sound(1, &snare, 1500), sound(2, &tom, 1500)];
    // Bitwig's copies: silence before and after. The tom's last sample is different, so it isn't the same sound.
    let pad = |mut audio: Vec<f32>| {
        let mut out = vec![0.0; 900];
        out.append(&mut audio);
        out.extend(std::iter::repeat_n(0.0, 4000));
        out
    };
    let padded_snare = dir.join("padded snare.wav");
    wav(&padded_snare, &pad(tone(29, 1500)));
    let mut tom_changed = tone(31, 1500);
    tom_changed[1499] = 0.125;
    let padded_tom = dir.join("padded tom.wav");
    wav(&padded_tom, &pad(tom_changed));
    let m = multisample(
        &dir.join("Stylus.multisample"),
        &[("Snare 036 - c1.wav", fs::read(&padded_snare).unwrap()), ("Tom.wav", fs::read(&padded_tom).unwrap())],
    );
    let found = look(&[m], &library, &temp, &AtomicBool::new(false), |_, _| {});
    assert_eq!(found.sounds, vec![1]);
    assert_eq!(found.types, vec![]);
    assert_eq!((found.samples_found, found.samples_unpadded), (1, 1));
    assert_eq!(found.missing.len(), 1);
    assert_eq!((found.missing[0].sample.as_str(), found.missing[0].why), ("Tom.wav", Why::OtherAudio));
    let said = summary(&found, Marked { newly: 1, already: 0, typed: 0 });
    assert!(
        said.contains(" 1 was the same sound once the silence Bitwig added before and after it was left out."),
        "{said}"
    );
    assert_eq!(fs::read_dir(&temp).unwrap().count(), 0);
}
