//! Reading a library.db made with the C# app's tables, and making sure nothing here can change it.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use mo_library::{Confidence, Library, PluginKind};
use rusqlite::Connection;

/// The C# app's tables as its first release made them (later columns are added by `newer`).
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
CREATE TABLE plugins (project_id INTEGER, name TEXT, name_key TEXT, vendor TEXT, kind INTEGER, format TEXT, path TEXT, installed INTEGER);
CREATE TABLE samples (project_id INTEGER, path TEXT, found INTEGER);
CREATE TABLE tags (project_id INTEGER, tag TEXT, UNIQUE(project_id, tag));
CREATE TABLE installed_plugins (name TEXT, name_key TEXT PRIMARY KEY);
CREATE TABLE sample_index (path TEXT PRIMARY KEY COLLATE NOCASE, file_name TEXT COLLATE NOCASE, stem_key TEXT);
";

const NEWER: &str = "
ALTER TABLE projects ADD COLUMN detected_key TEXT;
ALTER TABLE projects ADD COLUMN created INTEGER;
ALTER TABLE projects ADD COLUMN note_count INTEGER;
ALTER TABLE projects ADD COLUMN length_bars INTEGER;
ALTER TABLE projects ADD COLUMN track_count INTEGER;
ALTER TABLE samples ADD COLUMN unplayed INTEGER NOT NULL DEFAULT 0;
";

const ROWS: &str = r#"
INSERT INTO roots VALUES ('C:\beats');
INSERT INTO settings VALUES ('fix_check:1', 'C:\beats\Didums_2 (fixed).flp'), ('fix_check:2', ''), ('unset', NULL), ('sample_folders', '["D:\\Samples","E:\\More"]');
INSERT INTO song_choices VALUES ('join', 'a', 'b'), ('split', 'C:\BEATS\Odd.flp', '');
INSERT INTO projects (id, path, name, daw, folder, size, mtime, tempo, version, title, confidence, can_midi, info, hardware, status, rating, favourite, musical_key, notes, last_opened)
  VALUES (1, 'C:\beats\Didums.flp', 'Didums', 'FL Studio', 'C:\beats', 1000, 1500000000, 140.0, '20.8', 'Didums!', 0, 1,
          '{"Tracks":["Kick","Bass"],"Details":{"Time signature":"4/4","Bars":"64"},"Warnings":["Damaged"]}', 'Digitakt, Syntakt', '', 3, 1, 'A minor', 'needs a drop', 1600000000);
INSERT INTO projects (id, path, name, daw, folder, mtime, confidence, info, status)
  VALUES (2, 'C:\beats\Didums_2.flp', 'Didums_2', 'FL Studio', 'C:\beats', 1700000000, 2, 'not json', 'Mixing');
INSERT INTO plugins VALUES (1, 'Serum', 'serum', 'Xfer', 1, 'VST3', '', 1), (1, 'Fruity Reverb 2', 'fruityreverb2', 'Image-Line', 2, 'Native', '', 1),
  (1, 'Gone Synth', 'gonesynth', '', 1, 'VST', '', 0);
INSERT INTO samples (project_id, path, found) VALUES (1, 'C:\s\kick.wav', 1), (1, 'C:\s\lost.wav', 0);
INSERT INTO tags VALUES (1, 'techno'), (1, 'dark'), (2, 'dark');
"#;

fn make(name: &str, newer: bool) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-library");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    for end in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{end}", path.display()));
    }
    let c = Connection::open(&path).unwrap();
    c.execute_batch(SCHEMA).unwrap();
    if newer {
        c.execute_batch(NEWER).unwrap();
    }
    c.execute_batch(ROWS).unwrap();
    if newer {
        c.execute_batch("UPDATE projects SET note_count = 512, track_count = 9, created = 1400000000 WHERE id = 1")
            .unwrap();
        c.execute_batch("UPDATE samples SET unplayed = 1 WHERE path LIKE '%kick%'").unwrap();
    }
    drop(c);
    path
}

