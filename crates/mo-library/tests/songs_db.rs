//! songs.db, the app's own song library made from the C# app's library.db: everything the app reads from it reads the
//! same as from library.db, the switch at start happens only once it's in step, and nothing of the user's is ever
//! replaced.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use mo_library::folders::{self, FolderList};
use mo_library::songs_db::{self, State};
use mo_library::{Library, UsageRules};
use rusqlite::Connection;

/// The C# app's library as its last version makes it (`LibraryDb.cs` and its parts), in WAL mode as it keeps it.
const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
CREATE TABLE song_choices (kind TEXT, a TEXT, b TEXT, PRIMARY KEY (kind, a));
CREATE TABLE roots (path TEXT PRIMARY KEY);
CREATE TABLE projects (
    id INTEGER PRIMARY KEY,
    path TEXT UNIQUE NOT NULL COLLATE NOCASE,
    name TEXT NOT NULL, daw TEXT NOT NULL, folder TEXT, size INTEGER, mtime INTEGER,
    tempo REAL, version TEXT, title TEXT, confidence INTEGER,
    can_midi INTEGER DEFAULT 0, can_presets INTEGER DEFAULT 0,
    info TEXT, error TEXT, file_missing INTEGER DEFAULT 0, hardware TEXT DEFAULT '',
    status TEXT DEFAULT 'Idea', rating INTEGER DEFAULT 0, favourite INTEGER DEFAULT 0,
    musical_key TEXT DEFAULT '', notes TEXT DEFAULT '', last_opened INTEGER
);
CREATE TABLE plugins (
    project_id INTEGER REFERENCES projects(id) ON DELETE CASCADE,
    name TEXT, name_key TEXT, vendor TEXT, kind INTEGER, format TEXT, path TEXT, installed INTEGER
);
CREATE TABLE samples (project_id INTEGER REFERENCES projects(id) ON DELETE CASCADE, path TEXT, found INTEGER);
CREATE TABLE tags (project_id INTEGER REFERENCES projects(id) ON DELETE CASCADE, tag TEXT, UNIQUE(project_id, tag));
CREATE TABLE installed_plugins (name TEXT, name_key TEXT PRIMARY KEY);
CREATE TABLE sample_index (path TEXT PRIMARY KEY COLLATE NOCASE, file_name TEXT COLLATE NOCASE, stem_key TEXT);
CREATE INDEX ix_plugins_project ON plugins(project_id);
CREATE INDEX ix_plugins_key ON plugins(name_key);
CREATE INDEX ix_samples_project ON samples(project_id);
CREATE INDEX ix_tags_tag ON tags(tag);
CREATE INDEX ix_sample_index_name ON sample_index(file_name);
CREATE INDEX ix_projects_folder ON projects(folder);
CREATE TABLE plugin_scans (path TEXT PRIMARY KEY COLLATE NOCASE, size INTEGER, mtime INTEGER, scanned INTEGER, error TEXT);
CREATE TABLE scanned_plugins (
    path TEXT COLLATE NOCASE, format TEXT, id TEXT, name TEXT, name_key TEXT, vendor TEXT, version TEXT, kind TEXT, json TEXT
);
CREATE INDEX ix_scanned_plugins_path ON scanned_plugins(path);
CREATE INDEX ix_scanned_plugins_key ON scanned_plugins(name_key);
CREATE TABLE plugin_ids (project_id INTEGER REFERENCES projects(id) ON DELETE CASCADE, name_key TEXT, plugin_id TEXT);
CREATE INDEX ix_plugin_ids_project ON plugin_ids(project_id, name_key);
CREATE TABLE plugin_space (name_key TEXT PRIMARY KEY, name TEXT, vendor TEXT, folders TEXT, measured INTEGER);
CREATE TABLE folder_space (folder TEXT PRIMARY KEY COLLATE NOCASE, bytes INTEGER, measured INTEGER);
ALTER TABLE projects ADD COLUMN detected_key TEXT;
ALTER TABLE projects ADD COLUMN created INTEGER;
ALTER TABLE projects ADD COLUMN note_count INTEGER;
ALTER TABLE projects ADD COLUMN length_bars INTEGER;
ALTER TABLE projects ADD COLUMN track_count INTEGER;
ALTER TABLE samples ADD COLUMN unplayed INTEGER NOT NULL DEFAULT 0;
CREATE INDEX ix_samples_path ON samples(path COLLATE NOCASE);
";

