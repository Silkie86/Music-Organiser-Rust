//! Files the backup keeps that are gone from your folders, found on folders made in the test build's own fixtures folder
//! under fixed names, never a random temp one and never a real folder.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

use mo_backup::copy::{Options, run};
use mo_backup::gone::{Gone, Kept, find};
use mo_backup::pack::{Packer, SONGS};
use mo_backup::plan::place_on_backup;
use mo_space::size::{FileEntry, files};

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

/// A folder link, as Windows has them (a junction) or a symbolic link elsewhere.
fn folder_link(target: &Path, at: &Path) {
    #[cfg(windows)]
    junction::create(target, at).unwrap();
    #[cfg(not(windows))]
    std::os::unix::fs::symlink(target, at).unwrap();
}

/// Backs up these files the plain way (folders).
fn back_up(list: &[FileEntry], backup: &Path) {
    let stop = AtomicBool::new(false);
    run(list, backup, &Options::new("2026-11-03 14.40".to_owned()), &stop).unwrap();
}

/// What's gone, with `music` walked for the files in your folders now.
fn gone_from(backup: &Path, music: &Path) -> Vec<Gone> {
    let now: Vec<PathBuf> = files(music, &AtomicBool::new(false)).unwrap().into_iter().map(|f| f.path).collect();
    let present: Vec<&Path> = now.iter().map(PathBuf::as_path).collect();
    find(backup, &present, &AtomicBool::new(false)).unwrap()
}

/// Where each gone file was, as its place on the backup (a path in a test folder can be written two ways on Windows,
/// with or without `\\?\`; its place on the backup is the same either way).
fn places(backup: &Path, gone: &[Gone]) -> Vec<PathBuf> {
    gone.iter().map(|g| place_on_backup(backup, &g.was).unwrap()).collect()
}

#[test]
fn a_file_or_a_folder_you_deleted_is_found_with_where_the_backup_keeps_it() {
    let root = fixtures("backup-gone-deleted");
    let music = root.join("music");
    let backup = root.join("backup");
    fs::create_dir_all(&backup).unwrap();
    write(&music.join("Samples/Kick.wav"), &[1u8; 3000], 100);
    write(&music.join("Samples/Snare.wav"), &[2u8; 3000], 100);
    write(&music.join("Old song/Old.flp"), &[3u8; 500], 100);
    write(&music.join("Old song/Old (2).flp"), &[4u8; 600], 100);
    back_up(&files(&music, &AtomicBool::new(false)).unwrap(), &backup);
    assert!(gone_from(&backup, &music).is_empty(), "nothing is gone yet");

    fs::remove_file(music.join("Samples/Snare.wav")).unwrap();
    fs::remove_dir_all(music.join("Old song")).unwrap();
    let gone = gone_from(&backup, &music);
    let on_backup = |p: &str| place_on_backup(&backup, &music.join(p)).unwrap();
    assert_eq!(
        places(&backup, &gone),
        [on_backup("Old song/Old (2).flp"), on_backup("Old song/Old.flp"), on_backup("Samples/Snare.wav")],
        "in order of where they were"
    );
    let snare = &gone[2];
    assert_eq!(snare.size, 3000);
    assert!(snare.time > 0);
    assert_eq!(snare.kept, [Kept::Copy(on_backup("Samples/Snare.wav"))]);
    // Nothing on the backup was touched.
    assert_eq!(fs::read(on_backup("Samples/Snare.wav")).unwrap(), vec![2u8; 3000]);

    // A copy removed from the backup by hand isn't kept any more: no warning for it.
    fs::remove_file(on_backup("Samples/Snare.wav")).unwrap();
    assert_eq!(gone_from(&backup, &music).len(), 2);
}

#[test]
fn a_file_still_there_is_never_gone() {
    let root = fixtures("backup-gone-still-there");
    let music = root.join("music");
    let backup = root.join("backup");
    fs::create_dir_all(&backup).unwrap();
    write(&music.join("Samples/Kick.wav"), &[1u8; 3000], 100);
    back_up(&files(&music, &AtomicBool::new(false)).unwrap(), &backup);
    // Not among the files walked (its folder taken off the list, say), but still there.
    assert!(find(&backup, &[], &AtomicBool::new(false)).unwrap().is_empty());
    // Nothing backed up at all, or no backup there: nothing to warn about.
    assert!(find(&root.join("empty backup"), &[], &AtomicBool::new(false)).unwrap().is_empty());
}

#[test]
fn a_file_behind_a_link_whose_place_has_gone_is_not_counted() {
    let root = fixtures("backup-gone-link");
    let music = root.join("music");
    let elsewhere = root.join("elsewhere");
    let backup = root.join("backup");
    fs::create_dir_all(&backup).unwrap();
    fs::create_dir_all(&music).unwrap();
    write(&elsewhere.join("Pack/Clap.wav"), &[5u8; 3000], 100);
    folder_link(&elsewhere, &music.join("Linked"));
    // Backed up through the link.
    let clap = music.join("Linked/Pack/Clap.wav");
    let entry = files(&elsewhere, &AtomicBool::new(false)).unwrap().into_iter().next().unwrap();
    back_up(&[FileEntry { path: clap.clone(), ..entry }], &backup);
    assert!(place_on_backup(&backup, &clap).unwrap().is_file());

    // The link's place is gone (a drive unplugged, a folder moved): what was behind it can't be told from gone.
    fs::remove_dir_all(&elsewhere).unwrap();
    assert!(find(&backup, &[], &AtomicBool::new(false)).unwrap().is_empty());
}

#[test]
fn a_file_in_a_song_zip_counts_and_both_formats_are_named() {
    let root = fixtures("backup-gone-zip");
    let music = root.join("music");
    let backup = root.join("backup");
    fs::create_dir_all(&backup).unwrap();
    let save = music.join("Song/Song.flp");
    let kick = music.join("Samples/Kick.wav");
    write(&save, &[7u8; 500], 100);
    write(&kick, &[1u8; 3000], 100);
    let stop = AtomicBool::new(false);
    Packer::new(&backup).unwrap().pack_song(&[save.clone(), kick.clone()], "Song.zip", 1, &stop).unwrap();
    fs::remove_file(&kick).unwrap();
    let gone = gone_from(&backup, &music);
    let zip = backup.join(SONGS).join("Song.zip");
    assert_eq!(places(&backup, &gone), [place_on_backup(&backup, &kick).unwrap()]);
    assert_eq!(gone[0].kept, [Kept::InZip(zip.clone())]);
    assert_eq!(gone[0].size, 3000);

    // Kept in the backup's folders too (the format changed once): both places are named, the copy first.
    write(&kick, &[1u8; 3000], 100);
    back_up(&files(&music, &AtomicBool::new(false)).unwrap(), &backup);
    fs::remove_file(&kick).unwrap();
    let gone = gone_from(&backup, &music);
    assert_eq!(gone.len(), 1);
    assert_eq!(gone[0].kept, [Kept::Copy(place_on_backup(&backup, &kick).unwrap()), Kept::InZip(zip)]);
}

#[test]
fn a_stopped_look_says_so() {
    let root = fixtures("backup-gone-stopped");
    let music = root.join("music");
    let backup = root.join("backup");
    fs::create_dir_all(&backup).unwrap();
    write(&music.join("Samples/Kick.wav"), &[1u8; 3000], 100);
    back_up(&files(&music, &AtomicBool::new(false)).unwrap(), &backup);
    let err = find(&backup, &[], &AtomicBool::new(true)).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::Interrupted);
}
