//! What happens to moved folders afterwards (lost, replaced by an update, doubled by a later move) and putting it
//! right, on folders made in the test build's own fixtures folder under fixed names. Other folders there stand in for
//! the other drives. Links are junctions on Windows, as the app makes.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

use mo_space::admin::NoAdmin;
use mo_space::after_move::{
    KEPT_BEFORE_SUFFIX, OldHome, doubled, find_sharers, home_of, kept_beside, link_again, lost, move_home,
    old_home_can_go, old_homes_json, only_in_old_home, parse_old_homes, real_place, remove_old_home, replaced, repoint,
};
use mo_space::clean::{Bin, Problem, Removed};
use mo_space::folder_move::{CantMove, Drives, MovedFolder, move_back, move_folder, only_in_old_copy};
use mo_space::key;
use mo_space::plugin_space::{PluginSpace, SpaceFolder, SpaceKind};

mod common;

use common::{folder_link, is_link, unlink};

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

/// Every folder is on its own drive; the ones in `exfat` hold no links (exFAT drives, say).
struct NoLinks<'a> {
    exfat: &'a Path,
}

impl Drives for NoLinks<'_> {
    fn same(&self, _a: &Path, _b: &Path) -> bool {
        false
    }
    fn free(&self, _path: &Path) -> Option<u64> {
        None
    }
    fn holds_links(&self, path: &Path) -> bool {
        !path.starts_with(self.exfat)
    }
}

fn nothing(_: u64) {}

fn move_it(from: &Path, to: &Path, all: &[MovedFolder]) -> MovedFolder {
    let stop = AtomicBool::new(false);
    move_folder(&s(from), &s(to), 1, ("omnisphere", SpaceKind::Sounds, &[]), &OtherDrive, all, &[], &nothing, &stop)
        .unwrap()
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
    let linked = link_again(&moved, &all, 5, false, &OtherDrive, &nothing, &stop).unwrap();
    let home = linked.set_aside.clone();
    assert_eq!(home.path, s(&c.join("STEAM (after update 2)")));
    assert_eq!(home.note, "left by an update");
    assert_eq!(home.copy, moved.to);
    assert!(is_link(&steam) && replaced(&all).is_empty());
    // The plugin finds the update's files and the preset saved before it.
    assert_eq!(read(&steam.join("Pad.db")), "pad 2");
    assert_eq!(read(&steam.join("Update.db")), "new");
    assert_eq!(read(&steam.join("User.db")), "mine");
    assert_eq!(read(&steam.join("Old.db")), "old from the update");
    // Every file of the copy that was replaced is kept beside it first: the newer Old.db, and Pad.db too.
    let kept_in = PathBuf::from(format!("{}{KEPT_BEFORE_SUFFIX}", moved.to));
    assert_eq!(linked.replaced, ["Old.db", "Pad.db"]);
    assert_eq!(read(&kept_in.join("Old.db")), "old, changed");
    assert_eq!(read(&kept_in.join("Pad.db")), "pad");
    assert!(!kept_in.join("User.db").exists());
    let kept = linked.kept.clone().unwrap();
    assert_eq!(kept.path, s(&kept_in));
    assert_eq!(kept.note, "replaced by linking again");
    assert!(copy.join("Pad.db").exists());
    // Folders of kept files are found beside the copy even when nothing listed them (a try that stopped partway); a
    // folder that only looks like one isn't.
    fs::create_dir_all(d.join("STEAM (kept from before x)")).unwrap();
    assert_eq!(kept_beside(&all, &[], &stop), std::slice::from_ref(&kept));
    assert!(kept_beside(&all, &[home.clone(), kept.clone()], &stop).is_empty());

    // The folder set aside goes once nothing is only in it.
    assert!(old_home_can_go(&home, &all));
    assert_eq!(only_in_old_home(&home, &stop).unwrap(), Some(Vec::new()));
    let bin = bin(&dir);
    let done = remove_old_home(&home, &all, false, &bin, &stop);
    assert!(done.problems.is_empty(), "{:?}", done.problems);
    assert!(!Path::new(&home.path).exists());
    // The files kept from before have nothing to check against: they go only once you're sure.
    assert_eq!(only_in_old_home(&kept, &stop).unwrap(), None);
    let stays = remove_old_home(&kept, &all, false, &bin, &stop);
    assert!(stays.removed.is_empty() && stays.problems[0].why.contains("sure"));
    assert_eq!(remove_old_home(&kept, &all, true, &bin, &stop).removed.len(), 1);
    assert!(!kept_in.exists());

    // Linking again when it isn't needed is refused, and touches nothing.
    assert!(link_again(&moved, &all, 5, false, &OtherDrive, &nothing, &stop).is_err());
    assert!(is_link(&steam));
}