/// A bit of everything the app reads: saves with and without track details (one not JSON, one with letters from
/// beyond English), plugins and samples (one of each whose save has gone), tags, settings, choices and folders, and
/// what the plugin scanner and Space measured.
const ROWS: &str = r#"
INSERT INTO roots VALUES ('C:\beats'), ('K:\Bitwig Studio\Projects');
INSERT INTO settings VALUES ('fix_check:1', 'C:\beats\Didums_2 (fixed).flp'), ('fix_check:2', ''), ('unset', NULL),
  ('sample_folders', '["D:\\Samples","E:\\More"]'), ('plugin_folders', '["C:\\VST"]'), ('number', 7);
INSERT INTO song_choices VALUES ('join', 'a', 'b'), ('split', 'C:\BEATS\Odd.flp', '');
INSERT INTO projects (id, path, name, daw, folder, size, mtime, tempo, version, title, confidence, can_midi, can_presets,
    info, hardware, status, rating, favourite, musical_key, notes, last_opened, detected_key, created, note_count,
    length_bars, track_count)
  VALUES (1, 'C:\beats\Didums.flp', 'Didums', 'FL Studio', 'C:\beats', 1000, 1500000000, 140.0, '20.8', 'Didums!', 0, 1, 1,
          '{"Tracks":["Kick","Bass"],"Details":{"Time signature":"4/4","Bars":"64"},"Warnings":["Damaged"]}',
          'Digitakt, Syntakt', 'Finished', 3, 1, 'A minor', 'needs a drop', 1600000000, 'A minor', 1400000000, 512, 64, 9);
INSERT INTO projects (id, path, name, daw, folder, mtime, confidence, info, status)
  VALUES (2, 'C:\beats\Didums_2.flp', 'Didums_2', 'FL Studio', 'C:\beats', 1700000000, 2, 'not json', 'Mixing');
INSERT INTO projects (id, path, name, daw, folder, size, mtime, confidence, info, file_missing, error)
  VALUES (5, 'K:\Bitwig Studio\Projects\Café\Café.bwproject', 'Café', 'Bitwig', 'K:\Bitwig Studio\Projects\Café', 2048,
          1750000000, 1, '{"Tracks":["Pads – wide","Bass ♭"],"Details":{"Tempo":"92.5"}}', 1, 'The file has gone');
INSERT INTO projects (id, path, name, daw, folder, mtime, confidence)
  VALUES (9, 'C:\beats\Odd.flp', 'odd', 'FL Studio', 'C:\beats', 1650000000, 3);
INSERT INTO plugins VALUES (1, 'Serum', 'serum', 'Xfer', 1, 'VST3', 'C:\VST\Serum.vst3', 1),
  (1, 'Fruity Reverb 2', 'fruityreverb2', 'Image-Line', 2, 'Native', '', 1), (1, 'Gone Synth', 'gonesynth', '', 1, 'VST', '', 0),
  (5, 'Polymer', 'polymer', 'Bitwig', 1, 'Native', '', 1), (77, 'Orphan', 'orphan', '', 1, 'VST', '', 1);
INSERT INTO samples (project_id, path, found, unplayed) VALUES (1, 'C:\s\kick.wav', 1, 1), (1, 'C:\s\lost.wav', 0, 0),
  (2, 'C:\S\KICK.wav', 1, 2), (5, 'Samples\pad.wav', 1, 0), (9, 'kick.wav', 1, 0), (77, 'C:\s\orphan.wav', 1, 0);
INSERT INTO tags VALUES (1, 'techno'), (1, 'dark'), (2, 'dark'), (5, 'ambient');
INSERT INTO installed_plugins VALUES ('Serum', 'serum'), ('Fruity Reverb 2', 'fruityreverb2');
INSERT INTO sample_index VALUES ('D:\Samples\kick.wav', 'kick.wav', 'kick');
INSERT INTO plugin_scans VALUES ('C:\VST\Serum.vst3', 4096, 1500000000, 1500000100, NULL), ('C:\VST\Broken.dll', 10, 1, 2, 'unreadable');
INSERT INTO scanned_plugins VALUES ('C:\VST\Serum.vst3', 'VST3', 'ABCD', 'Serum', 'serum', 'Xfer', '1.3', 'Instrument', '{"shell":""}'),
  ('C:\VST\WaveShell.dll', 'VST', 'WAVE', 'Waves CLA-2A', 'wavescla2a', 'Waves', '14', 'Effect', '{"shell":"WaveShell"}');
