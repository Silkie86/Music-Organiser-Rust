//! How long FL Studio had a song open, as the library keeps it in each save's details, and the songs put off the
//! Songs page's Dust off card ("Not now"): kept in one setting, with every other setting left as it was.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use mo_library::{Library, dust_off, songs_db};
use rusqlite::Connection;

/// A data folder of its own for each test.
fn folder(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-dust-off").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A library.db in `dir` as the C# app left it: three FL saves, two with FL's time spent among their details (one
/// under an hour), one without, and one Bitwig save; and one setting of another kind.
fn library(dir: &Path) -> PathBuf {
    let path = dir.join("library.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        r#"CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
         CREATE TABLE roots (path TEXT PRIMARY KEY);
         CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE,
             name TEXT NOT NULL, daw TEXT NOT NULL, folder TEXT, size INTEGER, mtime INTEGER, info TEXT,
             status TEXT DEFAULT 'Idea', rating INTEGER DEFAULT 0, notes TEXT DEFAULT '', last_opened INTEGER);
         CREATE TABLE tags (project_id INTEGER REFERENCES projects(id) ON DELETE CASCADE, tag TEXT,
             UNIQUE(project_id, tag));
         INSERT INTO projects (id, path, name, daw, folder, size, mtime, info) VALUES
             (1, 'C:\Music\Night\Night_11.flp', 'Night_11', 'FL Studio', 'C:\Music\Night', 10, 100,
                 '{"Tracks":[],"Details":{"Version":"21.2","Time spent":"41 h 12 min"},"Warnings":[]}'),
             (2, 'C:\Music\Kite\Kite_2.flp', 'Kite_2', 'FL Studio', 'C:\Music\Kite', 10, 100,
                 '{"Tracks":[],"Details":{"Time spent":"13 min"},"Warnings":[]}'),
             (3, 'C:\Music\Tape\Tape.flp', 'Tape', 'FL Studio', 'C:\Music\Tape', 10, 100,
                 '{"Tracks":[],"Details":{"Version":"20.8"},"Warnings":[]}'),
             (4, 'C:\Music\Low\Low.bwproject', 'Low', 'Bitwig Studio', 'C:\Music\Low', 10, 100, NULL);
         INSERT INTO settings (key, value) VALUES ('export_folder', 'C:\Exports');"#,
    )
    .unwrap();
    path
}

/// The settings kept in the library at `path`, by key.
fn settings(path: &Path) -> HashMap<String, String> {
    let conn = Connection::open(path).unwrap();
    let mut st = conn.prepare("SELECT key, value FROM settings").unwrap();
    st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap()
}

#[test]
fn the_time_fl_had_a_save_open_is_read_from_its_details_in_both_libraries() {
    let dir = folder("minutes");
    let old = library(&dir);
    let want = HashMap::from([(1, 41 * 60 + 12), (2, 13)]);
    let lib = Library::open_read_only(&old).unwrap();
    assert_eq!(lib.minutes_spent(&[1, 2, 3, 4, 99]).unwrap(), want);
    drop(lib);
    // songs.db keeps the details packed: read the same.
    let songs = dir.join(songs_db::FILE_NAME);
    songs_db::make(&old, &songs, &mut |_| {}).unwrap();
    let lib = Library::open_read_only(&songs).unwrap();
    let ids: Vec<i64> = lib.saves().unwrap().iter().map(|s| s.id).collect();
    let found = lib.minutes_spent(&ids).unwrap();
    let mut minutes: Vec<u32> = found.values().copied().collect();
    minutes.sort_unstable();
    assert_eq!(minutes, [13, 41 * 60 + 12]);
}

#[test]
fn a_song_put_off_is_kept_with_when_it_comes_back_and_ones_come_back_are_let_go() {
    let dir = folder("put-off");
    let path = library(&dir);
    let read = || dust_off::read(&Library::open_read_only(&path).unwrap()).unwrap();
    assert!(read().is_empty());
    assert!(dust_off::put_off(&path, "night", 500, 100).unwrap());
    assert!(dust_off::put_off(&path, "kite", 300, 100).unwrap());
    assert_eq!(read(), HashMap::from([("night".to_owned(), 500), ("kite".to_owned(), 300)]));
    // Put off again: one line for it, with the new day.
    assert!(dust_off::put_off(&path, "night", 700, 100).unwrap());
    assert_eq!(read(), HashMap::from([("night".to_owned(), 700), ("kite".to_owned(), 300)]));
    // Kite came back at 300: let go as the next is kept.
    assert!(dust_off::put_off(&path, "tape", 900, 400).unwrap());
    assert_eq!(read(), HashMap::from([("night".to_owned(), 700), ("tape".to_owned(), 900)]));
    assert_eq!(settings(&path)["export_folder"], r"C:\Exports");
}

#[test]
fn a_list_it_cant_read_is_left_as_it_is() {
    let dir = folder("unread");
    let path = library(&dir);
    Connection::open(&path)
        .unwrap()
        .execute("INSERT INTO settings (key, value) VALUES (?1, 'not a list')", [dust_off::SETTING])
        .unwrap();
    assert!(dust_off::put_off(&path, "night", 500, 100).is_err());
    assert_eq!(settings(&path)[dust_off::SETTING], "not a list");
    assert!(dust_off::read(&Library::open_read_only(&path).unwrap()).unwrap().is_empty());
}

#[test]
fn a_guess_put_away_is_kept_for_that_save_as_it_is() {
    use std::collections::HashSet;
    let dir = folder("stage-not-now");
    let path = library(&dir);
    let read = || mo_library::stage::not_now(&Library::open_read_only(&path).unwrap()).unwrap();
    assert!(read().is_empty());
    assert!(mo_library::stage::put_away(&path, 1, 100).unwrap());
    assert!(!mo_library::stage::put_away(&path, 1, 100).unwrap());
    assert!(mo_library::stage::put_away(&path, 2, 100).unwrap());
    // The same save saved again: one line for it, with its new time.
    assert!(mo_library::stage::put_away(&path, 1, 200).unwrap());
    assert_eq!(read(), HashSet::from([(1, 200), (2, 100)]));
    assert_eq!(settings(&path)["export_folder"], r"C:\Exports");
}