/// The update made the folder again in other capitals ("steam" for "STEAM"), on a drive that doesn't tell them apart
/// (Windows and a Mac; where the drive tells them apart it's another folder, so there's nothing to try). Checking it
/// can be renamed gives it the name the move noted, in its capitals: it's the same folder, and it's linked again.
#[test]
fn a_folder_an_update_made_in_other_capitals_is_linked_again() {
    let dir = fixtures("mo-space-after-update-capitals");
    let (c, d) = (dir.join("C"), dir.join("D"));
    let steam = c.join("STEAM");
    write(&steam, "Pad.db", "pad");
    fs::create_dir_all(&d).unwrap();
    let moved = move_it(&steam, &d, &[]);
    let all = vec![moved.clone()];
    unlink(&steam);
    write(&c, "steam/Pad.db", "pad 2");
    if !steam.join("Pad.db").exists() {
        return;
    }
    assert_eq!(replaced(&all), [&moved]);
    let stop = AtomicBool::new(false);
    let linked = link_again(&moved, &all, 5, false, &OtherDrive, &nothing, &stop).unwrap();
    assert!(is_link(&steam) && replaced(&all).is_empty());
    assert_eq!(read(&steam.join("Pad.db")), "pad 2");
    assert_eq!(linked.replaced, ["Pad.db"]);
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

    // A folder left behind in there, and one elsewhere.
    fs::create_dir_all(data.join("Kontakt (after update)")).unwrap();
    let homes = [
        OldHome {
            path: s(&data.join("Kontakt (after update)")),
            bytes: 1,
            note: "left by an update".to_owned(),
            copy: kontakt.to.clone(),
            kind: 2,
        },
        OldHome { path: s(&c.join("Old")), bytes: 1, note: "its old place".to_owned(), copy: String::new(), kind: 2 },
    ];

    // A link that leads somewhere else (a home move cut off after the links were pointed at the new place, before the
    // folder went there) is lost too, though its copy is still where the record says.
    let elsewhere = d.join("Not yet");
    fs::create_dir_all(&elsewhere).unwrap();
    unlink(&c.join("STEAM"));
    folder_link(&elsewhere, &c.join("STEAM"));
    assert_eq!(lost(&all), [&all[0]]);
    unlink(&c.join("STEAM"));
    folder_link(Path::new(&all[0].to), &c.join("STEAM"));
    assert!(lost(&all).is_empty());

    // Plugin Data renamed by hand: both links lead nowhere.
    let by_hand = d.join("Data");
    fs::rename(&data, &by_hand).unwrap();
    assert_eq!(lost(&all).len(), 2);
    // Kontakt's link was replaced by a real folder meanwhile: it's left alone, but its record follows the copy, so it
    // shows as replaced by an update.
    unlink(&c.join("Kontakt"));
    write(&c, "Kontakt/b.nki", "b");
    let now = repoint(&s(&data), &s(&by_hand), &all, &homes, &NoAdmin);
    assert!(now.problems.is_empty(), "{:?}", now.problems);
    assert!(now.old_home.is_none());
    assert!(lost(&now.moved).is_empty());
    assert_eq!(read(&c.join("STEAM/Pad.db")), "pad");
    assert_eq!(now.moved[0].to, s(&by_hand.join("STEAM")));
    assert_eq!(now.moved[1].to, s(&by_hand.join("Kontakt")));
    assert_eq!(replaced(&now.moved), [&now.moved[1]]);
    assert!(!is_link(&c.join("Kontakt")));
    assert_eq!(read(&c.join("Kontakt/b.nki")), "b");
    // The folder left behind in there follows too; the other stays as it was.
    assert_eq!(now.old_homes[0].path, s(&by_hand.join("Kontakt (after update)")));
    assert_eq!(now.old_homes[0].copy, s(&by_hand.join("Kontakt")));
    assert_eq!(now.old_homes[1], homes[1]);

    // A copy that isn't at the new place is named, and its link left as it was.
    fs::rename(by_hand.join("STEAM"), d.join("STEAM elsewhere")).unwrap();
    let again = repoint(&s(&by_hand), &s(&data), &now.moved, &now.old_homes, &NoAdmin);
    assert_eq!(again.problems.len(), 2);
    assert_eq!(again.moved, now.moved);
}

