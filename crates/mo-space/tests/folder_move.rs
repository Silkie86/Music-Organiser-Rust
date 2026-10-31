//! Moving a plugin's folder to another drive and back, on folders made in the test build's own fixtures folder under
//! fixed names. Other folders there stand in for the other drives. Links are junctions on Windows, as the app makes.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use mo_space::clean::{Bin, Problem, Removed, undo};
use mo_space::folder_move::{
    CantMove, Drives, MovedFolder, OLD_SUFFIX, move_back, move_folder, moved_json, old_copy_can_go, only_in_old_copy,
    parse_moved, problem, remove_old_copy,
};
use mo_space::key;
use mo_space::plugin_space::SpaceKind;

fn fixtures(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    writable(&dir);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Makes the read-only files a test left writable again, so its folder can be cleared.
fn writable(dir: &Path) {
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            writable(&entry.path());
        } else if kind.is_file()
            && let Ok(meta) = entry.metadata()
            && meta.permissions().readonly()
        {
            let mut permissions = meta.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
            let _ = fs::set_permissions(entry.path(), permissions);
        }
    }
}

fn write(dir: &Path, rel: &str, text: &str) {
    let p = dir.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// A folder link, as the app makes one.
fn folder_link(target: &Path, at: &Path) {
    #[cfg(windows)]
    junction::create(target, at).unwrap();
    #[cfg(not(windows))]
    std::os::unix::fs::symlink(target, at).unwrap();
}

fn unlink(at: &Path) {
    if cfg!(windows) { fs::remove_dir(at).unwrap() } else { fs::remove_file(at).unwrap() }
}

fn is_link(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink())
}

fn real(p: &Path) -> PathBuf {
    fs::canonicalize(p).unwrap()
}

fn set_time(p: &Path, when: SystemTime) {
    fs::File::options().write(true).open(p).unwrap().set_modified(when).unwrap();
}

fn time(p: &Path) -> SystemTime {
    fs::metadata(p).unwrap().modified().unwrap()
}

/// Any file a copy left half-way under its temporary name.
fn part_files(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let kind = entry.file_type().unwrap();
        if kind.is_dir() {
            found.extend(part_files(&entry.path()));
        } else if entry.file_name().to_string_lossy().ends_with(".mo-copying") {
            found.push(entry.path());
        }
    }
    found
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

