//! The reminder of old copies and folders moves left behind, on folders made in the test build's own fixtures folder
//! under fixed names. Other folders there stand in for the other drives. Links are junctions on Windows, as the app
//! makes.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use mo_space::after_move::{NOTHING_TO_CHECK, OldHome};
use mo_space::clean::{Bin, Problem, Removed};
use mo_space::folder_move::{Drives, KEPT_SUFFIX, MovedFolder, move_folder, remove_old_copy};
use mo_space::key;
use mo_space::old_copies::{self, HOME_GONE, Left, MOVE_GONE, MOVED_SINCE, not_kept_stays, remind};
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
    move_folder(&s(from), &s(to), 1, whose(), &OtherDrive, &[], &[], &nothing, &stop).unwrap()
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
    // A folder linking again set aside, kept in the records (its file joined the copy); and files a stopped try kept
    // aside, not kept yet.
    write(&c, "Spectrasonics/STEAM (after update)/u.db", "0123456789abcde");
    write(&d, "STEAM/u.db", "0123456789abcde");
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
    // Nothing is only in the old copies or the folder set aside; the files kept aside have nothing to check against.
    let only: Vec<Option<Vec<String>>> = reminder.leftovers.iter().map(|l| l.only.clone()).collect();
    assert_eq!(only, [Some(Vec::new()), Some(Vec::new()), Some(Vec::new()), None]);
    assert!(reminder.leftovers.iter().all(|l| l.can_go()));

    // Something saved into an old copy after the move is named, and that one can't go until you've looked.
    write(Path::new(&presets.old_copy), "late.fxp", "late");
    let reminder = remind(&moved, std::slice::from_ref(&home), &[], &stop).unwrap();
    let late = reminder.leftovers.iter().find(|l| l.path == presets.old_copy).unwrap();
    assert_eq!((late.only.clone(), late.can_go()), (Some(vec!["late.fxp".to_owned()]), false));
    assert_eq!(reminder.bytes(), 38);
    assert_eq!(reminder.title(), "3 old copies left after moving folders (38 bytes), and 1 that can't go yet");
    fs::remove_file(Path::new(&presets.old_copy).join("late.fxp")).unwrap();

    // The copy on the other drive went missing (unplugged, say): that old copy is still listed, but it stays, and the
    // headline counts only what can go.
    fs::rename(&steam.to, d.join("STEAM renamed")).unwrap();
    let reminder = remind(&moved, &[], &[], &stop).unwrap();
    let first = &reminder.leftovers[0];
    assert_eq!(first.path, steam.old_copy);
    assert!(first.stays.as_deref().is_some_and(|w| w.contains("only copy the plugin could use")), "{first:?}");
    assert_eq!(reminder.leftovers.len(), 3);
    assert_eq!(reminder.bytes(), 8);
    assert_eq!(reminder.title(), "2 old copies left after moving folders (8 bytes), and 1 that can't go yet");
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
fn the_ones_you_pick_go_together_each_checked_again_against_the_records_now() {
    let dir = fixtures("mo-space-remind-remove");
    let (c, d) = (dir.join("C"), dir.join("D"));
    for name in ["STEAM", "Presets", "Kontakt", "Drums"] {
        write(&c, &format!("{name}/a.nki"), "good");
    }
    fs::create_dir_all(&d).unwrap();
    let moves: Vec<MovedFolder> =
        ["STEAM", "Presets", "Kontakt", "Drums"].iter().map(|name| moved_folder(&c.join(name), &d)).collect();
    let [steam, presets, kontakt, _drums] = &moves[..] else { unreachable!() };
    // A folder linking again set aside (its file joined the copy), and one the records will have lost by Remove.
    write(&c, "STEAM (after update)/u.db", "update");
    write(&d, "STEAM/u.db", "update");
    write(&c, "Old Drums/x.wav", "x");
    let home = |path: &Path, copy: &str| OldHome {
        path: s(path),
        bytes: 1,
        note: "left by an update".to_owned(),
        copy: copy.to_owned(),
        kind: 2,
    };
    let homes = [home(&c.join("STEAM (after update)"), &steam.to), home(&c.join("Old Drums"), "")];
    // Files a stopped try kept aside: nothing to check them against, so the page ticks them only when you do.
    let kept_in = format!("{}{KEPT_SUFFIX}", presets.from);
    write(Path::new(&kept_in), "p.fxp", "kept");
    let stop = AtomicBool::new(false);
    let look = remind(&moves, &homes, &[], &stop).unwrap();
    assert!(look.leftovers.iter().all(|l| l.stays.is_none()), "{:?}", look.leftovers);
    // Since the look: something is saved into one old copy, and the copy of another goes bad (the same size and time,
    // other bytes), which only comparing what the files hold shows.
    write(Path::new(&presets.old_copy), "late.fxp", "late");
    let bad = Path::new(&kontakt.to).join("a.nki");
    let when = fs::metadata(&bad).unwrap().modified().unwrap();
    fs::write(&bad, "bad!").unwrap();
    fs::File::options().write(true).open(&bad).unwrap().set_modified(when).unwrap();
    // And the records change: the Drums move and the second folder left behind aren't in them now.
    let (moves_now, homes_now) = (&moves[..3], &homes[..1]);
    // One picked as it was shown, though a folder around it moved since.
    let at = |path: &str| look.leftovers.iter().find(|l| key(&l.path) == key(path)).unwrap().clone();
    let shown_elsewhere = old_copies::Leftover { path: s(&dir.join("elsewhere")), ..at(&steam.old_copy) };
    let mut picked = look.leftovers.clone();
    picked.push(shown_elsewhere.clone());
    let bin = FolderBin { dir: dir.join("bin"), held: RefCell::new(Vec::new()) };

    let done = old_copies::remove(&picked, (&moves, &homes), (moves_now, homes_now), &bin, &stop);
    let went: Vec<String> = done.removed.iter().map(|r| key(&r.path)).collect();
    let mut expected = vec![key(&steam.old_copy), key(&homes[0].path), key(&kept_in)];
    let mut sorted = went.clone();
    sorted.sort();
    expected.sort();
    assert_eq!(sorted, expected, "{:?}", done.problems);
    // Each as the look measured it, and in the bin.
    assert_eq!(done.removed.iter().find(|r| key(&r.path) == key(&steam.old_copy)).unwrap().bytes, 4);
    assert!(done.removed.iter().all(|r| r.bin_id.is_some()));
    let why = |path: &str| done.problems.iter().find(|p| key(&p.path) == key(path)).map(|p| p.why.clone());
    assert!(why(&presets.old_copy).is_some_and(|w| w.contains("late.fxp")), "{:?}", done.problems);
    assert!(why(&kontakt.old_copy).is_some_and(|w| w.contains("a.nki")), "{:?}", done.problems);
    assert_eq!(why(&moves[3].old_copy).as_deref(), Some(MOVE_GONE));
    assert_eq!(why(&homes[1].path).as_deref(), Some(HOME_GONE));
    assert_eq!(why(&shown_elsewhere.path).as_deref(), Some(MOVED_SINCE));
    assert_eq!(done.problems.len(), 5);
    // What stayed is where it was, and the plugin still reaches its files through the link.
    assert_eq!(fs::read_to_string(Path::new(&kontakt.old_copy).join("a.nki")).unwrap(), "good");
    assert!(Path::new(&presets.old_copy).join("late.fxp").exists() && Path::new(&homes[1].path).exists());
    assert!(!Path::new(&steam.old_copy).exists());
    assert_eq!(fs::read_to_string(Path::new(&steam.from).join("a.nki")).unwrap(), "good");
}

