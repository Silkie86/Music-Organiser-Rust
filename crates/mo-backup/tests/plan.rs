//! The backup list: what goes and what's left out. Only made-up paths: nothing here reads or writes a folder.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use mo_backup::plan::{BigSaves, Folders, LeftOut, Rules, SongInfo, Used, place_on_backup, plan};
use mo_space::size::FileEntry;
use mo_space::song::{CompactItem, CompactKind, SongSave, SongSpaceReport};

const MB: u64 = 1 << 20;

/// The backup somewhere none of the test's files are, and no folder marked "always".
fn nowhere() -> Folders<'static> {
    Folders { backup: Path::new("/backup"), always: &[] }
}

/// A save, written `hours` after noon on 14 March 2026.
fn save(path: &str, hours: i64) -> SongSave {
    SongSave {
        path: path.to_owned(),
        size: 0,
        modified: NaiveDate::from_ymd_opt(2026, 3, 14).unwrap().and_hms_opt(12, 0, 0).unwrap()
            + chrono::TimeDelta::hours(hours),
    }
}

fn file(path: &str, size: u64) -> FileEntry {
    FileEntry {
        path: PathBuf::from(path),
        size,
        modified: NaiveDate::from_ymd_opt(2026, 3, 14).unwrap().and_hms_opt(12, 0, 0).unwrap(),
    }
}

fn item(kind: CompactKind, path: &str) -> CompactItem {
    CompactItem { kind, path: path.to_owned(), bytes: 0, modified: None, same_as: None }
}

fn why(p: &mo_backup::plan::Plan, path: &str) -> Option<LeftOut> {
    p.left_out.iter().find(|s| s.file.path == Path::new(path)).map(|s| s.why)
}

fn goes(p: &mo_backup::plan::Plan, path: &str) -> bool {
    p.copy.iter().any(|f| f.path == Path::new(path))
}

#[test]
fn big_saves_from_the_middle_of_a_sitting_stay_out_small_ones_go() {
    let report = SongSpaceReport {
        items: vec![
            item(CompactKind::ThinnedSave, "/music/Song/Song 2.flp"),
            item(CompactKind::ThinnedSave, "/music/Song/Song 3.flp"),
        ],
        ..SongSpaceReport::default()
    };
    // In name order, not newest first: the newest is found by when it was saved.
    let saves = [
        save("/music/Song/Song 2.flp", 1),
        save("/music/Song/Song 3.flp", 2),
        save("/music/Song/Song 4.flp", 30),
        save("/music/Song/Song.flp", 0),
    ];
    let files = vec![
        file("/music/Song/Song.flp", 6 * MB),
        file("/music/Song/Song 2.flp", 6 * MB),
        file("/music/Song/Song 3.flp", MB),
        file("/music/Song/Song 4.flp", 6 * MB),
    ];
    let songs = [SongInfo { report: &report, saves: &saves }];
    let p = plan(files.clone(), &songs, &Used::default(), nowhere(), Rules::default());
    assert!(goes(&p, "/music/Song/Song.flp"), "the first save goes");
    assert_eq!(why(&p, "/music/Song/Song 2.flp"), Some(LeftOut::MidSittingSave));
    assert!(goes(&p, "/music/Song/Song 3.flp"), "a small save goes even from the middle of a sitting");
    assert!(goes(&p, "/music/Song/Song 4.flp"), "the newest goes");
    assert_eq!(p.copy_bytes(), 13 * MB);
    assert_eq!(p.left_out_bytes(), 6 * MB);

    let newest_only = Rules { big_saves: BigSaves::NewestOnly, ..Rules::default() };
    let p = plan(files, &songs, &Used::default(), nowhere(), newest_only);
    assert_eq!(why(&p, "/music/Song/Song.flp"), Some(LeftOut::OlderBigSave));
    assert!(goes(&p, "/music/Song/Song 3.flp"), "small saves still go");
    assert!(goes(&p, "/music/Song/Song 4.flp"));
}

