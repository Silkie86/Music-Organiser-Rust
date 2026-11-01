//! How many projects use each sample, as the Sounds page counts it (the C# app's `SampleUsageByAge`).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use chrono::NaiveDate;
use mo_library::{Library, SampleUse, UsageRules};
use rusqlite::Connection;

fn make(name: &str, unplayed: bool) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-library-usage");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    for end in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{end}", path.display()));
    }
    let c = Connection::open(&path).unwrap();
    let column = if unplayed { ", unplayed INTEGER NOT NULL DEFAULT 0" } else { "" };
    c.execute_batch(&format!(
        "CREATE TABLE song_choices (kind TEXT, a TEXT, b TEXT, PRIMARY KEY (kind, a));
         CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE, name TEXT NOT NULL,
             daw TEXT NOT NULL, folder TEXT, size INTEGER, mtime INTEGER, file_missing INTEGER DEFAULT 0);
         CREATE TABLE samples (project_id INTEGER, path TEXT, found INTEGER{column});
         -- Song and Song_2 are one song (Song_2 the newest); Other another; Gone's file has gone.
         INSERT INTO projects (id, path, name, daw, folder, mtime) VALUES
             (1, 'C:\\beats\\Song.flp', 'Song', 'FL Studio', 'C:\\beats', 1590969600),
             (2, 'C:\\beats\\Song_2.flp', 'Song_2', 'FL Studio', 'C:\\beats', 1748736000),
             (3, 'C:\\beats\\Other.flp', 'Other', 'FL Studio', 'C:\\beats', 1717200000);
         INSERT INTO projects (id, path, name, daw, folder, mtime, file_missing) VALUES
             (4, 'C:\\beats\\Gone.flp', 'Gone', 'FL Studio', 'C:\\beats', 1717200000, 1);
         INSERT INTO samples (project_id, path, found) VALUES
             (1, 'C:\\s\\kick.wav', 1), (2, 'C:\\S\\KICK.wav', 1), (3, 'c:\\s\\kick.wav', 1), (4, 'C:\\s\\kick.wav', 1),
             (1, 'C:\\s\\pad.wav', 1), (3, 'C:\\s\\pad.wav', 1), (3, 'C:\\s\\pad.wav', 1),
             (2, 'snare.wav', 1);"
    ))
    .unwrap();
    if unplayed {
        // The pad has no notes in Song; the kick is muted in Song_2, the song's newest save.
        c.execute_batch(
            "UPDATE samples SET unplayed = 1 WHERE project_id = 1 AND path LIKE '%pad%';
             UPDATE samples SET unplayed = 2 WHERE project_id = 2 AND path LIKE '%kick%';",
        )
        .unwrap();
    }
    drop(c);
    path
}

fn by_name(uses: &[SampleUse], name: &str) -> (u32, u32, Vec<i32>) {
    let u = uses.iter().find(|u| u.path.to_lowercase().ends_with(name)).expect(name);
    (u.projects, u.recent, u.years.clone())
}

fn since() -> chrono::NaiveDateTime {
    NaiveDate::from_ymd_opt(2023, 1, 1).unwrap().and_hms_opt(0, 0, 0).unwrap()
}

#[test]
fn counts_each_samples_projects_ignoring_case_and_gone_files() {
    let lib = Library::open_read_only(&make("usage.db", true)).unwrap();
    let uses = lib.sample_usage(since(), UsageRules::default()).unwrap();
    assert_eq!(uses.len(), 3);
    assert_eq!(by_name(&uses, "kick.wav"), (3, 2, vec![2020, 2024, 2025]));
    // A project listing a sample twice counts once.
    assert_eq!(by_name(&uses, "pad.wav"), (2, 1, vec![2020, 2024]));
    assert_eq!(by_name(&uses, "snare.wav"), (1, 1, vec![2025]));

    let notes = lib.sample_usage(since(), UsageRules { needs_notes: true, ..UsageRules::default() }).unwrap();
    assert_eq!(by_name(&notes, "pad.wav"), (1, 1, vec![2024]));
    assert_eq!(by_name(&notes, "kick.wav").0, 3);
}

#[test]
fn a_sample_muted_in_a_songs_newest_save_doesnt_count_for_that_song() {
    let lib = Library::open_read_only(&make("muted.db", true)).unwrap();
    let rules = UsageRules { not_muted_in_newest: true, ..UsageRules::default() };
    let uses = lib.sample_usage(since(), rules).unwrap();
    // Song's two saves are left out; Other still uses it.
    assert_eq!(by_name(&uses, "kick.wav"), (1, 1, vec![2024]));
    assert_eq!(by_name(&uses, "pad.wav"), (2, 1, vec![2020, 2024]));
    assert_eq!(by_name(&uses, "snare.wav").0, 1);
}

#[test]
fn an_older_library_without_unplayed_counts_everything() {
    let lib = Library::open_read_only(&make("older.db", false)).unwrap();
    for rules in [UsageRules::default(), UsageRules { needs_notes: true, not_muted_in_newest: true }] {
        let uses = lib.sample_usage(since(), rules).unwrap();
        assert_eq!(by_name(&uses, "kick.wav").0, 3);
        assert_eq!(by_name(&uses, "pad.wav").0, 2);
    }
}
