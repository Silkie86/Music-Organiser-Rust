//! Reading Bitwig projects: the C# app's `BitwigTests`, on the same made-up files.

mod common;

use std::collections::HashMap;
use std::path::Path;

use common::bitwig::*;
use common::test_dir;
use mo_formats::bitwig::{self, Project};
use mo_formats::{Confidence, NoteEvent, PluginKind};

fn decoded(path: &Path) -> Project {
    Project::open(std::fs::read(path).expect("read the file")).unwrap_or_else(|_| panic!("not a Bitwig file"))
}

fn note(start: i64, length: i64, key: u8, velocity: u8) -> NoteEvent {
    NoteEvent { start, length, key, velocity, muted: false, step: false }
}

/// The samples a test expects, in the order the C# tests compare them (ordinal).
fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

#[test]
fn reads_tempo_version_plugins_and_hardware() {
    let path = bitwig_project(&test_dir("bitwig-read-plugins"), "Loop Idea");
    let info = bitwig::read(&path).expect("read");
    assert_eq!(info.confidence, Confidence::Partial);
    assert!(info.warnings.is_empty());
    assert_eq!(info.version, "5.2.4");
    assert_eq!(info.tempo, Some(128.5));
    let plugins: HashMap<&str, _> = info.plugins.iter().map(|p| (p.name.as_str(), p)).collect();
    let keyscape = plugins["Keyscape"];
    assert_eq!(
        (keyscape.vendor.as_str(), keyscape.format.as_str(), keyscape.kind),
        ("Spectrasonics", "VST3", PluginKind::Instrument)
    );
    assert!(keyscape.path.ends_with("Keyscape.vst3"));
    assert_eq!(plugins["FabFilter Pro-L 2"].kind, PluginKind::Effect);
    assert_eq!((plugins["Delay+"].vendor.as_str(), plugins["Delay+"].format.as_str()), ("Bitwig", "Native"));
    assert_eq!(info.details["Hardware I/O"], "Elektron Analog Rytm MKII");
    assert_eq!(info.hardware, ["Elektron Analog Rytm MKII"]);
    assert_eq!(info.details["Notes"], "3");
    assert_eq!(info.details["Stored plugin states"], "2");
    assert!(info.can_extract_midi && info.can_extract_presets);
}

#[test]
fn uses_bitwigs_display_names_for_tracks() {
    let path = bitwig_project(&test_dir("bitwig-read-tracks"), "Loop Idea");
    assert_eq!(bitwig::read(&path).expect("read").tracks, ["Keys Display", "FX 1"]);
}

#[test]
fn finds_samples() {
    let path = bitwig_project(&test_dir("bitwig-read-samples"), "Loop Idea");
    let samples = bitwig::read(&path).expect("read").samples;
    assert!(samples.contains(&r"D:\Splice\Snare 808.wav".to_owned()));
    for end in ["Kick 01.wav", "Pad.multisample", "Recorded Bass.wav"] {
        assert!(samples.iter().any(|s| s.ends_with(end)), "{end} in {samples:?}");
    }
}

#[test]
fn extracts_note_clips() {
    let path = bitwig_project(&test_dir("bitwig-read-midi"), "Loop Idea");
    let clips = decoded(&path).midi();
    assert_eq!(clips.len(), 1);
    let mut clip = clips[0].clone();
    assert_eq!((clip.name.as_str(), clip.ppq), ("Keys Display 1", 960));
    assert_eq!(*clip.track("Keys Display"), [note(0, 960, 60, 64), note(1440, 480, 60, 127), note(1920, 960, 64, 102)]);
}

#[test]
fn extracts_plugin_states_named_by_track() {
    let path = bitwig_project(&test_dir("bitwig-read-presets"), "Loop Idea");
    let presets = decoded(&path).presets();
    let by_plugin: HashMap<&str, _> = presets.iter().map(|p| (p.plugin.as_str(), p)).collect();
    let keys = by_plugin["Keyscape"];
    assert_eq!((keys.name.as_str(), &*keys.extension), ("Keys Display - Keyscape", ".vstpreset"));
    assert_eq!(keys.data, b"VST3 keys state");
    assert_eq!(by_plugin["FabFilter Pro-L 2"].name, "Master - FabFilter Pro-L 2");
}

#[test]
fn falls_back_to_a_text_scan_when_the_file_cant_be_decoded() {
    let path = bitwig_undecodable(&test_dir("bitwig-read-scan"), "Loop Idea");
    let info = bitwig::read(&path).expect("read");
    assert_eq!(info.confidence, Confidence::Heuristic);
    assert_eq!(info.version, "5.2.4");
    let names: Vec<&str> = info.plugins.iter().map(|p| p.name.as_str()).collect();
    for name in ["Polymer", "Delay+", "Valhalla Room", "Analog Rytm MKII"] {
        assert!(names.contains(&name), "{name} in {names:?}");
    }
    // Not installed, so plain text isn't trusted as a plugin.
    assert!(!names.contains(&"Serum"));
    // From the compressed section.
    assert!(info.samples.contains(&r"D:\Splice\Snare 808.wav".to_owned()));
    assert!(info.samples.iter().any(|s| s.ends_with("Recorded Bass.wav")));
    // What the text scan reads next to a path doesn't stick to it, and bare names aren't files.
    assert!(info.samples.contains(&r"F:\Bitwig Studio\Projects\Loop Idea\recordings\Syntakt 2-1.wav".to_owned()));
    let recorded = path.parent().expect("a folder").join("recordings/Untitled Project [0m27s-1m22s].wav");
    assert!(info.samples.contains(&recorded.to_string_lossy().into_owned()), "{:?}", info.samples);
    assert!(!info.samples.iter().any(|s| s.contains("BASSDRUM")));
    assert!(info.samples.iter().all(|s| s.starts_with("D:") || s.starts_with("F:") || Path::new(s).is_absolute()));
    assert!(!info.can_extract_midi);
    assert!(decoded(&path).midi().is_empty());

    let project = decoded(&path);
    let known = bitwig::read_project(Some(&project), &project.doc.raw, &path, &|s| s.eq_ignore_ascii_case("Serum"));
    assert!(known.plugins.iter().any(|p| p.name == "Serum"));
}