/// A link the plugin points elsewhere just before a move's swap (after the last pass made it again) leads there only
/// from the old copy, and is still named once the folder the moved folders live in has moved to another drive, where
/// the copy's links are made again.
#[test]
fn a_link_pointed_elsewhere_just_before_the_swap_is_still_named_once_the_moved_folders_home_moves() {
    let dir = fixtures("mo-space-after-home-link-repointed");
    let (c, d, e) = (dir.join("C"), dir.join("D"), dir.join("E"));
    let library = c.join("Library");
    write(&library, "Sub/f.db", "x");
    fs::create_dir_all(dir.join("KitsA")).unwrap();
    fs::create_dir_all(dir.join("KitsB")).unwrap();
    folder_link(&dir.join("KitsA"), &library.join("Kits"));
    let data = d.join("Plugin Data");
    fs::create_dir_all(&data).unwrap();
    fs::create_dir_all(&e).unwrap();
    let stop = AtomicBool::new(false);
    // The plugin saves f.db again as the first pass copies it, so the last pass copies it too, after the top folder's
    // link (its turn comes first); as it does, the plugin points the link at the other kits.
    let progress = |bytes: u64| {
        if bytes == 1 {
            fs::write(library.join("Sub/f.db"), "yy").unwrap();
        } else if bytes == 3 {
            unlink(&library.join("Kits"));
            folder_link(&dir.join("KitsB"), &library.join("Kits"));
        }
    };
    let whose = ("omnisphere", SpaceKind::Sounds, &[] as &[String]);
    let moved = move_folder(&s(&library), &s(&data), 1, whose, &OtherDrive, &[], &[], &progress, &stop).unwrap();
    let all = vec![moved];
    assert_eq!(only_in_old_copy(&all[0], &all, &stop).unwrap(), ["Kits"]);
    let done = move_home(&s(&data), &s(&e), &all, &[], 3, &OtherDrive, &nothing, &stop).unwrap();
    assert_eq!(done.moved[0].to, s(&e.join("Plugin Data").join("Library")));
    assert_eq!(only_in_old_copy(&done.moved[0], &done.moved, &stop).unwrap(), ["Kits"]);
}

