//! The Plugins page's reads from a library.db made with the C# app's plugin tables, and from one made before they
//! existed.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use mo_library::plugins::ScanCounts;
use mo_library::{Library, PluginKind};
use rusqlite::Connection;

const OLD: &str = "
CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE, name TEXT NOT NULL,
    daw TEXT NOT NULL, folder TEXT, size INTEGER, mtime INTEGER);
INSERT INTO projects VALUES (1, 'C:\\a.flp', 'a', 'FL Studio', 'C:\\', 1, 1500000000);
";

const NOW: &str = r#"
CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE, name TEXT NOT NULL,
    daw TEXT NOT NULL, folder TEXT, size INTEGER, mtime INTEGER, file_missing INTEGER DEFAULT 0);
CREATE TABLE plugins (project_id INTEGER, name TEXT, name_key TEXT, vendor TEXT, kind INTEGER, format TEXT, path TEXT, installed INTEGER);
CREATE TABLE installed_plugins (name TEXT, name_key TEXT PRIMARY KEY);
CREATE TABLE plugin_scans (path TEXT PRIMARY KEY COLLATE NOCASE, size INTEGER, mtime INTEGER, scanned INTEGER, error TEXT);
CREATE TABLE scanned_plugins (path TEXT COLLATE NOCASE, format TEXT, id TEXT, name TEXT, name_key TEXT, vendor TEXT,
    version TEXT, kind TEXT, json TEXT);
CREATE TABLE plugin_space (name_key TEXT PRIMARY KEY, name TEXT, vendor TEXT, folders TEXT, measured INTEGER);
INSERT INTO projects VALUES (1, 'C:\a.flp', 'a', 'FL Studio', 'C:\', 1, 1500000000, 0),
                            (2, 'C:\gone.flp', 'gone', 'FL Studio', 'C:\', 1, 1600000000, 1);
INSERT INTO plugins VALUES (1, 'Pro-Q 3', 'proq3', 'FabFilter', 2, 'VST3', '', 1), (1, 'Gone Synth', '', '', 1, 'VST2', '', 0),
                           (2, 'Serum', 'serum', 'Xfer', 1, 'VST3', '', 1);
INSERT INTO installed_plugins VALUES ('FabFilter Pro-Q 3', 'fabfilterproq3');
INSERT INTO plugin_scans VALUES ('C:\VST3\FabFilter Pro-Q 3.vst3', 1, 1, 1, ''), ('C:\VST\bad.dll', 1, 1, 1, 'crashed'),
                                ('C:\VST\helper.dll', 1, 1, 1, 'not a VST2 plugin');
INSERT INTO scanned_plugins VALUES ('C:\VST3\FabFilter Pro-Q 3.vst3', 'VST3', '72C4DB717A4D459AB97E51745D84B39D',
    'FabFilter Pro-Q 3', 'fabfilterproq3', 'FabFilter', '3.24', 'effect',
    '{"id":"72C4DB717A4D459AB97E51745D84B39D","params":[{"id":1},{"id":2}]}'),
  ('C:\VST3\Other.vst3', 'VST3', 'X', 'Other', 'other', '', '', '', 'not json');
INSERT INTO plugin_space VALUES ('fabfilterproq3', 'FabFilter Pro-Q 3', 'FabFilter',
    '[{"Path":"C:\\VST3\\FabFilter Pro-Q 3.vst3","Kind":0,"Bytes":40000000,"SharedWith":[],"Added":false}]', 1760000000),
  ('broken', 'Broken', '', 'oops', 0);
INSERT INTO settings VALUES ('space_unused_years', '3');
"#;

/// A plugins table from before the app stored name keys, makers and formats.
const EARLY: &str = "
CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE, name TEXT NOT NULL,
    daw TEXT NOT NULL, folder TEXT, size INTEGER, mtime INTEGER);
CREATE TABLE plugins (project_id INTEGER, name TEXT, installed INTEGER);
INSERT INTO projects VALUES (1, 'C:\\a.flp', 'a', 'FL Studio', 'C:\\', 1, 1500000000);
INSERT INTO plugins VALUES (1, 'Serum 2', 1);
";

#[test]
fn reads_an_early_plugins_table() {
    let lib = Library::open_read_only(&make("early.db", EARLY)).unwrap();
    let uses = lib.plugin_uses().unwrap();
    assert_eq!(uses.len(), 1);
    assert_eq!((uses[0].name.as_str(), uses[0].key.as_str(), uses[0].installed), ("Serum 2", "serum2", true));
    assert_eq!((uses[0].vendor.as_str(), uses[0].format.as_str()), ("", ""));
}

fn make(name: &str, sql: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-library-plugins");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let _ = std::fs::remove_file(&path);
    Connection::open(&path).unwrap().execute_batch(sql).unwrap();
    path
}

#[test]
fn reads_what_the_plugins_page_needs() {
    let lib = Library::open_read_only(&make("now.db", NOW)).unwrap();
    let uses = lib.plugin_uses().unwrap();
    // The save whose file has gone is left out.
    assert_eq!(uses.len(), 2);
    assert_eq!((uses[0].name.as_str(), uses[0].kind, uses[0].installed), ("Pro-Q 3", PluginKind::Effect, true));
    assert_eq!(uses[0].daw, "FL Studio");
    // A row stored without its key gets one.
    assert_eq!(uses[1].key, "gonesynth");

    assert_eq!(lib.installed_names().unwrap(), ["FabFilter Pro-Q 3"]);
    let copies = lib.scanned_named("Pro-Q 3").unwrap();
    assert_eq!(copies.len(), 1);
    assert_eq!((copies[0].version.as_str(), copies[0].parameters), ("3.24", Some(2)));
    assert!(lib.scanned_named("Nothing here").unwrap().is_empty());
    // A line that isn't JSON still reads, with nothing from its JSON.
    assert_eq!(lib.scanned_named("Other").unwrap()[0].parameters, None);

    assert_eq!(lib.scan_counts().unwrap(), ScanCounts { files: 3, plugins: 2, failed: 1 });
    let spaces = lib.plugin_spaces().unwrap();
    assert_eq!(spaces["fabfilterproq3"].folders[0].bytes, 40_000_000);
    assert!(spaces["fabfilterproq3"].measured.is_some());
    assert!(spaces["broken"].folders.is_empty() && spaces["broken"].measured.is_none());
    assert_eq!(lib.setting(mo_library::plugins::UNUSED_YEARS_KEY).unwrap().as_deref(), Some("3"));
}

#[test]
fn a_library_from_before_the_plugin_tables_reads_as_empty() {
    let lib = Library::open_read_only(&make("old.db", OLD)).unwrap();
    assert!(lib.plugin_uses().unwrap().is_empty());
    assert!(lib.installed_names().unwrap().is_empty());
    assert!(lib.scanned_named("Serum").unwrap().is_empty());
    assert_eq!(lib.scan_counts().unwrap(), ScanCounts::default());
    assert!(lib.plugin_spaces().unwrap().is_empty());
}
