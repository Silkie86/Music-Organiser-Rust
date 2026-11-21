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
use mo_convert::save::{bitwig_file, bitwig_from_fl, bitwig_name, can_save_over, is_left_behind};

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
    let found = snare(&dir);
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
    assert!(list.contains("found somewhere else and used"), "{list}");
    // The Bitwig project points at where it was found.
    let bytes = std::fs::read(&saved.file).unwrap();
    assert!(bytes.windows(found.len()).any(|w| w == found.as_bytes()));
}

/// The made-up song with its snare saved as one of FL Studio's own sounds instead: the path's text swapped in the
/// save's bytes (its length byte and the save's own length with it).
fn project_with_fl_sound(dir: &Path, sample: &str) -> PathBuf {
    let utf16 = |t: &str| -> Vec<u8> { t.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect() };
    let (was, now) = (utf16(r"C:\Samples\Snare 01.wav"), utf16(sample));
    assert!(now.len() < 128, "one length byte");
    let mut bytes = common::fl_studio_25_automation(false, false);
    let at = bytes.windows(was.len()).position(|w| w == was).unwrap();
    assert_eq!(usize::from(bytes[at - 1]), was.len());
    bytes[at - 1] = now.len() as u8;
    bytes.splice(at..at + was.len(), now.iter().copied());
    // "FLhd", 4 bytes of length, 6 of header, "FLdt", then the length of the rest.
    assert_eq!(&bytes[14..18], b"FLdt");
    let rest = (bytes.len() - 22) as u32;
    bytes[18..22].copy_from_slice(&rest.to_le_bytes());
    let path = dir.join("Songs").join("civil_18.flp");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, bytes).unwrap();
    path
}

/// A sound from mo-audio's fixtures: half a second of a tone, stereo, 22,050 Hz.
fn tone(name: &str) -> Vec<u8> {
    std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("../mo-audio/tests/fixtures").join(name)).unwrap()
}

/// A WAV as FL Studio keeps its own sounds: format 0x674F, a whole Ogg Vorbis stream in the data chunk.
fn ogg_in_wav() -> Vec<u8> {
    let ogg = tone("tone.ogg");
    let mut wav = b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0\x4f\x67\x02\0\x22\x56\0\0\x88\x58\x01\0\x04\0\x10\0data".to_vec();
    wav.extend_from_slice(&(ogg.len() as u32).to_le_bytes());
    wav.extend_from_slice(&ogg);
    wav
}

/// Whole-app review 19 Nov 2026, CN-1: FL Studio's own sounds ("%FLStudioFactoryData%…") are used from FL Studio's
/// folder, and never swapped for another file with the name.
#[test]
fn fl_studios_own_sound_is_used_from_fl_studios_folder_and_nowhere_else() {
    use mo_library::samples::SampleLocator;
    let dir = folder("fl own sound");
    let saved_as = r"%FLStudioFactoryData%\Data\Patches\Packs\Snare 01.wav";
    let song = project_with_fl_sound(&dir, saved_as);
    let packs =
        ["Image-Line", "FL Studio 2024", "Data", "Patches", "Packs"].iter().fold(dir.clone(), |p, part| p.join(part));
    std::fs::create_dir_all(&packs).unwrap();
    // A file with the same name beside the song: another sound, never used for FL Studio's own.
    let beside = snare(&dir);
    let content = packs.to_str().unwrap().to_owned();
    let locator = SampleLocator::new(Box::new(move |_| vec![content.clone()]));
    let at = song.to_str().unwrap().to_owned();
    let find = |s: &str| locator.locate_to_use(s, &at);
    let template = bitwig_template(Template::default());
    let convert = |name: &str| {
        bitwig_from_fl(&song, &dir.join("Exports"), &template, name, None, Options::default(), &find).unwrap()
    };

    // Not in FL Studio's folder: said, and the file beside the song is left alone.
    let missing = convert("missing");
    assert!(missing.found.is_empty(), "{:?}", missing.found);
    assert!(missing.notes.iter().any(|n| n.contains("couldn't read %FLStudioFactoryData%")), "{:?}", missing.notes);
    let bytes = std::fs::read(&missing.file).unwrap();
    assert!(!bytes.windows(beside.len()).any(|w| w == beside.as_bytes()), "never the same-named file");

    // In FL Studio's folder, as FL Studio keeps it (Ogg Vorbis inside a WAV): used from there and timed.
    let own = packs.join("Snare 01.wav");
    std::fs::write(&own, ogg_in_wav()).unwrap();
    let own = own.to_str().unwrap().to_owned();
    let found = convert("found");
    assert!(found.found.is_empty(), "FL Studio's own sound in its own folder isn't listed as moved");
    assert!(!found.notes.iter().any(|n| n.contains("couldn't")), "{:?}", found.notes);
    assert!(
        found.notes.iter().any(|n| n.contains("packed inside a WAV") && n.contains("Snare 01.wav")),
        "{:?}",
        found.notes
    );
    let bytes = std::fs::read(&found.file).unwrap();
    assert!(bytes.windows(own.len()).any(|w| w == own.as_bytes()), "the project points at FL Studio's file");
    assert!(!bytes.windows(saved_as.len()).any(|w| w == saved_as.as_bytes()), "not at FL Studio's placeholder");
}