/// A moved folder inside another (Reaktor 6 first, then Native Instruments around it): once the folder the moved
/// folders live in moves to another drive, the copy's link to Reaktor 6 leads to its new place, and the old copy's
/// still to the old one. That link is the app's own, so it isn't taken for something only in the old copy.
#[test]
fn an_inner_moved_folders_link_isnt_only_in_the_old_copy_once_the_moved_folders_home_moves() {
    let dir = fixtures("mo-space-after-home-inner-link");
    let (c, d, e) = (dir.join("C"), dir.join("D"), dir.join("E"));
    write(&c, "NI/Reaktor 6/r.ens", "r");
    write(&c, "NI/k.nki", "k");
    let data = d.join("Plugin Data");
    fs::create_dir_all(&data).unwrap();
    fs::create_dir_all(&e).unwrap();
    let mut all = vec![move_it(&c.join("NI").join("Reaktor 6"), &data, &[])];
    all.push(move_it(&c.join("NI"), &data, &all));
    let stop = AtomicBool::new(false);
    assert!(only_in_old_copy(&all[1], &all, &stop).unwrap().is_empty());
    let done = move_home(&s(&data), &s(&e), &all, &[], 2, &OtherDrive, &nothing, &stop).unwrap();
    let new_home = e.join("Plugin Data");
    assert_eq!(done.moved[1].to, s(&new_home.join("NI")));
    assert_eq!(real(&new_home.join("NI").join("Reaktor 6")), real(&new_home.join("Reaktor 6")));
    assert!(only_in_old_copy(&done.moved[1], &done.moved, &stop).unwrap().is_empty());
}

/// Moving back a folder with a moved folder inside it (Reaktor 6 first, then Native Instruments around it), once the
/// folder the moved folders live in has moved to another drive: the old copy's link to Reaktor 6 still leads to its
/// old place, so it's made as the other drive's is, leading to where Reaktor 6 is now.
#[test]
fn moving_back_makes_an_inner_moved_folders_link_lead_where_it_is_now() {
    let dir = fixtures("mo-space-after-home-inner-back");
    let (c, d, e) = (dir.join("C"), dir.join("D"), dir.join("E"));
    write(&c, "NI/Reaktor 6/r.ens", "r");
    write(&c, "NI/k.nki", "k");
    let data = d.join("Plugin Data");
    fs::create_dir_all(&data).unwrap();
    fs::create_dir_all(&e).unwrap();
    let mut all = vec![move_it(&c.join("NI").join("Reaktor 6"), &data, &[])];
    all.push(move_it(&c.join("NI"), &data, &all));
    let stop = AtomicBool::new(false);
    let done = move_home(&s(&data), &s(&e), &all, &[], 2, &OtherDrive, &nothing, &stop).unwrap();
    // The plugin saves through the links, at Reaktor 6's new place.
    fs::write(c.join("NI/Reaktor 6/r.ens"), "r-new").unwrap();
    let back = move_back(&done.moved[1], &done.moved, &OtherDrive, &nothing, &stop).unwrap();
    assert!(back.links_left.is_empty(), "{:?}", back.links_left);
    let inner = c.join("NI").join("Reaktor 6");
    assert!(is_link(&inner) && !is_link(&c.join("NI")));
    assert_eq!(real(&inner), real(&e.join("Plugin Data").join("Reaktor 6")));
    assert_eq!(read(&inner.join("r.ens")), "r-new");
}