INSERT INTO plugin_ids VALUES (1, 'serum', 'ABCD');
INSERT INTO plugin_space VALUES ('serum', 'Serum', 'Xfer', '["C:\\VST"]', 1600000000);
INSERT INTO folder_space VALUES ('C:\beats', 123456, 1600000000);
"#;

/// A test folder of its own, holding a C# app's library.db.
fn library(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-songs-db").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("library.db");
    let c = Connection::open(&path).unwrap();
    c.execute_batch(SCHEMA).unwrap();
    // Rows whose save has gone, as an older C# app (before it turned foreign keys on) could leave them.
    c.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
    c.execute_batch(ROWS).unwrap();
    // The track details as big as the C# app's are (tens of KB each): what songs.db keeps compressed.
    let tracks: Vec<String> = (0..2_000).map(|i| format!("\"Track {i}: Serum, Fruity Reverb 2, Sidechain\"")).collect();
    let big = format!(r#"{{"Tracks":[{}],"Details":{{"Bars":"128"}}}}"#, tracks.join(","));
    c.execute("UPDATE projects SET info = ?1 WHERE id = 9", [&big]).unwrap();
    drop(c);
    path
}

/// This build, as the start names it.
const APP: &str = "0.1.0 (test)";

fn songs_beside(library: &Path) -> PathBuf {
    library.with_file_name(songs_db::FILE_NAME)
}

fn make(library: &Path) -> songs_db::Made {
    songs_db::make(library, &songs_beside(library), &mut |_| {}).unwrap()
}

/// Everything the app reads from a library, written out, so two libraries can be compared at a glance.
fn everything(path: &Path) -> Vec<String> {
    let lib = Library::open_read_only(path).unwrap();
    let mut out = Vec::new();
    let saves = lib.saves().unwrap();
    out.push(format!("saves {saves:?}"));
    for save in &saves {
        out.push(format!("detail {:?}", lib.detail(save.id).unwrap()));
    }
    out.push(format!("no detail {:?}", lib.detail(4).unwrap()));
    out.push(format!("save at {:?}", lib.save_at("c:\\BEATS\\didums.FLP").unwrap()));
    out.push(format!("known {:?}", lib.known_files().unwrap()));
    let installed = lib.installed().unwrap();
    let sorted = |set: &std::collections::HashSet<String>| {
        let mut v: Vec<&String> = set.iter().collect();
        v.sort();
        format!("{v:?}")
    };
    out.push(format!(
        "installed {} {} {} {}",
        sorted(&installed.keys),
        sorted(&installed.names),
        sorted(&installed.ids),
        sorted(&installed.waves)
    ));
    let mut samples: Vec<(i64, Vec<String>)> = lib.sample_paths().unwrap().into_iter().collect();
    samples.sort();
    out.push(format!("sample paths {samples:?}"));
    out.push(format!("choices {:?}", lib.song_choices().unwrap()));
    for key in ["fix_check:1", "unset", "sample_folders", "number", "missing"] {
        out.push(format!("setting {key} {:?} {:?}", lib.setting(key).unwrap(), lib.list_setting(key).unwrap()));
        out.push(format!("strict {key} {:?} {:?}", lib.strict_setting(key).ok(), lib.strict_list_setting(key).ok()));
    }
    out.push(format!("roots {:?}", lib.roots().unwrap()));
    let fixes = lib.pending_fixes().unwrap();
    out.push(format!("fixes {} {}", fixes.len(), fixes.contains("c:\\beats\\didums_2 (fixed).flp")));
    out.push(format!("tags {:?}", lib.all_tags().unwrap()));
    let since = NaiveDate::from_ymd_opt(2023, 1, 1).unwrap().and_hms_opt(0, 0, 0).unwrap();
    for (needs_notes, not_muted_in_newest) in [(false, false), (true, false), (true, true)] {
        let rules = UsageRules { needs_notes, not_muted_in_newest };
        out.push(format!("usage {:?}", lib.sample_usage(since, rules).unwrap()));
    }
    out.push(format!("plugin uses {:?}", lib.plugin_uses().unwrap()));
    out.push(format!("protected {:?}", lib.protected_roots().unwrap()));
    let index = lib.sample_index().unwrap();
    out.push(format!(
        "sample index {} {} {:?}",
        index.len(),
        index.named("KICK.wav"),
        index.inside("C:\\s").or_else(|| index.inside("c:/s"))
    ));
    out.push(format!("installed names {:?}", lib.installed_names().unwrap()));
    out.push(format!("scanned {:?} {:?}", lib.scanned_named("Serum").unwrap(), lib.scanned_named("CLA-2A").unwrap()));
    out.push(format!("scan counts {:?}", lib.scan_counts().unwrap()));
    let mut spaces: Vec<_> = lib.plugin_spaces().unwrap().into_iter().collect::<HashMap<_, _>>().into_iter().collect();
    spaces.sort_by(|a, b| a.0.cmp(&b.0));
    out.push(format!("spaces {spaces:?}"));
    out
}

#[test]
fn songs_db_reads_as_library_db_does() {
    let old = library("same");
    let made = make(&old);
    let new = songs_beside(&old);
    assert_eq!(made.saves, 4);
    let (before, after) = (everything(&old), everything(&new));
    assert_eq!(before.len(), after.len());
    for (b, a) in before.iter().zip(&after) {
        assert_eq!(b, a);
    }
    // The track details really are there (and compressed: the file is smaller than the library it was made from).
    let lib = Library::open_read_only(&new).unwrap();
    let d = lib.detail(5).unwrap().unwrap();
    assert_eq!(d.tracks, ["Pads – wide", "Bass ♭"]);
    assert_eq!(lib.detail(9).unwrap().unwrap().tracks.len(), 2_000);
    assert!(made.bytes < made.from_bytes, "{made:?}");
    let kept: i64 = Connection::open(&new)
        .unwrap()
        .query_row("SELECT length(info) FROM project_info WHERE project_id = 9", [], |r| r.get(0))
        .unwrap();
    assert!(kept < 10_000, "the biggest track details, 90 KB as text, take {kept} bytes");
    // The list says where it came from.
    assert_eq!(lib.saves_and_source().unwrap().1.as_deref(), Some("from songs.db"));
}

#[test]
fn songs_db_leaves_out_what_the_app_never_reads() {
    let old = library("left-out");
    make(&old);
    let c = Connection::open(songs_beside(&old)).unwrap();
    let tables: Vec<String> = c
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(!tables.iter().any(|t| t == "sample_index" || t == "plugin_ids"), "{tables:?}");
    assert!(tables.iter().any(|t| t == "project_info"), "{tables:?}");
    let columns: Vec<String> = c
        .prepare("SELECT name FROM pragma_table_info('projects')")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(!columns.iter().any(|c| c == "info"), "{columns:?}");
    // A save with no track details (or details that aren't text) has no row.
    let details: i64 = c.query_row("SELECT COUNT(*) FROM project_info", [], |r| r.get(0)).unwrap();
    assert_eq!(details, 4);
    let journal: String = c.query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
    assert_eq!(journal, "wal");
    // No half-made file is left beside it.
    let left: Vec<String> = std::fs::read_dir(old.parent().unwrap())
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.contains(".new-"))
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

#[test]
fn the_app_switches_to_songs_db_at_the_next_start_once_its_in_step() {
    let old = library("switch");
    let dir = old.parent().unwrap();
    let new = songs_beside(&old);
    // No songs.db yet: library.db, and one is to be made.
    let first = songs_db::at_start(dir, APP);
    assert_eq!(first.library, old);
    assert!(first.make_wanted() && !first.is_songs_db(), "{first:?}");
    make(&old);
    // library.db changed since it was made (a setting written): library.db again, and it's made again.
    Connection::open(&old).unwrap().execute("INSERT INTO settings VALUES ('later', 'x')", []).unwrap();
    let changed = songs_db::at_start(dir, APP);
    assert_eq!(changed.library, old);
    assert!(changed.make_wanted(), "{changed:?}");
    assert!(songs_db::own_since(&new).is_none());
    make(&old);
    // In step: songs.db from now on, the app's own.
    let switched = songs_db::at_start(dir, APP);
    assert_eq!(switched.library, new);
    assert_eq!(switched.state, State::Switched);
    assert!(songs_db::own_since(&new).is_some());
    assert_eq!(Library::open_read_only(&new).unwrap().setting("later").unwrap().as_deref(), Some("x"));
    // From then on, whatever happens to library.db.
    Connection::open(&old).unwrap().execute("INSERT INTO settings VALUES ('after', 'y')", []).unwrap();
    let own = songs_db::at_start(dir, APP);
    assert_eq!((own.library.clone(), own.state.clone()), (new.clone(), State::Own));
    assert!(!own.make_wanted());
    // The app's own library is never made over.
    let refused = songs_db::make(&old, &new, &mut |_| {}).unwrap_err().to_string();
    assert!(refused.contains("the app's own library already"), "{refused}");
    assert_eq!(Library::open_read_only(&new).unwrap().setting("after").unwrap(), None);
}

#[test]
fn songs_db_is_used_once_library_db_has_gone() {
    let old = library("gone");
    let dir = old.parent().unwrap().to_owned();
    make(&old);
    Connection::open(&old).unwrap().execute("INSERT INTO settings VALUES ('later', 'x')", []).unwrap();
    // Out of step, but library.db went (left in the old folder when the data folder moved): songs.db is all there is.
    for end in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{end}", old.display()));
    }
    let start = songs_db::at_start(&dir, APP);
    assert_eq!(start.library, songs_beside(&old));
    assert_eq!(start.state, State::Switched);
    // Neither file: there's no library yet, and nothing to make.
    let empty = old.parent().unwrap().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let none = songs_db::at_start(&empty, APP);
    assert_eq!(none.library, empty.join("library.db"));
    assert!(!none.make_wanted(), "{none:?}");
}