#[test]
fn the_text_scan_reads_network_and_accented_paths_whole_and_counts_collected_files_once() {
    // The metadata reads but the body doesn't (a value of a kind this app doesn't know, as 6.1's lists of doubles
    // were).
    let [accented, network, cyrillic, japanese] = SCANNED_PATHS;
    let dir = test_dir("bitwig-read-scan-paths").join("Song");
    std::fs::create_dir_all(&dir).expect("make the folder");
    let path = dir.join("Song.bwproject");
    let bytes = bitwig_scan_paths_bytes();
    std::fs::write(&path, bytes).expect("write");

    let info = bitwig::read(&path).expect("read");
    assert_eq!(info.confidence, Confidence::Heuristic);
    // The recording collected into the project counts once, as its copy there, not again where it was recorded.
    let copy = dir.join("recordings").join("BD 1-2.wav").to_string_lossy().into_owned();
    let expected = [copy.as_str(), network, r"F:\Samples\Drums\Kick 01.wav", accented, cyrillic, japanese];
    assert_eq!(sorted(info.samples), sorted(expected.map(String::from).to_vec()));
}

#[test]
fn a_file_collected_into_the_project_hides_only_its_own_original() {
    for decodable in [true, false] {
        // Kick-01 and Snare (as "Snare-2", the name being taken) were collected into the project; Kick-02 from the
        // same pack wasn't, so it's still listed where it is.
        let dir = test_dir(&format!("bitwig-read-collected-{decodable}")).join("Song");
        std::fs::create_dir_all(&dir).expect("make the folder");
        let path = dir.join("Song.bwproject");
        let bytes = bitwig_collected_bytes(decodable);
        std::fs::write(&path, bytes).expect("write");

        let info = bitwig::read(&path).expect("read");
        let confidence = if decodable { Confidence::Partial } else { Confidence::Heuristic };
        assert_eq!(info.confidence, confidence);
        let in_project = |name: &str| dir.join("samples").join(name).to_string_lossy().into_owned();
        let expected = vec![in_project("Kick-01.wav"), in_project("Snare-2.wav"), r"D:\Packs\Drums\Kick-02.wav".into()];
        assert_eq!(sorted(info.samples), sorted(expected), "decodable: {decodable}");
    }
}

#[test]
fn survives_files_cut_short() {
    let bytes = bitwig_project_bytes();
    let path = test_dir("bitwig-read-cut").join("cut.bwproject");
    std::fs::write(&path, &bytes[..bytes.len() / 2]).expect("write");
    let info = bitwig::read(&path).expect("read");
    assert_eq!(info.confidence, Confidence::Heuristic);
    assert_eq!(info.version, "5.2.4");
}

#[test]
fn a_file_that_isnt_bitwigs_is_scanned_and_says_so() {
    let path = test_dir("bitwig-read-not").join("not.bwproject");
    std::fs::write(&path, b"Polymer\0\0C:\\Samples\\Kick.wav\0").expect("write");
    let info = bitwig::read(&path).expect("read");
    assert_eq!(info.confidence, Confidence::Heuristic);
    assert!(info.warnings[0].contains("doesn't start with Bitwig's header"));
    assert_eq!(info.samples, [r"C:\Samples\Kick.wav"]);
    assert_eq!(info.plugins[0].name, "Polymer");
}

#[cfg(unix)]
#[test]
fn audio_in_a_linked_folder_beside_the_project_is_found_once_however_it_links_back() {
    let dir = test_dir("bitwig-read-linked");
    let path = bitwig_project(&dir, "Loop Idea");
    let library = dir.join("Sample library");
    std::fs::create_dir_all(&library).expect("make the folder");
    std::fs::write(library.join("Clap.wav"), [0]).expect("write a sample");
    let samples = dir.join("Loop Idea").join("samples");
    std::os::unix::fs::symlink(&library, samples.join("Library")).expect("link the library in");
    // A link back up, which would go round for ever.
    std::os::unix::fs::symlink(&samples, library.join("Back")).expect("link back");
    let found = bitwig::read(&path).expect("read").samples;
    assert_eq!(found.iter().filter(|s| s.ends_with("Clap.wav")).count(), 1, "{found:?}");
    assert_eq!(found.iter().filter(|s| s.ends_with("Recorded Bass.wav")).count(), 1, "{found:?}");
}