/// As the last pass copies the folder the moved folders live in, it's put aside, with a link to another folder in its
/// place, and that link is pointed at nothing just as the last file comes in: what was read through it isn't the
/// moved folders', so the move stops, the copy goes, and the links still lead where they did.
#[cfg(unix)]
#[test]
fn moving_the_moved_folders_home_stops_when_it_turns_into_a_link_that_leads_nowhere() {
    let dir = fixtures("mo-space-after-home-turns-into-link");
    let (c, d, e) = (dir.join("C"), dir.join("D"), dir.join("E"));
    for n in 1..=3 {
        write(&c, &format!("STEAM/{n}.db"), "x");
        write(&dir, &format!("Other/STEAM/{n}.db"), "zz");
    }
    let data = d.join("Plugin Data");
    fs::create_dir_all(&data).unwrap();
    fs::create_dir_all(&e).unwrap();
    let all = vec![move_it(&c.join("STEAM"), &data, &[])];
    let steam = data.join("STEAM");
    let stop = AtomicBool::new(false);
    // The plugin saves them all again as the first pass ends, so the last pass copies them too; as the first of those
    // comes in, the folder is put aside with a link to the other folder in its place, and as the last comes in, that
    // link is pointed at nothing.
    let progress = |bytes: u64| {
        if bytes == 3 {
            (1..=3).for_each(|n| fs::write(steam.join(format!("{n}.db")), "yy").unwrap());
        } else if bytes == 5 {
            fs::rename(&data, d.join("Plugin Data aside")).unwrap();
            folder_link(&dir.join("Other"), &data);
        } else if bytes == 9 {
            unlink(&data);
            folder_link(&dir.join("Nowhere"), &data);
        }
    };
    let result = move_home(&s(&data), &s(&e), &all, &[], 3, &OtherDrive, &progress, &stop);
    let Err(CantMove::Failed(why)) = &result else { panic!("{result:?}") };
    assert!(why.starts_with(&format!("{} turned into a link", s(&data))), "{why}");
    assert!(!e.join("Plugin Data").exists());
    assert_eq!(fs::read_link(c.join("STEAM")).unwrap(), steam);
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
    let done = move_home(&s(&data), &s(&e), &all, &[], 3, &OtherDrive, &nothing, &stop).unwrap();
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
    assert_eq!(only_in_old_home(&old, &stop).unwrap(), Some(Vec::new()));
    // Something saved there after the move is named first.
    write(&data, "late.txt", "late");
    assert_eq!(only_in_old_home(&old, &stop).unwrap(), Some(vec!["late.txt".to_owned()]));
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
    write(&c, "NI/Reaktor 6/r.ens", "r");
    write(&c, "NI/k.nki", "k");
    let mut all = vec![move_it(&c.join("STEAM"), &data, &[])];
    // A moved folder inside another: Reaktor 6 first, then Native Instruments around it (its link stays a link).
    all.push(move_it(&c.join("NI").join("Reaktor 6"), &data, &all));
    all.push(move_it(&c.join("NI"), &data, &all));
    fs::create_dir_all(data.join("STEAM (after update)")).unwrap();
    let homes = [OldHome {
        path: s(&data.join("STEAM (after update)")),
        bytes: 1,
        note: "left by an update".to_owned(),
        copy: all[0].to.clone(),
        kind: 2,
    }];
    let stop = AtomicBool::new(false);
    let done = move_home(&s(&data), &s(&d.join("Elsewhere")), &all, &homes, 3, &SameDrive, &nothing, &stop).unwrap();
    assert!(done.problems.is_empty() && done.old_home.is_none());
    assert!(!data.exists());
    let new_home = d.join("Elsewhere").join("Plugin Data");
    assert_eq!(read(&c.join("STEAM/Pad.db")), "pad");
    assert_eq!(real(&c.join("STEAM")), real(&new_home.join("STEAM")));
    assert_eq!(done.moved[0].to, s(&new_home.join("STEAM")));
    assert_eq!(read(&c.join("NI/Reaktor 6/r.ens")), "r");
    assert_eq!(real(&c.join("NI").join("Reaktor 6")), real(&new_home.join("Reaktor 6")));
    assert_eq!(real(&c.join("NI")), real(&new_home.join("NI")));
    assert_eq!(done.moved[1].to, s(&new_home.join("Reaktor 6")));
    assert_eq!(done.moved[2].to, s(&new_home.join("NI")));
    assert!(lost(&done.moved).is_empty() && doubled(&done.moved).is_empty());
    assert_eq!(done.old_homes[0].path, s(&new_home.join("STEAM (after update)")));
    assert_eq!(done.old_homes[0].copy, done.moved[0].to);
}

#[test]
fn an_old_home_that_is_a_link_now_stays() {
    let dir = fixtures("mo-space-after-old-home-link");
    let (d, e) = (dir.join("D"), dir.join("E"));
    write(&e, "Plugin Data/STEAM/Pad.db", "pad");
    fs::create_dir_all(&d).unwrap();
    // You put a link where the old home was, so old paths still work: removing it would take nothing of its own.
    folder_link(&e.join("Plugin Data"), &d.join("Plugin Data"));
    let old = OldHome {
        path: s(&d.join("Plugin Data")),
        bytes: 3,
        note: "its old place".to_owned(),
        copy: s(&e.join("Plugin Data")),
        kind: 2,
    };
    assert!(!old_home_can_go(&old, &[]));
    let done = remove_old_home(&old, &[], true, &bin(&dir), &AtomicBool::new(false));
    assert!(done.removed.is_empty());
    assert_eq!(done.problems[0].why, "It's a link now, so removing it frees nothing.");
    assert!(is_link(&d.join("Plugin Data")));
    assert_eq!(read(&e.join("Plugin Data/STEAM/Pad.db")), "pad");
}

