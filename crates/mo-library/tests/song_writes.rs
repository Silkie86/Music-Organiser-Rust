//! The app keeping what it reads in its own songs.db: saves read, read again, moved and gone missing, each batch all
//! or nothing, only ever in the app's own songs.db, and no save's row ever removed.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use mo_library::song_writes::{self, Batch, Missing, Put};
use mo_library::songs_db::{self, State};
use mo_library::{Library, PluginKind, ProjectPlugin, ProjectSample, Save, SaveDetail};
use rusqlite::Connection;

/// A data folder holding a library.db with one save (id 1), and the songs.db made from it, the app's own.
fn library(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-song-writes").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Connection::open(dir.join("library.db"))
        .unwrap()
        .execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
             CREATE TABLE roots (path TEXT PRIMARY KEY);
             CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE,
                 name TEXT NOT NULL, daw TEXT NOT NULL, folder TEXT, size INTEGER, mtime INTEGER, info TEXT,
                 status TEXT DEFAULT 'Idea', rating INTEGER DEFAULT 0, notes TEXT DEFAULT '', last_opened INTEGER);
             CREATE TABLE tags (project_id INTEGER REFERENCES projects(id) ON DELETE CASCADE, tag TEXT,
                 UNIQUE(project_id, tag));
             INSERT INTO projects (id, path, name, daw, folder, size, mtime, status, rating, notes, last_opened)
                 VALUES (1, 'C:\\Music\\Night\\Night_11.flp', 'Night_11', 'FL Studio', 'C:\\Music\\Night', 10, 100,
                     'Mixing', 4, 'louder kick', 1700000000);
             INSERT INTO tags VALUES (1, 'dark');",
        )
        .unwrap();
    let songs = dir.join(songs_db::FILE_NAME);
    songs_db::make(&dir.join("library.db"), &songs, &mut |_| {}).unwrap();
    assert_eq!(songs_db::at_start(&dir, "test").state, State::Switched);
    songs
}

/// A save as the reader gives it, with two plugins, a missing sample, its tracks and details.
fn read(id: i64, path: &str) -> SaveDetail {
    let (folder, name) = path.rsplit_once('\\').unwrap();
    let save = Save {
        id,
        path: path.to_owned(),
        name: name.trim_end_matches(".flp").to_owned(),
        daw: "FL Studio".into(),
        folder: folder.into(),
        size: 2_000,
        mtime: 1_700_000_500,
        tempo: Some(128.0),
        version: "24.1.1".into(),
        hardware: vec!["Digitakt".into()],
        tags: vec!["digitakt".into()],
        track_count: 2,
        ..Save::default()
    };
    SaveDetail {
        save,
        title: "Night".into(),
        plugins: vec![
            ProjectPlugin {
                name: "Serum".into(),
                vendor: "Xfer".into(),
                kind: PluginKind::Instrument,
                format: "VST3".into(),
                path: "C:\\VST3\\Serum.vst3".into(),
                installed: true,
            },
            ProjectPlugin {
                name: "Pro-Q 3".into(),
                vendor: "FabFilter".into(),
                kind: PluginKind::Effect,
                format: "VST3".into(),
                path: String::new(),
                installed: false,
            },
        ],
        samples: vec![ProjectSample { path: "C:\\Samples\\kick \"1\".wav".into(), found: false, unplayed: true }],
        tracks: vec!["Lead".into(), "Drums".into()],
        details: vec![("Time signature".into(), "4/4".into()), ("Bars".into(), "64".into())],
        warnings: vec!["A plugin's state couldn't be read".into()],
        can_extract_midi: true,
        can_extract_presets: false,
    }
}

fn put(detail: SaveDetail, new: bool) -> Batch {
    Batch { put: vec![Put { detail, new }], ..Batch::default() }
}

#[test]
fn a_save_read_is_kept_and_reads_back_as_it_was_read() {
    let songs = library("new");
    let mut new = read(2, "C:\\Music\\Night\\Night_12.flp");
    new.save.status = "Mixing".into();
    let written = song_writes::write(&songs, &put(new.clone(), true)).unwrap();
    assert_eq!((written.put, written.ids.clone()), (1, Vec::new()));
    let lib = Library::open_read_only(&songs).unwrap();
    let back = lib.detail(2).unwrap().unwrap();
    assert_eq!(back.save.path, new.save.path);
    assert_eq!((back.save.size, back.save.mtime, back.save.tempo), (2_000, 1_700_000_500, Some(128.0)));
    assert_eq!((back.save.status.as_str(), back.save.hardware.clone()), ("Mixing", vec!["Digitakt".to_owned()]));
    assert_eq!((back.save.plugin_count, back.save.missing_plugins), (2, 1));
    assert_eq!((back.save.sample_count, back.save.missing_samples), (1, 1));
    assert_eq!(back.save.tags, vec!["digitakt".to_owned()]);
    assert_eq!((back.save.track_count, back.save.note_count), (2, -1));
    assert_eq!(back.title, "Night");
    assert_eq!(back.plugins, new.plugins);
    assert_eq!(back.samples, new.samples);
    assert_eq!(back.tracks, new.tracks);
    assert_eq!(back.details, new.details, "the details keep their order");
    assert_eq!(back.warnings, new.warnings);
    assert!(back.can_extract_midi && !back.can_extract_presets);
    assert_eq!(lib.saves().unwrap().len(), 2);
}

