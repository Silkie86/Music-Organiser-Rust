//! Reading Bitwig projects: the C# app's `BitwigTests`, on the same made-up files.

// Test files may unwrap: a test that stops there has failed, which is what it should do.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::HashMap;
use std::path::Path;

use common::bitwig::*;
use common::test_dir;
use mo_formats::bitwig::{self, Project};
use mo_formats::{Confidence, NoteAttribute, NoteEvent, PluginKind};

fn decoded(path: &Path) -> Project {
    Project::open(std::fs::read(path).expect("read the file")).unwrap_or_else(|_| panic!("not a Bitwig file"))
}

/// A note as read from Bitwig, its velocity as Bitwig writes it (0 to 1).
fn note(start: i64, length: i64, key: u8, bitwig_velocity: f64) -> NoteEvent {
    let velocity = mo_formats::velocity::from_unit(bitwig_velocity);
    NoteEvent { start, length, key, velocity, muted: false, step: false, attribute: NoteAttribute::NONE }
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
fn plugin_settings_cut_off_the_end_of_the_file_are_warned_about() {
    let path = bitwig_project(&test_dir("bitwig-read-zip-cut"), "Loop Idea");
    let raw = std::fs::read(&path).expect("read the file");
    // A save interrupted while Bitwig wrote the plugin settings: the zip is cut short, or gone (the file ends where
    // the header says it starts).
    let zip_at = usize::from_str_radix(std::str::from_utf8(&raw[24..40]).unwrap(), 16).unwrap();
    for end in [raw.len() - 30, raw.len() - 300.min(raw.len() / 4), zip_at] {
        let info = bitwig::read_from(&raw[..end], &path);
        assert!(info.warnings.contains(&bitwig::ZIP_DAMAGED.to_owned()), "{end}: {:?}", info.warnings);
        assert_eq!((info.confidence, info.can_extract_presets), (Confidence::Partial, false), "{end}");
    }
    assert!(bitwig::read_from(&raw, &path).warnings.is_empty());
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
    let notes = clip.track("Keys Display").clone();
    assert_eq!(notes, [note(0, 960, 60, 0.5), note(1440, 480, 60, 1.0), note(1920, 960, 64, 0.8)]);
    // As MIDI 1.0 velocities, what the C# app gives.
    assert_eq!(notes.iter().map(NoteEvent::velocity7).collect::<Vec<_>>(), [64, 127, 102]);
}

#[test]
fn extracts_plugin_states_named_by_track() {
    let path = bitwig_project(&test_dir("bitwig-read-presets"), "Loop Idea");
    let presets = decoded(&path).presets();
    let by_plugin: HashMap<&str, _> = presets.iter().map(|p| (p.plugin.as_str(), p)).collect();
    let keys = by_plugin["Keyscape"];
    assert_eq!((keys.name.as_str(), &*keys.extension), ("Keys Display - Keyscape", ".vstpreset"));
    assert_eq!(keys.data, b"VST3 keys state");
    let limiter = by_plugin["FabFilter Pro-L 2"];
    assert_eq!(limiter.name, "Master - FabFilter Pro-L 2");
    // What each plugin is, from its device (here by its category: Synth, Dynamics).
    assert_eq!((keys.kind, limiter.kind), (PluginKind::Instrument, PluginKind::Effect));
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
    let recorded = path.parent().expect("a folder").join("recordings").join("Untitled Project [0m27s-1m22s].wav");
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

/// A multisample a sample file finds outside the project by its full path (not collected, so not in `used_files`)
/// is one of the save's samples; a path it was found at before isn't (the whole-app review of 19 Nov, FS-5).
#[test]
fn a_multisample_found_outside_the_project_by_its_full_path_is_one_of_its_samples() {
    use mo_formats::bitwig::{Field, Object, Slot, Text, Value};
    let mut doc = bitwig::decode(bitwig_template(Template::default())).unwrap();
    let text = |id: u32, t: &str| Field { id, value: Value::Str(Text::new(t)) };
    let root = doc.root.unwrap();
    let mut sample = |name: &str, inside: Option<&str>, path: &str, before: &str, at: u32| {
        let mut reference = Object::new(0x2a);
        reference.fields.push(text(0x3b, path));
        let reference = doc.add(reference);
        let mut old = Object::new(0x2a);
        old.fields.push(text(0x3b, before));
        let old = doc.add(old);
        let mut found = Object::new(0xc5a);
        found.fields.push(Field { id: 0xc5b, value: Value::List(vec![Slot::Obj(old)]) });
        let found = doc.add(found);
        let mut file = Object::new(0x4a6);
        file.fields.push(text(0x512, name));
        if let Some(inside) = inside {
            file.fields.push(text(0xd3a, inside));
        }
        file.fields.push(Field { id: 0xd3b, value: Value::Obj(Slot::Obj(reference)) });
        file.fields.push(Field { id: 0x15, value: Value::Obj(Slot::Obj(found)) });
        let file = doc.add(file);
        doc.objects[reference.0 as usize].parent = Some((file, 0xd3b));
        doc.objects[found.0 as usize].parent = Some((file, 0x15));
        doc.objects[old.0 as usize].parent = Some((found, 0xc5b));
        doc.objects[file.0 as usize].parent = Some((root, at));
        doc.obj_mut(root).fields.push(Field { id: at, value: Value::Obj(Slot::Obj(file)) });
    };
    let other = r"K:\Bitwig Studio\Projects\Facebook Garage\multi-samples\Snare 5 Select.multisample";
    sample("Snare 5 Select.multisample", Some(""), other, r"D:\Library\Snare 5 Select.multisample", 0x7f20);
    // One inside the project is the project's own (in `used_files` when Bitwig collected it).
    sample(
        "Kick.multisample",
        Some("multi-samples/Kick.multisample"),
        r"I:\Old\Kick.multisample",
        r"D:\Old.multisample",
        0x7f21,
    );
    // A save without the field (from an older Bitwig, perhaps): outside unless it's in this project's own multi-samples
    // folder by name (saved when the drive had another letter).
    let older = r"K:\Bitwig Studio\Projects\Facebook Garage\multi-samples\Bass.multisample";
    sample("Bass.multisample", None, older, r"D:\Old.multisample", 0x7f22);
    let own = r"I:\Projects\Jazzy Garage\multi-samples\Pad.multisample";
    sample("Pad.multisample", None, own, r"D:\Old.multisample", 0x7f23);
    let zip = doc.zip().map(<[u8]>::to_vec);
    let raw = bitwig::encode(&doc, zip.as_deref()).unwrap();
    let info = bitwig::read_from(&raw, Path::new(r"K:\Bitwig Studio\Projects\jazzy garage\jazzy garage 14.bwproject"));
    assert!(info.samples.iter().any(|s| s == other), "{:?}", info.samples);
    assert!(info.samples.iter().any(|s| s == older), "{:?}", info.samples);
    for not in ["D:\\Library", "I:\\Old\\Kick", "D:\\Old", "I:\\Projects"] {
        assert!(!info.samples.iter().any(|s| s.starts_with(not)), "{not}: {:?}", info.samples);
    }
}

/// Where a save says each file in its own folder was copied from: the full path in the file reference of a sample
/// file with a place inside the project (Solomon's Nuero garage 7, 26 Nov 2026: "samples/Air Trumpets-2.wav" from
/// Loopmasters Khords). Not one outside the project, not a path it was found at before, not a path that isn't full.
#[test]
fn a_save_says_where_each_copy_in_its_folder_was_copied_from() {
    use mo_formats::bitwig::{Field, Object, Slot, Text, Value};
    let mut doc = bitwig::decode(bitwig_template(Template::default())).unwrap();
    let text = |id: u32, t: &str| Field { id, value: Value::Str(Text::new(t)) };
    let root = doc.root.unwrap();
    let mut sample = |inside: &str, path: &str, at: u32| {
        let mut reference = Object::new(0x2a);
        reference.fields.push(text(0x3b, path));
        let reference = doc.add(reference);
        let mut old = Object::new(0x2a);
        old.fields.push(text(0x3b, r"I:\Bitwig Studio\Projects\Nuero garage\samples\Old-2.wav"));
        let old = doc.add(old);
        let mut found = Object::new(0x2b4);
        found.fields.push(Field { id: 0xc5b, value: Value::List(vec![Slot::Obj(old)]) });
        let found = doc.add(found);
        let mut file = Object::new(0x4a6);
        file.fields.push(text(0x512, "x.wav"));
        file.fields.push(text(0xd3a, inside));
        file.fields.push(Field { id: 0xd3b, value: Value::Obj(Slot::Obj(reference)) });
        file.fields.push(Field { id: 0x15, value: Value::Obj(Slot::Obj(found)) });
        let file = doc.add(file);
        doc.objects[reference.0 as usize].parent = Some((file, 0xd3b));
        doc.objects[found.0 as usize].parent = Some((file, 0x15));
        doc.objects[old.0 as usize].parent = Some((found, 0xc5b));
        doc.objects[file.0 as usize].parent = Some((root, at));
        doc.obj_mut(root).fields.push(Field { id: at, value: Value::Obj(Slot::Obj(file)) });
    };
    let khords = r"C:\ProgramData\Loopmasters\Khords\Packs\Ambient Textures\Samples\Minor Feel\Air Trumpets.wav";
    sample("samples/Air Trumpets-2.wav", khords, 0x7f20);
    // The same file used twice is said once.
    sample("samples/Air Trumpets-2.wav", khords, 0x7f21);
    // Outside the project: nothing in its folder.
    sample("", r"D:\Pack\Kick.wav", 0x7f22);
    // A path that isn't full says nothing about where it came from.
    sample("samples/Snare-2.wav", "Snare.wav", 0x7f23);
    let zip = doc.zip().map(<[u8]>::to_vec);
    let raw = bitwig::encode(&doc, zip.as_deref()).unwrap();
    let back = bitwig::decode(raw).unwrap();
    assert_eq!(bitwig::copied_from(&back), vec![("samples/Air Trumpets-2.wav".to_owned(), khords.to_owned())]);
}
