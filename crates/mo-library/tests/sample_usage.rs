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
fn the_projects_counted_come_with_their_files() {
    let lib = Library::open_read_only(&make("projects.db", true)).unwrap();
    let got = lib.sample_usage_with_projects(since(), UsageRules::default(), &|_| true).unwrap();
    assert_eq!(got.uses, lib.sample_usage(since(), UsageRules::default()).unwrap());
    // Gone's file has gone, so it isn't counted, and isn't here either.
    let mut ids: Vec<i64> = got.projects.keys().copied().collect();
    ids.sort_unstable();
    assert_eq!(ids, [1, 2, 3]);
    let other = &got.projects[&3];
    assert_eq!(
        (other.id, other.path.as_str(), other.mtime, &*other.song_name, other.recent),
        (3, r"C:\beats\Other.flp", 1_717_200_000, "Other", true)
    );
    // Each says which song it's a save of, as the Songs page groups them: Song and Song_2 are one song, Other another.
    assert_eq!(got.projects[&1].song, got.projects[&2].song);
    assert_ne!(got.projects[&1].song, got.projects[&3].song);
    assert_eq!((&*got.projects[&1].song_name, &*got.projects[&2].song_name), ("Song", "Song"));
    // Saved since the date asked about: Song_2 (2025) and Other (2024), not Song (2020).
    assert_eq!([1, 2, 3].map(|id| got.projects[&id].recent), [false, true, true]);
    // Each sample says which projects use it.
    let kick = got.uses.iter().find(|u| u.path.to_lowercase().ends_with("kick.wav")).unwrap();
    assert_eq!(kick.project_ids, [1, 2, 3]);
    let pad = got.uses.iter().find(|u| u.path.to_lowercase().ends_with("pad.wav")).unwrap();
    assert_eq!(pad.project_ids, [1, 3]);
}

#[test]
fn the_saves_the_songs_page_leaves_out_dont_count() {
    let lib = Library::open_read_only(&make("listed.db", true)).unwrap();
    // As an FL backup copy or a Fix copy waiting for Keep is left out: Song_2 isn't listed.
    let listed = |path: &str| !path.ends_with("Song_2.flp");
    let rules = UsageRules { not_muted_in_newest: true, ..UsageRules::default() };
    let got = lib.sample_usage_with_projects(since(), rules, &listed).unwrap();
    let mut ids: Vec<i64> = got.projects.keys().copied().collect();
    ids.sort_unstable();
    assert_eq!(ids, [1, 3]);
    // Song is a song of its own now, and its newest save, so the kick muted in Song_2 counts for it again.
    assert_eq!(by_name(&got.uses, "kick.wav"), (2, 1, vec![2020, 2024]));
    assert_ne!(got.projects[&1].song, got.projects[&3].song);
    assert_eq!(&*got.projects[&1].song_name, "Song");
    // The snare was only in Song_2.
    assert!(got.uses.iter().all(|u| !u.path.ends_with("snare.wav")));
}

#[test]
fn a_copy_of_a_save_in_another_folder_says_so() {
    // Night drive, and the same file copied to a backup folder later: the Songs page shows the song once, from the one
    // found first, and hides the copy.
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-library-usage");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("copies.db");
    for end in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{end}", path.display()));
    }
    let c = Connection::open(&path).unwrap();
    c.execute_batch(
        "CREATE TABLE song_choices (kind TEXT, a TEXT, b TEXT, PRIMARY KEY (kind, a));
         CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE, name TEXT NOT NULL,
             daw TEXT NOT NULL, folder TEXT, size INTEGER, mtime INTEGER, file_missing INTEGER DEFAULT 0,
             note_count INTEGER, track_count INTEGER);
         CREATE TABLE samples (project_id INTEGER, path TEXT, found INTEGER);
         INSERT INTO projects (id, path, name, daw, folder, size, mtime, note_count, track_count) VALUES
             (1, 'C:\\beats\\Night drive.flp', 'Night drive', 'FL Studio', 'C:\\beats', 52000, 1717200000, 120, 8),
             (2, 'E:\\Backup\\beats\\Night drive.flp', 'Night drive', 'FL Studio', 'E:\\Backup\\beats', 52000,
              1748736000, 120, 8);
         INSERT INTO samples (project_id, path, found) VALUES (1, 'C:\\s\\kick.wav', 1), (2, 'C:\\s\\kick.wav', 1);",
    )
    .unwrap();
    drop(c);
    let lib = Library::open_read_only(&path).unwrap();
    let got = lib.sample_usage_with_projects(since(), UsageRules::default(), &|_| true).unwrap();
    let (save, copy) = (&got.projects[&1], &got.projects[&2]);
    assert_eq!((save.song, &*save.song_name, save.copy), (copy.song, "Night drive", false));
    assert!(copy.copy);
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

#[test]
fn a_samples_first_use_compares_the_saves_of_one_song() {
    use chrono::Datelike;
    let lib = Library::open_read_only(&make("first-use.db", true)).unwrap();
    let uses = lib.sample_usage(since(), UsageRules::default()).unwrap();
    let first = |name: &str| {
        uses.iter().find(|u| u.path.to_lowercase().ends_with(name)).and_then(|u| u.first_used).map(|d| d.year())
    };
    // The kick is in Song's first save (2020) and its newer save (2025): it's dated by the first.
    assert_eq!(first("kick.wav"), Some(2020));
    // The snare is only in the newer save: put in later, so it's dated by that save.
    assert_eq!(first("snare.wav"), Some(2025));
    assert_eq!(first("pad.wav"), Some(2020));
}
