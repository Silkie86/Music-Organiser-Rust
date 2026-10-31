//! The page's whole list: preset folders and many saves, on made-up files in a folder of the test's own.

// Test files may unwrap: a test that stops there has failed, which is what it should do.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::PathBuf;

use mo_formats::presets::{fxp, vstpreset};
use mo_presets::Thresholds;
use mo_presets::library::{self, Source};

/// A fresh folder under the build's own test folder, always the same name for the same test.
fn folder(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-presets").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn finds_preset_files_in_folders_and_skips_hidden_ones() {
    let root = folder("find");
    let pack = root.join("Xfer").join("Basses");
    std::fs::create_dir_all(&pack).unwrap();
    std::fs::create_dir_all(root.join(".cache")).unwrap();
    std::fs::write(pack.join("Wobble.fxp"), fxp::wrap_chunk(&[1], "Wobble Bass", i32::from_be_bytes(*b"XfsX")))
        .unwrap();
    std::fs::write(root.join("Pad.vstpreset"), vstpreset::build("ABCDEF0123456789ABCDEF0123456789", &[1], &[]))
        .unwrap();
    std::fs::write(root.join(".cache").join("Hidden.fxp"), b"CcnK").unwrap();
    std::fs::write(root.join("Broken.fxp"), b"nope").unwrap();
    std::fs::write(root.join("readme.txt"), b"not a preset").unwrap();

    let found = library::find_preset_files(&[root.clone(), root.join("missing")]);
    let names: Vec<_> = found.iter().map(|f| PathBuf::from(&f.path).file_name().unwrap().to_owned()).collect();
    assert_eq!(names, ["Broken.fxp", "Pad.vstpreset", "Wobble.fxp"]);
    assert!(found[0].file.is_err());

    let rows = library::rows(&[], &found);
    assert_eq!(rows.len(), 2, "a broken file isn't a row");
    let wobble = rows.iter().find(|r| r.name == "Wobble Bass").unwrap();
    assert_eq!((wobble.plugin.as_str(), wobble.place.as_str(), wobble.extension.as_str()), ("XfsX", "Basses", ".fxp"));
    assert_eq!(wobble.guess.role, Some(mo_presets::Role::Bass), "from its name");
    assert!(matches!(&wobble.source, Source::File { path } if path.ends_with("Wobble.fxp")));
    assert!(mo_presets::filter::search_matches("wobble xfsx", &wobble.search_texts()));
}

#[test]
fn a_save_that_cant_be_read_is_listed_with_why() {
    let dir = folder("saves");
    let damaged = dir.join("Damaged.flp");
    std::fs::write(&damaged, b"FLhd but not really").unwrap();
    let saves = vec![
        damaged.to_string_lossy().into_owned(),
        dir.join("Gone.bwproject").to_string_lossy().into_owned(),
        dir.join("Song.song").to_string_lossy().into_owned(),
    ];
    let read = library::read_saves(&saves, &Thresholds::default());
    assert_eq!(read.len(), 2, "Studio One saves are left out");
    assert!(read.iter().all(|s| s.error.is_some() && s.sounds.is_empty()));
    assert!(read[0].path.ends_with("Damaged.flp"), "in the order given");
    assert_eq!(read[0].error.as_deref(), Some(mo_formats::fl::events::DAMAGED), "the FL reader's own words");
}
