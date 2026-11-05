//! Adding and removing folders in a library.db made with the C# app's tables: only the list changed is touched, and
//! a library that isn't there is never made.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use mo_library::Library;
use mo_library::folders::{Changed, EXPORT_FOLDER, FolderList, add_folder, remove_folder, set_export_folder};
use rusqlite::Connection;

const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
CREATE TABLE roots (path TEXT PRIMARY KEY);
CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE, name TEXT NOT NULL, daw TEXT NOT NULL);
INSERT INTO roots VALUES ('C:\beats');
INSERT INTO settings VALUES ('sample_folders', '["D:\\Samples"]'), ('theme', 'Paper');
INSERT INTO projects VALUES (1, 'C:\beats\Didums.flp', 'Didums', 'FL Studio');
"#;

fn make(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-library-folders");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    for end in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{end}", path.display()));
    }
    Connection::open(&path).unwrap().execute_batch(SCHEMA).unwrap();
    path
}

fn roots(path: &std::path::Path) -> Vec<String> {
    Library::open_read_only(path).unwrap().roots().unwrap()
}

#[test]
fn project_folders_are_added_and_removed_and_nothing_else_changes() {
    let path = make("roots.db");
    assert_eq!(
        add_folder(&path, FolderList::Projects, r"E:\Songs", &[]).unwrap(),
        Changed::Done(vec![r"C:\beats".into(), r"E:\Songs".into()])
    );
    // Already there, whatever its letter case or a separator at the end.
    assert_eq!(add_folder(&path, FolderList::Projects, r"c:\BEATS\", &[]).unwrap(), Changed::AsItWas);
    assert_eq!(roots(&path), [r"C:\beats", r"E:\Songs"]);
    assert_eq!(
        remove_folder(&path, FolderList::Projects, r"c:\beats", &[]).unwrap(),
        Changed::Done(vec![r"E:\Songs".into()])
    );
    assert_eq!(remove_folder(&path, FolderList::Projects, r"Z:\never", &[]).unwrap(), Changed::AsItWas);
    assert_eq!(roots(&path), [r"E:\Songs"]);
    // The folder's projects stay listed, and other settings are as they were.
    let lib = Library::open_read_only(&path).unwrap();
    assert_eq!(lib.setting("theme").unwrap().as_deref(), Some("Paper"));
    assert_eq!(lib.list_setting("sample_folders").unwrap(), [r"D:\Samples"]);
    let projects: i64 =
        Connection::open(&path).unwrap().query_row("SELECT COUNT(*) FROM projects", [], |r| r.get(0)).unwrap();
    assert_eq!(projects, 1);
}

#[test]
fn listed_folders_are_kept_as_the_csharp_app_reads_them() {
    let path = make("lists.db");
    add_folder(&path, FolderList::Samples, r"E:\Splice ✓", &[]).unwrap();
    add_folder(&path, FolderList::Midi, r"L:\New Midi", &[]).unwrap();
    let lib = Library::open_read_only(&path).unwrap();
    assert_eq!(lib.list_setting("sample_folders").unwrap(), [r"D:\Samples", r"E:\Splice ✓"]);
    assert_eq!(lib.list_setting("midi_folders").unwrap(), [r"L:\New Midi"]);
    drop(lib);
    remove_folder(&path, FolderList::Samples, r"D:\Samples", &[]).unwrap();
    assert_eq!(Library::open_read_only(&path).unwrap().list_setting("sample_folders").unwrap(), [r"E:\Splice ✓"]);
}

#[test]
fn plugin_folders_start_from_the_standard_ones() {
    let path = make("plugins.db");
    let defaults = vec![r"C:\Program Files\Common Files\VST3".to_owned()];
    // Adding to no list keeps the standard folders too, as the C# app does.
    assert_eq!(
        add_folder(&path, FolderList::Plugins, r"D:\VST", &defaults).unwrap(),
        Changed::Done(vec![defaults[0].clone(), r"D:\VST".into()])
    );
    remove_folder(&path, FolderList::Plugins, r"D:\VST", &defaults).unwrap();
    // Taking the last one off goes back to the standard ones (an empty list).
    remove_folder(&path, FolderList::Plugins, &defaults[0], &defaults).unwrap();
    assert!(Library::open_read_only(&path).unwrap().list_setting("plugin_folders").unwrap().is_empty());
}

#[test]
fn a_missing_library_or_table_is_never_made() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-library-folders");
    std::fs::create_dir_all(&dir).unwrap();
    let missing = dir.join("not-there.db");
    let _ = std::fs::remove_file(&missing);
    assert!(add_folder(&missing, FolderList::Projects, r"E:\Songs", &[]).is_err());
    assert!(!missing.exists());
    let bare = dir.join("bare.db");
    let _ = std::fs::remove_file(&bare);
    Connection::open(&bare).unwrap().execute_batch("CREATE TABLE projects (id INTEGER)").unwrap();
    let e = add_folder(&bare, FolderList::Samples, r"E:\x", &[]).unwrap_err();
    assert!(e.to_string().contains("no settings table"), "{e}");
    let tables: i64 = Connection::open(&bare)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(tables, 1);
}

#[test]
fn a_list_it_cant_read_is_never_written_over() {
    let path = make("unreadable.db");
    let c = Connection::open(&path).unwrap();
    c.execute_batch(
        r#"UPDATE settings SET value = '[{"path":"D:\\Samples"}]' WHERE key = 'sample_folders';
                       INSERT INTO settings VALUES ('midi_folders', 7);
                       INSERT INTO roots VALUES (NULL);"#,
    )
    .unwrap();
    let e = add_folder(&path, FolderList::Samples, r"E:\New", &[]).unwrap_err();
    assert!(e.to_string().contains("left as it is"), "{e}");
    assert!(add_folder(&path, FolderList::Midi, r"E:\New", &[]).is_err());
    let value: String =
        c.query_row("SELECT value FROM settings WHERE key = 'sample_folders'", [], |r| r.get(0)).unwrap();
    assert_eq!(value, r#"[{"path":"D:\\Samples"}]"#);
    // A root that isn't text doesn't stop the others changing, and stays as it was.
    add_folder(&path, FolderList::Projects, r"E:\Songs", &[]).unwrap();
    let nulls: i64 = c.query_row("SELECT COUNT(*) FROM roots WHERE path IS NULL", [], |r| r.get(0)).unwrap();
    assert_eq!(nulls, 1);
}

fn export_setting(path: &std::path::Path) -> Option<String> {
    Library::open_read_only(path).unwrap().setting(EXPORT_FOLDER).unwrap()
}

#[test]
fn the_export_folder_is_set_and_taken_back_and_nothing_else_changes() {
    let path = make("exports.db");
    assert!(set_export_folder(&path, Some(r"K:\Music Exports")).unwrap());
    assert_eq!(export_setting(&path).as_deref(), Some(r"K:\Music Exports"));
    // The same folder again, with a separator at the end: nothing to do. Spelt another way, it's kept as spelt
    // (on Linux it's another folder).
    assert!(!set_export_folder(&path, Some(r"K:\Music Exports\")).unwrap());
    assert_eq!(export_setting(&path).as_deref(), Some(r"K:\Music Exports"));
    assert!(set_export_folder(&path, Some(r"K:\music exports")).unwrap());
    assert_eq!(export_setting(&path).as_deref(), Some(r"K:\music exports"));
    assert!(set_export_folder(&path, Some(r"I:\Exports")).unwrap());
    assert_eq!(export_setting(&path).as_deref(), Some(r"I:\Exports"));
    // Back to Documents: kept as "", as the C# app keeps "none"; blank is Documents too.
    assert!(set_export_folder(&path, None).unwrap());
    assert_eq!(export_setting(&path).as_deref(), Some(""));
    assert!(!set_export_folder(&path, Some("  ")).unwrap());
    assert!(!set_export_folder(&path, None).unwrap());
    // The folders and the other settings are as they were.
    let lib = Library::open_read_only(&path).unwrap();
    assert_eq!(lib.setting("theme").unwrap().as_deref(), Some("Paper"));
    assert_eq!(lib.list_setting("sample_folders").unwrap(), [r"D:\Samples"]);
    assert_eq!(lib.roots().unwrap(), [r"C:\beats"]);
}

#[test]
fn an_export_folder_it_cant_read_or_a_missing_library_is_never_written() {
    let path = make("exports-unreadable.db");
    let c = Connection::open(&path).unwrap();
    c.execute("INSERT INTO settings VALUES (?1, x'00')", [EXPORT_FOLDER]).unwrap();
    let e = set_export_folder(&path, Some(r"K:\Exports")).unwrap_err();
    assert!(e.to_string().contains("isn't text"), "{e}");
    let kind: String =
        c.query_row("SELECT typeof(value) FROM settings WHERE key = ?1", [EXPORT_FOLDER], |r| r.get(0)).unwrap();
    assert_eq!(kind, "blob");
    // No library: none is made. No settings table: none is made.
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-library-folders");
    let missing = dir.join("exports-not-there.db");
    let _ = std::fs::remove_file(&missing);
    assert!(set_export_folder(&missing, Some(r"K:\Exports")).is_err());
    assert!(!missing.exists());
    let bare = dir.join("exports-bare.db");
    let _ = std::fs::remove_file(&bare);
    Connection::open(&bare).unwrap().execute_batch("CREATE TABLE projects (id INTEGER)").unwrap();
    let e = set_export_folder(&bare, Some(r"K:\Exports")).unwrap_err();
    assert!(e.to_string().contains("no settings table"), "{e}");
}
