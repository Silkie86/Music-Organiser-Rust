//! The reminder of old copies and folders moves left behind, on folders made in the test build's own fixtures folder
//! under fixed names. Other folders there stand in for the other drives. Links are junctions on Windows, as the app
//! makes.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use mo_space::after_move::OldHome;
use mo_space::clean::{Bin, Problem, Removed};
use mo_space::folder_move::{Drives, KEPT_SUFFIX, MovedFolder, move_folder, remove_old_copy};
use mo_space::key;
use mo_space::old_copies::{Left, remind};
use mo_space::plugin_space::SpaceKind;

mod common;

use common::{folder_link, is_link};

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

fn real(p: &Path) -> String {
    s(&fs::canonicalize(p).unwrap())
}

/// Every folder is on its own drive, with room to spare.
struct OtherDrive;

impl Drives for OtherDrive {
    fn same(&self, _a: &Path, _b: &Path) -> bool {
        false
    }
    fn free(&self, _path: &Path) -> Option<u64> {
        None
    }
}

fn whose() -> (&'static str, SpaceKind, &'static [String]) {
    ("omnisphere", SpaceKind::Sounds, &[])
}

fn nothing(_: u64) {}

fn moved_folder(from: &Path, to: &Path) -> MovedFolder {
    let stop = AtomicBool::new(false);
    move_folder(&s(from), &s(to), 1, whose(), &OtherDrive, &[], &nothing, &stop).unwrap()
}

#[test]
fn every_old_copy_and_folder_left_behind_is_listed_with_its_size_biggest_first() {
    let dir = fixtures("mo-space-remind");
    let (c, d) = (dir.join("C"), dir.join("D"));
    write(&c, "Spectrasonics/STEAM/a.db", "0123456789");
    write(&c, "Spectrasonics/STEAM/b.db", "0123456789");
    write(&c, "Presets/p.fxp", "12345");
    fs::create_dir_all(&d).unwrap();
    let steam = moved_folder(&c.join("Spectrasonics/STEAM"), &d);
    let presets = moved_folder(&c.join("Presets"), &d);
    // A folder linking again set aside, kept in the records; and files a stopped try kept aside, not kept yet.
    write(&c, "Spectrasonics/STEAM (after update)/u.db", "0123456789abcde");
    let home = OldHome {
        path: s(&c.join("Spectrasonics/STEAM (after update)")),
        bytes: 1,
        note: "left by an update".to_owned(),
        copy: steam.to.clone(),
        kind: 2,
    };
    let kept_in = format!("{}{KEPT_SUFFIX}", presets.from);
    write(Path::new(&kept_in), "p.fxp", "123");
    let moved = [steam.clone(), presets.clone()];
    let stop = AtomicBool::new(false);

    let reminder = remind(&moved, std::slice::from_ref(&home), &[], &stop).unwrap();
    let listed: Vec<(&str, u64)> = reminder.leftovers.iter().map(|l| (l.path.as_str(), l.bytes)).collect();
    assert_eq!(
        listed,
        [
            (steam.old_copy.as_str(), 20),
            (home.path.as_str(), 15),
            (presets.old_copy.as_str(), 5),
            (kept_in.as_str(), 3)
        ]
    );
    assert_eq!(reminder.bytes(), 43);
    assert_eq!(reminder.title(), "4 old copies left after moving folders (43 bytes)");
    assert_eq!(reminder.leftovers[0].what, Left::OldCopy(0));
    assert_eq!(reminder.leftovers[0].now_in, steam.to);
    assert!(reminder.leftovers[0].since.is_some());
    assert_eq!(reminder.leftovers[1].what, Left::Behind(0));
    assert!(matches!(&reminder.leftovers[3].what, Left::Kept(h) if key(&h.path) == key(&kept_in)));
    assert!(reminder.leftovers.iter().all(|l| l.stays.is_none()), "{:?}", reminder.leftovers);

    // The copy on the other drive went missing (unplugged, say): that old copy is still listed, but it stays.
    fs::rename(&steam.to, d.join("STEAM renamed")).unwrap();
    let reminder = remind(&moved, &[], &[], &stop).unwrap();
    let first = &reminder.leftovers[0];
    assert_eq!(first.path, steam.old_copy);
    assert!(first.stays.as_deref().is_some_and(|w| w.contains("only copy the plugin could use")), "{first:?}");
    fs::rename(d.join("STEAM renamed"), &steam.to).unwrap();

    // Once they're gone there's nothing to remind you of.
    for path in [&steam.old_copy, &presets.old_copy, &home.path, &kept_in] {
        fs::remove_dir_all(path).unwrap();
    }
    let reminder = remind(&moved, std::slice::from_ref(&home), &[], &stop).unwrap();
    assert!(reminder.is_empty());
    assert_eq!(reminder.title(), "");
}

#[test]
fn one_old_copy_is_one() {
    let dir = fixtures("mo-space-remind-one");
    let (c, d) = (dir.join("C"), dir.join("D"));
    write(&c, "Presets/p.fxp", "12345");
    fs::create_dir_all(&d).unwrap();
    let presets = moved_folder(&c.join("Presets"), &d);
    let stop = AtomicBool::new(false);
    let reminder = remind(std::slice::from_ref(&presets), &[], &[], &stop).unwrap();
    assert_eq!(reminder.title(), "1 old copy left after a move (5 bytes)");
}