#[test]
fn a_songs_db_the_app_didnt_make_or_cant_read_is_left_alone() {
    let old = library("foreign");
    let dir = old.parent().unwrap();
    let new = songs_beside(&old);
    // A database of something else at its name.
    Connection::open(&new).unwrap().execute_batch("CREATE TABLE mine (x); INSERT INTO mine VALUES (1);").unwrap();
    let start = songs_db::at_start(dir, APP);
    assert_eq!(start.library, old);
    assert!(!start.make_wanted(), "{start:?}");
    assert!(songs_db::make(&old, &new, &mut |_| {}).is_err());
    let kept: i64 = Connection::open(&new).unwrap().query_row("SELECT x FROM mine", [], |r| r.get(0)).unwrap();
    assert_eq!(kept, 1);
    // One a newer version of the app made.
    std::fs::remove_file(&new).unwrap();
    make(&old);
    Connection::open(&new).unwrap().execute_batch("PRAGMA user_version = 99;").unwrap();
    let start = songs_db::at_start(dir, APP);
    assert_eq!(start.library, old);
    assert!(matches!(&start.state, State::Kept { why } if why.contains("newer")), "{start:?}");
    assert!(songs_db::make(&old, &new, &mut |_| {}).is_err());
}

#[test]
fn a_library_with_something_the_app_doesnt_know_isnt_copied() {
    for (name, change) in [
        ("unknown-table", "CREATE TABLE mystery (x); INSERT INTO mystery VALUES (1);"),
        ("unknown-column", "ALTER TABLE tags ADD COLUMN colour TEXT;"),
    ] {
        let old = library(name);
        Connection::open(&old).unwrap().execute_batch(change).unwrap();
        let why = songs_db::make(&old, &songs_beside(&old), &mut |_| {}).unwrap_err().to_string();
        assert!(why.contains("doesn't know"), "{why}");
        let files: Vec<String> = std::fs::read_dir(old.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|n| n.starts_with("songs.db"))
            .collect();
        assert!(files.is_empty(), "{files:?}");
    }
}

