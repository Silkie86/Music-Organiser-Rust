//! The Presets tab's presets: a folder of made-up preset files, found as the plugin finds the app's, filtered with the
//! app's own filters, and a preset's own file handed to a drag.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use mo_app::presets::{FORMAT, Filters, PART};
use mo_plugin::presets::{self, Read};
use std::path::PathBuf;

/// A VST2 program: "CcnK", size, "FxCk", version, the plugin's four letters, its version, parameter count, name.
fn fxp(plugin: &[u8; 4], name: &str) -> Vec<u8> {
    let mut b = b"CcnK".to_vec();
    b.extend(0u32.to_be_bytes());
    b.extend(b"FxCk");
    b.extend(1u32.to_be_bytes());
    b.extend(plugin);
    b.extend(1u32.to_be_bytes());
    b.extend(0u32.to_be_bytes());
    let mut n = name.as_bytes().to_vec();
    n.resize(28, 0);
    b.extend(n);
    b
}

/// A folder of made-up preset files (and one that isn't a preset), in the tests' own folder.
fn folder(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("presets").join("Synth")).unwrap();
    std::fs::write(dir.join("presets/Synth/Deep Bass.fxp"), fxp(b"Abcd", "Deep Bass")).unwrap();
    std::fs::write(dir.join("presets/Synth/Warm Pad.fxp"), fxp(b"Abcd", "Warm Pad")).unwrap();
    std::fs::write(dir.join("presets/Synth/Broken.vstpreset"), b"nope").unwrap();
    dir
}

fn ready(read: Read) -> std::sync::Arc<presets::Presets> {
    match read {
        Read::Ready(p) => p,
        Read::Not(why) => panic!("not read: {why}"),
    }
}

#[test]
fn preset_files_are_listed_and_filtered_as_the_app_lists_them() {
    let dir = folder("presets-listed");
    let store = dir.join("data").join(mo_presets::file_store::FILE_NAME);
    let presets = ready(presets::read_from(&[dir.join("presets")], &store));
    assert!(!store.exists(), "the plugin makes no store of its own");
    let names: Vec<&str> = presets.items.iter().map(|i| i.row.name.as_str()).collect();
    assert_eq!(names, ["Deep Bass", "Warm Pad"], "files that aren't presets are left out");

    let all = presets.view(&Filters::default());
    assert_eq!(all.shown.len(), 2);
    let year = all.years.first().expect("the files' year").0;
    let mut bass = Filters::default();
    bass.toggle(PART, "Bass");
    let shown = presets.view(&bass).shown;
    assert_eq!(shown.len(), 1);
    assert_eq!(presets.items[shown[0]].row.name, "Deep Bass");
    let mut vst3 = Filters::default();
    vst3.toggle(FORMAT, "VST3");
    let none = presets.view(&vst3);
    assert!(none.shown.is_empty());
    assert!(none.years.iter().all(|y| y.1 == 0), "the year strip counts the filtered list: {:?}", none.years);
    let this_year = Filters { years: Some((year, year)), ..Filters::default() };
    assert_eq!(presets.view(&this_year).shown.len(), 2);
    let chips = presets.chips(Some(&all.counts), &Filters::default());
    assert!(chips.part.chips.iter().chain(&chips.part.more).any(|c| c.text == "Bass" && c.count == "1"));
    assert_eq!(presets::count_line(2, None), "2 presets");
    assert_eq!(presets::count_line(2, Some(1)), "1 of 2 presets");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_dragged_preset_is_its_own_file() {
    let dir = folder("presets-dragged");
    let store = dir.join("data").join(mo_presets::file_store::FILE_NAME);
    let presets = ready(presets::read_from(&[dir.join("presets")], &store));
    let file = presets.file(0).unwrap();
    assert_eq!(file, dir.join("presets/Synth/Deep Bass.fxp"));
    std::fs::remove_file(&file).unwrap();
    assert!(presets.file(0).is_err(), "a file gone since is said, not carried");
    assert!(presets.file(9).is_err());
    assert!(presets::hint(&presets.items[1]).contains("VST2"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn no_preset_files_says_so() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("presets-none");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    assert!(matches!(presets::read_from(std::slice::from_ref(&dir), &dir.join("store.db")), Read::Not(_)));
    let _ = std::fs::remove_dir_all(&dir);
}
