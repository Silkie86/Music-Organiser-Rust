//! Scanning sample folders into the app's own sound library (stage 2, review finding X1-own1). The library and the
//! sample folders are made in the test build's own fixtures folder, under fixed names, never a random temp one.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};

use mo_app::sound_scan::{Scanned, scan};
use mo_sounds::compact::{self, grow};
use mo_sounds::{SoundsDb, count_clap_could_hear};
use rusqlite::Connection;

fn folder(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-app-sound-scan").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A C# app's `sounds.db` at `dir` with the sounds at `paths`, one of them heard by CLAP, made the app's own library.
fn own_library(dir: &Path, paths: &[&Path]) -> PathBuf {
    let db = dir.join("sounds.db");
    let c = Connection::open(&db).unwrap();
    c.execute_batch(
        "PRAGMA journal_mode = WAL;
         CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
         CREATE TABLE sounds (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE, size INTEGER,
             mtime INTEGER, error TEXT, analysis TEXT, features BLOB, clap BLOB, name_type TEXT, user_type TEXT,
             type TEXT, type_source TEXT, type_conf REAL, favourite INTEGER DEFAULT 0, rating INTEGER DEFAULT 0,
             plays INTEGER DEFAULT 0, last_played INTEGER);
         CREATE TABLE sound_tags (sound_id INTEGER REFERENCES sounds(id) ON DELETE CASCADE, tag TEXT NOT NULL,
             source TEXT NOT NULL DEFAULT 'you', UNIQUE(sound_id, tag));",
    )
    .unwrap();
    for (i, path) in paths.iter().enumerate() {
        let m = fs::metadata(path).unwrap();
        let mtime = m.modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        let analysis = r#"{"Seconds":0.5,"SampleRate":44100,"ChannelCount":1}"#;
        let clap: Vec<u8> = (0..4).flat_map(|k| (k as f32).to_le_bytes()).collect();
        c.execute(
            "INSERT INTO sounds (id, path, size, mtime, analysis, clap, type, type_source, type_conf, rating)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'Unknown', '', 1, 3)",
            rusqlite::params![i as i64 + 1, path.to_str().unwrap(), m.len() as i64, mtime, analysis, clap],
        )
        .unwrap();
        c.execute("INSERT INTO sound_tags (sound_id, tag) VALUES (?1, 'mine')", [i as i64 + 1]).unwrap();
    }
    drop(c);
    let copy = compact::copy_of(&db);
    compact::sync(&db, &copy).unwrap();
    compact::last_sync(&db, &copy).unwrap();
    assert!(compact::own_since(&copy).is_some());
    db
}

/// A short tone at `path`.
fn tone(path: &Path, seconds: f32, hz: f32) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let n = (44_100.0 * seconds) as usize;
    let wave: Vec<f32> = (0..n).map(|i| (i as f32 * hz * std::f32::consts::TAU / 44_100.0).sin() * 0.5).collect();
    mo_audio::write_wav(path, &[wave], 44_100, 16).unwrap();
}

fn scanned(db: &Path, folders: &[String]) -> (Option<Scanned>, Vec<(usize, usize)>) {
    let said = RefCell::new(Vec::new());
    let s = scan(db, folders, &|done, of| said.borrow_mut().push((done, of))).unwrap();
    (s, said.into_inner())
}

