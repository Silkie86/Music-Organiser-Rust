//! The backup's older versions, listed and removed, on folders made in the test build's own fixtures folder under fixed
//! names, never a random temp one and never a real folder.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

use chrono::NaiveDateTime;
use mo_backup::copy::{CantStart, OLDER_VERSIONS, Options, Outcome, run};
use mo_backup::older::{Kept, Run, read, remove, stamp, stamp_time};
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

fn at(text: &str) -> NaiveDateTime {
    NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M").unwrap()
}

/// A folder link, as Windows has them (a junction) or a symbolic link elsewhere.
fn folder_link(target: &Path, at: &Path) {
    #[cfg(windows)]
    junction::create(target, at).unwrap();
    #[cfg(not(windows))]
    std::os::unix::fs::symlink(target, at).unwrap();
}

fn is_link(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink())
}

fn no_progress(_: usize, _: usize) {}

/// Every file under a folder, relative, with forward slashes, sorted (links aren't followed).
fn tree(dir: &Path) -> Vec<String> {
    let mut all: Vec<String> = files(dir, &AtomicBool::new(false))
        .unwrap()
        .into_iter()
        .map(|f| f.path.strip_prefix(dir).unwrap().to_string_lossy().replace('\\', "/"))
        .collect();
    all.sort();
    all
}

/// Two runs' older versions, and things the backup never made: a folder of notes, a loose file, and a file named like
/// a run's folder.
fn two_runs(root: &Path) -> PathBuf {
    let backup = root.join("backup");
    let older = backup.join(OLDER_VERSIONS);
    write(&older.join("2026-01-05 10.00/I/Projects/Song/Song.flp"), b"jan song", 100);
    write(&older.join("2026-01-05 10.00/I/Projects/Song/Song (2).flp"), b"jan song 2", 100);
    write(&older.join("2026-10-31 22.40/I/Projects/Other/Other.flp"), b"october", 100);
    write(&older.join("My notes/what I kept.txt"), b"mine", 100);
    write(&older.join("readme.txt"), b"mine too", 100);
    write(&older.join("2026-02-01 10.00"), b"a file, not a run's folder", 100);
    backup
}

#[test]
fn a_runs_folder_name_is_its_time_and_nothing_else_counts() {
    let when = at("2026-11-03 14:40");
    assert_eq!(stamp(when), "2026-11-03 14.40");
    assert_eq!(stamp_time("2026-11-03 14.40"), Some(when));
    for not_a_run in [
        "2026-1-3 4.05",
        "2026-11-03 14.40 (2)",
        "2026-11-03 14:40",
        "2026-11-03 14.40.00",
        "2026-13-01 10.00",
        " 2026-11-03 14.40",
        "x",
        "",
        "../2026-11-03 14.40",
    ] {
        assert_eq!(stamp_time(not_a_run), None, "{not_a_run:?}");
    }
}

