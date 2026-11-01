//! The backup screen's work from start to end, on folders and a small library made in the test build's own folder:
//! never a real music folder or drive.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use mo_app::backup::{Format, Settings, prepare, run};
use mo_backup::folders::{Folder, Mode};
use mo_backup::pack::SONGS;
use mo_library::Library;
use rusqlite::Connection;

fn fixtures(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::canonicalize(&dir).unwrap()
}

fn write(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn library(dir: &Path, save1: &Path, save2: &Path, kick: &Path) -> Library {
    let db = dir.join("library.db");
    let c = Connection::open(&db).unwrap();
    let folder = save1.parent().unwrap().display().to_string();
    c.execute_batch(
        "CREATE TABLE song_choices (kind TEXT, a TEXT, b TEXT, PRIMARY KEY (kind, a));
         CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE, name TEXT NOT NULL,
             daw TEXT NOT NULL, folder TEXT, size INTEGER, mtime INTEGER, file_missing INTEGER DEFAULT 0);
         CREATE TABLE samples (project_id INTEGER, path TEXT, found INTEGER);",
    )
    .unwrap();
    c.execute(
        "INSERT INTO projects (id, path, name, daw, folder, size, mtime) VALUES (1, ?1, 'Song', 'FL Studio', ?3, 5, 1700000000),
             (2, ?2, 'Song_2', 'FL Studio', ?3, 5, 1750000000)",
        rusqlite::params![save1.display().to_string(), save2.display().to_string(), folder],
    )
    .unwrap();
    c.execute("INSERT INTO samples (project_id, path, found) VALUES (2, ?1, 1)", [kick.display().to_string()]).unwrap();
    drop(c);
    Library::open_read_only(&db).unwrap()
}

#[test]
fn a_song_goes_with_the_sample_it_uses_from_outside_the_folders() {
    let root = fixtures("backup-screen");
    let music = root.join("music");
    let save1 = music.join("Song").join("Song.flp");
    let save2 = music.join("Song").join("Song_2.flp");
    let kick = root.join("elsewhere").join("Kick.wav");
    write(&save1, b"save1");
    write(&save2, b"save2");
    write(&kick, b"kick");
    // A preset no song owns.
    let preset = music.join("Presets").join("Lead.fxp");
    write(&preset, b"preset");
    let backup = root.join("backup");
    fs::create_dir_all(&backup).unwrap();
    let lib = library(&root, &save1, &save2, &kick);
    let stop = AtomicBool::new(false);

    let mut settings = Settings {
        drive: Some(backup.clone()),
        folders: vec![
            Folder { path: music.clone(), mode: Mode::Slimmed },
            Folder { path: root.join("unplugged"), mode: Mode::Whole },
        ],
        ..Settings::default()
    };
    let prepared = prepare(&lib, &settings, &stop).unwrap();
    let going: Vec<&Path> = prepared.plan.copy.iter().map(|f| f.path.as_path()).collect();
    assert!(going.contains(&kick.as_path()), "the sample the song uses goes, though its folder isn't on the list");
    assert!(going.contains(&save1.as_path()) && going.contains(&save2.as_path()));
    assert_eq!(prepared.missing, vec![root.join("unplugged")]);
    assert_eq!(prepared.songs.len(), 1);

    let ran = run(&prepared, &settings, &stop, &|_| {});
    let last = &ran.last;
    // The song's newest save went (its id is its first save's).
    assert_eq!(ran.went, vec![(1, prepared.newest[0].2)]);
    assert_eq!(prepared.newest[0].1, save2);
    assert!(last.line.starts_with("4 checked · 4 new or changed"), "{}", last.line);
    assert!(!last.ok, "a folder wasn't plugged in");
    let copies = fs::read_dir(&backup).unwrap().count();
    assert!(copies >= 1);

    settings.format = Format::Zips;
    let ran = run(&prepared, &settings, &stop, &|_| {});
    let last = &ran.last;
    assert_eq!(ran.went.len(), 1, "a zipped song went too");
    // The preset is no song's: it still goes, as a plain folder (up to date from the first run); the song is zipped.
    assert!(last.line.starts_with("2 checked · 1 new or changed"), "{}", last.line);
    let zips: Vec<_> = fs::read_dir(backup.join(SONGS)).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(zips.len(), 1);
}
