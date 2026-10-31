//! Converting an FL project to a Bitwig project on disk, in the test build's own fixtures folder under fixed names:
//! the C# `Exporter` tests for `BitwigFromFl`, on the same made-up song and template.

// Test files may unwrap: a test that stops there has failed, which is what it should do.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[allow(unused_imports)]
#[path = "../../mo-formats/tests/common/mod.rs"]
mod common;

use std::path::{Path, PathBuf};

use common::bitwig::{Template, bitwig_template};
use mo_convert::Options;
use mo_convert::sample::read_sample;
use mo_convert::save::{bitwig_file, bitwig_from_fl, bitwig_name, can_save_over};

/// A folder of its own for one test, emptied first.
fn folder(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-convert").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn project(dir: &Path) -> PathBuf {
    let path = dir.join("Songs").join("civil_18.flp");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, common::fl_studio_25_automation(false, false)).unwrap();
    path
}

#[test]
fn converts_into_its_own_folder_and_never_touches_the_song_or_the_template() {
    let dir = folder("converts");
    let song = project(&dir);
    let before = std::fs::read(&song).unwrap();
    let template = bitwig_template(Template::default());
    let exports = dir.join("Exports");
    let saved = bitwig_from_fl(&song, &exports, &template, "civil_18", None, Options::default(), &|_| None).unwrap();
    assert_eq!(saved.file, exports.join("civil_18/Bitwig/civil_18/civil_18.bwproject"));
    let doc = mo_formats::bitwig::decode(std::fs::read(&saved.file).unwrap()).unwrap();
    assert_eq!(doc.body_error, None);
    assert_eq!(std::fs::read(&song).unwrap(), before, "the FL project is only read");
    assert_eq!(template, bitwig_template(Template::default()));
    // The snare's sample isn't on this computer: Bitwig will ask where it is, and that's said beside the project.
    let notes = std::fs::read_to_string(saved.file.with_file_name("civil_18 - not carried over.txt")).unwrap();
    assert!(notes.contains(r"Snare: couldn't read C:\Samples\Snare 01.wav"), "{notes}");
    // Nothing left over from saving.
    let left: Vec<_> =
        std::fs::read_dir(saved.file.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(left.len(), 2, "{left:?}");
}

#[test]
fn a_new_convert_isnt_saved_over_an_earlier_one_changed_in_bitwig() {
    let dir = folder("save over");
    let song = project(&dir);
    let exports = dir.join("Exports");
    let template = bitwig_template(Template::default());
    let file = bitwig_file(&song, &exports, "civil_18");
    assert!(can_save_over(&song, &file, None), "nothing there yet");
    let first = bitwig_from_fl(&song, &exports, &template, "civil_18", None, Options::default(), &|_| None).unwrap();
    // Its own earlier convert, unchanged: converting again saves over it.
    assert!(can_save_over(&song, &file, Some(&first.written)));
    let again =
        bitwig_from_fl(&song, &exports, &template, "civil_18", Some(&first.written), Options::default(), &|_| None)
            .unwrap();
    // One made before the app remembered it is kept.
    assert!(!can_save_over(&song, &file, None));
    assert!(bitwig_from_fl(&song, &exports, &template, "civil_18", None, Options::default(), &|_| None).is_err());
    // Another song with the same name, in another folder, doesn't count as its own.
    let other = dir.join("Other folder").join("civil_18.flp");
    std::fs::create_dir_all(other.parent().unwrap()).unwrap();
    std::fs::copy(&song, &other).unwrap();
    assert!(!can_save_over(&other, &bitwig_file(&other, &exports, "civil_18"), Some(&again.written)));

    // Saved in Bitwig since: kept, and the new convert gets the next name.
    let bytes = std::fs::read(&file).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(&file, &bytes).unwrap();
    assert!(!can_save_over(&song, &file, Some(&again.written)));
    let name = bitwig_name(&song, |n| !can_save_over(&song, &bitwig_file(&song, &exports, n), Some(&again.written)));
    assert_eq!(name, "civil_18 (2)");
    let second = bitwig_from_fl(&song, &exports, &template, &name, None, Options::default(), &|_| None).unwrap();
    // Each in its own folder named like it, the layout Bitwig opens without asking, next to the first.
    assert_eq!(second.file, exports.join("civil_18/Bitwig/civil_18 (2)/civil_18 (2).bwproject"));
    assert_eq!(std::fs::read(&file).unwrap(), bytes, "the one changed in Bitwig is kept");
}

#[test]
fn a_samples_md5_and_size_are_what_bitwig_checks() {
    use md5::{Digest, Md5};
    let dir = folder("sample");
    let path = dir.join("Kick.wav");
    let mut wav = b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\x44\xac\0\0\x88\x58\x01\0\x02\0\x10\0data".to_vec();
    wav.extend_from_slice(&8820u32.to_le_bytes());
    wav.resize(wav.len() + 8820, 0);
    std::fs::write(&path, &wav).unwrap();
    let s = read_sample(&path).unwrap();
    assert_eq!((s.bytes, s.rate, s.channels), (wav.len() as u64, 44100, 1));
    assert!((s.seconds - 0.1).abs() < 1e-9);
    let expected: [u8; 16] = Md5::digest(&wav).into();
    assert_eq!(s.md5, expected);
    std::fs::write(&path, b"not audio").unwrap();
    assert_eq!(read_sample(&path).unwrap().seconds, 0.0, "read, but its length is left for Bitwig");
    assert_eq!(read_sample(&dir.join("missing.wav")), None);
}

#[test]
fn a_sample_that_moved_is_found_and_used() {
    let dir = folder("moved sample");
    let song = project(&dir);
    let snare = dir.join("Songs").join("Samples").join("Snare 01.wav");
    std::fs::create_dir_all(snare.parent().unwrap()).unwrap();
    let mut wav = b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\x44\xac\0\0\x88\x58\x01\0\x02\0\x10\0data".to_vec();
    wav.extend_from_slice(&8820u32.to_le_bytes());
    wav.resize(wav.len() + 8820, 0);
    std::fs::write(&snare, &wav).unwrap();
    let found = snare.to_string_lossy().into_owned();
    let asked = std::sync::Mutex::new(Vec::new());
    let find = |s: &str| {
        asked.lock().unwrap().push(s.to_owned());
        s.ends_with("Snare 01.wav").then(|| found.clone())
    };
    let template = bitwig_template(Template::default());
    let saved =
        bitwig_from_fl(&song, &dir.join("Exports"), &template, "civil_18", None, Options::default(), &find).unwrap();
    assert_eq!(*asked.lock().unwrap(), [r"C:\Samples\Snare 01.wav"], "only missing samples are looked for, once");
    assert_eq!(saved.found, [format!(r"Snare: C:\Samples\Snare 01.wav wasn't there, so {found} is used")]);
    assert!(!saved.notes.iter().any(|n| n.contains("couldn't read")), "{:?}", saved.notes);
    let list = std::fs::read_to_string(saved.file.with_file_name("civil_18 - not carried over.txt")).unwrap();
    assert!(list.contains("Samples found somewhere else"), "{list}");
    // The Bitwig project points at where it was found.
    let bytes = std::fs::read(&saved.file).unwrap();
    assert!(bytes.windows(found.len()).any(|w| w == found.as_bytes()));
}