#[test]
fn a_folder_left_behind_goes_only_as_far_as_the_look_could_check_it() {
    let dir = fixtures("mo-space-remind-home-copy");
    let (c, d) = (dir.join("C"), dir.join("D"));
    write(&c, "Home A/a.db", "same");
    write(&d, "A/a.db", "same");
    write(&c, "Home B/b.db", "mine");
    fs::create_dir_all(d.join("B")).unwrap();
    write(&c, "Home C/c.db", "only");
    let home = |name: &str, copy: Option<&str>| OldHome {
        path: s(&c.join(format!("Home {name}"))),
        bytes: 4,
        note: "left by an update".to_owned(),
        copy: copy.map(|copy| s(&d.join(copy))).unwrap_or_default(),
        kind: 2,
    };
    // A says where its files went, and they're all there; B doesn't say; C's copy isn't there (its drive is out).
    let homes = [home("A", Some("A")), home("B", None), home("C", Some("C"))];
    let stop = AtomicBool::new(false);
    let look = remind(&[], &homes, &[], &stop).unwrap();
    let at = |name: &str| look.leftovers.iter().find(|l| l.path.ends_with(&format!("Home {name}"))).unwrap();
    assert_eq!((at("A").only.clone(), at("A").can_go()), (Some(Vec::new()), true));
    assert_eq!((at("B").only.clone(), at("B").can_go()), (None, true));
    // Not "C:\…\c.db is only here": where its files went isn't there to look in, and this may be all there is of them.
    let gone = at("C");
    assert!(gone.stays.as_deref().is_some_and(|w| w.contains("isn't there now")), "{gone:?}");
    assert_eq!((gone.only.clone(), gone.can_go()), (None, false));
    // By Remove, the records say A's files went nowhere, and B's went to a copy that doesn't have them.
    let homes_now = [home("A", None), home("B", Some("B")), home("C", Some("C"))];
    let bin = FolderBin { dir: dir.join("bin"), held: RefCell::new(Vec::new()) };

    let done = old_copies::remove(&look.leftovers, (&[], &homes), (&[], &homes_now), &bin, &stop);
    assert!(done.removed.is_empty(), "{:?}", done.removed);
    let why =
        |name: &str| done.problems.iter().find(|p| p.path.ends_with(&format!("Home {name}"))).unwrap().why.clone();
    // A was ticked for you because the look could check it: with nothing to check it against now, it stays.
    assert_eq!(why("A"), NOTHING_TO_CHECK);
    // B was ticked by you with nothing to check it against, but there's a copy to check it against now: it's checked.
    assert!(why("B").contains("b.db"), "{}", why("B"));
    assert!(why("C").contains("isn't there now"), "{}", why("C"));
    for name in ["A", "B", "C"] {
        assert!(c.join(format!("Home {name}")).exists());
    }
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
    let Left::NotKept(taken) = &found.what else { panic!("{found:?}") };
    // It stays until the app keeps its move: once it's gone, nothing would show the link beside it was a move's.
    assert_eq!(found.stays, Some(not_kept_stays(&taken.from)));
    assert_eq!((reminder.bytes(), found.can_go()), (0, false));
    let picked = std::slice::from_ref(found);
    let bin = FolderBin { dir: dir.join("bin"), held: RefCell::new(Vec::new()) };
    let done = old_copies::remove(picked, (&[], &[]), (&[], &[]), &bin, &stop);
    assert_eq!(done.problems, [Problem { path: found.path.clone(), why: not_kept_stays(&taken.from) }]);
    assert!(done.removed.is_empty() && Path::new(&lost.old_copy).exists());
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
    // Nothing is offered to free: it's what the plugin uses.
    assert_eq!(reminder.bytes(), 0);
    assert_eq!(reminder.title(), "1 old copy left after a move can't go yet");
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
    assert_eq!(reminder.title(), "2 old copies left after moving folders can't go yet");
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