/// CN-1 and CN-10: a sample whose header doesn't say how long it is (MP3, OGG, M4A, compressed WAVs) is timed by its
/// decoder, so the Sampler gets its real end and Bitwig its MD5.
#[test]
fn compressed_samples_are_timed_by_their_decoder() {
    use md5::{Digest, Md5};
    let dir = folder("compressed samples");
    let files = [
        ("tone.mp3", tone("tone.mp3"), false),
        ("tone.ogg", tone("tone.ogg"), false),
        ("tone.m4a", tone("tone.m4a"), false),
        ("tone-ms-adpcm.wav", tone("tone-ms-adpcm.wav"), false),
        ("fl-own.wav", ogg_in_wav(), true),
    ];
    for (name, bytes, inside) in files {
        let path = dir.join(name);
        std::fs::write(&path, &bytes).unwrap();
        let s = read_sample(&path).unwrap();
        assert_eq!((s.rate, s.channels, s.ogg_in_wav), (22050, 2, inside), "{name}");
        // MP3 and AAC pad their ends a little.
        assert!((s.seconds - 0.5).abs() < 0.06, "{name}: {}", s.seconds);
        let expected: [u8; 16] = Md5::digest(&bytes).into();
        assert_eq!((s.bytes, s.md5), (bytes.len() as u64, expected), "{name}");
    }
    // A sound as long as the start that's decoded to ask (FL Studio's drum hits are short), as an OGG and as FL Studio
    // keeps its own: timed, not mistaken for a file that didn't say.
    let own = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures");
    let short_ogg = std::fs::read(own.join("tone-short.ogg")).unwrap();
    let mut short_wav = ogg_in_wav();
    short_wav.truncate(short_wav.len() - tone("tone.ogg").len() - 4);
    short_wav.extend_from_slice(&(short_ogg.len() as u32).to_le_bytes());
    short_wav.extend_from_slice(&short_ogg);
    for (name, bytes) in [("short.ogg", &short_ogg), ("short.wav", &short_wav)] {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        let s = read_sample(&path).unwrap();
        assert!((s.seconds - 0.24).abs() < 1e-6, "{name}: {}", s.seconds);
    }
    // An MP3 with no length written in it is left for Bitwig: a decoder can only guess from its first frames and its
    // size (far off for this one, a tone and then silence).
    let s = read_sample(&own.join("tone-no-length.mp3")).unwrap();
    assert_eq!((s.seconds, s.md5), (0.0, [0; 16]));
    // A WAV of plain samples that was never finished isn't handed to a decoder: its header is all there is.
    let unfinished = dir.join("unfinished.wav");
    let mut wav = b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\x44\xac\0\0\x88\x58\x01\0\x02\0\x10\0".to_vec();
    wav.resize(4096, 0);
    std::fs::write(&unfinished, &wav).unwrap();
    assert_eq!(read_sample(&unfinished).unwrap().seconds, 0.0);
    // One the decoder can't read stays as before: read, its length left for Bitwig.
    let path = dir.join("broken.mp3");
    std::fs::write(&path, b"not audio").unwrap();
    let s = read_sample(&path).unwrap();
    assert_eq!((s.seconds, s.md5), (0.0, [0; 16]));
}

