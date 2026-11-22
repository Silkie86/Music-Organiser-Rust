//! Samples a song marked missing are looked for again once the Sounds library has new sounds: a sample folder added
//! in Settings and scanned clears an old project's "missing samples", though its save never changed. The libraries
//! and folders are made in the test build's own fixtures folder, under fixed names.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};

use mo_app::live::{Live, LiveDetails};
use mo_library::Library;
use mo_library::songs_db;
use mo_sounds::compact;
use rusqlite::Connection;

fn folder(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-app-missing-samples").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn tone(path: &Path, seconds: f32, hz: f32) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let n = (44_100.0 * seconds) as usize;
    let wave: Vec<f32> = (0..n).map(|i| (i as f32 * hz * std::f32::consts::TAU / 44_100.0).sin() * 0.5).collect();
    mo_audio::write_wav(path, &[wave], 44_100, 16).unwrap();
}

/// The app's own sound library in `dir`, holding the sound at `path`.
fn own_sounds(dir: &Path, path: &Path) -> PathBuf {
    let db = dir.join("sounds.db");
    let c = Connection::open(&db).unwrap();
    c.execute_batch(
        "PRAGMA journal_mode = WAL;
         CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
         CREATE TABLE sounds (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE, size INTEGER,
             mtime INTEGER, error TEXT, analysis TEXT, features BLOB, clap BLOB, name_type TEXT, user_type TEXT,
             type TEXT, type_source TEXT, type_conf REAL, favourite INTEGER DEFAULT 0, rating INTEGER DEFAULT 0,
             plays INTEGER DEFAULT 0, last_played INTEGER);",
    )
    .unwrap();
    let m = fs::metadata(path).unwrap();
    let mtime = m.modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    c.execute(
        "INSERT INTO sounds (id, path, size, mtime, analysis, type, type_source, type_conf)
         VALUES (1, ?1, ?2, ?3, '{\"Seconds\":0.3,\"SampleRate\":44100,\"ChannelCount\":1}', 'Unknown', '', 1)",
        rusqlite::params![path.to_str().unwrap(), m.len() as i64, mtime],
    )
    .unwrap();
    drop(c);
    let copy = compact::copy_of(&db);
    compact::sync(&db, &copy).unwrap();
    compact::last_sync(&db, &copy).unwrap();
    assert!(compact::own_since(&copy).is_some());
    db
}

/// The app's own songs.db in `dir`, with one old save whose two samples are on a drive long gone.
fn own_songs(dir: &Path) -> PathBuf {
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
    assert!(songs_db::at_start(dir, "test").is_songs_db());
    let project = dir.join("Old Projects").join("jazzy_2.flp");
    fs::create_dir_all(project.parent().unwrap()).unwrap();
    Connection::open(&songs)
        .unwrap()
        .execute(
            "INSERT INTO projects (id, path, name, daw, folder) VALUES (7, ?1, 'jazzy_2', 'FL Studio', ?2);",
            rusqlite::params![project.to_str().unwrap(), project.parent().unwrap().to_str().unwrap()],
        )
        .unwrap();
    Connection::open(&songs)
        .unwrap()
        .execute_batch(
            "INSERT INTO samples (project_id, path, found) VALUES
                 (7, 'D:\\harrys samples\\Old Pack\\Kick 01.wav', 0),
                 (7, 'D:\\harrys samples\\Old Pack\\Snare 01.wav', 0);",
        )
        .unwrap();
    songs
}

fn missing(live: &Live) -> u32 {
    live.songs().rows.iter().find(|r| r.song.latest.id == 7).unwrap().song.latest.missing_samples
}

#[test]
fn a_scan_that_finds_a_songs_samples_clears_them_from_missing() {
    let dir = folder("scan-clears");
    let samples = dir.join("Samples");
    let other = samples.join("Drums").join("Hat 01.wav");
    tone(&other, 0.2, 8000.0);
    let sounds = own_sounds(&dir, &other);
    let songs = own_songs(&dir);
    let lib = Library::open_read_only(&songs).unwrap();
    let mut live = Live::load(&lib, 6, LiveDetails::default())
        .unwrap()
        .with_writes(songs.clone())
        .with_sounds(compact::copy_of(&sounds));
    // At the start: looked for once, not found anywhere.
    live.catch_up();
    assert_eq!(missing(&live), 2);
    assert!(live.retries().is_none(), "nothing to look at again");

    // A sample folder holding the old pack is added and scanned: Kick 01 is found in a folder of the same name.
    let added = dir.join("Old Samples");
    tone(&added.join("Old Pack").join("Kick 01.wav"), 0.3, 60.0);
    let folders = vec![samples.to_str().unwrap().to_owned(), added.to_str().unwrap().to_owned()];
    let scanned = mo_app::sound_scan::scan(&sounds, &folders, &|_, _| {}).unwrap().unwrap();
    assert_eq!(scanned.kept.added, 1);
    // The Songs watch looks again at its next quiet moment, without any project file changing.
    let changes = live.retries().expect("the samples marked missing are due a look");
    let updated = live.update(&changes);
    assert_eq!(updated.rechecked, 1);
    assert_eq!(missing(&live), 1);
    let found: Vec<(String, i64)> = Connection::open(&songs)
        .unwrap()
        .prepare("SELECT path, found FROM samples ORDER BY path")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        found,
        [
            (r"D:\harrys samples\Old Pack\Kick 01.wav".to_owned(), 1),
            (r"D:\harrys samples\Old Pack\Snare 01.wav".to_owned(), 0)
        ]
    );
    assert!(live.retries().is_none(), "looked at once");

    // The next start: nothing changed in the Sounds library since, so nothing is looked at, and the count stands.
    let lib = Library::open_read_only(&songs).unwrap();
    let mut again = Live::load(&lib, 6, LiveDetails::default())
        .unwrap()
        .with_writes(songs.clone())
        .with_sounds(compact::copy_of(&sounds));
    assert_eq!(again.catch_up().rechecked, 0);
    assert_eq!(missing(&again), 1);
}