#[test]
fn on_the_same_drive_a_home_holding_a_moved_folders_link_stays() {
    let dir = fixtures("mo-space-after-rename-holding");
    let (d, e) = (dir.join("D"), dir.join("E"));
    let data = d.join("Plugin Data");
    write(&data, "Extra/x.db", "x");
    fs::create_dir_all(d.join("Elsewhere")).unwrap();
    fs::create_dir_all(&e).unwrap();
    // A folder in it was moved to another drive: its link and old copy are in it.
    let all = vec![move_it(&data.join("Extra"), &e, &[])];
    let stop = AtomicBool::new(false);
    let result = move_home(&s(&data), &s(&d.join("Elsewhere")), &all, &[], 3, &SameDrive, &nothing, &stop);
    let Err(CantMove::Failed(why)) = result else { panic!("{result:?}") };
    assert!(why.contains("Move that folder back first"), "{why}");
    assert!(is_link(&data.join("Extra")) && all[0].has_old_copy());
    assert_eq!(read(&data.join("Extra/x.db")), "x");
}

/// When a link won't change (here a folder the test can't write in, standing in for one Windows won't let go of), the
/// links changed are put back and nothing moves. Root can write anywhere, so it's only checked as a normal user (as CI
/// runs).
#[cfg(unix)]
#[test]
fn on_the_same_drive_a_link_that_wont_change_puts_it_all_back() {
    use std::os::unix::fs::PermissionsExt;

    use mo_space::folder_move::CantMove;
    let dir = fixtures("mo-space-after-rename-back");
    let (c, locked, d) = (dir.join("C"), dir.join("Locked"), dir.join("D"));
    write(&c, "STEAM/Pad.db", "pad");
    write(&locked, "Kontakt/a.nki", "a");
    let data = d.join("Plugin Data");
    fs::create_dir_all(&data).unwrap();
    fs::create_dir_all(d.join("Elsewhere")).unwrap();
    let kontakt = move_it(&locked.join("Kontakt"), &data, &[]);
    let steam = move_it(&c.join("STEAM"), &data, std::slice::from_ref(&kontakt));
    // STEAM's link is changed first, then Kontakt's won't: STEAM's goes back.
    let all = vec![kontakt, steam];
    let lock = |mode: u32| fs::set_permissions(&locked, fs::Permissions::from_mode(mode)).unwrap();
    lock(0o555);
    if fs::write(locked.join("probe"), "x").is_ok() {
        // Running as root: nothing stops it.
        let _ = fs::remove_file(locked.join("probe"));
        lock(0o755);
        return;
    }
    let stop = AtomicBool::new(false);
    let done = move_home(&s(&data), &s(&d.join("Elsewhere")), &all, &[], 3, &SameDrive, &nothing, &stop);
    lock(0o755);
    assert!(matches!(done, Err(CantMove::Failed(_))), "{done:?}");
    assert!(data.is_dir() && !d.join("Elsewhere").join("Plugin Data").exists());
    assert_eq!(real(&c.join("STEAM")), real(&data.join("STEAM")));
    assert_eq!(real(&locked.join("Kontakt")), real(&data.join("Kontakt")));
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
    let home = link_again(&inner, &all, 1, true, &OtherDrive, &nothing, &stop).unwrap().set_aside;
    assert_eq!(home.path, s(&e.join("NI").join("Reaktor 6 (second copy)")));
    assert_eq!(home.note, "a second copy");
    assert!(is_link(&e.join("NI").join("Reaktor 6")));
    assert_eq!(read(&ni.join("Reaktor 6/new.ens")), "new");
    assert_eq!(read(&d.join("Reaktor 6").join("new.ens")), "new");
    assert!(doubled(&all).is_empty());
}