#[test]
fn an_older_library_is_copied_with_the_columns_it_lacks_as_the_c_sharp_app_would_add_them() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-songs-db").join("older");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let old = dir.join("library.db");
    // The first release's tables only: no newer columns, no plugin scans, no Space.
    Connection::open(&old)
        .unwrap()
        .execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
             CREATE TABLE roots (path TEXT PRIMARY KEY);
             CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE, name TEXT NOT NULL,
                 daw TEXT NOT NULL, folder TEXT, size INTEGER, mtime INTEGER, info TEXT, rating INTEGER DEFAULT 0);
             CREATE TABLE samples (project_id INTEGER, path TEXT, found INTEGER);
             INSERT INTO projects (id, path, name, daw, folder, mtime, info, rating)
                 VALUES (3, 'C:\\a\\One.flp', 'One', 'FL Studio', 'C:\\a', 1600000000, '{\"Tracks\":[\"Kick\"]}', 2);
             INSERT INTO samples VALUES (3, 'kick.wav', 1);",
        )
        .unwrap();
    let made = make(&old);
    assert_eq!(made.saves, 1);
    let new = songs_beside(&old);
    let (a, b) = (Library::open_read_only(&old).unwrap(), Library::open_read_only(&new).unwrap());
    assert_eq!(format!("{:?}", a.saves().unwrap()), format!("{:?}", b.saves().unwrap()));
    assert_eq!(format!("{:?}", a.detail(3).unwrap()), format!("{:?}", b.detail(3).unwrap()));
    let since = NaiveDate::from_ymd_opt(2020, 1, 1).unwrap().and_hms_opt(0, 0, 0).unwrap();
    let rules = UsageRules { needs_notes: true, not_muted_in_newest: true };
    assert_eq!(a.sample_usage(since, rules).unwrap(), b.sample_usage(since, rules).unwrap());
}