#[test]
fn copies_old_autosaves_and_caches_stay_out_at_any_size() {
    let report = SongSpaceReport {
        items: vec![
            item(CompactKind::SameSave, "/music/S1/Tune copy.song"),
            item(CompactKind::Backup, "/music/Backup/Tune overwritten at 1.flp"),
            item(CompactKind::Cache, "/music/S1/Cache"),
        ],
        ..SongSpaceReport::default()
    };
    let saves = [save("/music/S1/Tune.song", 0), save("/music/S1/Tune copy.song", 0)];
    let files = vec![
        file("/music/S1/Tune.song", 100),
        file("/music/S1/Tune copy.song", 100),
        file("/music/Backup/Tune overwritten at 1.flp", 100),
        file("/music/S1/Cache/waves/a.cache", 100),
        file("/music/S1/Cachet.txt", 100),
    ];
    let p = plan(files, &[SongInfo { report: &report, saves: &saves }], &Used::default(), nowhere(), Rules::default());
    assert_eq!(why(&p, "/music/S1/Tune copy.song"), Some(LeftOut::SameSave));
    assert_eq!(why(&p, "/music/Backup/Tune overwritten at 1.flp"), Some(LeftOut::OldAutosave));
    assert_eq!(why(&p, "/music/S1/Cache/waves/a.cache"), Some(LeftOut::Cache));
    assert!(goes(&p, "/music/S1/Cachet.txt"), "only what's inside the cache folder");
    assert!(goes(&p, "/music/S1/Tune.song"));
}

#[test]
fn big_sounds_go_only_when_a_project_uses_them() {
    // Projects keep paths from the PC that saved them: a sample counts as used by name too.
    let used = Used::new(["C:/Old PC/Samples/Big Pad.wav", "/samples/Used Loop.WAV"]);
    let files = vec![
        file("/samples/Big Pad.wav", 50 * MB),
        file("/samples/used loop.wav", 50 * MB),
        file("/samples/Unused Stem.wav", 50 * MB),
        file("/samples/Small Unused.wav", MB),
        file("/samples/Library.nkx", 50 * MB),
        file("/videos/Gig.mp4", 500 * MB),
        file("/music/Huge.flp", 80 * MB),
    ];
    let p = plan(files.clone(), &[], &used, nowhere(), Rules::default());
    assert!(goes(&p, "/samples/Big Pad.wav"), "used by name");
    assert!(goes(&p, "/samples/used loop.wav"), "letter case aside");
    assert_eq!(why(&p, "/samples/Unused Stem.wav"), Some(LeftOut::UnusedSound));
    assert!(goes(&p, "/samples/Small Unused.wav"), "small unused sounds still go");
    assert!(goes(&p, "/samples/Library.nkx"), "a plugin uses sample libraries: they go unless you say");
    assert!(goes(&p, "/videos/Gig.mp4"), "a big file of a kind that isn't a sound goes");
    assert!(goes(&p, "/music/Huge.flp"), "a big project is never taken for a sound");

    let lean = Rules { leave_out_unused_libraries: true, ..Rules::default() };
    let p = plan(files, &[], &used, nowhere(), lean);
    assert_eq!(why(&p, "/samples/Library.nkx"), Some(LeftOut::UnusedSound));
    assert_eq!(p.left_out[0].file.path, Path::new("/samples/Library.nkx"), "biggest first, then by path");
}

