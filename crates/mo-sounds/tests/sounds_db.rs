//! Reading a sounds.db made with the C# app's tables, and making sure nothing here can change it.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use chrono::{Local, NaiveDate, NaiveDateTime, TimeZone};
use mo_sounds::{SoundActivity, SoundsDb, StarPoint};
use rusqlite::Connection;

/// The C# app's tables (`SoundLibrary.Schema` and `PrintSchema`).
const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
CREATE TABLE sounds (
    id INTEGER PRIMARY KEY,
    path TEXT UNIQUE NOT NULL COLLATE NOCASE,
    size INTEGER, mtime INTEGER, error TEXT,
    analysis TEXT, features BLOB, clap BLOB,
    name_type TEXT, user_type TEXT, type TEXT, type_source TEXT, type_conf REAL,
    favourite INTEGER DEFAULT 0, rating INTEGER DEFAULT 0, plays INTEGER DEFAULT 0, last_played INTEGER
);
CREATE TABLE sound_tags (sound_id INTEGER REFERENCES sounds(id) ON DELETE CASCADE, tag TEXT NOT NULL, source TEXT NOT NULL DEFAULT 'you', UNIQUE(sound_id, tag));
CREATE TABLE sound_events (sound_id INTEGER REFERENCES sounds(id) ON DELETE CASCADE, kind TEXT NOT NULL, at INTEGER NOT NULL);
CREATE TABLE sound_maps (space TEXT NOT NULL, sound_id INTEGER NOT NULL, x REAL NOT NULL, y REAL NOT NULL, PRIMARY KEY (space, sound_id));
CREATE TABLE sound_matches (a INTEGER NOT NULL, b INTEGER NOT NULL, kind TEXT NOT NULL, score REAL, offset REAL, apart REAL, typical REAL, PRIMARY KEY (a, b));
CREATE TABLE sound_hidden (sound_id INTEGER PRIMARY KEY, batch TEXT NOT NULL, at INTEGER NOT NULL);
";

/// "Now" for the tests: 1 June 2026, noon, local time. Recent means after 1 June 2025, noon.
fn now() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 6, 1).unwrap().and_hms_opt(12, 0, 0).unwrap()
}

fn at(y: i32, m: u32, d: u32) -> i64 {
    Local
        .from_local_datetime(&NaiveDate::from_ymd_opt(y, m, d).unwrap().and_hms_opt(12, 0, 0).unwrap())
        .unwrap()
        .timestamp()
}

fn make(name: &str, schema: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-sounds");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    for end in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{end}", path.display()));
    }
    let c = Connection::open(&path).unwrap();
    c.execute_batch(schema).unwrap();
    path
}

fn full(name: &str) -> PathBuf {
    let path = make(name, SCHEMA);
    let c = Connection::open(&path).unwrap();
    let analysis = r#"{"Seconds":2.5,"SampleRate":44100,"ChannelCount":2,"IsLoop":true,"Bpm":128.0,"BpmSource":"name",
        "Pitch":36.4,"KeyTonic":3,"KeyMinor":true,"Chord":"Ebm","Chords":[{"Start":0,"End":2.5,"Name":"Ebm"}],
        "Character":["punchy","dark"],"Waveform":[0.5,1.0],"Features":[1,2,3],"Unknown":"ignored"}"#;
    let features: Vec<u8> = [0.25f32, 0.5].iter().flat_map(|f| f.to_le_bytes()).collect();
    c.execute(
        "INSERT INTO sounds (id, path, size, mtime, analysis, features, name_type, type, type_source, type_conf, favourite, rating, plays)
         VALUES (1, 'D:\\Samples\\Kicks\\Kick_01.wav', 100, ?1, ?2, ?3, 'Kick', 'Kick', 'name', 1, 1, 4, 5)",
        rusqlite::params![at(2025, 1, 1), analysis, features],
    )
    .unwrap();
    c.execute_batch(&format!(
        "INSERT INTO sounds (id, path, size, mtime, analysis, clap) VALUES (2, 'd:\\samples\\a pad.wav', 50, 0, 'not json', x'000000');
         INSERT INTO sounds (id, path, error) VALUES (3, 'D:\\Samples\\broken.wav', 'Unreadable');
         INSERT INTO settings VALUES ('galaxy_basis', 'clap'), ('empty', NULL);
         INSERT INTO sound_tags VALUES (1, 'warm', 'you'), (1, 'big', 'learned'), (2, 'warm', 'you');
         INSERT INTO sound_events VALUES (1, 'play', {r}), (1, 'play', {o}), (1, 'drag', {r}), (2, 'send', {o}), (2, 'play', {r2});
         INSERT INTO sound_maps VALUES ('features', 1, 1.5, -2.0), ('clap', 2, 0, 0);
         INSERT INTO sound_matches VALUES (1, 2, 'Converted', 0.9, 0.0, 1.5, NULL);
         INSERT INTO sound_hidden VALUES (2, 'b1', 0);",
        r = at(2026, 1, 1),
        r2 = at(2026, 2, 1),
        o = at(2024, 1, 1)
    ))
    .unwrap();
    path
}