/// Whole-app review 19 Nov 2026, CN-5 and CN-6: the FL song's time signature and its playlist's markers are the
/// Bitwig project's, read from FL recipe save 16 (3/4, a marker "Verse").
#[test]
fn the_fl_songs_time_signature_and_markers_come_across() {
    let dir = folder("signature and markers");
    let recipe = Path::new(env!("CARGO_MANIFEST_DIR")).join("../mo-formats/tests/fixtures/recipe/fl");
    let song = dir.join("Songs").join("recipe 16.flp");
    std::fs::create_dir_all(song.parent().unwrap()).unwrap();
    std::fs::copy(recipe.join("MO Recipe 16 time signature and marker.flp"), &song).unwrap();
    let fl = &mo_formats::arrangement::read(&song).unwrap()[0];
    assert_eq!((fl.signature.top(), fl.signature.bottom()), (3, 4));
    assert!(!fl.markers.is_empty());
    let template = bitwig_template(Template { as_6_1_saves: true, ..Template::default() });
    let saved =
        bitwig_from_fl(&song, &dir.join("Exports"), &template, "recipe 16", None, Options::default(), &|_| None)
            .unwrap();
    let bitwig = &mo_formats::arrangement::read(&saved.file).unwrap()[0];
    assert_eq!(bitwig.signature, fl.signature);
    assert_eq!(bitwig.markers, fl.markers, "FL's markers, and not the template's own");
    // The song starts in 3/4 and never changes, so nothing is said about a change.
    assert!(!saved.notes.iter().any(|n| n.contains("changes time signature")), "{:?}", saved.notes);
}

/// A short WAV, as the snare the song plays.
fn snare(dir: &Path) -> String {
    let snare = dir.join("Songs").join("Samples").join("Snare 01.wav");
    std::fs::create_dir_all(snare.parent().unwrap()).unwrap();
    let mut wav = b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\x44\xac\0\0\x88\x58\x01\0\x02\0\x10\0data".to_vec();
    wav.extend_from_slice(&8820u32.to_le_bytes());
    wav.resize(wav.len() + 8820, 0);
    std::fs::write(&snare, &wav).unwrap();
    snare.to_string_lossy().into_owned()
}

#[test]
fn an_earlier_convert_saved_in_bitwig_while_samples_are_read_keeps_its_own_list() {
    let dir = folder("kept list");
    let song = project(&dir);
    let found = snare(&dir);
    let exports = dir.join("Exports");
    let template = bitwig_template(Template::default());
    let find = |s: &str| s.ends_with("Snare 01.wav").then(|| found.clone());
    let first = bitwig_from_fl(&song, &exports, &template, "civil_18", None, Options::default(), &find).unwrap();
    let list_file = first.file.with_file_name("civil_18 - not carried over.txt");
    let list = std::fs::read(&list_file).unwrap();
    // Converted again, this time with the snare not found; while that's looked for, the first is saved in Bitwig.
    let project = std::fs::read(&first.file).unwrap();
    let saved_in_bitwig = |_: &str| {
        std::fs::write(&first.file, &project).unwrap();
        // A time of its own, so a disk that keeps whole seconds sees the change too.
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options().write(true).open(&first.file).unwrap().set_modified(later).unwrap();
        None
    };
    let again = bitwig_from_fl(
        &song,
        &exports,
        &template,
        "civil_18",
        Some(&first.written),
        Options::default(),
        &saved_in_bitwig,
    );
    assert!(again.unwrap_err().contains("may hold work"));
    assert_eq!(std::fs::read(&list_file).unwrap(), list, "the kept project's list is its own");
    let left: Vec<_> =
        std::fs::read_dir(first.file.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(left.len(), 2, "nothing half-written left: {left:?}");
}

#[test]
fn a_half_written_file_an_earlier_save_left_is_cleared_and_nothing_else() {
    let dir = folder("left behind");
    let song = project(&dir);
    let exports = dir.join("Exports");
    let folder = bitwig_file(&song, &exports, "civil_18").parent().unwrap().to_owned();
    std::fs::create_dir_all(&folder).unwrap();
    let old = folder.join("civil_18.bwproject.12-3.saving");
    let fresh = folder.join("civil_18.bwproject.12-4.saving");
    let other = folder.join("Other.bwproject.12-3.saving");
    let yours = folder.join("civil_18.bwproject.saving");
    for f in [&old, &fresh, &other, &yours] {
        std::fs::write(f, b"x").unwrap();
    }
    let hour_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    for f in [&old, &other, &yours] {
        std::fs::File::options().write(true).open(f).unwrap().set_modified(hour_ago).unwrap();
    }
    let template = bitwig_template(Template::default());
    bitwig_from_fl(&song, &exports, &template, "civil_18", None, Options::default(), &|_| None).unwrap();
    // Only the old one of this file's own shape goes: one still being written, and other files, stay.
    assert!(!old.exists() && fresh.exists() && other.exists() && yours.exists());
    assert!(is_left_behind("a.flp.1-2.saving", "a.flp"));
    assert!(!is_left_behind("a.flp.1-.saving", "a.flp") && !is_left_behind("a.flp.1-2.saving.x", "a.flp"));
    assert!(!is_left_behind("b.flp.1-2.saving", "a.flp") && !is_left_behind("xa.flp.1-2.saving", "a.flp"));
}