const ROOMY: OtherDrive = OtherDrive { free: None };

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
    let steam = c.join("Spectrasonics").join("STEAM");
    write(&steam, "Pad.db", "pad");
    write(&steam, "Keys/Rhodes.db", "rhodes");
    fs::create_dir_all(&d).unwrap();
    // Links inside stay links, never followed: one to another place, one inside the folder, one that leads nowhere
    // now (a drive that isn't plugged in).
    write(&dir, "Elsewhere/big.wav", "big");
    folder_link(&dir.join("Elsewhere"), &steam.join("Linked"));
    folder_link(&steam.join("Keys"), &steam.join("Shortcut"));
    folder_link(&dir.join("Unplugged"), &steam.join("Unplugged"));
    // One worded from where it sits still reaches the same place from the other drive.
    #[cfg(unix)]
    std::os::unix::fs::symlink("../../../../Elsewhere", steam.join("Keys/Near")).unwrap();
    // A library's own read-only samples copy too, with their time.
    let locked = steam.join("Keys").join("Locked.db");
    fs::write(&locked, "").unwrap();
    let mut read_only = fs::metadata(&locked).unwrap().permissions();
    read_only.set_readonly(true);
    fs::set_permissions(&locked, read_only).unwrap();

    let from = s(&steam);
    let stop = AtomicBool::new(false);
    let seen = std::sync::Mutex::new(0u64);
    // The plugin saves a preset while the folder copies.
    let saved = AtomicBool::new(false);
    let progress = |n| {
        *seen.lock().unwrap() = n;
        if !saved.swap(true, Ordering::Relaxed) {
            write(&steam, "Late.db", "late");
        }
    };
    let moved = move_folder(&from, &s(&d), 9, whose(), &ROOMY, &[], &progress, &stop).unwrap();

    let copy = d.join("STEAM");
    assert_eq!(moved.to, s(&copy));
    assert_eq!(moved.old_copy, format!("{from}{OLD_SUFFIX}"));
    // The time is written as the C# app writes it: 2026-10-26T14:03:12.1234567+01:00.
    let (when, zone) = moved.moved.split_at(27);
    assert!(when.as_bytes()[19] == b'.' && when[20..].bytes().all(|b| b.is_ascii_digit()), "{}", moved.moved);
    assert!(zone.starts_with(['+', '-']) && zone.len() == 6, "{}", moved.moved);
    assert!(*seen.lock().unwrap() >= 9);
    assert!(is_link(&steam));
    // The plugin still finds its files where they were, and what it saved while the folder copied went across too.
    assert_eq!(fs::read_to_string(steam.join("Keys/Rhodes.db")).unwrap(), "rhodes");
    assert_eq!(fs::read_to_string(copy.join("Pad.db")).unwrap(), "pad");
    assert_eq!(fs::read_to_string(copy.join("Late.db")).unwrap(), "late");
    assert!(is_link(&copy.join("Linked")) && is_link(&copy.join("Unplugged")));
    assert_eq!(real(&copy.join("Shortcut")), real(&copy.join("Keys")));
    #[cfg(unix)]
    assert_eq!(fs::read_to_string(copy.join("Keys/Near/big.wav")).unwrap(), "big");
    assert!(fs::metadata(copy.join("Keys/Locked.db")).unwrap().permissions().readonly());
    assert!(moved.has_old_copy() && old_copy_can_go(&moved));
    let old = PathBuf::from(&moved.old_copy);
    assert_eq!(time(&copy.join("Pad.db")), time(&old.join("Pad.db")));
    assert_eq!(time(&copy.join("Keys/Locked.db")), time(&old.join("Keys/Locked.db")));
    assert_eq!(only_in_old_copy(&moved, std::slice::from_ref(&moved), &stop).unwrap(), Vec::<String>::new());

    // The plugin saves a preset through the link, and an update rewrites Pad.db keeping the older time it came with.
    write(&steam, "New.db", "new");
    fs::write(copy.join("Pad.db"), "pad v2").unwrap();
    set_time(&copy.join("Pad.db"), time(&old.join("Pad.db")) - Duration::from_secs(3600));
    // The old copy's Pad.db looks newer, so it's named before the old copy could go.
    assert_eq!(only_in_old_copy(&moved, std::slice::from_ref(&moved), &stop).unwrap(), ["Pad.db"]);

    // The move is undone: what changed on the other drive comes back, whatever its time.
    move_back(&moved, &ROOMY, &nothing, &stop).unwrap();
    assert!(!is_link(&steam));
    assert_eq!(fs::read_to_string(steam.join("New.db")).unwrap(), "new");
    assert_eq!(fs::read_to_string(steam.join("Pad.db")).unwrap(), "pad v2");
    assert_eq!(fs::read_to_string(steam.join("Keys/Rhodes.db")).unwrap(), "rhodes");
    assert!(part_files(&steam).is_empty());
    assert!(!moved.has_old_copy());
    // The copy on the other drive is left for you.
    assert!(copy.join("Pad.db").exists());
}

