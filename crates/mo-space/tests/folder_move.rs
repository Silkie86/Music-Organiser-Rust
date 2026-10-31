//! Moving a plugin's folder to another drive and back, on folders made in the test build's own fixtures folder under
//! fixed names. A second folder there stands in for the other drive.
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![cfg(unix)]

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use mo_space::clean::{Bin, Problem, Removed, undo};
use mo_space::folder_move::{
    CantMove, Drives, MovedFolder, OLD_SUFFIX, move_back, move_folder, moved_json, old_copy_can_go, parse_moved,
    problem, remove_old_copy,
};
use mo_space::key;
use mo_space::plugin_space::SpaceKind;

fn fixtures(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &Path, rel: &str, text: &str) {
    let p = dir.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Every folder is on its own drive, with this much room.
struct OtherDrive {
    free: Option<u64>,
}

impl Drives for OtherDrive {
    fn same(&self, _a: &Path, _b: &Path) -> bool {
        false
    }
    fn free(&self, _path: &Path) -> Option<u64> {
        self.free
    }
}

struct SameDrive;

impl Drives for SameDrive {
    fn same(&self, _a: &Path, _b: &Path) -> bool {
        true
    }
    fn free(&self, _path: &Path) -> Option<u64> {
        None
    }
}

fn whose() -> (&'static str, SpaceKind, &'static [String]) {
    ("omnisphere", SpaceKind::Sounds, &[])
}

fn nothing(_: u64) {}

#[test]
fn a_folder_moves_to_the_other_drive_and_back_with_what_was_saved_meanwhile() {
    let dir = fixtures("mo-space-move");
    let c = dir.join("C");
    let d = dir.join("D");
    write(&c, "Spectrasonics/STEAM/Pad.db", "pad");
    write(&c, "Spectrasonics/STEAM/Keys/Rhodes.db", "rhodes");
    fs::create_dir_all(&d).unwrap();
    // A link inside stays a link, never followed.
    write(&dir, "Elsewhere/big.wav", "big");
    std::os::unix::fs::symlink(dir.join("Elsewhere"), c.join("Spectrasonics/STEAM/Linked")).unwrap();
    // One worded from where it sits still reaches the same place from the other drive.
    std::os::unix::fs::symlink("../../../../Elsewhere", c.join("Spectrasonics/STEAM/Keys/Near")).unwrap();
    // A library's own read-only samples copy too, with their time.
    let locked = c.join("Spectrasonics/STEAM/Keys/Locked.db");
    fs::write(&locked, "").unwrap();
    let mut read_only = fs::metadata(&locked).unwrap().permissions();
    read_only.set_readonly(true);
    fs::set_permissions(&locked, read_only).unwrap();
    let from = s(&c.join("Spectrasonics/STEAM"));
    let stop = AtomicBool::new(false);
    let seen = std::sync::Mutex::new(0u64);
    let moved =
        move_folder(&from, &s(&d), 9, whose(), &OtherDrive { free: None }, &[], &|n| *seen.lock().unwrap() = n, &stop)
            .unwrap();

    assert_eq!(moved.to, s(&d.join("STEAM")));
    assert_eq!(moved.old_copy, format!("{from}{OLD_SUFFIX}"));
    // The time is written as the C# app writes it: 2026-10-26T14:03:12.1234567+01:00.
    let (when, zone) = moved.moved.split_at(27);
    assert!(when.as_bytes()[19] == b'.' && when[20..].bytes().all(|b| b.is_ascii_digit()), "{}", moved.moved);
    assert!(zone.starts_with(['+', '-']) && zone.len() == 6, "{}", moved.moved);
    assert_eq!(*seen.lock().unwrap(), 9);
    assert!(fs::symlink_metadata(&from).unwrap().file_type().is_symlink());
    // The plugin still finds its files where they were.
    assert_eq!(fs::read_to_string(c.join("Spectrasonics/STEAM/Keys/Rhodes.db")).unwrap(), "rhodes");
    assert_eq!(fs::read_to_string(d.join("STEAM/Pad.db")).unwrap(), "pad");
    assert!(fs::symlink_metadata(d.join("STEAM/Linked")).unwrap().file_type().is_symlink());
    assert_eq!(fs::read_to_string(d.join("STEAM/Keys/Near/big.wav")).unwrap(), "big");
    assert!(fs::metadata(d.join("STEAM/Keys/Locked.db")).unwrap().permissions().readonly());
    assert!(moved.has_old_copy() && old_copy_can_go(&moved));
    // Times are kept.
    let time = |p: PathBuf| fs::metadata(p).unwrap().modified().unwrap();
    assert_eq!(time(d.join("STEAM/Pad.db")), time(PathBuf::from(&moved.old_copy).join("Pad.db")));

    // The plugin saves a preset through the link, then the move is undone.
    write(&c, "Spectrasonics/STEAM/New.db", "new");
    move_back(&moved, &nothing, &stop).unwrap();
    assert!(!fs::symlink_metadata(&from).unwrap().file_type().is_symlink());
    assert_eq!(fs::read_to_string(c.join("Spectrasonics/STEAM/New.db")).unwrap(), "new");
    assert_eq!(fs::read_to_string(c.join("Spectrasonics/STEAM/Keys/Rhodes.db")).unwrap(), "rhodes");
    assert!(!moved.has_old_copy());
    // The copy on the other drive is left for you.
    assert!(d.join("STEAM/Pad.db").exists());
}

#[test]
fn moving_back_once_the_old_copy_is_gone_copies_the_folder_back() {
    let dir = fixtures("mo-space-move-back");
    let (c, d) = (dir.join("C"), dir.join("D"));
    write(&c, "NI/Kontakt Library/a.nki", "a");
    fs::create_dir_all(&d).unwrap();
    let from = s(&c.join("NI/Kontakt Library"));
    let stop = AtomicBool::new(false);
    let moved = move_folder(&from, &s(&d), 1, whose(), &OtherDrive { free: None }, &[], &nothing, &stop).unwrap();

    // Deleted to the Recycle Bin once the plugin opened fine...
    let bin = FolderBin { dir: dir.join("bin"), held: RefCell::new(Vec::new()) };
    let done = remove_old_copy(&moved, &bin);
    assert!(done.problems.is_empty(), "{:?}", done.problems);
    assert!(!moved.has_old_copy());
    // ...and later moved back after all.
    move_back(&moved, &nothing, &stop).unwrap();
    assert_eq!(fs::read_to_string(c.join("NI/Kontakt Library/a.nki")).unwrap(), "a");
    assert!(!fs::symlink_metadata(&from).unwrap().file_type().is_symlink());
    // The old copy can still come back from the bin, beside the folder.
    assert!(undo(&done, &bin).is_empty());
    assert!(c.join("NI/Kontakt Library (old copy)/a.nki").exists());
}

#[test]
fn a_folder_only_moves_where_it_makes_sense() {
    let dir = fixtures("mo-space-move-problems");
    let (c, d) = (dir.join("C"), dir.join("D"));
    write(&c, "Plugin/Sounds/a.wav", "a");
    write(&d, "Sounds/other.wav", "x");
    write(&c, "Other/Samples/b.wav", "b");
    fs::create_dir_all(c.join("Taken")).unwrap();
    fs::create_dir_all(c.join("Taken (old copy)")).unwrap();
    let stop = AtomicBool::new(false);
    let free = OtherDrive { free: None };
    let p = |from: &Path, to: &Path, drives: &dyn Drives| problem(&s(from), &s(to), 1, drives, &[], &stop);
    assert_eq!(p(&c.join("Gone"), &d, &free), Some(CantMove::Gone));
    assert_eq!(p(&c.join("Plugin/Sounds"), &d, &SameDrive), Some(CantMove::SameDrive));
    assert_eq!(p(&c.join("Plugin/Sounds"), &d, &free), Some(CantMove::TargetThere(s(&d.join("Sounds")))));
    assert_eq!(p(&c.join("Plugin"), &c.join("Plugin/Sounds"), &free), Some(CantMove::DestinationInside));
    assert_eq!(p(&c.join("Taken"), &d, &free), Some(CantMove::OldCopyThere));
    assert_eq!(p(&c.join("Other/Samples"), &dir.join("E"), &free), Some(CantMove::DestinationGone));
    let tight = OtherDrive { free: Some(1 << 30) };
    assert_eq!(p(&c.join("Other/Samples"), &d, &tight), Some(CantMove::NoRoom { free: 1 << 30, needs: 1 }));
    assert_eq!(p(&c.join("Other/Samples"), &d, &free), None);

    // Moved already: it's a link now, and what's inside it isn't on this drive.
    let moved = move_folder(&s(&c.join("Other")), &s(&d), 1, whose(), &free, &[], &nothing, &stop).unwrap();
    assert_eq!(p(&c.join("Other"), &d, &free), Some(CantMove::Link));
    assert_eq!(
        problem(&s(&c.join("Other/Samples")), &s(&dir), 1, &free, std::slice::from_ref(&moved), &stop),
        Some(CantMove::InsideMoved(s(&c.join("Other"))))
    );
}

#[test]
fn a_stopped_move_leaves_everything_as_it_was() {
    let dir = fixtures("mo-space-move-stopped");
    let (c, d) = (dir.join("C"), dir.join("D"));
    for n in 0..5 {
        write(&c, &format!("Library/{n}.wav"), "x");
    }
    fs::create_dir_all(&d).unwrap();
    let stop = AtomicBool::new(false);
    let from = s(&c.join("Library"));
    let result = move_folder(
        &from,
        &s(&d),
        5,
        whose(),
        &OtherDrive { free: None },
        &[],
        &|_| stop.store(true, Ordering::Relaxed),
        &stop,
    );
    assert_eq!(result, Err(CantMove::Stopped));
    assert!(!d.join("Library").exists());
    assert!(!fs::symlink_metadata(&from).unwrap().file_type().is_symlink());
    assert_eq!(fs::read_dir(&from).unwrap().count(), 5);
    assert!(!c.join("Library (old copy)").exists());
}

#[test]
fn an_old_copy_stays_while_it_is_the_only_copy_the_plugin_could_use() {
    let dir = fixtures("mo-space-move-old-copy");
    let (c, d) = (dir.join("C"), dir.join("D"));
    write(&c, "Presets/a.fxp", "a");
    fs::create_dir_all(&d).unwrap();
    let stop = AtomicBool::new(false);
    let moved =
        move_folder(&s(&c.join("Presets")), &s(&d), 1, whose(), &OtherDrive { free: None }, &[], &nothing, &stop)
            .unwrap();
    // The copy on the other drive was renamed (or the drive unplugged).
    fs::rename(d.join("Presets"), d.join("Presets renamed")).unwrap();
    assert!(!old_copy_can_go(&moved));
    let bin = FolderBin { dir: dir.join("bin"), held: RefCell::new(Vec::new()) };
    let done = remove_old_copy(&moved, &bin);
    assert!(done.removed.is_empty() && done.problems.len() == 1);
    assert!(moved.has_old_copy());
}

#[test]
fn moves_are_kept_as_the_csharp_app_keeps_them() {
    let json = r#"[{"From":"C:\\ProgramData\\Spectrasonics\\STEAM","To":"I:\\Plugin Data\\STEAM","OldCopy":"C:\\ProgramData\\Spectrasonics\\STEAM (old copy)","Bytes":64424509440,"Moved":"2026-10-26T14:03:12.1234567+01:00","Key":"omnisphere","Kind":2,"SharedWith":["keyscape"]}]"#;
    let moved = parse_moved(json);
    assert_eq!(moved.len(), 1);
    assert_eq!(moved[0].bytes, 64_424_509_440);
    assert_eq!(moved[0].space_kind(), SpaceKind::Sounds);
    assert!(moved[0].belongs("keyscape") && !moved[0].belongs(""));
    assert_eq!(parse_moved(&moved_json(&moved)), moved);
    // Moves kept before the key, kind and sharers were: sounds, nobody's.
    let old: Vec<MovedFolder> =
        parse_moved(r#"[{"From":"a","To":"b","OldCopy":"a (old copy)","Bytes":1,"Moved":"2026-10-26T14:03:12"}]"#);
    assert_eq!(old[0].space_kind(), SpaceKind::Sounds);
    assert!(old[0].key.is_empty() && old[0].shared_with.is_empty());
    assert!(parse_moved("not json").is_empty());
}

struct FolderBin {
    dir: PathBuf,
    held: RefCell<Vec<(String, PathBuf)>>,
}

impl Bin for FolderBin {
    fn send(&self, path: &Path) -> Result<(), String> {
        let mut held = self.held.borrow_mut();
        let to = self.dir.join(held.len().to_string());
        fs::create_dir_all(&self.dir).unwrap();
        fs::rename(path, &to).map_err(|e| e.to_string())?;
        held.push((path.to_string_lossy().into_owned(), to));
        Ok(())
    }
    fn find(&self, removed: &mut [Removed]) -> Vec<Problem> {
        for r in removed {
            r.bin_id = self.held.borrow().iter().find(|(p, _)| key(p) == key(&r.real)).map(|(_, at)| s(at));
        }
        Vec::new()
    }
    fn put_back(&self, removed: &[Removed]) -> Vec<Problem> {
        let mut problems = Vec::new();
        for r in removed {
            let at = self.held.borrow().iter().find(|(_, at)| Some(s(at)) == r.bin_id).map(|(_, at)| at.clone());
            if let Some(at) = at
                && fs::rename(at, &r.path).is_err()
            {
                problems.push(Problem { path: r.path.clone(), why: "couldn't".into() });
            }
        }
        problems
    }
}
