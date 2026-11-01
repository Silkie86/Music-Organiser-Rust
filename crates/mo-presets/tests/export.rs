//! Exporting presets, into a folder of the test's own (never a real export folder or a project's).

// Test files may unwrap: a test that stops there has failed, which is what it should do.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::PathBuf;

use mo_formats::presets::{PresetBlob, fxp};
use mo_presets::export;
use mo_presets::filter::Choice;

fn folder(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-presets-export").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn serum(name: &str, place: &str) -> PresetBlob {
    let data = fxp::wrap_chunk(&[1, 2, 3], "Serum", i32::from_be_bytes(*b"XfsX"));
    PresetBlob { place: place.into(), ..PresetBlob::new(name, "Serum", ".fxp", data) }
}

fn names(dir: &std::path::Path) -> Vec<String> {
    let mut n: Vec<_> =
        std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    n.sort();
    n
}

#[test]
fn exports_into_the_songs_presets_folder_and_overwrites_the_last_export() {
    let root = folder("plain");
    let presets = [serum("Wobble", ""), serum("Wobble", ""), serum("Lead", "Insert 2 · Synths")];
    let first = export::export(&presets, r"C:\Songs\My: Song.flp", &root, None).unwrap();
    assert_eq!(first.folder, root.join("My_ Song").join("Presets"));
    assert_eq!(first.files, ["Serum - Wobble.fxp", "Serum - Wobble 2.fxp", "Serum - Lead (Insert 2 Synths).fxp"]);
    assert_eq!(std::fs::read(first.folder.join("Serum - Wobble.fxp")).unwrap(), presets[0].data);

    // Again, with the sound changed: the same names, written over, no copies, nothing half-written left.
    let changed =
        PresetBlob { data: fxp::wrap_chunk(&[4, 5], "Serum", i32::from_be_bytes(*b"XfsX")), ..serum("Wobble", "") };
    export::export(std::slice::from_ref(&changed), r"C:\Songs\My: Song.flp", &root, None).unwrap();
    assert_eq!(
        names(&first.folder),
        ["Serum - Lead (Insert 2 Synths).fxp", "Serum - Wobble 2.fxp", "Serum - Wobble.fxp"]
    );
    assert_eq!(std::fs::read(first.folder.join("Serum - Wobble.fxp")).unwrap(), changed.data);
}

#[test]
fn a_preset_that_doesnt_read_back_isnt_put_in_place() {
    let root = folder("broken");
    let broken = PresetBlob::new("Odd", "Serum", ".fxp", b"not a preset".to_vec());
    assert!(export::export(&[broken], "Song.flp", &root, None).is_err());
    assert!(names(&export::folder_for(&root, "Song.flp")).is_empty(), "nothing left, not even the part-written file");
    // FL's own plugins' raw states have no layout to check: written as they are.
    let raw = PresetBlob::new("Fruity Limiter", "Fruity Limiter", ".flstate", vec![9, 9]);
    assert_eq!(export::export(&[raw], "Song.flp", &root, None).unwrap().files, ["Fruity Limiter.flstate"]);

    // One that doesn't read back doesn't stop the others, and is told; the last good export of it stays.
    let good = serum("Wobble", "");
    let folder = export::export(std::slice::from_ref(&good), "Song.flp", &root, None).unwrap().folder;
    let bad = PresetBlob::new("Wobble", "Serum", ".fxp", b"not a preset".to_vec());
    let lead = serum("Lead", "");
    let both = export::export(&[bad, lead], "Song.flp", &root, None).unwrap();
    assert_eq!(both.files, ["Serum - Lead.fxp"]);
    assert_eq!(both.failed.len(), 1);
    assert!(both.failed[0].starts_with("Serum - Wobble.fxp: "), "{:?}", both.failed);
    assert_eq!(std::fs::read(folder.join("Serum - Wobble.fxp")).unwrap(), good.data);
}

#[test]
fn settings_let_go_after_listing_are_never_written_as_an_empty_file() {
    let root = folder("forgotten");
    let good = serum("Wobble", "");
    let folder = export::export(std::slice::from_ref(&good), "Song.flp", &root, None).unwrap().folder;
    let forgotten = PresetBlob { data: Vec::new(), ..good.clone() };
    assert!(export::export(&[forgotten], "Song.flp", &root, None).is_err());
    assert_eq!(std::fs::read(folder.join("Serum - Wobble.fxp")).unwrap(), good.data);
    let raw = PresetBlob::new("Fruity Limiter", "Fruity Limiter", ".flstate", Vec::new());
    assert!(export::export(&[raw], "Song.flp", &root, None).is_err());
    assert_eq!(names(&folder), ["Serum - Wobble.fxp"]);
}

#[test]
fn a_save_that_cant_be_read_exports_nothing() {
    let root = folder("save");
    let save = root.join("Damaged.flp");
    std::fs::write(&save, b"FLhd not really").unwrap();
    assert!(
        export::export_from_save(save.to_str().unwrap(), Choice::default(), None, &root.join("out"), None).is_err()
    );
    assert!(!root.join("out").exists());
}

#[test]
fn half_written_files_a_crash_left_are_cleared() {
    let root = folder("left-behind");
    let presets = export::folder_for(&root, "Song.flp");
    std::fs::create_dir_all(presets.join("Kept.fxp.mo-part-1-1")).unwrap();
    let old = presets.join("Serum - Lead.fxp.mo-part-99-3");
    let fresh = presets.join("Serum - Pad.fxp.mo-part-98-0");
    let yours = presets.join("Lead.part-1-2");
    for f in [&old, &fresh, &yours] {
        std::fs::write(f, b"x").unwrap();
    }
    let hour_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    for f in [&old, &yours] {
        std::fs::File::options().write(true).open(f).unwrap().set_modified(hour_ago).unwrap();
    }
    export::export(&[serum("Lead", "")], "Song.flp", &root, None).unwrap();
    // Only the old one of the export's own shape goes; one still being written, other files and folders stay.
    assert_eq!(
        names(&presets),
        ["Kept.fxp.mo-part-1-1", "Lead.part-1-2", "Serum - Lead.fxp", "Serum - Pad.fxp.mo-part-98-0"]
    );
}

#[test]
fn a_leftover_with_the_same_name_is_passed_over_not_touched() {
    let root = folder("taken");
    let path = root.join("Serum - Lead.fxp");
    // The names this run could take first are already there.
    let mut taken = Vec::new();
    for n in 0..5 {
        let p = PathBuf::from(format!("{}.mo-part-{}-{n}", path.display(), std::process::id()));
        std::fs::write(&p, b"someone else's").unwrap();
        taken.push(p);
    }
    let data = fxp::wrap_chunk(&[1], "Lead", 1);
    export::write_checked(&path, &data).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), data);
    for p in taken {
        assert_eq!(std::fs::read(p).unwrap(), b"someone else's");
    }
}