#[test]
fn the_sample_folders_are_scanned_into_the_apps_own_library() {
    let dir = folder("scan");
    let samples = dir.join("Samples");
    let kept = samples.join("Old Pack").join("Kick 01.wav");
    let renamed = samples.join("Old Pack").join("Snare 01.wav");
    let gone = samples.join("Old Pack").join("Hat 01.wav");
    // Each its own length: a rename is told by a file's size and time.
    for (path, seconds, hz) in [(&kept, 0.3, 60.0), (&renamed, 0.4, 200.0), (&gone, 0.6, 8000.0)] {
        tone(path, seconds, hz);
    }
    let db = own_library(&dir, &[&kept, &renamed, &gone]);
    let copy = compact::copy_of(&db);
    let folders = vec![samples.to_str().unwrap().to_owned()];
    // Nothing to scan: no folders, or a library that isn't the app's own.
    assert_eq!(scan(&db, &[], &|_, _| {}).unwrap(), None);
    // A new pack; one renamed; one deleted; and a file that isn't a sound file at all.
    tone(&samples.join("New Pack").join("Bass 01.wav"), 0.5, 55.0);
    tone(&samples.join("New Pack").join("Pad 01.wav"), 1.0, 440.0);
    fs::write(samples.join("New Pack").join("broken.wav"), b"RIFF not really").unwrap();
    let moved_to = samples.join("Old Pack").join("Snare Top.wav");
    fs::rename(&renamed, &moved_to).unwrap();
    fs::remove_file(&gone).unwrap();
    let (s, said) = scanned(&db, &folders);
    let s = s.unwrap();
    assert_eq!(s.found, 5);
    assert_eq!(s.kept, grow::Kept { added: 3, updated: 0, moved: 1, forgotten: 1 });
    // The broken file is new: it isn't counted as a sound found.
    assert_eq!((s.unreadable, s.unheard_new, s.stopped.as_deref()), (1, 1, None));
    assert_eq!(mo_app::sound_scan::scanned_card(&s).0, "Found 2 new sounds");
    assert_eq!(said.first(), Some(&(0, 3)));
    assert_eq!(said.last(), Some(&(3, 3)));
    let db_now = SoundsDb::open_read_only(&copy).unwrap();
    let sounds = db_now.all(chrono::Local::now().naive_local()).unwrap();
    let by_name = |name: &str| sounds.iter().find(|s| s.path.ends_with(name)).cloned();
    let pad = by_name("Pad 01.wav").unwrap();
    assert!(pad.analysis.as_ref().is_some_and(|a| (a.seconds - 1.0).abs() < 0.01), "{:?}", pad.analysis);
    assert_eq!((pad.rating, pad.tags.len()), (0, 0));
    assert!(by_name("broken.wav").unwrap().error.is_some());
    let snare = by_name("Snare Top.wav").unwrap();
    assert_eq!((snare.id, snare.rating, snare.tags.clone()), (2, 3, vec!["mine".to_owned()]));
    assert!(by_name("Hat 01.wav").is_none());
    assert_eq!(by_name("Kick 01.wav").unwrap().rating, 3);
    // The new sounds aren't counted against the 9 in 10 CLAP must have heard: it never hears them in this app.
    let new_since = grow::new_since(&copy);
    assert_eq!(new_since, Some(4));
    assert_eq!(count_clap_could_hear(&sounds, new_since), 2);
    // Scanned again with nothing changed: nothing to do.
    let (s, said) = scanned(&db, &folders);
    assert_eq!(s.unwrap().kept, grow::Kept::default());
    assert!(said.is_empty());
    // A sound changed: listened to again, keeping its stars.
    tone(&kept, 2.0, 60.0);
    let (s, _) = scanned(&db, &folders);
    assert_eq!(s.unwrap().kept, grow::Kept { updated: 1, ..grow::Kept::default() });
    let sounds = SoundsDb::open_read_only(&copy).unwrap().all(chrono::Local::now().naive_local()).unwrap();
    let kick = sounds.iter().find(|s| s.path.ends_with("Kick 01.wav")).unwrap();
    assert_eq!(kick.rating, 3);
    assert!(kick.analysis.as_ref().is_some_and(|a| (a.seconds - 2.0).abs() < 0.01));
    // An unplugged drive's folder keeps its sounds.
    let (s, _) = scanned(&db, &[dir.join("Not plugged in").to_str().unwrap().to_owned(), folders[0].clone()]);
    assert_eq!(s.unwrap().kept.forgotten, 0);
}

#[test]
fn a_scan_that_stops_part_way_keeps_what_it_kept() {
    let dir = folder("stops");
    let samples = dir.join("Samples");
    let kick = samples.join("Kick 01.wav");
    tone(&kick, 0.3, 60.0);
    let db = own_library(&dir, &[&kick]);
    let copy = compact::copy_of(&db);
    // More new sounds than are kept in one write.
    for i in 0..70 {
        tone(&samples.join("New Pack").join(format!("Perc {i:02}.wav")), 0.05 + i as f32 * 0.001, 300.0 + i as f32);
    }
    // Once the first few are kept, the library stops being one the scan may write (as if something else changed it).
    let said = RefCell::new(Vec::new());
    let s = scan(&db, &[samples.to_str().unwrap().to_owned()], &|done, of| {
        said.borrow_mut().push((done, of));
        if done > 0 {
            let c = Connection::open(&copy).unwrap();
            c.execute("DELETE FROM copy_info WHERE key = 'own_since'", []).unwrap();
        }
    })
    .unwrap()
    .unwrap();
    // What was kept stays kept, and says so; the rest is left for the next scan.
    assert_eq!(s.kept, grow::Kept { added: 64, ..grow::Kept::default() });
    assert!(s.stopped.is_some(), "{s:?}");
    assert_eq!(said.into_inner(), [(0, 70), (64, 70)]);
    let sounds = SoundsDb::open_read_only(&copy).unwrap().all(chrono::Local::now().naive_local()).unwrap();
    assert_eq!(sounds.len(), 65);
    let (title, detail) = mo_app::sound_scan::stopped_card(&s);
    assert_eq!(title, "Couldn't keep all your new sounds");
    assert!(detail.starts_with("64 were kept, then: "), "{detail}");
}