#[test]
fn lists_each_runs_older_versions_oldest_first_and_leaves_the_rest_out() {
    let root = fixtures("backup-older-list");
    let backup = two_runs(&root);
    let kept = read(&backup, &AtomicBool::new(false)).unwrap();
    let stamps: Vec<&str> = kept.runs.iter().map(|r| r.stamp.as_str()).collect();
    assert_eq!(stamps, ["2026-01-05 10.00", "2026-10-31 22.40"]);
    assert_eq!(kept.runs[0].when, at("2026-01-05 10:00"));
    assert_eq!(kept.runs[0].versions.len(), 2);
    assert_eq!(kept.runs[0].bytes(), 18);
    assert_eq!((kept.files(), kept.bytes()), (3, 25));
    // My notes, readme.txt and the file named like a run's folder.
    assert_eq!(kept.others, 3);
    assert_eq!(kept.before(Some(at("2026-06-01 00:00"))).len(), 1);
    assert_eq!(kept.before(Some(at("2026-01-05 10:00"))).len(), 0, "a run kept at the cutoff isn't before it");
    assert_eq!(kept.before(None).len(), 2);

    // A backup with no older versions yet has none; one that isn't plugged in can't be read.
    let empty = root.join("empty backup");
    fs::create_dir_all(&empty).unwrap();
    assert_eq!(read(&empty, &AtomicBool::new(false)).unwrap(), Kept::default());
    assert_eq!(
        read(&root.join("unplugged"), &AtomicBool::new(false)).unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
}

#[test]
fn a_real_run_keeps_older_versions_where_read_finds_them() {
    let root = fixtures("backup-older-real-run");
    let music = root.join("music");
    let backup = root.join("backup");
    fs::create_dir_all(&backup).unwrap();
    write(&music.join("Song/Song.flp"), b"first", 100);
    let stop = AtomicBool::new(false);
    let when = at("2026-11-03 14:40");
    run(&files(&music, &stop).unwrap(), &backup, &Options::new(stamp(when)), &stop).unwrap();
    write(&music.join("Song/Song.flp"), b"second", 10);
    let lines = run(&files(&music, &stop).unwrap(), &backup, &Options::new(stamp(when)), &stop).unwrap().lines;
    assert_eq!(lines[0].outcome, Outcome::Updated { kept_older: true });

    let kept = read(&backup, &stop).unwrap();
    assert_eq!(kept.runs.len(), 1);
    assert_eq!(kept.runs[0].when, when);
    let rel = place_on_backup(&backup, &music.join("Song/Song.flp")).unwrap();
    let rel = rel.strip_prefix(&backup).unwrap();
    assert_eq!(kept.runs[0].versions[0].path, backup.join(OLDER_VERSIONS).join("2026-11-03 14.40").join(rel));
    assert_eq!(fs::read(&kept.runs[0].versions[0].path).unwrap(), b"first");
}

#[test]
fn removes_only_the_runs_picked_and_only_what_was_listed() {
    let root = fixtures("backup-older-remove");
    let backup = two_runs(&root);
    let older = backup.join(OLDER_VERSIONS);
    let stop = AtomicBool::new(false);
    let kept = read(&backup, &stop).unwrap();
    let heard = std::cell::RefCell::new(Vec::new());
    let picked = kept.before(Some(at("2026-06-01 00:00")));
    let removed = remove(&backup, &picked, &stop, &|done, of| heard.borrow_mut().push((done, of))).unwrap();
    assert_eq!((removed.files, removed.bytes), (2, 18));
    assert!(removed.left.is_empty() && !removed.stopped && !removed.gone, "{removed:?}");
    assert_eq!(heard.borrow().last(), Some(&(2, 2)));
    // The January run's folder went with its files; the October run and everything the backup didn't make stayed.
    assert!(!older.join("2026-01-05 10.00").exists());
    assert_eq!(
        tree(&older),
        ["2026-02-01 10.00", "2026-10-31 22.40/I/Projects/Other/Other.flp", "My notes/what I kept.txt", "readme.txt"]
    );

    // Then all of them: the October run goes too, the rest still stays.
    let kept = read(&backup, &stop).unwrap();
    let removed = remove(&backup, &kept.before(None), &stop, &no_progress).unwrap();
    assert_eq!(removed.files, 1);
    assert_eq!(tree(&older), ["2026-02-01 10.00", "My notes/what I kept.txt", "readme.txt"]);
}

#[test]
fn a_version_changed_or_added_since_the_listing_stays() {
    let root = fixtures("backup-older-changed");
    let backup = two_runs(&root);
    let run_folder = backup.join(OLDER_VERSIONS).join("2026-01-05 10.00");
    let stop = AtomicBool::new(false);
    let kept = read(&backup, &stop).unwrap();
    // After the listing: one version is written over (another size), one gets another time, and a new file appears.
    let song = run_folder.join("I/Projects/Song/Song.flp");
    write(&song, b"written over since", 100);
    let song2 = run_folder.join("I/Projects/Song/Song (2).flp");
    fs::File::options().write(true).open(&song2).unwrap().set_modified(SystemTime::now()).unwrap();
    write(&run_folder.join("I/Projects/New/put here since.txt"), b"new", 100);

    let removed = remove(&backup, &kept.before(Some(at("2026-06-01 00:00"))), &stop, &no_progress).unwrap();
    assert_eq!(removed.files, 0);
    let left: Vec<&Path> = removed.left.iter().map(|(p, _)| p.as_path()).collect();
    assert_eq!(left.len(), 2, "{removed:?}");
    assert!(left.contains(&song.as_path()) && left.contains(&song2.as_path()));
    assert_eq!(removed.others, 1, "the new file isn't the backup's to remove");
    assert_eq!(
        tree(&run_folder),
        ["I/Projects/New/put here since.txt", "I/Projects/Song/Song (2).flp", "I/Projects/Song/Song.flp"]
    );
}

#[test]
fn empty_folders_go_and_a_run_with_nothing_left_is_tidied_away() {
    let root = fixtures("backup-older-empty");
    let backup = root.join("backup");
    let run_folder = backup.join(OLDER_VERSIONS).join("2026-03-01 09.15");
    fs::create_dir_all(run_folder.join("I/Projects/Gone")).unwrap();
    let kept = read(&backup, &AtomicBool::new(false)).unwrap();
    assert_eq!((kept.runs.len(), kept.files()), (1, 0));
    let removed = remove(&backup, &kept.before(None), &AtomicBool::new(false), &no_progress).unwrap();
    assert_eq!(removed.files, 0);
    assert!(!run_folder.exists());
    assert!(backup.join(OLDER_VERSIONS).is_dir(), "Older versions itself stays");
}

#[test]
fn links_are_never_followed_or_removed() {
    let root = fixtures("backup-older-links");
    let music = root.join("music");
    write(&music.join("Song/Song.flp"), b"my song", 100);
    let backup = root.join("backup");
    let run_folder = backup.join(OLDER_VERSIONS).join("2026-01-05 10.00");
    write(&run_folder.join("I/Song.flp"), b"older", 100);
    // A folder link in a run's folder, leading to your music.
    folder_link(&music, &run_folder.join("I/To my music"));
    // And on a Mac or Linux a file link (Windows only lets an administrator make one).
    #[cfg(unix)]
    std::os::unix::fs::symlink(music.join("Song/Song.flp"), run_folder.join("I/Song link.flp")).unwrap();

    let stop = AtomicBool::new(false);
    let kept = read(&backup, &stop).unwrap();
    assert_eq!(kept.files(), 1, "only the older version, nothing through the links");
    assert_eq!(kept.others, if cfg!(unix) { 2 } else { 1 });
    let removed = remove(&backup, &kept.before(None), &stop, &no_progress).unwrap();
    assert_eq!(removed.files, 1);
    assert_eq!(tree(&music), ["Song/Song.flp"]);
    assert_eq!(fs::read(music.join("Song/Song.flp")).unwrap(), b"my song");
    // The links stay, so their folders and the run's folder stay too.
    assert!(is_link(&run_folder.join("I/To my music")));
    #[cfg(unix)]
    assert!(is_link(&run_folder.join("I/Song link.flp")));
}

#[test]
fn an_older_versions_folder_that_is_a_link_is_left_alone() {
    let root = fixtures("backup-older-linked-folder");
    let elsewhere = root.join("elsewhere");
    write(&elsewhere.join("2026-01-05 10.00/I/Song.flp"), b"not the backup's", 100);
    let backup = root.join("backup");
    fs::create_dir_all(&backup).unwrap();
    folder_link(&elsewhere, &backup.join(OLDER_VERSIONS));
    let kept = read(&backup, &AtomicBool::new(false)).unwrap();
    assert!(kept.runs.is_empty());
    assert_eq!(kept.others, 1);

    // Even a run made up by hand, naming the file through the link as it is now, removes nothing.
    let through = backup.join(OLDER_VERSIONS).join("2026-01-05 10.00/I/Song.flp");
    let meta = fs::metadata(&through).unwrap();
    let forged = Run {
        stamp: "2026-01-05 10.00".into(),
        when: at("2026-01-05 10:00"),
        versions: vec![mo_backup::older::Version { path: through, size: meta.len(), modified: meta.modified().ok() }],
    };
    let removed = remove(&backup, &[&forged], &AtomicBool::new(false), &no_progress).unwrap();
    assert_eq!((removed.files, removed.left.len()), (0, 1));
    assert_eq!(tree(&elsewhere), ["2026-01-05 10.00/I/Song.flp"]);
}

#[test]
fn a_made_up_run_never_reaches_outside_its_folder() {
    let root = fixtures("backup-older-made-up");
    let music = root.join("music");
    write(&music.join("Song.flp"), b"my song", 100);
    let backup = two_runs(&root);
    let stop = AtomicBool::new(false);
    let song = fs::metadata(music.join("Song.flp")).unwrap();
    let outside =
        mo_backup::older::Version { path: music.join("Song.flp"), size: song.len(), modified: song.modified().ok() };
    // A name that isn't a run's, and a real run's name listing a file outside it.
    let climbs = Run { stamp: "../../music".into(), when: at("2026-01-05 10:00"), versions: vec![outside.clone()] };
    let real = Run { stamp: "2026-01-05 10.00".into(), when: at("2026-01-05 10:00"), versions: vec![outside] };
    let removed = remove(&backup, &[&climbs, &real], &stop, &no_progress).unwrap();
    assert_eq!(removed.files, 0);
    assert_eq!(fs::read(music.join("Song.flp")).unwrap(), b"my song");
    // The real run's own versions weren't listed, so they stay too.
    assert_eq!(removed.others, 2);
    assert_eq!(read(&backup, &stop).unwrap().files(), 3);
}

#[test]
fn a_hard_linked_version_goes_and_the_file_it_shares_with_stays() {
    let root = fixtures("backup-older-hard-link");
    let backup = root.join("backup");
    let current = backup.join("I/Samples/Kick.wav");
    write(&current, &[5u8; 4096], 100);
    let version = backup.join(OLDER_VERSIONS).join("2026-01-05 10.00/I/Pack/Kick.wav");
    fs::create_dir_all(version.parent().unwrap()).unwrap();
    fs::hard_link(&current, &version).unwrap();
    let kept = read(&backup, &AtomicBool::new(false)).unwrap();
    let removed = remove(&backup, &kept.before(None), &AtomicBool::new(false), &no_progress).unwrap();
    assert_eq!(removed.files, 1);
    assert!(!version.exists());
    assert_eq!(fs::read(&current).unwrap(), vec![5u8; 4096]);
}

#[test]
fn a_stopped_removal_removes_nothing_more() {
    let root = fixtures("backup-older-stop");
    let backup = two_runs(&root);
    let kept = read(&backup, &AtomicBool::new(false)).unwrap();
    let removed = remove(&backup, &kept.before(None), &AtomicBool::new(true), &no_progress).unwrap();
    assert!(removed.stopped);
    assert_eq!(removed.files, 0);
    assert_eq!(read(&backup, &AtomicBool::new(false)).unwrap().files(), 3);
    // Reading stops too.
    assert_eq!(read(&backup, &AtomicBool::new(true)).unwrap_err().kind(), std::io::ErrorKind::Interrupted);
}

#[test]
fn no_backup_folder_no_removal() {
    let root = fixtures("backup-older-unplugged");
    let backup = two_runs(&root);
    let kept = read(&backup, &AtomicBool::new(false)).unwrap();
    let unplugged = root.join("unplugged");
    assert_eq!(
        remove(&unplugged, &kept.before(None), &AtomicBool::new(false), &no_progress),
        Err(CantStart::NoBackupFolder(unplugged.clone()))
    );
    assert!(!unplugged.exists(), "nothing is made in its place");
}

/// On Windows a read-only file isn't removed (someone marked it to keep); elsewhere read-only doesn't stop a removal.
#[cfg(windows)]
#[test]
fn a_read_only_version_stays_on_windows() {
    let root = fixtures("backup-older-read-only");
    let backup = two_runs(&root);
    let song = backup.join(OLDER_VERSIONS).join("2026-10-31 22.40/I/Projects/Other/Other.flp");
    let mut permissions = fs::metadata(&song).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&song, permissions).unwrap();
    let kept = read(&backup, &AtomicBool::new(false)).unwrap();
    let removed = remove(&backup, &kept.before(None), &AtomicBool::new(false), &no_progress).unwrap();
    assert_eq!(removed.files, 2);
    assert_eq!(removed.left.len(), 1);
    assert!(song.exists());
    let mut permissions = fs::metadata(&song).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    fs::set_permissions(&song, permissions).unwrap();
}