#[test]
fn a_save_read_again_is_the_same_row_with_what_you_gave_it_and_when_you_opened_it() {
    let songs = library("again");
    let mut again = read(1, "C:\\Music\\Night\\Night_11.flp");
    // As the watch hands it over: your fields kept from the save as it was.
    again.save.status = "Mixing".into();
    again.save.rating = 4;
    again.save.notes = "louder kick".into();
    again.save.tags = vec!["dark".into(), "digitakt".into()];
    again.plugins.truncate(1);
    song_writes::write(&songs, &put(again, false)).unwrap();
    let lib = Library::open_read_only(&songs).unwrap();
    let saves = lib.saves().unwrap();
    assert_eq!(saves.len(), 1);
    let s = &saves[0];
    assert_eq!((s.id, s.rating, s.notes.as_str(), s.plugin_count, s.size), (1, 4, "louder kick", 1, 2_000));
    assert_eq!(s.tags, vec!["dark".to_owned(), "digitakt".to_owned()]);
    assert!(s.last_opened.is_some(), "when you last opened it isn't the file's to change");
}

#[test]
fn a_moved_save_keeps_its_row_and_a_file_the_library_has_keeps_its_id() {
    let songs = library("moved");
    // Moved to another folder: the same save, in its new place.
    let moved = read(1, "D:\\Music\\Night_11.flp");
    let written = song_writes::write(&songs, &put(moved, false)).unwrap();
    assert!(written.ids.is_empty());
    let lib = Library::open_read_only(&songs).unwrap();
    assert_eq!(lib.saves().unwrap()[0].path, "D:\\Music\\Night_11.flp");
    drop(lib);
    // Given a new id, but the library has its file already (in other capitals): it's that save.
    let known = read(9, "d:\\music\\NIGHT_11.flp");
    let written = song_writes::write(&songs, &put(known, true)).unwrap();
    assert_eq!(written.ids, vec![(9, 1)]);
    // Given an id another save has meanwhile (another copy of the app): the next free one.
    let other = read(1, "D:\\Music\\Day_1.flp");
    let written = song_writes::write(&songs, &put(other, true)).unwrap();
    assert_eq!(written.ids, vec![(1, 2)]);
    let lib = Library::open_read_only(&songs).unwrap();
    assert_eq!(lib.saves().unwrap().len(), 2);
}

#[test]
fn a_gone_save_is_marked_missing_and_keeps_all_it_had() {
    let songs = library("gone");
    song_writes::write(&songs, &put(read(2, "C:\\Music\\Night\\Night_12.flp"), true)).unwrap();
    let missing = |id: i64, path: &str| Missing { id, path: path.to_owned() };
    // Only the row with both its id and its path: one that's another save's by now is left alone.
    let batch = Batch {
        missing: vec![missing(2, "c:\\music\\night\\NIGHT_12.flp"), missing(1, "C:\\Somewhere\\Else.flp")],
        ..Batch::default()
    };
    let written = song_writes::write(&songs, &batch).unwrap();
    assert_eq!(written.missing, 1);
    let lib = Library::open_read_only(&songs).unwrap();
    let saves = lib.saves().unwrap();
    assert_eq!(saves.len(), 2);
    let night_12 = saves.iter().find(|s| s.id == 2).unwrap();
    assert!(night_12.file_missing);
    assert_eq!((night_12.plugin_count, night_12.sample_count), (2, 1), "a missing save keeps what it had");
    assert!(!saves.iter().find(|s| s.id == 1).unwrap().file_missing);
}

#[test]
fn only_the_apps_own_songs_db_is_written_and_a_busy_one_is_left_as_it_was() {
    let songs = library("refused");
    let dir = songs.parent().unwrap();
    // library.db, and a songs.db not yet the app's own: never written.
    let why = song_writes::write(&dir.join("library.db"), &put(read(2, "C:\\a.flp"), true)).unwrap_err();
    assert!(why.to_string().contains("isn't the app's own"), "{why}");
    let not_own = dir.join("other").join(songs_db::FILE_NAME);
    std::fs::create_dir_all(not_own.parent().unwrap()).unwrap();
    songs_db::make(&dir.join("library.db"), &not_own, &mut |_| {}).unwrap();
    assert!(song_writes::write(&not_own, &put(read(2, "C:\\a.flp"), true)).is_err());
    assert_eq!(Library::open_read_only(&not_own).unwrap().saves().unwrap().len(), 1);
    // Another copy of the app writing meanwhile: busy, nothing changed, and it can be tried again.
    let holder = Connection::open(&songs).unwrap();
    holder.execute_batch("BEGIN IMMEDIATE;").unwrap();
    let busy = song_writes::write(&songs, &put(read(2, "C:\\a.flp"), true)).unwrap_err();
    assert!(busy.is_busy(), "{busy}");
    holder.execute_batch("ROLLBACK;").unwrap();
    drop(holder);
    assert_eq!(Library::open_read_only(&songs).unwrap().saves().unwrap().len(), 1);
    song_writes::write(&songs, &put(read(2, "C:\\a.flp"), true)).unwrap();
    assert_eq!(Library::open_read_only(&songs).unwrap().saves().unwrap().len(), 2);
}
