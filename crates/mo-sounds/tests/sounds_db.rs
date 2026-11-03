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
CREATE TABLE sound_prints (sound_id INTEGER PRIMARY KEY, mtime INTEGER, size INTEGER, version INTEGER, exact BLOB, frames INTEGER, bands BLOB, keys BLOB, sketch BLOB, seconds REAL, sfp BLOB, compared INTEGER NOT NULL DEFAULT 0);
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
         INSERT INTO sound_hidden VALUES (2, 'b1', 0);
         INSERT INTO sound_prints (sound_id, exact) VALUES (1, x'0aff'), (2, x'0aff'), (3, x'01');",
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
    let mut warm = db.tagged("warm", "you").unwrap();
    warm.sort_unstable();
    assert_eq!(warm, [1, 2]);
    assert_eq!(db.tagged("big", "learned").unwrap(), [1]);
    assert!(db.tagged("big", "you").unwrap().is_empty());

    let m = db.matches().unwrap();
    assert_eq!((m[0].a, m[0].b, m[0].kind.as_str(), m[0].apart, m[0].typical), (1, 2, "Converted", Some(1.5), None));

    // Only hashes more than one sound has, in capitals.
    let mut exact = db.exact_copies().unwrap();
    exact.sort();
    assert_eq!(exact, [(1, "0AFF".to_owned()), (2, "0AFF".to_owned())]);
    let sizes = db.sizes().unwrap();
    assert_eq!((sizes[&1], sizes[&2], sizes[&3]), (100, 50, 0));

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
    assert!(db.tagged("warm", "you").unwrap().is_empty());
    assert_eq!(db.setting("x").unwrap(), None);
    assert_eq!(db.matches().unwrap()[0].apart, None);
}

