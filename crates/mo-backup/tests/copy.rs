//! Copying to a backup, on folders made in the test build's own fixtures folder under fixed names, never a random temp
//! one and never a real folder.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

use mo_backup::copy::{CantStart, OLDER_VERSIONS, Options, Outcome, run};
use mo_backup::plan::place_on_backup;
use mo_space::size::files;

fn fixtures(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    // Absolute, as the backup places files by their whole path.
    fs::canonicalize(&dir).unwrap()
}

fn write(path: &Path, bytes: &[u8], secs_ago: u64) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
    let when = SystemTime::now() - Duration::from_secs(secs_ago);
    fs::File::options().write(true).open(path).unwrap().set_modified(when).unwrap();
}

fn of_lines(lines: &[mo_backup::copy::Line], name: &str) -> Outcome {
    lines.iter().find(|l| l.source.ends_with(name)).unwrap().outcome.clone()
}

fn tree(dir: &Path) -> Vec<String> {
    let mut all: Vec<String> = files(dir, &AtomicBool::new(false))
        .unwrap()
        .into_iter()
        .map(|f| f.path.strip_prefix(dir).unwrap().to_string_lossy().replace('\\', "/"))
        .collect();
    all.sort();
    all
}

#[test]
fn copies_checks_and_keeps_small_older_versions() {
    let root = fixtures("backup-copy");
    let music = root.join("music");
    let backup = root.join("backup");
    write(&music.join("Song/Song.flp"), b"first", 100);
    write(&music.join("Song/Big.wav"), &[1u8; 64], 100);
    let stop = AtomicBool::new(false);
    let options = Options { keep_older_below: 32, stamp: "2026-10-31 22.40".into() };

    fs::create_dir_all(&backup).unwrap();
    let lines = run(&files(&music, &stop).unwrap(), &backup, &options, &stop).unwrap().lines;
    assert!(lines.iter().all(|l| l.outcome == Outcome::New), "{lines:?}");
    let song_copy = place_on_backup(&backup, &music.join("Song/Song.flp")).unwrap();
    assert_eq!(fs::read(&song_copy).unwrap(), b"first");
    assert_eq!(
        fs::metadata(&song_copy).unwrap().modified().unwrap(),
        fs::metadata(music.join("Song/Song.flp")).unwrap().modified().unwrap(),
        "the copy keeps the file's time"
    );

    // Nothing changed: nothing written.
    let lines = run(&files(&music, &stop).unwrap(), &backup, &options, &stop).unwrap().lines;
    assert!(lines.iter().all(|l| l.outcome == Outcome::UpToDate && l.written == 0), "{lines:?}");

    // Both changed: the small one's old copy is kept, the big one's isn't.
    write(&music.join("Song/Song.flp"), b"second", 10);
    write(&music.join("Song/Big.wav"), &[2u8; 64], 10);
    let lines = run(&files(&music, &stop).unwrap(), &backup, &options, &stop).unwrap().lines;
    let of = |name: &str| lines.iter().find(|l| l.source.ends_with(name)).unwrap().outcome.clone();
    assert_eq!(of("Song.flp"), Outcome::Updated { kept_older: true });
    assert_eq!(of("Big.wav"), Outcome::Updated { kept_older: false });
    assert_eq!(fs::read(&song_copy).unwrap(), b"second");
    let rel = song_copy.strip_prefix(&backup).unwrap();
    let older = backup.join(OLDER_VERSIONS).join("2026-10-31 22.40").join(rel);
    assert_eq!(fs::read(older).unwrap(), b"first");
    assert!(!tree(&backup).iter().any(|p| p.ends_with(".mo-part")), "no half copies left");

    // Saved again within the same stamp: the version kept before isn't replaced.
    write(&music.join("Song/Song.flp"), b"third", 5);
    run(&files(&music, &stop).unwrap(), &backup, &options, &stop).unwrap();
    let older2 = backup.join(OLDER_VERSIONS).join("2026-10-31 22.40").join(rel.with_file_name("Song (2).flp"));
    assert_eq!(fs::read(older2).unwrap(), b"second");
    assert_eq!(fs::read(backup.join(OLDER_VERSIONS).join("2026-10-31 22.40").join(rel)).unwrap(), b"first");

    // Only its time changed: no copy and no older version, the backup's copy just takes the time.
    let versions = tree(&backup).len();
    write(&music.join("Song/Song.flp"), b"third", 1);
    let lines = run(&files(&music, &stop).unwrap(), &backup, &options, &stop).unwrap().lines;
    assert_eq!(of_lines(&lines, "Song.flp"), Outcome::UpToDate);
    assert_eq!(tree(&backup).len(), versions);
    let lines = run(&files(&music, &stop).unwrap(), &backup, &options, &stop).unwrap().lines;
    assert!(lines.iter().all(|l| l.written == 0));

    // The music folder is only read.
    assert_eq!(tree(&music), ["Song/Big.wav", "Song/Song.flp"]);
}

