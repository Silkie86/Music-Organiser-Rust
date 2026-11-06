//! Remembering the Bitwig copies Open in Bitwig saves (review X1-5): only ever in the app's own songs.db, with the
//! records of copies that have gone let go as a new one is kept, and every other setting left as it was.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use mo_library::songs_db::{self, State};
use mo_library::{Library, converts};
use rusqlite::Connection;

/// A data folder of its own for each test.
fn folder(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-converts").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A library.db in `dir` with one save and these settings, as the C# app left it.
fn library(dir: &Path, settings: &[(String, String)]) -> PathBuf {
    let path = dir.join("library.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
         CREATE TABLE roots (path TEXT PRIMARY KEY);
         CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE,
             name TEXT NOT NULL, daw TEXT NOT NULL, folder TEXT, size INTEGER, mtime INTEGER, info TEXT,
             status TEXT DEFAULT 'Idea', rating INTEGER DEFAULT 0, notes TEXT DEFAULT '', last_opened INTEGER);
         CREATE TABLE tags (project_id INTEGER REFERENCES projects(id) ON DELETE CASCADE, tag TEXT,
             UNIQUE(project_id, tag));
         INSERT INTO projects (id, path, name, daw, folder, size, mtime)
             VALUES (1, 'C:\\Music\\Night\\Night_11.flp', 'Night_11', 'FL Studio', 'C:\\Music\\Night', 10, 100);",
    )
    .unwrap();
    for (key, value) in settings {
        conn.execute("INSERT INTO settings (key, value) VALUES (?1, ?2)", [key, value]).unwrap();
    }
    path
}

/// The settings kept in the library at `path`, in key order.
fn settings(path: &Path) -> Vec<(String, String)> {
    let conn = Connection::open(path).unwrap();
    let mut st = conn.prepare("SELECT key, value FROM settings ORDER BY key").unwrap();
    st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap()
}

/// A converted copy at `name` in the export folder, there on disk.
fn copy(dir: &Path, name: &str) -> PathBuf {
    let file = dir.join("Exports").join(name).join(format!("{name}.bwproject"));
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, b"bitwig").unwrap();
    file
}

#[test]
fn a_convert_is_remembered_in_the_apps_own_songs_db_and_gone_copies_are_let_go() {
    let dir = folder("own");
    let kept = copy(&dir, "Night");
    let gone = dir.join("Exports").join("Old").join("Old.bwproject");
    let theme = ("theme".to_owned(), "Graphite".to_owned());
    let earlier = |file: &Path| (converts::key(file), format!("638000000000000000 C:\\Music\\{}.flp", file.display()));
    library(&dir, &[theme.clone(), earlier(&kept), earlier(&gone)]);
    let songs = dir.join(songs_db::FILE_NAME);
    songs_db::make(&dir.join("library.db"), &songs, &mut |_| {}).unwrap();
    assert_eq!(songs_db::at_start(&dir, "test").state, State::Switched);

    let new = copy(&dir, "Day");
    assert!(converts::remember(&songs, &new, "638100000000000000 C:\\Music\\Day.flp").unwrap());
    let mut want =
        vec![(converts::key(&new), "638100000000000000 C:\\Music\\Day.flp".to_owned()), earlier(&kept), theme.clone()];
    want.sort();
    // The gone copy's record went; the one still there and every other setting stayed.
    assert_eq!(settings(&songs), want);
    // As the convert reads it back.
    let lib = Library::open_read_only(&songs).unwrap();
    assert_eq!(lib.setting(&converts::key(&new)).unwrap().as_deref(), Some("638100000000000000 C:\\Music\\Day.flp"));
    assert_eq!(lib.settings_starting(converts::PREFIX).unwrap().len(), 2);
    drop(lib);

    // A convert saved over its own copy: the record is the new one.
    assert!(converts::remember(&songs, &new, "638200000000000000 C:\\Music\\Day.flp").unwrap());
    let lib = Library::open_read_only(&songs).unwrap();
    assert_eq!(lib.setting(&converts::key(&new)).unwrap().as_deref(), Some("638200000000000000 C:\\Music\\Day.flp"));
}

#[test]
fn a_record_that_cant_be_kept_lets_no_other_go() {
    let dir = folder("one-change");
    let gone = dir.join("Exports").join("Old").join("Old.bwproject");
    let record = (converts::key(&gone), "638000000000000000 C:\\Music\\Old.flp".to_owned());
    library(&dir, std::slice::from_ref(&record));
    let songs = dir.join(songs_db::FILE_NAME);
    songs_db::make(&dir.join("library.db"), &songs, &mut |_| {}).unwrap();
    assert_eq!(songs_db::at_start(&dir, "test").state, State::Switched);
    // What's kept for the new copy isn't text, so it's left as it is: the change is refused whole, and the gone copy's
    // record stays with it (one transaction).
    let new = copy(&dir, "Day");
    let conn = Connection::open(&songs).unwrap();
    conn.execute("INSERT INTO settings (key, value) VALUES (?1, x'00')", [converts::key(&new)]).unwrap();
    drop(conn);
    assert!(converts::remember(&songs, &new, "638100000000000000 C:\\Music\\Day.flp").is_err());
    let conn = Connection::open(&songs).unwrap();
    let kept = |key: &str| -> i64 {
        conn.query_row("SELECT COUNT(*) FROM settings WHERE key = ?1", [key], |r| r.get(0)).unwrap()
    };
    assert_eq!((kept(&record.0), kept(&converts::key(&new))), (1, 1));
}

#[test]
fn library_db_is_never_written() {
    let dir = folder("library-db");
    let gone = dir.join("Exports").join("Old").join("Old.bwproject");
    let record = (converts::key(&gone), "638000000000000000 C:\\Music\\Old.flp".to_owned());
    let path = library(&dir, std::slice::from_ref(&record));
    let new = copy(&dir, "Day");
    assert!(!converts::remember(&path, &new, "638100000000000000 C:\\Music\\Day.flp").unwrap());
    // Not even the gone copy's record went.
    assert_eq!(settings(&path), [record]);
}
