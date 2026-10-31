//! The app's own files, on a folder made in the test build's own fixtures folder.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use mo_space::app_files::{databases, measure};

#[test]
fn databases_count_with_their_journals_and_the_rest_by_top_folder() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-space-app-files");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("logs")).unwrap();
    fs::create_dir_all(dir.join("models/clap")).unwrap();
    fs::write(dir.join("library.db"), [0; 100]).unwrap();
    fs::write(dir.join("library.db-wal"), [0; 20]).unwrap();
    fs::write(dir.join("sounds.db"), [0; 50]).unwrap();
    fs::write(dir.join("logs/today.txt"), [0; 5]).unwrap();
    fs::write(dir.join("models/clap/model.onnx"), [0; 300]).unwrap();
    fs::write(dir.join("template.bwproject"), [0; 7]).unwrap();
    let dbs = databases(dir.join("library.db"), dir.join("sounds.db"), dir.join("midi.db"));
    let stop = AtomicBool::new(false);
    let parts: Vec<(&str, u64)> = measure(&dir, &dbs, &stop).unwrap().iter().map(|p| (p.name, p.bytes)).collect();
    assert_eq!(
        parts,
        [("Search models", 300), ("Your library", 120), ("Sound Lab", 50), ("Everything else", 7), ("Logs", 5)]
    );

    // The app's folder can't be read: the databases still count.
    let elsewhere = measure(&dir.join("not there"), &dbs, &stop).unwrap();
    assert_eq!(elsewhere.iter().map(|p| p.bytes).sum::<u64>(), 170);
}