#[test]
fn a_file_that_cant_be_read_fails_alone() {
    let root = fixtures("backup-copy-fail");
    let music = root.join("music");
    let backup = root.join("backup");
    write(&music.join("a.mid"), b"notes", 100);
    let stop = AtomicBool::new(false);
    let mut list = files(&music, &stop).unwrap();
    let mut gone = list[0].clone();
    gone.path = music.join("gone.mid");
    list.push(gone);
    fs::create_dir_all(&backup).unwrap();
    let lines = run(&list, &backup, &Options::new("x"), &stop).unwrap().lines;
    assert_eq!(lines.iter().filter(|l| l.outcome == Outcome::New).count(), 1);
    assert!(matches!(lines.iter().find(|l| l.source.ends_with("gone.mid")).unwrap().outcome, Outcome::Failed(_)));
}

#[test]
fn a_stopped_run_says_so_and_lists_every_file() {
    let root = fixtures("backup-copy-stop");
    let music = root.join("music");
    write(&music.join("a.mid"), b"notes", 100);
    let backup = root.join("backup");
    fs::create_dir_all(&backup).unwrap();
    let list = files(&music, &AtomicBool::new(false)).unwrap();
    let report = run(&list, &backup, &Options::new("x"), &AtomicBool::new(true)).unwrap();
    assert!(report.stopped);
    assert_eq!(report.lines.len(), 1);
    assert_eq!(report.lines[0].outcome, Outcome::NotReached);
}

#[test]
fn no_backup_folder_no_run() {
    let root = fixtures("backup-copy-missing");
    let music = root.join("music");
    write(&music.join("a.mid"), b"notes", 100);
    let list = files(&music, &AtomicBool::new(false)).unwrap();
    let backup = root.join("not plugged in");
    assert_eq!(
        run(&list, &backup, &Options::new("x"), &AtomicBool::new(false)),
        Err(CantStart::NoBackupFolder(backup.clone()))
    );
    assert!(!backup.exists(), "nothing is made in its place");
}

#[test]
fn two_files_for_one_place_go_once() {
    let root = fixtures("backup-copy-clash");
    let music = root.join("music");
    let backup = root.join("backup");
    fs::create_dir_all(&backup).unwrap();
    write(&music.join("a.mid"), b"notes", 100);
    let mut list = files(&music, &AtomicBool::new(false)).unwrap();
    let mut other = list[0].clone();
    other.path = music.join("A.MID");
    list.push(other);
    let lines = run(&list, &backup, &Options::new("x"), &AtomicBool::new(false)).unwrap().lines;
    assert_eq!(lines[0].outcome, Outcome::New);
    // Windows and (at first) a Mac ignore letter case in names, so "A.MID" would land on "a.mid"; Linux keeps both.
    let clash = matches!(&lines[1].outcome, Outcome::Failed(why) if why.contains("same place"));
    assert_eq!(clash, cfg!(any(windows, target_os = "macos")), "{:?}", lines[1]);
}

#[test]
fn a_file_named_like_a_half_copy_is_never_mixed_up_with_one() {
    let root = fixtures("backup-copy-part");
    let music = root.join("music");
    let backup = root.join("backup");
    fs::create_dir_all(&backup).unwrap();
    write(&music.join("x.flp"), b"song", 100);
    write(&music.join("x.flp.mo-part"), b"odd", 100);
    let list = files(&music, &AtomicBool::new(false)).unwrap();
    let lines = run(&list, &backup, &Options::new("x"), &AtomicBool::new(false)).unwrap().lines;
    assert_eq!(of_lines(&lines, "x.flp"), Outcome::New);
    assert!(matches!(of_lines(&lines, "x.flp.mo-part"), Outcome::Failed(_)));
}