#[test]
fn a_songs_own_audio_goes_even_when_no_project_names_it() {
    // A mixdown, a stem or a recording beside a save, or anywhere in a song's own folder, is your music.
    let report = SongSpaceReport { own_folder: true, folder: "/music/Song".to_owned(), ..SongSpaceReport::default() };
    let saves = [save("/music/Song/Song.flp", 0), save("/projects/Other.flp", 0)];
    let files = vec![
        file("/music/Song/Bounces/Final.wav", 40 * MB),
        file("/projects/Other mixdown.wav", 40 * MB),
        file("/projects/Samples/Unused.wav", 40 * MB),
        file("/exports/Old master.wav", 40 * MB),
    ];
    let songs = [SongInfo { report: &report, saves: &saves }];
    let p = plan(files.clone(), &songs, &Used::default(), nowhere(), Rules::default());
    assert!(goes(&p, "/music/Song/Bounces/Final.wav"));
    assert!(goes(&p, "/projects/Other mixdown.wav"), "right beside a save");
    assert_eq!(why(&p, "/projects/Samples/Unused.wav"), Some(LeftOut::UnusedSound), "only right beside a save");
    assert_eq!(why(&p, "/exports/Old master.wav"), Some(LeftOut::UnusedSound));

    let always = [PathBuf::from("/exports")];
    let p = plan(
        files,
        &songs,
        &Used::default(),
        Folders { backup: Path::new("/backup"), always: &always },
        Rules::default(),
    );
    assert!(goes(&p, "/exports/Old master.wav"), "a folder you mark always goes");
}

#[test]
fn the_newest_save_goes_whatever_another_songs_list_says() {
    let a = SongSpaceReport::default();
    let b = SongSpaceReport {
        items: vec![item(CompactKind::ThinnedSave, "/music/Song.flp"), item(CompactKind::SameSave, "/music/Song.flp")],
        ..SongSpaceReport::default()
    };
    let a_saves = [save("/music/Song.flp", 5)];
    let b_saves = [save("/music/Song.flp", 5), save("/music/Song 9.flp", 9)];
    let songs = [SongInfo { report: &a, saves: &a_saves }, SongInfo { report: &b, saves: &b_saves }];
    let p = plan(vec![file("/music/Song.flp", 50 * MB)], &songs, &Used::default(), nowhere(), Rules::default());
    assert!(goes(&p, "/music/Song.flp"));
}

#[test]
fn the_backup_never_copies_itself() {
    let files = vec![file("/music/Backup of music/music/a.wav", 10), file("/music/a.wav", 10)];
    let folders = Folders { backup: Path::new("/music/Backup of music"), always: &[] };
    let p = plan(files, &[], &Used::default(), folders, Rules::default());
    assert_eq!(p.copy.len(), 1);
    assert!(goes(&p, "/music/a.wav"));
    assert!(p.left_out.is_empty());
}

#[test]
fn a_file_met_twice_goes_once() {
    let files = vec![file("/music/a.mid", 10), file("/music/a.mid", 10), file("/music/A.MID", 10)];
    let p = plan(files, &[], &Used::default(), nowhere(), Rules::default());
    // Windows ignores letter case in names; a Mac or Linux can hold both.
    assert_eq!(p.copy.len(), if cfg!(windows) { 1 } else { 2 });
}

#[test]
fn place_on_backup_keeps_the_path_under_the_drive() {
    #[cfg(windows)]
    {
        assert_eq!(
            place_on_backup(Path::new(r"E:\Music Backup"), Path::new(r"i:\Fl Studio Data\x.flp")),
            Some(PathBuf::from(r"E:\Music Backup\I\Fl Studio Data\x.flp"))
        );
        assert_eq!(
            place_on_backup(Path::new(r"E:\B"), Path::new(r"\\nas\music\a.wav")),
            Some(PathBuf::from(r"E:\B\Network\nas\music\a.wav"))
        );
        assert_eq!(place_on_backup(Path::new(r"E:\B"), Path::new(r"I:\a\..\b.wav")), None);
    }
    assert_eq!(
        place_on_backup(Path::new("/backup"), Path::new("/home/me/x.flp")),
        if cfg!(windows) { None } else { Some(PathBuf::from("/backup/home/me/x.flp")) }
    );
    assert_eq!(place_on_backup(Path::new("/backup"), Path::new("relative/x.flp")), None);
    assert_eq!(place_on_backup(Path::new("/backup"), Path::new("/a/../b")), None);
}