#[test]
fn reads_saves_newest_first_with_their_counts() {
    let lib = Library::open_read_only(&make("newer.db", true)).unwrap();
    let saves = lib.saves().unwrap();
    assert_eq!(saves.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Didums_2", "Didums"]);
    let d = &saves[1];
    assert_eq!(
        (d.id, d.daw.as_str(), d.folder.as_str(), d.size, d.mtime),
        (1, "FL Studio", r"C:\beats", 1000, 1_500_000_000)
    );
    assert_eq!((d.tempo, d.version.as_str(), d.confidence), (Some(140.0), "20.8", Confidence::Full));
    assert_eq!((d.plugin_count, d.missing_plugins, d.sample_count, d.missing_samples), (3, 1, 2, 1));
    assert_eq!(d.tags, ["dark", "techno"]);
    assert_eq!(d.plugin_names.len(), 3);
    assert_eq!(d.hardware, ["Digitakt", "Syntakt"]);
    // An empty status is an idea, as in the C# app.
    assert_eq!((d.status.as_str(), d.rating, d.favourite), ("Idea", 3, true));
    assert_eq!((d.musical_key.as_str(), d.notes.as_str()), ("A minor", "needs a drop"));
    assert_eq!((d.note_count, d.track_count, d.length_bars), (512, 9, -1));
    assert!(d.created.is_some() && d.last_opened.is_some());
    assert_eq!((saves[0].status.as_str(), saves[0].confidence), ("Mixing", Confidence::Heuristic));
}

#[test]
fn a_library_made_before_the_newer_columns_still_reads() {
    let lib = Library::open_read_only(&make("older.db", false)).unwrap();
    let saves = lib.saves().unwrap();
    assert_eq!(saves.len(), 2);
    assert_eq!((saves[1].note_count, saves[1].created, saves[1].detected_key.as_str()), (-1, None, ""));
    let detail = lib.detail(1).unwrap().unwrap();
    assert!(detail.samples.iter().all(|s| !s.unplayed));
}

#[test]
fn reads_one_saves_detail() {
    let lib = Library::open_read_only(&make("detail.db", true)).unwrap();
    let d = lib.detail(1).unwrap().unwrap();
    assert_eq!(d.title, "Didums!");
    assert!(d.can_extract_midi && !d.can_extract_presets);
    assert_eq!(d.tracks, ["Kick", "Bass"]);
    // The details keep the order the reader found them in.
    assert_eq!(d.details, [("Time signature".into(), "4/4".into()), ("Bars".into(), "64".into())]);
    assert_eq!(d.warnings, ["Damaged"]);
    // Instruments first, then by name.
    let plugins: Vec<_> = d.plugins.iter().map(|p| (p.name.as_str(), p.kind, p.installed)).collect();
    assert_eq!(
        plugins,
        [
            ("Gone Synth", PluginKind::Instrument, false),
            ("Serum", PluginKind::Instrument, true),
            ("Fruity Reverb 2", PluginKind::Effect, true)
        ]
    );
    // Missing samples first.
    assert_eq!(
        d.samples.iter().map(|s| (s.path.as_str(), s.found, s.unplayed)).collect::<Vec<_>>(),
        [(r"C:\s\lost.wav", false, false), (r"C:\s\kick.wav", true, true)]
    );

    // Damaged JSON leaves the findings empty rather than failing.
    let other = lib.detail(2).unwrap().unwrap();
    assert!(other.tracks.is_empty() && other.details.is_empty());
    assert!(lib.detail(99).unwrap().is_none());
}

#[test]
fn reads_settings_roots_choices_and_tags() {
    let lib = Library::open_read_only(&make("settings.db", true)).unwrap();
    assert_eq!(lib.roots().unwrap(), [r"C:\beats"]);
    let fixes = lib.pending_fixes().unwrap();
    assert_eq!(fixes.len(), 1);
    // Matched ignoring case, as the library matches paths.
    assert!(fixes.contains(r"c:\BEATS\didums_2 (FIXED).flp"));
    assert_eq!(lib.list_setting("sample_folders").unwrap(), [r"D:\Samples", r"E:\More"]);
    assert_eq!(lib.setting("nothing").unwrap(), None);
    assert_eq!(lib.setting("unset").unwrap(), None);
    assert_eq!(lib.all_tags().unwrap(), [("dark".to_owned(), 2), ("techno".to_owned(), 1)]);
    let choices = lib.song_choices().unwrap();
    assert_eq!(choices.follow("a"), "b");
    assert!(choices.is_split(r"c:\beats\odd.flp"));
    // The path is matched ignoring case, as the library does.
    assert_eq!(lib.save_at(r"c:\BEATS\didums.flp").unwrap().map(|s| s.id), Some(1));
}

#[test]
fn nothing_can_write_to_the_library_and_a_missing_one_isnt_made() {
    let path = make("readonly.db", true);
    let before = std::fs::read(&path).unwrap();
    let lib = Library::open_read_only(&path).unwrap();
    assert_eq!(lib.saves().unwrap().len(), 2);
    lib.detail(1).unwrap();
    // A write is refused by SQLite itself, even through the library's own connection settings.
    let c = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    assert!(c.execute("UPDATE projects SET rating = 5", []).is_err());
    drop(c);
    drop(lib);
    assert_eq!(std::fs::read(&path).unwrap(), before);

    let missing = path.with_file_name("not-there.db");
    assert!(Library::open_read_only(&missing).is_err());
    assert!(!missing.exists());

    let not_a_library = path.with_file_name("plain.db");
    let _ = std::fs::remove_file(&not_a_library);
    Connection::open(&not_a_library).unwrap().execute_batch("CREATE TABLE x (y)").unwrap();
    let err = Library::open_read_only(&not_a_library).err().unwrap().to_string();
    assert!(err.contains("isn't a Music Organiser library"), "{err}");
}

#[test]
fn a_library_missing_later_tables_reads_them_as_empty() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-library");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bare.db");
    let _ = std::fs::remove_file(&path);
    let c = Connection::open(&path).unwrap();
    c.execute_batch(
        "CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT, name TEXT, daw TEXT, mtime INTEGER);
         INSERT INTO projects VALUES (1, 'C:\\a\\Song.flp', 'Song', 'FL Studio', 5);",
    )
    .unwrap();
    drop(c);
    let lib = Library::open_read_only(&path).unwrap();
    let saves = lib.saves().unwrap();
    assert_eq!((saves.len(), saves[0].plugin_count, saves[0].tags.len()), (1, 0, 0));
    assert!(lib.song_choices().unwrap().is_empty());
    assert!(lib.roots().unwrap().is_empty() && lib.all_tags().unwrap().is_empty());
    assert!(lib.pending_fixes().unwrap().is_empty() && lib.setting("x").unwrap().is_none());
    let d = lib.detail(1).unwrap().unwrap();
    assert!(d.plugins.is_empty() && d.samples.is_empty());
}