#[test]
fn never_changes_the_library() {
    let path = full("unchanged.db");
    let before = std::fs::read(&path).unwrap();
    let db = SoundsDb::open_read_only(&path).unwrap();
    // Everything SoundsDb reads, read through its own connection: the file stays byte for byte as it was.
    db.all(now()).unwrap();
    db.activity(now()).unwrap();
    db.history(10).unwrap();
    db.map("features").unwrap();
    db.vectors("clap").unwrap();
    db.tag_counts().unwrap();
    db.tagged("warm", "you").unwrap();
    db.matches().unwrap();
    db.known_files().unwrap();
    db.setting("galaxy_basis").unwrap();
    // SQLite itself refuses a write through a read-only connection.
    let err = db_write(&path);
    assert!(err.is_some());
    drop(db);
    assert!(std::fs::read(&path).unwrap() == before, "reading changed the library");
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

#[test]
fn a_damaged_row_reads_as_the_csharp_app_reads_it() {
    let path = make("damaged.db", SCHEMA);
    let c = Connection::open(&path).unwrap();
    c.execute_batch(
        "INSERT INTO sounds (id, path, mtime, favourite, rating, plays, type, features)
             VALUES (1, 'D:\\x.wav', 'soon', 'yes', 'five', 'lots', 7, 'not a blob');
         INSERT INTO sounds (id, path, plays) VALUES (2, '\\\\server\\share\\Kicks\\k.wav', 1);
         INSERT INTO sound_matches VALUES (1, 2, 'Mystery', 1, 0, NULL, NULL);",
    )
    .unwrap();
    drop(c);
    let db = SoundsDb::open_read_only(&path).unwrap();
    let all = db.all(now()).unwrap();
    let x = all.iter().find(|s| s.id == 1).unwrap();
    assert!(!x.favourite);
    assert_eq!((x.rating, x.plays), (0, 0));
    assert_eq!(x.sound_type, "7", "a number in a text column is stored as text");
    // A file at the top of a drive has no folder, as in the C# app.
    assert_eq!(x.folder(), "");
    let k = all.iter().find(|s| s.id == 2).unwrap();
    assert_eq!(k.folder(), "Kicks");
    assert_eq!(k.activity.plays, 1);
    assert!(db.vectors("features").unwrap().is_empty());
    assert_eq!(db.matches().unwrap()[0].kind, "Converted");
}

/// A library of `n` sounds with all the C# app's kinds of rows: analyses, vectors, tags, plays, hidden copies, gaps
/// in the ids and a few damaged rows.
fn big(name: &str, n: i64) -> PathBuf {
    let path = make(name, SCHEMA);
    let mut c = Connection::open(&path).unwrap();
    let tx = c.transaction().unwrap();
    for id in 1..=n {
        let analysis = match id % 7 {
            0 => None,
            1 => Some("not json".to_owned()),
            _ => Some(format!(
                r#"{{"Seconds":{},"IsLoop":{},"Bpm":{},"KeyTonic":{},"Waveform":[0.5,{}],"Chroma":[0.1,0.2],"Character":["dark"],"Features":[1,2]}}"#,
                id as f64 / 10.0,
                id % 2 == 0,
                100 + id % 60,
                id % 12,
                (id % 10) as f64 / 10.0
            )),
        };
        let floats =
            |k: usize, at: i64| -> Vec<u8> { (0..k).flat_map(|i| (at as f32 + i as f32).to_le_bytes()).collect() };
        let features = (id % 5 != 0).then(|| floats(4, id));
        let clap = (id % 3 == 0).then(|| floats(8, -id));
        // Mixed case, so the order is SQLite's NOCASE one and not the stored one.
        let path = if id % 2 == 0 {
            format!("D:\\Samples\\Pack {}\\kick {id}.wav", id % 13)
        } else {
            format!("d:\\samples\\PACK {}\\Snare {id}.wav", id % 13)
        };
        tx.execute(
            "INSERT INTO sounds (id, path, size, mtime, analysis, features, clap, name_type, type, type_source,
                 type_conf, favourite, rating, plays)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'Kick', ?8, 'name', 0.5, ?9, ?10, ?11)",
            rusqlite::params![
                id,
                path,
                id * 10,
                at(2025, 1, 1) + id,
                analysis,
                features,
                clap,
                if id % 4 == 0 { "Kick" } else { "Snare" },
                id % 11 == 0,
                id % 6,
                id % 9
            ],
        )
        .unwrap();
        if id % 17 == 0 {
            tx.execute(
                "INSERT INTO sound_tags VALUES (?1, 'warm', 'you'), (?1, 'Big', 'learned'), (?1, 'airy', 'you')",
                [id],
            )
            .unwrap();
        }
        if id % 23 == 0 {
            tx.execute(
                "INSERT INTO sound_events VALUES (?1, 'play', ?2), (?1, 'drag', ?3)",
                rusqlite::params![id, at(2026, 1, 1), at(2024, 1, 1)],
            )
            .unwrap();
        }
        if id % 29 == 0 {
            tx.execute("INSERT INTO sound_hidden VALUES (?1, 'b', 0)", [id]).unwrap();
        }
    }
    // Gaps where sounds were forgotten.
    tx.execute("DELETE FROM sounds WHERE id % 37 = 0 OR id BETWEEN 1000 AND 1500", []).unwrap();
    // Damaged values the C# app reads as none.
    tx.execute(
        "UPDATE sounds SET rating = 'five', favourite = 'yes', type = 7, features = 'not a blob' WHERE id % 41 = 0",
        [],
    )
    .unwrap();
    tx.commit().unwrap();
    path
}

#[test]
fn everything_is_what_all_and_vectors_read() {
    for path in [full("everything.db"), big("everything-big.db", 600)] {
        let db = SoundsDb::open_read_only(&path).unwrap();
        let e = db.everything(now()).unwrap();
        assert_eq!(e.sounds, db.all(now()).unwrap());
        assert_eq!(e.features, db.vectors("features").unwrap());
        assert_eq!(e.clap, db.vectors("clap").unwrap());
    }
}

