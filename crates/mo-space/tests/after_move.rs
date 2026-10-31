//! What happens to moved folders afterwards (lost, replaced by an update, doubled by a later move) and putting it
//! right, on folders made in the test build's own fixtures folder under fixed names. Other folders there stand in for
//! the other drives. Links are junctions on Windows, as the app makes.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

use mo_space::after_move::{
    KEPT_BEFORE_SUFFIX, OldHome, doubled, find_sharers, home_of, link_again, lost, move_home, old_home_can_go,
    old_homes_json, only_in_old_home, parse_old_homes, real_place, remove_old_home, replaced, repoint,
};
use mo_space::clean::{Bin, Problem, Removed};
use mo_space::folder_move::{Drives, MovedFolder, move_folder};
use mo_space::key;
use mo_space::plugin_space::{PluginSpace, SpaceFolder, SpaceKind};

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

fn read(p: &Path) -> String {
    fs::read_to_string(p).unwrap()
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

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

/// Copies a folder, following the links in it (a stand-in for what the C# app's older move did).
fn copy_all(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_all(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

struct OtherDrive;

impl Drives for OtherDrive {
    fn same(&self, _a: &Path, _b: &Path) -> bool {
        false
    }
    fn free(&self, _path: &Path) -> Option<u64> {
        None
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

fn nothing(_: u64) {}

fn move_it(from: &Path, to: &Path, all: &[MovedFolder]) -> MovedFolder {
    let stop = AtomicBool::new(false);
    move_folder(&s(from), &s(to), 1, ("omnisphere", SpaceKind::Sounds, &[]), &OtherDrive, all, &nothing, &stop).unwrap()
}

fn bin(dir: &Path) -> FolderBin {
    FolderBin { dir: dir.join("bin"), held: RefCell::new(Vec::new()) }
}

#[test]
fn a_folder_an_update_made_where_the_link_was_is_linked_again() {
    let dir = fixtures("mo-space-after-update");
    let (c, d) = (dir.join("C"), dir.join("D"));
    let steam = c.join("STEAM");
    write(&steam, "Pad.db", "pad");
    write(&steam, "Old.db", "old");
    fs::create_dir_all(&d).unwrap();
    let moved = move_it(&steam, &d, &[]);
    let all = vec![moved.clone()];
    let copy = d.join("STEAM");
    // A preset saved through the link, and Old.db changed there too.
    write(&steam, "User.db", "mine");
    write(&steam, "Old.db", "old, changed");
    assert!(replaced(&all).is_empty() && lost(&all).is_empty());

    // The update deletes the link and makes the folder again, with a new Pad.db and an Old.db carrying the older time it
    // came with.
    unlink(&steam);
    write(&steam, "Pad.db", "pad 2");
    write(&steam, "Update.db", "new");
    write(&steam, "Old.db", "old from the update");
    set_time(&steam.join("Old.db"), SystemTime::now() - Duration::from_secs(3600));
    assert_eq!(replaced(&all), [&moved]);
    assert!(doubled(&all).is_empty());

    let stop = AtomicBool::new(false);
    // One set aside before keeps its name: this one gets a number.
    fs::create_dir_all(c.join("STEAM (after update)")).unwrap();
    let home = link_again(&moved, &all, 5, false, &OtherDrive, &nothing, &stop).unwrap();
    assert_eq!(home.path, s(&c.join("STEAM (after update 2)")));
    assert_eq!(home.note, "left by an update");
    assert_eq!(home.copy, moved.to);
    assert!(is_link(&steam) && replaced(&all).is_empty());
    // The plugin finds the update's files and the preset saved before it.
    assert_eq!(read(&steam.join("Pad.db")), "pad 2");
    assert_eq!(read(&steam.join("Update.db")), "new");
    assert_eq!(read(&steam.join("User.db")), "mine");
    assert_eq!(read(&steam.join("Old.db")), "old from the update");
    // The copy's own newer Old.db was kept beside it, not replaced.
    assert_eq!(read(&PathBuf::from(format!("{}{KEPT_BEFORE_SUFFIX}", moved.to)).join("Old.db")), "old, changed");
    assert!(copy.join("Pad.db").exists());

    // The folder set aside goes once nothing is only in it.
    assert!(old_home_can_go(&home, &all));
    assert_eq!(only_in_old_home(&home, &stop).unwrap(), Vec::<String>::new());
    let bin = bin(&dir);
    let done = remove_old_home(&home, &all, false, &bin, &stop);
    assert!(done.problems.is_empty(), "{:?}", done.problems);
    assert!(!Path::new(&home.path).exists());
}

#[test]
fn a_lost_link_and_a_home_moved_by_hand_are_found_and_pointed_again() {
    let dir = fixtures("mo-space-after-lost");
    let (c, d) = (dir.join("C"), dir.join("D"));
    write(&c, "STEAM/Pad.db", "pad");
    write(&c, "Kontakt/a.nki", "a");
    let data = d.join("Plugin Data");
    fs::create_dir_all(&data).unwrap();
    let steam = move_it(&c.join("STEAM"), &data, &[]);
    let kontakt = move_it(&c.join("Kontakt"), &data, std::slice::from_ref(&steam));
    let all = vec![steam.clone(), kontakt.clone()];
    assert_eq!(home_of(&steam), s(&data));

    // Plugin Data renamed by hand: both links lead nowhere.
    fs::rename(&data, d.join("Data")).unwrap();
    assert_eq!(lost(&all).len(), 2);
    // Kontakt's link was replaced by a real folder meanwhile: it's left alone.
    unlink(&c.join("Kontakt"));
    write(&c, "Kontakt/b.nki", "b");
    let (now, problems) = repoint(&s(&data), &s(&d.join("Data")), &all);
    assert!(lost(&now).is_empty());
    assert_eq!(read(&c.join("STEAM/Pad.db")), "pad");
    assert_eq!(now[0].to, s(&d.join("Data").join("STEAM")));
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0].path, kontakt.from);
    assert_eq!(now[1].to, kontakt.to);
    assert_eq!(read(&c.join("Kontakt/b.nki")), "b");
}

#[test]
fn the_folder_moved_folders_live_in_moves_as_a_whole() {
    let dir = fixtures("mo-space-after-home");
    let (c, d, e) = (dir.join("C"), dir.join("D"), dir.join("E"));
    write(&c, "STEAM/Pad.db", "pad");
    write(&c, "Kontakt/a.nki", "a");
    write(&c, "Reaktor/r.ens", "r");
    let data = d.join("Plugin Data");
    fs::create_dir_all(&data).unwrap();
    fs::create_dir_all(&e).unwrap();
    let mut all = Vec::new();
    for name in ["STEAM", "Kontakt", "Reaktor"] {
        let m = move_it(&c.join(name), &data, &all);
        all.push(m);
    }
    write(&data, "notes.txt", "mine");
    // Reaktor's copy went missing: its link can't be pointed at the new place.
    fs::rename(data.join("Reaktor"), d.join("Reaktor elsewhere")).unwrap();

    let stop = AtomicBool::new(false);
    let done = move_home(&s(&data), &s(&e), &all, 3, &OtherDrive, &nothing, &stop).unwrap();
    let new_home = e.join("Plugin Data");
    assert_eq!(read(&new_home.join("notes.txt")), "mine");
    assert_eq!(real(&c.join("STEAM")), real(&new_home.join("STEAM")));
    assert_eq!(read(&c.join("Kontakt/a.nki")), "a");
    assert_eq!(done.moved[0].to, s(&new_home.join("STEAM")));
    assert_eq!(done.moved[2].to, all[2].to);
    assert_eq!(done.problems.len(), 1);
    assert_eq!(done.problems[0].path, all[2].from);
    // Its old place stays while Reaktor's link still leads into it.
    let old = done.old_home.unwrap();
    assert_eq!(old.path, s(&data));
    assert!(!old_home_can_go(&old, &done.moved));
    let bin = bin(&dir);
    assert!(remove_old_home(&old, &done.moved, true, &bin, &stop).removed.is_empty());
    // Once that's sorted (Reaktor's record gone, say), it can go, and nothing is only in it.
    let sorted: Vec<MovedFolder> = done.moved[..2].to_vec();
    assert!(old_home_can_go(&old, &sorted));
    assert_eq!(only_in_old_home(&old, &stop).unwrap(), Vec::<String>::new());
    // Something saved there after the move is named first.
    write(&data, "late.txt", "late");
    assert_eq!(only_in_old_home(&old, &stop).unwrap(), ["late.txt"]);
    let kept = remove_old_home(&old, &sorted, false, &bin, &stop);
    assert!(kept.removed.is_empty() && kept.problems[0].why.contains("late.txt"));
    assert_eq!(remove_old_home(&old, &sorted, true, &bin, &stop).removed.len(), 1);
}

#[test]
fn on_the_same_drive_the_home_is_renamed() {
    let dir = fixtures("mo-space-after-rename");
    let (c, d) = (dir.join("C"), dir.join("D"));
    write(&c, "STEAM/Pad.db", "pad");
    let data = d.join("Plugin Data");
    fs::create_dir_all(&data).unwrap();
    fs::create_dir_all(d.join("Elsewhere")).unwrap();
    let all = vec![move_it(&c.join("STEAM"), &data, &[])];
    let stop = AtomicBool::new(false);
    let done = move_home(&s(&data), &s(&d.join("Elsewhere")), &all, 3, &SameDrive, &nothing, &stop).unwrap();
    assert!(done.problems.is_empty() && done.old_home.is_none());
    assert!(!data.exists());
    assert_eq!(read(&c.join("STEAM/Pad.db")), "pad");
    assert_eq!(done.moved[0].to, s(&d.join("Elsewhere").join("Plugin Data").join("STEAM")));
}

#[test]
fn a_second_copy_a_later_move_made_is_found_and_linked_again() {
    let dir = fixtures("mo-space-after-doubled");
    let (c, d, e) = (dir.join("C"), dir.join("D"), dir.join("E"));
    let ni = c.join("NI");
    write(&ni, "Reaktor 6/r.ens", "r");
    write(&ni, "Kontakt/k.nki", "k");
    fs::create_dir_all(&d).unwrap();
    fs::create_dir_all(&e).unwrap();
    let inner = move_it(&ni.join("Reaktor 6"), &d, &[]);
    // The C# app's older move of the folder around it: Reaktor 6's link was followed and its files copied.
    copy_all(&ni, &e.join("NI"));
    fs::rename(&ni, c.join("NI (old copy)")).unwrap();
    folder_link(&e.join("NI"), &ni);
    let outer = MovedFolder {
        from: s(&ni),
        to: s(&e.join("NI")),
        old_copy: s(&c.join("NI (old copy)")),
        bytes: 2,
        moved: "2026-10-27T10:00:00.0000000+00:00".to_owned(),
        key: "kontakt".to_owned(),
        kind: 2,
        shared_with: Vec::new(),
    };
    let all = vec![inner.clone(), outer];
    assert_eq!(doubled(&all), [(&inner, s(&ni))]);
    assert!(replaced(&all).is_empty());
    let aside = s(&ni.join("Reaktor 6 (second copy)"));
    assert_eq!(real_place(&aside, &all), s(&e.join("NI").join("Reaktor 6 (second copy)")));

    // The plugin saved into the second copy since.
    write(&ni, "Reaktor 6/new.ens", "new");
    let stop = AtomicBool::new(false);
    let home = link_again(&inner, &all, 1, true, &OtherDrive, &nothing, &stop).unwrap();
    assert_eq!(home.path, s(&e.join("NI").join("Reaktor 6 (second copy)")));
    assert_eq!(home.note, "a second copy");
    assert!(is_link(&e.join("NI").join("Reaktor 6")));
    assert_eq!(read(&ni.join("Reaktor 6/new.ens")), "new");
    assert_eq!(read(&d.join("Reaktor 6").join("new.ens")), "new");
    assert!(doubled(&all).is_empty());
}

#[test]
fn moved_folders_shared_with_other_plugins_are_filled_in() {
    let m = |from: &str, key: &str, shared: &[&str]| MovedFolder {
        from: from.to_owned(),
        to: format!("I:\\{from}"),
        old_copy: format!("{from} (old copy)"),
        bytes: 1,
        moved: String::new(),
        key: key.to_owned(),
        kind: 2,
        shared_with: shared.iter().map(|s| (*s).to_owned()).collect(),
    };
    let space = |key: &str, folder: &str| PluginSpace {
        key: key.to_owned(),
        name: key.to_owned(),
        vendor: String::new(),
        folders: vec![SpaceFolder {
            path: folder.to_owned(),
            kind: SpaceKind::Sounds,
            bytes: 1,
            shared_with: Vec::new(),
            added: false,
        }],
    };
    let moved = [m(r"C:\Toontrack", "ezkeys2", &[]), m(r"C:\Spectrasonics", "omnisphere", &["keyscape"])];
    let spaces = [
        space("ezkeys2", r"C:\Toontrack"),
        space("superior3", r"c:\toontrack"),
        space("keyscape", r"C:\Spectrasonics"),
        space("trilian", r"C:\Spectrasonics"),
    ];
    let (now, stale) = find_sharers(&moved, &spaces);
    assert_eq!(now[0].shared_with, ["superior3"]);
    // A folder with sharers already keeps them.
    assert_eq!(now[1].shared_with, ["keyscape"]);
    assert_eq!(stale, ["superior3"]);
}

#[test]
fn old_homes_are_kept_as_the_csharp_app_keeps_them() {
    let homes =
        parse_old_homes(r#"[{"Path":"I:\\Plugin Data","Bytes":5,"Note":"its old place"},{"Path":"x","Bytes":1}]"#);
    assert_eq!(homes.len(), 2);
    assert_eq!(homes[1].note, "its old place");
    assert!(homes[0].copy.is_empty());
    assert_eq!(parse_old_homes(&old_homes_json(&homes)), homes);
    // The C# app reads it back: the field it doesn't know is only written when there is one.
    assert!(!old_homes_json(&homes).contains("Copy"));
    let with = OldHome { copy: "E:\\Plugin Data".to_owned(), ..homes[0].clone() };
    assert!(old_homes_json(&[with]).contains(r#""Copy":"E:\\Plugin Data""#));
    assert!(parse_old_homes("not json").is_empty());
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
    fn put_back(&self, _removed: &[Removed]) -> Vec<Problem> {
        Vec::new()
    }
}