#[test]
fn the_same_bytes_are_stored_once() {
    let root = fixtures("backup-copy-share");
    let music = root.join("music");
    let backup = root.join("backup");
    fs::create_dir_all(&backup).unwrap();
    let kick: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    write(&music.join("Pack/Kick.wav"), &kick, 300);
    let stop = AtomicBool::new(false);
    let report = run(&files(&music, &stop).unwrap(), &backup, &Options::new("x"), &stop).unwrap();
    assert!(report.record_not_saved.is_none(), "{report:?}");

    // A copied folder with the same kick, saved at another time.
    write(&music.join("Song/Samples/Kick.wav"), &kick, 100);
    let lines = run(&files(&music, &stop).unwrap(), &backup, &Options::new("x"), &stop).unwrap().lines;
    let line = lines.iter().find(|l| l.source.ends_with("Song/Samples/Kick.wav")).unwrap();
    assert_eq!(line.outcome, Outcome::New);
    assert_eq!(line.written, 0);
    // Some drives can't share (FAT, exFAT): the test's own drive can on Linux, Mac and Windows (NTFS).
    assert!(line.shared, "{line:?}");
    let copy = place_on_backup(&backup, &music.join("Song/Samples/Kick.wav")).unwrap();
    assert_eq!(fs::read(&copy).unwrap(), kick);

    // Both are up to date next time, though they share one time between them.
    let lines = run(&files(&music, &stop).unwrap(), &backup, &Options::new("x"), &stop).unwrap().lines;
    assert!(lines.iter().all(|l| l.outcome == Outcome::UpToDate && l.written == 0), "{lines:?}");

    // One of them changes: only its place on the backup changes; the other keeps the old bytes.
    let mut louder = kick.clone();
    louder[100] = 0xFF;
    write(&music.join("Song/Samples/Kick.wav"), &louder, 10);
    run(&files(&music, &stop).unwrap(), &backup, &Options::new("x"), &stop).unwrap();
    assert_eq!(fs::read(&copy).unwrap(), louder);
    assert_eq!(fs::read(place_on_backup(&backup, &music.join("Pack/Kick.wav")).unwrap()).unwrap(), kick);
}

#[test]
fn a_record_line_that_lies_is_never_used_to_share() {
    let root = fixtures("backup-copy-record-lies");
    let music = root.join("music");
    let backup = root.join("backup");
    fs::create_dir_all(&backup).unwrap();
    let bytes: Vec<u8> = (0..100_000u32).map(|i| (i % 7) as u8).collect();
    write(&music.join("a.wav"), &bytes, 300);
    let stop = AtomicBool::new(false);
    run(&files(&music, &stop).unwrap(), &backup, &Options::new("x"), &stop).unwrap();
    // Someone changes the backup's copy by hand (same size): the record still names its old bytes.
    let a_copy = place_on_backup(&backup, &music.join("a.wav")).unwrap();
    fs::write(&a_copy, vec![9u8; bytes.len()]).unwrap();
    // And a line of nonsense.
    let record = backup.join(".mo-backup/record.tsv");
    let mut text = fs::read_to_string(&record).unwrap();
    text.push_str("not a line\n");
    fs::write(&record, text).unwrap();

    write(&music.join("b.wav"), &bytes, 100);
    let lines = run(&files(&music, &stop).unwrap(), &backup, &Options::new("x"), &stop).unwrap().lines;
    let b = lines.iter().find(|l| l.source.ends_with("b.wav")).unwrap();
    assert!(b.written == bytes.len() as u64 || b.shared, "{b:?}");
    assert_eq!(fs::read(place_on_backup(&backup, &music.join("b.wav")).unwrap()).unwrap(), bytes);
    // The changed copy of a.wav isn't taken as up to date: it's put right.
    let a = lines.iter().find(|l| l.source.ends_with("a.wav")).unwrap();
    assert!(matches!(a.outcome, Outcome::Updated { .. }), "{a:?}");
    assert_eq!(fs::read(&a_copy).unwrap(), bytes);
}

#[test]
fn a_lost_record_is_built_again_and_sharing_still_works() {
    let root = fixtures("backup-copy-lost-record");
    let music = root.join("music");
    let backup = root.join("backup");
    fs::create_dir_all(&backup).unwrap();
    let bytes: Vec<u8> = (0..100_000u32).map(|i| (i % 13) as u8).collect();
    write(&music.join("a.wav"), &bytes, 300);
    let stop = AtomicBool::new(false);
    run(&files(&music, &stop).unwrap(), &backup, &Options::new("x"), &stop).unwrap();
    fs::remove_file(backup.join(".mo-backup/record.tsv")).unwrap();
    // A run with nothing new records what's there again.
    run(&files(&music, &stop).unwrap(), &backup, &Options::new("x"), &stop).unwrap();
    write(&music.join("copy/a.wav"), &bytes, 100);
    let lines = run(&files(&music, &stop).unwrap(), &backup, &Options::new("x"), &stop).unwrap().lines;
    let copy = lines.iter().find(|l| l.source.ends_with("copy/a.wav")).unwrap();
    assert!(copy.shared, "{copy:?}");
}

#[test]
fn a_record_that_cant_be_read_is_left_alone() {
    let root = fixtures("backup-copy-unreadable-record");
    let music = root.join("music");
    let backup = root.join("backup");
    // Something that can't be read as a file where the record goes.
    fs::create_dir_all(backup.join(".mo-backup/record.tsv")).unwrap();
    write(&music.join("a.mid"), b"notes", 100);
    let stop = AtomicBool::new(false);
    let report = run(&files(&music, &stop).unwrap(), &backup, &Options::new("x"), &stop).unwrap();
    assert_eq!(report.lines[0].outcome, Outcome::New);
    assert!(report.record_not_saved.is_some());
    assert!(backup.join(".mo-backup/record.tsv").is_dir());
}