#[test]
fn a_second_copy_behind_a_lost_link_is_found_once_the_link_is_pointed_again() {
    let dir = fixtures("mo-space-after-doubled-lost");
    let (c, d) = (dir.join("C"), dir.join("D"));
    let ni = c.join("NI");
    write(&ni, "Reaktor 6/r.ens", "r");
    write(&ni, "k.nki", "k");
    let data = d.join("Plugin Data");
    fs::create_dir_all(&data).unwrap();
    let inner = move_it(&ni.join("Reaktor 6"), &data, &[]);
    // The C# app's older move of the folder around it, into the same place: a second copy of Reaktor 6 inside.
    copy_all(&ni, &data.join("NI"));
    fs::rename(&ni, c.join("NI (old copy)")).unwrap();
    folder_link(&data.join("NI"), &ni);
    let outer = MovedFolder {
        from: s(&ni),
        to: s(&data.join("NI")),
        old_copy: s(&c.join("NI (old copy)")),
        bytes: 2,
        moved: "2026-10-27T10:00:00.0000000+00:00".to_owned(),
        key: "kontakt".to_owned(),
        kind: 2,
        shared_with: Vec::new(),
    };
    let all = vec![inner, outer];
    assert_eq!(doubled(&all).len(), 1);

    // Plugin Data renamed by hand: the second copy can't be seen until Native Instruments' link leads somewhere.
    let by_hand = d.join("Data");
    fs::rename(&data, &by_hand).unwrap();
    let now = repoint(&s(&data), &s(&by_hand), &all, &[], &NoAdmin);
    assert!(now.problems.is_empty(), "{:?}", now.problems);
    assert_eq!(now.moved[1].to, s(&by_hand.join("NI")));
    // The second copy is left alone, and Reaktor 6's record follows its copy, so it shows as doubled.
    assert_eq!(now.moved[0].to, s(&by_hand.join("Reaktor 6")));
    assert!(!is_link(&ni.join("Reaktor 6")));
    assert_eq!(doubled(&now.moved), [(&now.moved[0], s(&ni))]);
}

