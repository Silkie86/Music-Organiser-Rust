//! The Space page's App files card: what each of the app's own files counts as, on a data folder made in the test
//! build's own temp folder under a fixed name (no sounds are scanned). The files are made-up bytes.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use mo_app::space::app_databases;
use mo_space::app_files::{CACHES, DAMAGED_SOUND_LAB, NEW_SONG_LIBRARY, OLD_DATABASES, SOUND_LAB_BACKUP, measure};

/// The card's parts for the data folder `data` while the app reads the song library `library` there, with their sizes.
fn parts(data: &Path, library: &str) -> Vec<(&'static str, u64)> {
    let databases = app_databases(data, data.join(library));
    let stop = AtomicBool::new(false);
    measure(data, &databases, &stop).unwrap().iter().map(|p| (p.name, p.bytes)).collect()
}

/// The sound library's backup copy and a damaged one set aside, and the app's caches, are parts of their own: before,
/// they were "Everything else: Your Bitwig template and the rest", so a backup copy as big as the library read as the
/// template (the whole-app review of 10 Nov, T-4). So is songs.db while the app still reads library.db, and a copy
/// being made counts with the database it's a copy of.
#[test]
fn the_backup_the_caches_and_a_new_song_library_are_parts_of_their_own() {
    let data = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-app-space-app-files");
    let _ = fs::remove_dir_all(&data);
    fs::create_dir_all(data.join("Bitwig template")).unwrap();
    let write = |name: &str, bytes: usize| fs::write(data.join(name), vec![0u8; bytes]).unwrap();
    // The app's own song and sound libraries and MIDI index, which it reads.
    write("songs.db", 40);
    write("sounds-rust.db", 60);
    write("midi-index.db", 8);
    // The old app's.
    write("library.db", 100);
    write("sounds.db", 50);
    write("midi.db", 30);
    // The sound library's backup copy, and a damaged one set aside with SQLite's log beside it.
    write("sounds-rust.db.backup", 55);
    write("sounds-rust.db.damaged", 20);
    write("sounds-rust.db.damaged-wal", 5);
    // Each cache, one with SQLite's log beside it.
    let caches = [
        "sound-copies.db",
        "bitwig-prints.db",
        "space-song-prints.db",
        "backup-song-prints.db",
        "library-list-rust.db",
        "arrangement-peaks.db",
        "preset-files.db",
        "preset-sounds.db",
    ];
    for name in caches {
        write(name, 2);
    }
    write("space-song-prints.db-wal", 3);
    // The rest: the Bitwig template, the settings.
    fs::write(data.join("Bitwig template").join("Music Organiser template.bwproject"), [0u8; 7]).unwrap();
    write("rust-settings.json", 3);

    // songs.db is the app's own song library: library.db is only kept from the old app.
    assert_eq!(
        parts(&data, "songs.db"),
        [
            (OLD_DATABASES, 180),
            ("Sound Lab", 60),
            (SOUND_LAB_BACKUP, 55),
            ("Your library", 40),
            (DAMAGED_SOUND_LAB, 25),
            (CACHES, 19),
            ("Everything else", 10),
            ("MIDI parts", 8),
        ]
    );
    // songs.db made, and library.db read until the next start: songs.db isn't "Everything else" meanwhile.
    assert_eq!(
        parts(&data, "library.db"),
        [
            ("Your library", 100),
            (OLD_DATABASES, 80),
            ("Sound Lab", 60),
            (SOUND_LAB_BACKUP, 55),
            (NEW_SONG_LIBRARY, 40),
            (DAMAGED_SOUND_LAB, 25),
            (CACHES, 19),
            ("Everything else", 10),
            ("MIDI parts", 8),
        ]
    );
    // Copies being made, each named after its database and the app's process number, with SQLite's files beside them:
    // each counts with its database, a backup copy made again as big as the library too.
    write("sounds-rust.db.part-4321", 6);
    write("sounds-rust.db.backup.part-4321", 30);
    write("songs.db.new-4321", 20);
    write("songs.db.new-4321-journal", 1);
    write("midi-index.db.new-4321", 4);
    write("library-list-rust.db.new-4321", 2);
    // Not one: no number, or another ending.
    write("songs.db.new-", 1);
    write("songs.db.newer", 1);
    assert_eq!(
        parts(&data, "library.db"),
        [
            ("Your library", 100),
            (SOUND_LAB_BACKUP, 85),
            (OLD_DATABASES, 80),
            ("Sound Lab", 66),
            (NEW_SONG_LIBRARY, 61),
            (DAMAGED_SOUND_LAB, 25),
            (CACHES, 21),
            ("Everything else", 12),
            ("MIDI parts", 12),
        ]
    );
    let _ = fs::remove_dir_all(&data);
}