#[test]
fn reads_every_sound() {
    let db = SoundsDb::open_read_only(&full("every.db")).unwrap();
    assert_eq!(db.count().unwrap(), 3);
    assert_eq!(db.count_with_clap().unwrap(), 1);
    let all = db.all(now()).unwrap();
    // Ordered by path, ignoring letter case.
    let paths: Vec<&str> = all.iter().map(|s| s.path.as_str()).collect();
    assert_eq!(paths, ["d:\\samples\\a pad.wav", "D:\\Samples\\broken.wav", "D:\\Samples\\Kicks\\Kick_01.wav"]);

    let kick = &all[2];
    assert_eq!(kick.name(), "Kick_01");
    assert_eq!(kick.folder(), "Kicks");
    assert_eq!(kick.sound_type, "Kick");
    assert_eq!(kick.type_source, "name");
    assert!(kick.favourite);
    assert_eq!(kick.rating, 4);
    assert_eq!(kick.tags, ["big", "warm"]);
    assert!(!kick.hidden);
    assert_eq!(kick.seconds(), 2.5);
    assert!(kick.is_loop());
    assert_eq!(kick.bpm(), Some(128.0));
    assert_eq!(kick.key(), "Ebm");
    assert_eq!(kick.root_note(), "C2");
    assert_eq!(kick.chord(), "Ebm");
    assert_eq!(kick.character(), "punchy, dark");
    let a = kick.analysis.as_ref().unwrap();
    assert_eq!(a.chords.len(), 1);
    assert_eq!(a.waveform, [0.5, 1.0]);
    assert_eq!(a.camelot().as_deref(), Some("2A"));
    // 5 plays counted, 2 of them dated: the 3 undated count as recent.
    assert_eq!(kick.activity, SoundActivity { plays: 4, older_plays: 1, drags: 1, ..Default::default() });
    assert_eq!(kick.added.date(), NaiveDate::from_ymd_opt(2025, 1, 1).unwrap());

    let pad = &all[0];
    assert!(pad.analysis.is_none(), "an analysis that can't be read shows as none");
    assert_eq!(pad.sound_type, "Unknown");
    assert!(pad.hidden);
    assert_eq!(pad.activity, SoundActivity { plays: 1, older_sends: 1, ..Default::default() });

    let broken = &all[1];
    assert_eq!(broken.error.as_deref(), Some("Unreadable"));
    assert_eq!(broken.key(), "");
}

#[test]
fn reads_the_rest() {
    let db = SoundsDb::open_read_only(&full("rest.db")).unwrap();
    assert_eq!(db.setting("galaxy_basis").unwrap().as_deref(), Some("clap"));
    assert_eq!(db.setting("empty").unwrap(), None);
    assert_eq!(db.setting("nope").unwrap(), None);

    let history = db.history(10).unwrap();
    assert_eq!(history.iter().map(|h| h.0).collect::<Vec<_>>(), [2, 1]);

    assert_eq!(db.map("features").unwrap().get(&1), Some(&StarPoint { x: 1.5, y: -2.0 }));
    assert_eq!(db.map("clap").unwrap().len(), 1);
    assert_eq!(db.vectors("features").unwrap().get(&1), Some(&vec![0.25, 0.5]));
    // A blob that isn't whole floats is left out.
    assert!(db.vectors("clap").unwrap().is_empty());

    let tags = db.tag_counts().unwrap();
    assert_eq!((tags[0].tag.as_str(), tags[0].yours, tags[0].learned), ("warm", 2, 0));
    assert_eq!((tags[1].tag.as_str(), tags[1].yours, tags[1].learned), ("big", 0, 1));

    let m = db.matches().unwrap();
    assert_eq!((m[0].a, m[0].b, m[0].kind.as_str(), m[0].apart, m[0].typical), (1, 2, "Converted", Some(1.5), None));

    let known = db.known_files().unwrap();
    assert_eq!(known.len(), 3);
    assert!(known.iter().any(|k| k.has_clap && k.size == 50));
}

#[test]
fn an_older_library_reads_with_what_it_has() {
    // The first release: no duplicate finder tables, and no events or maps.
    let path = make(
        "older.db",
        "CREATE TABLE sounds (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE, size INTEGER, mtime INTEGER,
            error TEXT, analysis TEXT, features BLOB, clap BLOB, name_type TEXT, user_type TEXT, type TEXT, type_source TEXT,
            type_conf REAL, favourite INTEGER DEFAULT 0, rating INTEGER DEFAULT 0, plays INTEGER DEFAULT 0, last_played INTEGER);
         CREATE TABLE sound_matches (a INTEGER, b INTEGER, kind TEXT, score REAL, offset REAL);
         INSERT INTO sounds (id, path, plays) VALUES (1, 'x.wav', 2);
         INSERT INTO sound_matches VALUES (1, 1, 'Exact', 1, 0);",
    );
    let db = SoundsDb::open_read_only(&path).unwrap();
    let all = db.all(now()).unwrap();
    assert_eq!(all[0].activity.plays, 2);
    assert!(db.history(5).unwrap().is_empty());
    assert!(db.map("features").unwrap().is_empty());
    assert!(db.tag_counts().unwrap().is_empty());
    assert_eq!(db.setting("x").unwrap(), None);
    assert_eq!(db.matches().unwrap()[0].apart, None);
}

#[test]
fn never_changes_the_library() {
    let path = full("unchanged.db");
    let db = SoundsDb::open_read_only(&path).unwrap();
    // SQLite itself refuses: the connection is read-only.
    let err = db_write(&path);
    assert!(err.is_some());
    drop(db);
    let missing = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-sounds").join("missing.db");
    assert!(SoundsDb::open_read_only(&missing).is_err());
    assert!(!missing.exists(), "opening never makes a library");
    let not_sounds = make("not-sounds.db", "CREATE TABLE other (x);");
    assert!(SoundsDb::open_read_only(&not_sounds).err().unwrap().to_string().contains("no sounds table"));
}

/// Tries a write through a read-only connection, as `SoundsDb` opens one.
fn db_write(path: &std::path::Path) -> Option<rusqlite::Error> {
    let c = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    c.execute("DELETE FROM sounds", []).err()
}