/// What the plugin saves while linking again copies is the update's, not yours: it goes in without being kept again.
#[test]
fn a_file_saved_while_linking_again_goes_in_once() {
    let dir = fixtures("mo-space-after-saved");
    let (c, d) = (dir.join("C"), dir.join("D"));
    let steam = c.join("STEAM");
    write(&steam, "Pad.db", "pad");
    fs::create_dir_all(&d).unwrap();
    let moved = move_it(&steam, &d, &[]);
    let all = vec![moved.clone()];
    unlink(&steam);
    write(&steam, "Pad.db", "pad 2");
    let stop = AtomicBool::new(false);
    let saved = AtomicBool::new(false);
    let save = |_: u64| {
        if !saved.swap(true, std::sync::atomic::Ordering::SeqCst) {
            write(&steam, "Pad.db", "pad, saved meanwhile");
        }
    };
    let linked = link_again(&moved, &all, 5, false, &OtherDrive, &save, &stop).unwrap();
    assert!(saved.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(read(&steam.join("Pad.db")), "pad, saved meanwhile");
    assert_eq!(linked.replaced, ["Pad.db"]);
    assert_eq!(read(&PathBuf::from(&linked.kept.unwrap().path).join("Pad.db")), "pad");
}

#[test]
fn linking_again_stopped_before_the_copy_changes_says_nothing_was_moved() {
    let dir = fixtures("mo-space-after-stopped");
    let (c, d) = (dir.join("C"), dir.join("D"));
    let steam = c.join("STEAM");
    write(&steam, "Pad.db", "pad");
    fs::create_dir_all(&d).unwrap();
    let moved = move_it(&steam, &d, &[]);
    let all = vec![moved.clone()];
    unlink(&steam);
    write(&steam, "Pad.db", "pad 2");
    write(&steam, "Update.db", "new");
    // Stop is pressed as the first file is copied: none has gone into the copy yet.
    let stop = AtomicBool::new(false);
    let pressed = |_: u64| stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let result = link_again(&moved, &all, 5, false, &OtherDrive, &pressed, &stop);
    assert_eq!(result.as_ref().err(), Some(&CantMove::Stopped), "{result:?}");
    let copy = d.join("STEAM");
    assert_eq!(read(&copy.join("Pad.db")), "pad");
    assert!(!copy.join("Update.db").exists());
    assert!(!PathBuf::from(format!("{}{KEPT_BEFORE_SUFFIX}", moved.to)).exists());
    assert!(!is_link(&steam));
    // Tried again, it goes through.
    stop.store(false, std::sync::atomic::Ordering::Relaxed);
    let linked = link_again(&moved, &all, 5, false, &OtherDrive, &nothing, &stop).unwrap();
    assert_eq!(linked.replaced, ["Pad.db"]);
    assert!(is_link(&steam));
    assert_eq!(read(&steam.join("Update.db")), "new");
}

#[test]
fn linking_again_finds_a_link_the_copys_drive_cant_hold_before_the_copy_changes() {
    let dir = fixtures("mo-space-after-no-links");
    let (c, d) = (dir.join("C"), dir.join("D"));
    let steam = c.join("STEAM");
    write(&steam, "Pad.db", "pad");
    fs::create_dir_all(&d).unwrap();
    let moved = move_it(&steam, &d, &[]);
    let all = vec![moved.clone()];
    // The update makes the folder again, with a link to a folder inside it.
    unlink(&steam);
    write(&steam, "Pad.db", "pad 2");
    write(&steam, "Update.db", "new");
    write(&dir, "Shared/Sounds.db", "sounds");
    folder_link(&dir.join("Shared"), &steam.join("Shared"));
    // The copy's drive holds no links (exFAT): that's found while the room is counted, before anything changes.
    let stop = AtomicBool::new(false);
    let result = link_again(&moved, &all, 5, false, &NoLinks { exfat: &d }, &nothing, &stop);
    assert_eq!(result.as_ref().err(), Some(&CantMove::NoLinksThere(s(&steam.join("Shared")))), "{result:?}");
    let copy = d.join("STEAM");
    assert_eq!(read(&copy.join("Pad.db")), "pad");
    assert!(!copy.join("Update.db").exists() && !copy.join("Shared").exists());
    assert!(!PathBuf::from(format!("{}{KEPT_BEFORE_SUFFIX}", moved.to)).exists());
    assert!(!is_link(&steam));
    // Where the drive holds links, the link is made again in the copy.
    link_again(&moved, &all, 5, false, &OtherDrive, &nothing, &stop).unwrap();
    assert!(is_link(&steam) && is_link(&copy.join("Shared")));
    assert_eq!(read(&steam.join("Shared/Sounds.db")), "sounds");
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

/// Whichever clean-up sends it, something on a network drive never reaches the bin, even a folder left behind that
/// you're sure about (on Linux, `//` at the start of a path is the same as `/`, so a real folder can be written as if
/// on a server).
#[cfg(target_os = "linux")]
#[test]
fn a_folder_left_behind_on_a_network_drive_stays() {
    let dir = fixtures("mo-space-old-home-network");
    write(&dir, "Old Spectrasonics/Pad.db", "pad");
    let home = OldHome {
        path: format!("/{}", s(&dir.join("Old Spectrasonics"))),
        bytes: 3,
        note: "its old place".to_owned(),
        copy: String::new(),
        kind: 2,
    };
    let done = remove_old_home(&home, &[], true, &bin(&dir), &AtomicBool::new(false));
    assert!(done.removed.is_empty());
    assert_eq!(done.problems[0].why, "It's on a network drive, which has no Recycle Bin.");
    assert!(dir.join("Old Spectrasonics/Pad.db").exists());
}