#[test]
fn the_settings_writers_change_songs_db_as_they_did_library_db() {
    let old = library("writers");
    make(&old);
    let new = songs_beside(&old);
    let changed = folders::add_folder(&new, FolderList::Samples, "F:\\New samples", &[]).unwrap();
    assert!(matches!(changed, folders::Changed::Done(_)), "{changed:?}");
    folders::add_folder(&new, FolderList::Projects, "L:\\Songs", &[]).unwrap();
    let lib = Library::open_read_only(&new).unwrap();
    assert_eq!(lib.list_setting("sample_folders").unwrap(), ["D:\\Samples", "E:\\More", "F:\\New samples"]);
    assert!(lib.roots().unwrap().contains(&"L:\\Songs".to_owned()));
    // library.db itself is never touched.
    assert_eq!(Library::open_read_only(&old).unwrap().list_setting("sample_folders").unwrap().len(), 2);
}

#[test]
fn a_songs_db_that_cant_be_made_twice_over_waits_rather_than_read_library_db_every_start() {
    let old = library("failing");
    let dir = old.parent().unwrap();
    let note = dir.join(songs_db::FAILED_NOTE);
    Connection::open(&old).unwrap().execute_batch("CREATE TABLE mystery (x);").unwrap();
    let fail = || {
        let why = songs_db::make(&old, &songs_beside(&old), &mut |_| {}).unwrap_err().to_string();
        songs_db::note_failed(dir, APP, &why);
    };
    // Once: it may have been a passing thing, so it's tried again.
    fail();
    assert!(songs_db::at_start(dir, APP).make_wanted());
    // Twice the same: it waits, and says why.
    fail();
    let waits = songs_db::at_start(dir, APP);
    assert_eq!(waits.library, old);
    assert!(matches!(&waits.state, State::Kept { why } if why.contains("doesn't know")), "{waits:?}");
    // Another build may make it.
    assert!(songs_db::at_start(dir, "0.1.1 (newer)").make_wanted());
    // And so may a library.db that's changed: made then, and the note goes.
    Connection::open(&old).unwrap().execute_batch("DROP TABLE mystery;").unwrap();
    assert!(songs_db::at_start(dir, APP).make_wanted());
    make(&old);
    assert!(!note.exists());
    assert_eq!(songs_db::at_start(dir, APP).state, State::Switched);
}

