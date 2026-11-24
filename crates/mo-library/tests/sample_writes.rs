//! Looking again at the samples songs.db has marked missing, once the Sounds library has new sounds: only from missing
//! to found, only when the Sounds library changed since the last look, and only in the app's own songs.db.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use mo_library::Library;
use mo_library::sample_writes::recheck_found;
use mo_library::songs_db::{self, State};
use rusqlite::Connection;

/// A data folder holding a library.db and the songs.db made from it: the app's own when `own`.
fn library(name: &str, own: bool) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-sample-writes").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Connection::open(dir.join("library.db"))
        .unwrap()
        .execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
             CREATE TABLE roots (path TEXT PRIMARY KEY);
             CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE,
                 name TEXT NOT NULL, daw TEXT NOT NULL, folder TEXT, size INTEGER, mtime INTEGER, info TEXT);",
        )
        .unwrap();
    let songs = dir.join(songs_db::FILE_NAME);
    songs_db::make(&dir.join("library.db"), &songs, &mut |_| {}).unwrap();
    if own {
        assert_eq!(songs_db::at_start(&dir, "test").state, State::Switched);
    }
    songs
}

fn never() -> bool {
    false
}

#[test]
fn samples_marked_missing_are_looked_for_again_when_the_sounds_library_changes() {
    let path = library("recheck", true);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "INSERT INTO projects (id, path, name, daw) VALUES
             (7, 'I:\\Old Projects\\jazzy_2.flp', 'jazzy_2', 'FL Studio'),
             (8, 'I:\\Old Projects\\night.flp', 'night', 'FL Studio');
         INSERT INTO samples (project_id, path, found) VALUES
             (7, 'D:\\harrys samples\\kick.wav', 0),
             (7, 'D:\\harrys samples\\gone.wav', 0),
             (7, 'I:\\Samples\\snare.wav', 1),
             (8, 'D:\\harrys samples\\kick.wav', 0);",
    )
    .unwrap();
    let asked = AtomicUsize::new(0);
    // The new sample folder has kick.wav, not gone.wav.
    let found = |sample: &str, _project: &str| {
        asked.fetch_add(1, Ordering::Relaxed);
        sample.ends_with("kick.wav")
    };

    let r = recheck_found(&path, "1:5", || Ok(found), never).unwrap();
    assert!(r.looked);
    assert_eq!((r.looked_for, r.found), (3, 2));
    // Each sample of each save asked once; the one found before isn't asked.
    assert_eq!(asked.load(Ordering::Relaxed), 3);
    assert_eq!(r.missing, [(7, 1), (8, 0)]);
    let lib = Library::open_read_only(&path).unwrap();
    assert_eq!(lib.detail(7).unwrap().unwrap().save.missing_samples, 1);
    assert_eq!(lib.detail(8).unwrap().unwrap().save.missing_samples, 0);

    // The Sounds library hasn't changed since: nothing is looked at.
    asked.store(0, Ordering::Relaxed);
    let again = recheck_found(&path, "1:5", || Ok(found), never).unwrap();
    assert_eq!(again, Default::default());
    assert_eq!(asked.load(Ordering::Relaxed), 0);

    // It has: only what's still missing is looked for, and a sample found is never marked missing here.
    let later = recheck_found(&path, "1:6", || Ok(|_: &str, _: &str| false), never).unwrap();
    assert!(later.looked);
    assert_eq!((later.looked_for, later.found), (1, 0));
    assert!(later.missing.is_empty());
    let lib = Library::open_read_only(&path).unwrap();
    assert_eq!(lib.detail(8).unwrap().unwrap().save.missing_samples, 0);
}

#[test]
fn a_look_stopped_part_way_is_looked_at_again() {
    let path = library("recheck-stopped", true);
    Connection::open(&path)
        .unwrap()
        .execute_batch(
            "INSERT INTO projects (id, path, name, daw) VALUES (7, 'I:\\Old\\a.flp', 'a', 'FL Studio');
             INSERT INTO samples (project_id, path, found) VALUES (7, 'D:\\kick.wav', 0);",
        )
        .unwrap();
    // The app closing: nothing is looked for, and the look isn't noted.
    let stopped = recheck_found(&path, "1:5", || Ok(|_: &str, _: &str| true), || true).unwrap();
    assert!(stopped.stopped && stopped.found == 0);
    // The Sounds library couldn't be read whole: an error, and not noted either.
    assert!(recheck_found(&path, "1:5", || Err::<fn(&str, &str) -> bool, _>("busy".to_owned()), never).is_err());
    let again = recheck_found(&path, "1:5", || Ok(|_: &str, _: &str| true), never).unwrap();
    assert!(again.looked && !again.stopped);
    assert_eq!((again.found, again.missing.clone()), (1, vec![(7, 0)]));
}

#[test]
fn samples_are_not_looked_at_in_a_library_not_the_apps_own() {
    let path = library("recheck-not-own", false);
    assert!(recheck_found(&path, "1:1", || Ok(|_: &str, _: &str| true), never).is_err());
}