#[test]
fn a_big_library_reads_as_the_csharp_app_lists_it() {
    let path = big("big.db", 5000);
    let e = SoundsDb::open_read_only(&path).unwrap().everything(now()).unwrap();
    // Every sound once, in SQLite's own path order, with its tags, plays and hidden mark.
    let c = Connection::open(&path).unwrap();
    let mut st = c.prepare("SELECT id FROM sounds ORDER BY path").unwrap();
    let ids: Vec<i64> = st.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    assert_eq!(e.sounds.iter().map(|s| s.id).collect::<Vec<_>>(), ids);
    let sound = |id: i64| e.sounds.iter().find(|s| s.id == id).unwrap();
    assert_eq!(sound(17).tags, ["Big", "airy", "warm"], "as ORDER BY tag sorts them: byte by byte");
    // One dated play this year and 5 counted: the 4 undated count as recent. One drag, before the year.
    assert_eq!((sound(23).activity.plays, sound(23).activity.older_drags), (5, 1));
    assert!(sound(29).hidden);
    let damaged = sound(41);
    assert_eq!((damaged.rating, damaged.favourite, damaged.sound_type.as_str()), (0, false, "7"));
    assert!(!e.features.contains_key(&41));
    assert!(sound(7).analysis.is_none());
    assert_eq!(sound(2).analysis.as_ref().unwrap().waveform, [0.5, 0.2]);
    assert_eq!(e.features.get(&2), Some(&vec![2.0, 3.0, 4.0, 5.0]));
    assert_eq!(e.clap.get(&3).map(Vec::len), Some(8));
}

#[test]
fn an_empty_library_reads_as_empty() {
    let path = make("empty-everything.db", SCHEMA);
    let e = SoundsDb::open_read_only(&path).unwrap().everything(now()).unwrap();
    assert!(e.sounds.is_empty() && e.features.is_empty() && e.clap.is_empty());
}

#[test]
fn the_list_sees_the_library_at_one_moment_while_the_csharp_app_saves() {
    let path = big("saving.db", 4000);
    let c = Connection::open(&path).unwrap();
    let last: i64 = c.query_row("SELECT MAX(id) FROM sounds", [], |r| r.get(0)).unwrap();
    // Plenty of tags, so reading them takes a while: they're read between the plays and the sounds.
    c.execute_batch(
        "WITH RECURSIVE n(k) AS (SELECT 0 UNION ALL SELECT k + 1 FROM n WHERE k < 19)
         INSERT OR IGNORE INTO sound_tags SELECT id, 'tag ' || k, 'learned' FROM sounds, n;",
    )
    .unwrap();
    drop(c);
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    // The C# app sending a sound into a DAW and giving it 5 stars in one save, then taking both back in the next. The
    // sends are read first and the stars last, so only one moment for the whole read keeps the two together.
    let writer = {
        let (path, stop) = (path.clone(), stop.clone());
        std::thread::spawn(move || {
            let mut c = Connection::open(&path).unwrap();
            c.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
            let mut saves = 0u64;
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                let tx = c.transaction().unwrap();
                if saves.is_multiple_of(2) {
                    tx.execute("INSERT INTO sound_events VALUES (?1, 'send', ?2)", [last, at(2026, 1, 1)]).unwrap();
                    tx.execute("UPDATE sounds SET rating = 5 WHERE id = ?1", [last]).unwrap();
                } else {
                    tx.execute("DELETE FROM sound_events WHERE sound_id = ?1 AND kind = 'send'", [last]).unwrap();
                    tx.execute("UPDATE sounds SET rating = 0 WHERE id = ?1", [last]).unwrap();
                }
                tx.commit().unwrap();
                saves += 1;
            }
            saves
        })
    };
    let db = SoundsDb::open_read_only(&path).unwrap();
    for _ in 0..40 {
        let e = db.everything(now()).unwrap();
        let s = e.sounds.iter().find(|s| s.id == last).unwrap();
        assert_eq!(s.activity.sends > 0, s.rating == 5, "the sends and the stars came from different moments");
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(writer.join().unwrap() > 0);
}