#[test]
fn moving_back_once_the_old_copy_is_gone_copies_the_folder_back() {
    let dir = fixtures("mo-space-move-back");
    let (c, d) = (dir.join("C"), dir.join("D"));
    let library = c.join("NI").join("Kontakt Library");
    write(&library, "a.nki", "a");
    write(&library, "Sub/b.nki", "b");
    folder_link(&library.join("Sub"), &library.join("Shortcut"));
    fs::create_dir_all(&d).unwrap();
    let from = s(&library);
    let stop = AtomicBool::new(false);
    let moved = move_folder(&from, &s(&d), 2, whose(), &ROOMY, &[], &nothing, &stop).unwrap();

    // Deleted to the Recycle Bin once the plugin opened fine...
    let bin = FolderBin { dir: dir.join("bin"), held: RefCell::new(Vec::new()) };
    let done = remove_old_copy(&moved, std::slice::from_ref(&moved), false, &bin, &stop);
    assert!(done.problems.is_empty(), "{:?}", done.problems);
    assert!(!moved.has_old_copy());
    // ...and later moved back after all, but only with room for it.
    let full = OtherDrive { free: Some(1) };
    assert_eq!(move_back(&moved, &full, &nothing, &stop), Err(CantMove::NoRoom { free: 1, needs: 2 }));
    assert!(is_link(&library));
    move_back(&moved, &ROOMY, &nothing, &stop).unwrap();
    assert_eq!(fs::read_to_string(library.join("a.nki")).unwrap(), "a");
    assert!(!is_link(&library));
    // The link inside leads inside the folder again, not to the other drive.
    assert_eq!(real(&library.join("Shortcut")), real(&library.join("Sub")));
    assert!(!c.join("NI").join("Kontakt Library (moving back)").exists());
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
    let p = |from: &Path, to: &Path, drives: &dyn Drives| problem(&s(from), &s(to), 1, drives, &[], &stop);
    assert_eq!(p(&c.join("Gone"), &d, &ROOMY), Some(CantMove::Gone));
    assert_eq!(p(&c.join("Plugin/Sounds"), &d, &SameDrive), Some(CantMove::SameDrive));
    assert_eq!(p(&c.join("Plugin/Sounds"), &d, &ROOMY), Some(CantMove::TargetThere(s(&d.join("Sounds")))));
    assert_eq!(p(&c.join("Plugin"), &c.join("Plugin/Sounds"), &ROOMY), Some(CantMove::DestinationInside));
    assert_eq!(p(&c.join("Taken"), &d, &ROOMY), Some(CantMove::OldCopyThere));
    assert_eq!(p(&c.join("Other/Samples"), &dir.join("E"), &ROOMY), Some(CantMove::DestinationGone));
    let tight = OtherDrive { free: Some(1 << 30) };
    assert_eq!(p(&c.join("Other/Samples"), &d, &tight), Some(CantMove::NoRoom { free: 1 << 30, needs: 1 }));
    assert_eq!(p(&c.join("Other/Samples"), &d, &ROOMY), None);

    // Moved already: it's a link now, and what's inside it isn't on this drive.
    let moved = move_folder(&s(&c.join("Other")), &s(&d), 1, whose(), &ROOMY, &[], &nothing, &stop).unwrap();
    let all = std::slice::from_ref(&moved);
    assert_eq!(p(&c.join("Other"), &d, &ROOMY), Some(CantMove::Link));
    assert_eq!(
        problem(&s(&c.join("Other/Samples")), &s(&dir), 1, &ROOMY, all, &stop),
        Some(CantMove::InsideMoved(s(&c.join("Other"))))
    );
    // An update put a real folder back where the link was: moving it again would lose track of the first copy.
    unlink(&c.join("Other"));
    write(&c, "Other/Samples/b.wav", "b");
    assert_eq!(
        problem(&s(&c.join("Other")), &s(&dir), 1, &ROOMY, all, &stop),
        Some(CantMove::MovedBefore(moved.to.clone()))
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
    let result = move_folder(&from, &s(&d), 5, whose(), &ROOMY, &[], &|_| stop.store(true, Ordering::Relaxed), &stop);
    assert_eq!(result, Err(CantMove::Stopped));
    assert!(!d.join("Library").exists());
    assert!(!is_link(Path::new(&from)));
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
    let presets = c.join("Presets");
    let moved = move_folder(&s(&presets), &s(&d), 1, whose(), &ROOMY, &[], &nothing, &stop).unwrap();
    let all = std::slice::from_ref(&moved);
    let bin = FolderBin { dir: dir.join("bin"), held: RefCell::new(Vec::new()) };

    // The copy on the other drive was renamed (or the drive unplugged).
    fs::rename(d.join("Presets"), d.join("Presets renamed")).unwrap();
    assert!(!old_copy_can_go(&moved));
    let done = remove_old_copy(&moved, all, true, &bin, &stop);
    assert!(done.removed.is_empty() && done.problems.len() == 1);
    assert!(moved.has_old_copy());
    assert!(move_back(&moved, &ROOMY, &nothing, &stop).is_err());
    fs::rename(d.join("Presets renamed"), d.join("Presets")).unwrap();

    // The link was pointed somewhere else: moving back leaves it alone.
    write(&dir, "E/Presets/e.fxp", "e");
    unlink(&presets);
    folder_link(&dir.join("E").join("Presets"), &presets);
    assert!(!old_copy_can_go(&moved));
    assert!(matches!(move_back(&moved, &ROOMY, &nothing, &stop), Err(CantMove::Failed(_))));
    assert_eq!(real(&presets), real(&dir.join("E").join("Presets")));
    unlink(&presets);
    folder_link(&d.join("Presets"), &presets);

    // Something was saved into the old copy after the swap: it's named, and the old copy stays unless you're sure.
    write(Path::new(&moved.old_copy), "late.fxp", "late");
    assert_eq!(only_in_old_copy(&moved, all, &stop).unwrap(), ["late.fxp"]);
    let done = remove_old_copy(&moved, all, false, &bin, &stop);
    assert!(done.removed.is_empty() && done.problems[0].why.contains("late.fxp"), "{:?}", done.problems);
    assert!(moved.has_old_copy());
    let done = remove_old_copy(&moved, all, true, &bin, &stop);
    assert_eq!(done.removed.len(), 1);
    assert!(!moved.has_old_copy());
}

#[test]
fn an_old_copy_holding_an_earlier_moves_old_copy_waits_for_its_link() {
    let dir = fixtures("mo-space-move-nested");
    let (c, d, e) = (dir.join("C"), dir.join("D"), dir.join("E"));
    let ni = c.join("NI");
    write(&ni, "Reaktor 6/r.ens", "r");
    write(&ni, "Kontakt/k.nki", "k");
    fs::create_dir_all(&d).unwrap();
    fs::create_dir_all(&e).unwrap();
    let stop = AtomicBool::new(false);
    // Reaktor 6 moves first, then the whole Native Instruments folder.
    let inner = move_folder(&s(&ni.join("Reaktor 6")), &s(&d), 1, whose(), &ROOMY, &[], &nothing, &stop).unwrap();
    let outer =
        move_folder(&s(&ni), &s(&e), 1, whose(), &ROOMY, std::slice::from_ref(&inner), &nothing, &stop).unwrap();
    let all = [inner.clone(), outer.clone()];
    // Reaktor 6's old copy stayed in the old copy, and its files are still found through both links.
    assert!(c.join("NI (old copy)").join("Reaktor 6 (old copy)").join("r.ens").exists());
    assert!(!e.join("NI").join("Reaktor 6 (old copy)").exists());
    assert_eq!(fs::read_to_string(ni.join("Reaktor 6/r.ens")).unwrap(), "r");
    assert_eq!(only_in_old_copy(&outer, &all, &stop).unwrap(), Vec::<String>::new());

    // Reaktor 6's copy went missing: its old copy is the only one left, so the outer old copy stays.
    fs::rename(d.join("Reaktor 6"), d.join("Reaktor 6 renamed")).unwrap();
    let bin = FolderBin { dir: dir.join("bin"), held: RefCell::new(Vec::new()) };
    let done = remove_old_copy(&outer, &all, true, &bin, &stop);
    assert!(done.removed.is_empty() && done.problems[0].why.contains("Reaktor 6"), "{:?}", done.problems);
    fs::rename(d.join("Reaktor 6 renamed"), d.join("Reaktor 6")).unwrap();
    let done = remove_old_copy(&outer, &all, false, &bin, &stop);
    assert!(done.problems.is_empty(), "{:?}", done.problems);
    assert_eq!(done.removed.len(), 1);
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

/// A Recycle Bin that's a folder in the fixtures.
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