#[test]
fn an_old_copy_no_move_was_kept_for_is_found_beside_its_link_and_goes_once_kept() {
    let dir = fixtures("mo-space-remind-not-kept");
    let (c, d) = (dir.join("C"), dir.join("D"));
    let spectrasonics = c.join("Spectrasonics");
    write(&spectrasonics, "STEAM/a.db", "0123456789");
    fs::create_dir_all(&d).unwrap();
    // The app stopped after the swap, before it kept the move: only the folders say there was one.
    let lost = moved_folder(&spectrasonics.join("STEAM"), &d);
    let stop = AtomicBool::new(false);
    let look_in = [s(&spectrasonics)];

    let reminder = remind(&[], &[], &look_in, &stop).unwrap();
    assert_eq!(reminder.leftovers.len(), 1);
    let found = &reminder.leftovers[0];
    assert_eq!((key(&found.path), found.bytes), (key(&lost.old_copy), 10));
    assert_eq!(found.stays, None);
    let Left::NotKept(taken) = &found.what else { panic!("{found:?}") };
    assert_eq!((key(&taken.from), key(&taken.old_copy)), (key(&lost.from), key(&lost.old_copy)));
    assert_eq!(key(&taken.to), key(&real(Path::new(&lost.to))));
    assert_eq!(found.now_in, taken.to);
    // Listed once, looked for twice.
    let twice = [look_in[0].clone(), look_in[0].clone()];
    assert_eq!(remind(&[], &[], &twice, &stop).unwrap().leftovers.len(), 1);
    // Once kept, it's a move like any other: it's listed as that move's, and it goes the same guarded way.
    let all = std::slice::from_ref(taken);
    let again = remind(all, &[], &look_in, &stop).unwrap();
    assert_eq!(again.leftovers.len(), 1);
    assert_eq!(again.leftovers[0].what, Left::OldCopy(0));
    let bin = FolderBin { dir: dir.join("bin"), held: RefCell::new(Vec::new()) };
    let done = remove_old_copy(taken, all, false, &bin, &stop);
    assert!(done.problems.is_empty(), "{:?}", done.problems);
    assert!(!Path::new(&lost.old_copy).exists());
    assert_eq!(fs::read_to_string(Path::new(&lost.from).join("a.db")).unwrap(), "0123456789");
}

#[test]
fn an_old_copy_its_link_leads_back_into_is_what_the_plugin_uses_and_stays() {
    let dir = fixtures("mo-space-remind-pointed-back");
    let plugins = dir.join("Plugins");
    // The other drive went, so the link was pointed back at the old copy, and no move was kept.
    let old_copy = plugins.join(format!("STEAM{}", mo_space::folder_move::OLD_SUFFIX));
    write(&old_copy, "a.db", "0123456789");
    let steam = plugins.join("STEAM");
    folder_link(&old_copy, &steam);
    let stop = AtomicBool::new(false);
    let reminder = remind(&[], &[], &[s(&plugins)], &stop).unwrap();
    assert_eq!(reminder.leftovers.len(), 1);
    let found = &reminder.leftovers[0];
    assert_eq!(found.what, Left::Unknown);
    assert!(found.stays.as_deref().is_some_and(|w| w.contains("leads into it")), "{found:?}");
    // A move kept that way goes no further: its old copy is the folder its link uses.
    let kept = MovedFolder {
        from: s(&steam),
        to: s(&old_copy),
        old_copy: s(&old_copy),
        bytes: 10,
        moved: String::new(),
        key: String::new(),
        kind: 2,
        shared_with: Vec::new(),
    };
    let all = std::slice::from_ref(&kept);
    let bin = FolderBin { dir: dir.join("bin"), held: RefCell::new(Vec::new()) };
    let done = remove_old_copy(&kept, all, true, &bin, &stop);
    assert!(done.removed.is_empty() && !done.problems.is_empty(), "{done:?}");
    assert_eq!(fs::read_to_string(steam.join("a.db")).unwrap(), "0123456789");
}

#[test]
fn an_old_copy_with_no_link_beside_it_stays_for_you_to_look_at() {
    let dir = fixtures("mo-space-remind-unknown");
    let plugins = dir.join("Plugins");
    // Beside a real folder: nothing to check it against.
    write(&plugins, "Serum Presets/a.fxp", "a");
    write(&plugins, "Serum Presets (old copy)/a.fxp", "a");
    // With nothing beside it: maybe the folder itself, renamed by a swap that was cut off.
    write(&plugins, "Kontakt Library (old copy)/k.nki", "kk");
    // Not old copies: a link, and names that only look like one.
    write(&dir, "Elsewhere/x.wav", "x");
    folder_link(&dir.join("Elsewhere"), &plugins.join("Linked (old copy)"));
    write(&plugins, " (old copy)/x", "x");
    write(&plugins, "Drums (old copy) 2/x", "x");
    write(&plugins, "Notes (old copy)", "a file");
    assert!(is_link(&plugins.join("Linked (old copy)")));
    let stop = AtomicBool::new(false);

    let reminder = remind(&[], &[], &[s(&plugins)], &stop).unwrap();
    let listed: Vec<&str> = reminder.leftovers.iter().map(|l| l.path.as_str()).collect();
    let kontakt = s(&plugins.join("Kontakt Library (old copy)"));
    let serum = s(&plugins.join("Serum Presets (old copy)"));
    assert_eq!(listed, [kontakt.as_str(), serum.as_str()]);
    assert!(reminder.leftovers.iter().all(|l| l.what == Left::Unknown));
    let why = |n: usize| reminder.leftovers[n].stays.clone().unwrap_or_default();
    assert!(why(0).contains("rename it back to \"Kontakt Library\""), "{}", why(0));
    assert!(why(1).contains("isn't a link to a copy"), "{}", why(1));
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