#[test]
fn songs_db_held_by_another_program_at_the_start_leaves_library_db_in_use_till_next_time() {
    let old = library("held");
    let dir = old.parent().unwrap();
    let new = songs_beside(&old);
    make(&old);
    // Another program writing to it: it can't be noted as the app's own, so the app doesn't write to it yet.
    let holder = Connection::open(&new).unwrap();
    holder.execute_batch("BEGIN IMMEDIATE;").unwrap();
    let held = songs_db::at_start(dir, APP);
    assert_eq!(held.library, old);
    assert!(!held.make_wanted() && !held.is_songs_db(), "{held:?}");
    assert!(held.line().contains("tried again next start"), "{}", held.line());
    holder.execute_batch("ROLLBACK;").unwrap();
    drop(holder);
    assert_eq!(songs_db::at_start(dir, APP).state, State::Switched);
}

#[test]
fn songs_db_held_as_the_app_starts_is_waited_for_and_settings_never_go_into_library_db() {
    let old = library("held-own");
    let dir = old.parent().unwrap().to_owned();
    let new = songs_beside(&old);
    make(&old);
    assert_eq!(songs_db::at_start(&dir, APP).state, State::Switched);
    // Another program holds it as the app starts, letting go a moment later: waited for, and read.
    let hold = |for_how_long: Option<std::time::Duration>| {
        let (held, wait) = std::sync::mpsc::channel();
        let new = new.clone();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let holder = std::thread::spawn(move || {
            let c = Connection::open(&new).unwrap();
            c.execute_batch(
                "PRAGMA locking_mode = EXCLUSIVE; BEGIN EXCLUSIVE;
                 UPDATE songs_info SET value = value WHERE key = 'made_from';",
            )
            .unwrap();
            held.send(()).unwrap();
            match for_how_long {
                Some(d) => std::thread::sleep(d),
                None => released.recv().unwrap(),
            }
            c.execute_batch("ROLLBACK;").unwrap();
        });
        wait.recv().unwrap();
        (holder, release)
    };
    let (holder, _release) = hold(Some(std::time::Duration::from_millis(400)));
    assert_eq!(songs_db::at_start(&dir, APP).state, State::Own);
    holder.join().unwrap();
    // Held all through the start: library.db this time, and a card says so.
    let (holder, release) = hold(None);
    let start = songs_db::at_start(&dir, APP);
    assert_eq!(start.library, old);
    assert!(matches!(&start.state, State::Unread { .. }), "{start:?}");
    assert!(!start.make_wanted() && !start.is_songs_db());
    assert!(start.notice().is_some_and(|n| n.contains("open it again")), "{start:?}");
    // Still held: it may be the app's own, so a settings change isn't written into library.db.
    let refused = folders::add_folder(&old, FolderList::Samples, "F:\\Lost samples", &[]).unwrap_err().to_string();
    assert!(refused.contains("open it again"), "{refused}");
    // With library.db put away (the app does that once songs.db has worked for a week), songs.db is all there is: it's
    // opened all the same, and no card speaks of a library.db that isn't there.
    let away = dir.join("library.db.away");
    std::fs::rename(&old, &away).unwrap();
    let start = songs_db::at_start(&dir, APP);
    assert_eq!(start.library, dir.join("songs.db"));
    assert!(matches!(&start.state, State::Only { .. }), "{start:?}");
    assert!(start.is_songs_db() && !start.make_wanted() && start.notice().is_none());
    assert!(start.line().contains("there's no library.db"), "{}", start.line());
    std::fs::rename(&away, &old).unwrap();
    release.send(()).unwrap();
    holder.join().unwrap();
    // Let go later: a settings change isn't written into library.db, where it would be lost at the next start.
    let refused = folders::add_folder(&old, FolderList::Samples, "F:\\Lost samples", &[]).unwrap_err().to_string();
    assert!(refused.contains("open it again"), "{refused}");
    assert_eq!(Library::open_read_only(&old).unwrap().list_setting("sample_folders").unwrap().len(), 2);
    // Before songs.db is the app's own, library.db is written as before (songs.db is made from it again).
    let other = library("held-not-own");
    make(&other);
    let changed = folders::add_folder(&other, FolderList::Samples, "F:\\New samples", &[]).unwrap();
    assert!(matches!(changed, folders::Changed::Done(_)), "{changed:?}");
}
