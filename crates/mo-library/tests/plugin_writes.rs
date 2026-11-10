//! The app keeping its own measure of your plugins in songs.db (review X1-2): both tables replaced whole and read back,
//! and only ever in the app's own songs.db.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use chrono::{NaiveDate, NaiveDateTime};
use mo_library::Library;
use mo_library::plugin_writes::{Measure, recheck_installed, write};
use mo_library::plugins::{StoredFolder, StoredSpace};
use mo_library::songs_db::{self, State};
use rusqlite::Connection;

/// A data folder holding a library.db that found one installed plugin, and the songs.db made from it: the app's own
/// when `own`.
fn library(name: &str, own: bool) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-plugin-writes").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Connection::open(dir.join("library.db"))
        .unwrap()
        .execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
             CREATE TABLE roots (path TEXT PRIMARY KEY);
             CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE,
                 name TEXT NOT NULL, daw TEXT NOT NULL, folder TEXT, size INTEGER, mtime INTEGER, info TEXT);
             CREATE TABLE installed_plugins (name TEXT, name_key TEXT PRIMARY KEY);
             INSERT INTO installed_plugins VALUES ('Old One', 'oldone');",
        )
        .unwrap();
    let songs = dir.join(songs_db::FILE_NAME);
    songs_db::make(&dir.join("library.db"), &songs, &mut |_| {}).unwrap();
    if own {
        assert_eq!(songs_db::at_start(&dir, "test").state, State::Switched);
    }
    songs
}

fn at(day: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 11, day).unwrap().and_hms_opt(20, 0, 0).unwrap()
}

fn measure() -> Measure {
    Measure {
        installed: vec!["Serum".into(), "Kontakt 8".into(), "SERUM".into()],
        spaces: vec![StoredSpace {
            key: "kontakt8".into(),
            name: "Kontakt 8".into(),
            vendor: "Native Instruments".into(),
            folders: vec![
                StoredFolder {
                    path: r"C:\VST3\Kontakt 8.vst3".into(),
                    kind: 0,
                    bytes: 10,
                    shared_with: Vec::new(),
                    added: false,
                },
                StoredFolder {
                    path: r"E:\Kontakt Libraries".into(),
                    kind: 2,
                    bytes: 5_000_000_000,
                    shared_with: vec!["kontakt7".into()],
                    added: true,
                },
            ],
            measured: Some(at(6)),
        }],
    }
}

#[test]
fn a_measure_replaces_the_last_one_and_reads_back_as_written() {
    let path = library("own", true);
    write(&path, &measure()).unwrap();
    let lib = Library::open_read_only(&path).unwrap();
    // The old list is gone, and Serum is there once.
    assert_eq!(lib.installed_names().unwrap(), ["Kontakt 8", "Serum"]);
    let spaces = lib.plugin_spaces().unwrap();
    assert_eq!(spaces.len(), 1);
    assert_eq!(spaces["kontakt8"], measure().spaces[0]);
    drop(lib);
    // A measure with nothing installed in it empties both.
    write(&path, &Measure::default()).unwrap();
    let lib = Library::open_read_only(&path).unwrap();
    assert!(lib.installed_names().unwrap().is_empty() && lib.plugin_spaces().unwrap().is_empty());
}

#[test]
fn a_song_library_that_isnt_the_apps_own_is_never_written() {
    let path = library("not-own", false);
    let e = write(&path, &measure()).unwrap_err();
    assert!(e.to_string().contains("isn't the app's own"), "{e}");
    assert_eq!(Library::open_read_only(&path).unwrap().installed_names().unwrap(), ["Old One"]);
    let none = path.with_file_name("none.db");
    assert!(write(&none, &measure()).is_err() && !none.exists());
}

#[test]
fn saves_plugins_are_looked_at_again_when_the_installed_plugins_or_rules_change() {
    let path = library("recheck", true);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "INSERT INTO projects (id, path, name, daw) VALUES (7, 'K:\\Syrial Lover.bwproject', 'Syrial Lover', 'Bitwig Studio');
         INSERT INTO plugins (project_id, name, vendor, format, path, installed) VALUES
             (7, 'Smack Attack Stereo', 'Waves', 'VST3', '', 0),
             (7, 'Gone Synth', 'Nobody', 'VST3', '', 0),
             (7, 'Serum', 'Xfer Records', 'VST3', '', 0);",
    )
    .unwrap();
    let measured = Measure { installed: vec!["WaveShell1-VST3 15.5_x64".into(), "Serum".into()], spaces: Vec::new() };
    write(&path, &measured).unwrap();

    let r = recheck_installed(&path).unwrap();
    assert!(r.looked);
    // Smack Attack is in the WaveShell and Serum is installed now: only Gone Synth is missing.
    assert_eq!(r.missing, [(7, 1)]);
    let flags: Vec<(String, i64)> = conn
        .prepare("SELECT name, installed FROM plugins ORDER BY name")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(flags, [("Gone Synth".into(), 0), ("Serum".into(), 1), ("Smack Attack Stereo".into(), 1)]);
    assert_eq!(Library::open_read_only(&path).unwrap().detail(7).unwrap().unwrap().save.missing_plugins, 1);

    // Nothing changed since: not looked at again.
    let again = recheck_installed(&path).unwrap();
    assert!(!again.looked && again.missing.is_empty());

    // Serum removed: looked at again.
    write(&path, &Measure { installed: vec!["WaveShell1-VST3 15.5_x64".into()], spaces: Vec::new() }).unwrap();
    assert_eq!(recheck_installed(&path).unwrap().missing, [(7, 2)]);
}

#[test]
fn saves_plugins_are_not_looked_at_in_a_library_not_the_apps_own() {
    let path = library("recheck-not-own", false);
    assert!(recheck_installed(&path).is_err());
}
