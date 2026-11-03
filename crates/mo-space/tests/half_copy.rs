//! Half copies: what a move leaves when the app or the computer stops part way, on folders made in the test build's own
//! fixtures folder under fixed names. Other folders there stand in for the other drives. A stop is stood in for by a
//! panic from the progress callback, which leaves everything as a crash would: nothing after it runs. Links are
//! junctions on Windows, as the app makes.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use mo_space::after_move::move_home;
use mo_space::folder_move::{
    CantMove, Drives, HEADROOM, MovedFolder, move_back, move_folder, only_in_old_copy, problem,
};
use mo_space::half_copy::{Cleared, MARKER, State, clear, find, half_copies};
use mo_space::plugin_space::SpaceKind;

mod common;

use common::{folder_link, is_link, unlink};

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

fn whose() -> (&'static str, SpaceKind, &'static [String]) {
    ("omnisphere", SpaceKind::Sounds, &[])
}

fn nothing(_: u64) {}

/// Runs a move that the computer stops part way: here a panic from its progress, once `after` bytes have copied.
fn stopped_part_way<T>(after: u64, job: impl FnOnce(&(dyn Fn(u64) + Sync)) -> T) {
    let progress = move |n: u64| {
        if n >= after {
            panic!("the computer stopped");
        }
    };
    let ran = catch_unwind(AssertUnwindSafe(|| job(&progress)));
    assert!(ran.is_err(), "the copy should have been stopped part way");
}

/// A library of six sounds and a patch: 35 bytes.
fn library(at: &Path) {
    for n in 0..6 {
        write(at, &format!("Sounds/{n}.db"), "sound");
    }
    write(at, "Patch.prt", "patch");
}

/// A marker as the app writes one, for a copy of `from` made in `to` that ends up as `settled`.
fn plant_marker(folder: &Path, from: &Path, to: &Path, settled: &Path) {
    let json = serde_json::json!({
        "From": s(from),
        "To": s(to),
        "Settled": s(settled),
        "Started": "2026-11-03T14:03:12.1234567+00:00",
    });
    fs::write(folder.join(MARKER), format!("Music Organiser is copying a folder into this one.\n\n{json}\n")).unwrap();
}

#[test]
fn a_move_the_computer_stopped_leaves_a_marked_half_copy_that_the_next_try_clears() {
    let dir = fixtures("mo-space-half-move");
    let (c, d) = (dir.join("C"), dir.join("D"));
    let steam = c.join("Spectrasonics").join("STEAM");
    library(&steam);
    fs::create_dir_all(&d).unwrap();
    let from = s(&steam);
    let stop = AtomicBool::new(false);
    stopped_part_way(10, |progress| move_folder(&from, &s(&d), 35, whose(), &ROOMY, &[], progress, &stop));

    // The folder is as it was; what was copied is a half copy, and its marker says so.
    let half = d.join("STEAM");
    assert!(!is_link(&steam) && !c.join("Spectrasonics/STEAM (old copy)").exists());
    let said = fs::read_to_string(half.join(MARKER)).unwrap();
    assert!(said.starts_with("Music Organiser is copying a folder into this one."), "{said}");
    let found = find(&half, &[], &stop).unwrap().unwrap();
    assert_eq!(found.state, State::CutOff);
    assert_eq!((found.path.as_str(), found.from.as_str()), (s(&half).as_str(), from.as_str()));
    assert!(found.bytes >= 10, "{}", found.bytes);
    assert_eq!(half_copies(&[], &[s(&d)], &stop).unwrap(), std::slice::from_ref(&found));

    // It isn't in the way: the move clears it first, so the room it takes counts as free.
    assert_eq!(problem(&from, &s(&d), 35, &ROOMY, &[], &stop), None);
    let room = |free: u64| OtherDrive { free: Some(free) };
    assert_eq!(problem(&from, &s(&d), 35, &room(HEADROOM + 35 - found.bytes), &[], &stop), None);
    let short = problem(&from, &s(&d), 35, &room(HEADROOM + 34 - found.bytes), &[], &stop);
    assert!(matches!(short, Some(CantMove::NoRoom { needs: 35, .. })), "{short:?}");

    // Tried again, the move clears it and goes through; in use, the copy has no marker.
    let moved = move_folder(&from, &s(&d), 35, whose(), &ROOMY, &[], &nothing, &stop).unwrap();
    assert!(is_link(&steam));
    assert!(!half.join(MARKER).exists());
    assert_eq!(fs::read_to_string(half.join("Patch.prt")).unwrap(), "patch");
    assert_eq!(fs::read_dir(half.join("Sounds")).unwrap().count(), 6);
    assert!(moved.has_old_copy());
    assert!(only_in_old_copy(&moved, std::slice::from_ref(&moved), &stop).unwrap().is_empty());
    assert!(half_copies(std::slice::from_ref(&moved), &[s(&d)], &stop).unwrap().is_empty());
}

#[test]
fn a_half_copy_holding_what_isnt_in_the_folder_stays_and_says_what() {
    let dir = fixtures("mo-space-half-stays");
    let (c, d) = (dir.join("C"), dir.join("D"));
    let presets = c.join("Presets");
    library(&presets);
    fs::create_dir_all(&d).unwrap();
    let from = s(&presets);
    let stop = AtomicBool::new(false);
    stopped_part_way(10, |progress| move_folder(&from, &s(&d), 35, whose(), &ROOMY, &[], progress, &stop));
    let half = d.join("Presets");

    // Something that isn't in the folder being moved (yours, or deleted from it since) keeps the half copy.
    write(&half, "Mine.fxp", "mine");
    let found = find(&half, &[], &stop).unwrap().unwrap();
    let State::Stays(why) = &found.state else { panic!("{found:?}") };
    assert!(why.contains("Mine.fxp"), "{why}");
    let said = problem(&from, &s(&d), 35, &ROOMY, &[], &stop);
    assert!(matches!(&said, Some(CantMove::HalfCopy { path, .. }) if *path == s(&half)), "{said:?}");
    assert!(said.unwrap().text().contains("Mine.fxp"));
    let tried = move_folder(&from, &s(&d), 35, whose(), &ROOMY, &[], &nothing, &stop);
    assert!(matches!(tried, Err(CantMove::HalfCopy { .. })), "{tried:?}");
    assert!(clear(&half, &[], &stop).is_err());
    assert!(half.join("Mine.fxp").exists() && half.join(MARKER).exists() && !is_link(&presets));

    // So does a folder where the folder being moved has a file, and a file where it has a folder.
    fs::remove_file(half.join("Mine.fxp")).unwrap();
    let _ = fs::remove_file(half.join("Patch.prt"));
    fs::create_dir_all(half.join("Patch.prt")).unwrap();
    let State::Stays(why) = find(&half, &[], &stop).unwrap().unwrap().state else { panic!() };
    assert!(why.contains("Patch.prt"), "{why}");
    fs::remove_dir(half.join("Patch.prt")).unwrap();
    let sounds = half.join("Sounds");
    if sounds.exists() {
        fs::remove_dir_all(&sounds).unwrap();
    }
    fs::write(&sounds, "a file").unwrap();
    let State::Stays(why) = find(&half, &[], &stop).unwrap().unwrap().state else { panic!() };
    assert!(why.contains("Sounds"), "{why}");
    fs::remove_file(&sounds).unwrap();

    // Once it holds only copies again, it's cleared, for good, with all the room it took.
    let bytes = find(&half, &[], &stop).unwrap().unwrap().bytes;
    assert_eq!(clear(&half, &[], &stop), Ok(Cleared::Copy(bytes)));
    assert!(!half.exists());
    assert_eq!(fs::read_dir(presets.join("Sounds")).unwrap().count(), 6);
}

#[test]
fn a_half_copy_goes_with_its_read_only_files_and_its_links_but_never_what_they_lead_to() {
    let dir = fixtures("mo-space-half-links");
    let (c, d) = (dir.join("C"), dir.join("D"));
    let library = c.join("Library");
    write(&dir, "Elsewhere/keep.wav", "keep");
    write(&library, "Samples/a.wav", "aaaa");
    write(&library, "Samples/b.wav", "bbbb");
    folder_link(&dir.join("Elsewhere"), &library.join("Linked"));
    let locked = library.join("Samples").join("b.wav");
    let mut read_only = fs::metadata(&locked).unwrap().permissions();
    read_only.set_readonly(true);
    fs::set_permissions(&locked, read_only).unwrap();
    fs::create_dir_all(&d).unwrap();
    let from = s(&library);
    let stop = AtomicBool::new(false);
    // Stopped as the last file goes in: the link and both samples are in the half copy, one of them read-only.
    stopped_part_way(8, |progress| move_folder(&from, &s(&d), 8, whose(), &ROOMY, &[], progress, &stop));
    let half = d.join("Library");
    assert!(is_link(&half.join("Linked")));
    assert!(fs::metadata(half.join("Samples/b.wav")).unwrap().permissions().readonly());

    assert!(matches!(clear(&half, &[], &stop), Ok(Cleared::Copy(_))));
    assert!(!half.exists());
    assert_eq!(fs::read_to_string(dir.join("Elsewhere/keep.wav")).unwrap(), "keep");
    assert_eq!(fs::read_to_string(library.join("Samples/b.wav")).unwrap(), "bbbb");
}

#[test]
fn a_marker_left_in_a_copy_in_use_is_all_that_goes() {
    let dir = fixtures("mo-space-half-in-use");
    let (c, d) = (dir.join("C"), dir.join("D"));
    let presets = c.join("Presets");
    library(&presets);
    fs::create_dir_all(&d).unwrap();
    let stop = AtomicBool::new(false);
    let moved = move_folder(&s(&presets), &s(&d), 35, whose(), &ROOMY, &[], &nothing, &stop).unwrap();
    let copy = d.join("Presets");
    // The app stopped between the swap and taking the marker out.
    plant_marker(&copy, &presets, &copy, &copy);
    let all = std::slice::from_ref(&moved);

    let found = half_copies(all, &[], &stop).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!((found[0].state.clone(), found[0].bytes), (State::MarkerOnly, 0));
    // Without the move's record too: the folder it copies leads here.
    assert_eq!(find(&copy, &[], &stop).unwrap().unwrap().state, State::MarkerOnly);
    assert_eq!(clear(&copy, all, &stop), Ok(Cleared::Marker));
    assert!(!copy.join(MARKER).exists());
    assert_eq!(fs::read_to_string(presets.join("Patch.prt")).unwrap(), "patch");
    assert_eq!(fs::read_dir(copy.join("Sounds")).unwrap().count(), 6);

    // The folder it copies is a link leading elsewhere now (moved on again, say). It was a real folder while it was
    // copied, so the swap went through: the copy was in use, and only its marker goes, even with no record of it.
    let elsewhere = dir.join("E").join("Presets");
    library(&elsewhere);
    unlink(&presets);
    folder_link(&elsewhere, &presets);
    plant_marker(&copy, &presets, &copy, &copy);
    assert_eq!(find(&copy, &[], &stop).unwrap().unwrap().state, State::MarkerOnly);
    assert_eq!(clear(&copy, &[], &stop), Ok(Cleared::Marker));
    assert_eq!(fs::read_dir(copy.join("Sounds")).unwrap().count(), 6);
    unlink(&presets);
    folder_link(&copy, &presets);

    // An update put a real folder back where the link was: the copy is still the move's, so it isn't touched.
    unlink(&presets);
    library(&presets);
    plant_marker(&copy, &presets, &copy, &copy);
    assert_eq!(find(&copy, all, &stop).unwrap().unwrap().state, State::MarkerOnly);
    // And moving back takes such a marker out first, so the copy left for you never looks like a half copy.
    unlink_or_skip(&presets);
    folder_link(&copy, &presets);
    move_back(&moved, &ROOMY, &nothing, &stop).unwrap();
    assert!(!copy.join(MARKER).exists() && !presets.join(MARKER).exists());
    assert!(find(&copy, &[], &stop).unwrap().is_none());
}

/// Removes the real folder an update put where the link was, to put the link back.
fn unlink_or_skip(at: &Path) {
    if is_link(at) {
        unlink(at);
    } else {
        fs::remove_dir_all(at).unwrap();
    }
}

#[test]
fn a_marker_in_a_folder_renamed_since_is_all_that_goes() {
    let dir = fixtures("mo-space-half-renamed");
    let (c, d) = (dir.join("C"), dir.join("D"));
    let presets = c.join("Presets");
    library(&presets);
    fs::create_dir_all(&d).unwrap();
    let stop = AtomicBool::new(false);
    stopped_part_way(10, |progress| move_folder(&s(&presets), &s(&d), 35, whose(), &ROOMY, &[], progress, &stop));
    // You renamed the half copy: it isn't where the copy was being made, so it's yours now.
    let renamed = d.join("Presets kept");
    fs::rename(d.join("Presets"), &renamed).unwrap();
    assert_eq!(find(&renamed, &[], &stop).unwrap().unwrap().state, State::MarkerOnly);
    assert_eq!(clear(&renamed, &[], &stop), Ok(Cleared::Marker));
    assert!(renamed.join("Patch.prt").exists() || renamed.join("Sounds").exists());
}

#[test]
fn a_half_copy_of_another_folder_or_with_a_broken_marker_is_left_alone() {
    let dir = fixtures("mo-space-half-other");
    let (c, d) = (dir.join("C"), dir.join("D"));
    let (a, b) = (c.join("A").join("Presets"), c.join("B").join("Presets"));
    library(&a);
    library(&b);
    fs::create_dir_all(&d).unwrap();
    let stop = AtomicBool::new(false);
    stopped_part_way(10, |progress| move_folder(&s(&a), &s(&d), 35, whose(), &ROOMY, &[], progress, &stop));
    let half = d.join("Presets");

    // B's Presets would go to the same place, but the half copy there is A's: it's in the way, as anything is.
    let there = CantMove::TargetThere(s(&half));
    assert_eq!(problem(&s(&b), &s(&d), 35, &ROOMY, &[], &stop), Some(there.clone()));
    assert_eq!(move_folder(&s(&b), &s(&d), 35, whose(), &ROOMY, &[], &nothing, &stop), Err(there));
    assert!(half.join(MARKER).exists());

    // A marker that isn't as the app writes it isn't one: the folder isn't known for a half copy.
    fs::write(half.join(MARKER), "nonsense").unwrap();
    assert_eq!(find(&half, &[], &stop).unwrap(), None);
    assert_eq!(problem(&s(&a), &s(&d), 35, &ROOMY, &[], &stop), Some(CantMove::TargetThere(s(&half))));
    assert!(clear(&half, &[], &stop).is_err());
    assert!(half.exists());
    // Nor is a marker that's a folder, or a folder that's a link to a half copy.
    fs::remove_file(half.join(MARKER)).unwrap();
    fs::create_dir(half.join(MARKER)).unwrap();
    assert_eq!(find(&half, &[], &stop).unwrap(), None);
    fs::remove_dir(half.join(MARKER)).unwrap();
    plant_marker(&half, &a, &half, &half);
    let link = d.join("Presets link");
    folder_link(&half, &link);
    assert_eq!(find(&link, &[], &stop).unwrap(), None);
    assert!(clear(&link, &[], &stop).is_err());
    assert!(half.join(MARKER).exists());
}

#[test]
fn a_swap_the_computer_stopped_says_what_to_rename() {
    let dir = fixtures("mo-space-half-swap");
    let (c, d) = (dir.join("C"), dir.join("D"));
    let presets = c.join("Presets");
    library(&presets);
    fs::create_dir_all(&d).unwrap();
    let stop = AtomicBool::new(false);
    stopped_part_way(35, |progress| move_folder(&s(&presets), &s(&d), 35, whose(), &ROOMY, &[], progress, &stop));
    let half = d.join("Presets");
    // Stopped just after the swap renamed the folder, before its link was made.
    let aside = c.join("Presets (old copy)");
    fs::rename(&presets, &aside).unwrap();
    let State::Stays(why) = find(&half, &[], &stop).unwrap().unwrap().state else { panic!() };
    assert!(why.contains("Rename it back to \"Presets\""), "{why}");
    assert!(clear(&half, &[], &stop).is_err());
    // Renamed back, it's a half copy again; with an "(old copy)" beside a folder, it stays.
    fs::rename(&aside, &presets).unwrap();
    assert_eq!(find(&half, &[], &stop).unwrap().unwrap().state, State::CutOff);
    library(&aside);
    let State::Stays(why) = find(&half, &[], &stop).unwrap().unwrap().state else { panic!() };
    assert!(why.contains("(old copy)"), "{why}");
}

#[test]
fn moving_back_the_computer_stopped_leaves_a_half_copy_the_next_try_clears() {
    let dir = fixtures("mo-space-half-back");
    let (c, d) = (dir.join("C"), dir.join("D"));
    let presets = c.join("Presets");
    library(&presets);
    fs::create_dir_all(&d).unwrap();
    let stop = AtomicBool::new(false);
    let moved = move_folder(&s(&presets), &s(&d), 35, whose(), &ROOMY, &[], &nothing, &stop).unwrap();
    // The old copy was deleted once the plugin opened fine, so moving back copies the folder back.
    fs::remove_dir_all(&moved.old_copy).unwrap();
    stopped_part_way(10, |progress| move_back(&moved, &ROOMY, progress, &stop));
    let back = c.join("Presets (moving back)");
    let all = std::slice::from_ref(&moved);
    assert!(back.join(MARKER).exists() && is_link(&presets));
    assert_eq!(find(&back, all, &stop).unwrap().unwrap().state, State::CutOff);
    // Found beside the moved folder too.
    assert_eq!(half_copies(all, &[], &stop).unwrap().len(), 1);

    // Tried again, moving back clears it first and goes through; in place, the folder has no marker.
    move_back(&moved, &ROOMY, &nothing, &stop).unwrap();
    assert!(!is_link(&presets) && !back.exists());
    assert!(!presets.join(MARKER).exists());
    assert_eq!(fs::read_dir(presets.join("Sounds")).unwrap().count(), 6);
}

#[test]
fn moving_back_cut_off_as_it_swapped_says_what_to_rename() {
    let dir = fixtures("mo-space-half-unswap");
    let (c, d) = (dir.join("C"), dir.join("D"));
    let presets = c.join("Presets");
    library(&presets);
    fs::create_dir_all(&d).unwrap();
    let stop = AtomicBool::new(false);
    let moved = move_folder(&s(&presets), &s(&d), 35, whose(), &ROOMY, &[], &nothing, &stop).unwrap();
    fs::remove_dir_all(&moved.old_copy).unwrap();
    stopped_part_way(35, |progress| move_back(&moved, &ROOMY, progress, &stop));
    let back = c.join("Presets (moving back)");
    let all = std::slice::from_ref(&moved);
    // Stopped after the link went, before the copy took its name.
    unlink(&presets);
    let State::Stays(why) = find(&back, all, &stop).unwrap().unwrap().state else { panic!() };
    assert!(why.contains("rename \"Presets (moving back)\" to \"Presets\""), "{why}");
    // A real folder in the link's place: the copy may be in use.
    library(&presets);
    let State::Stays(why) = find(&back, all, &stop).unwrap().unwrap().state else { panic!() };
    assert!(why.contains("isn't the link"), "{why}");
    let tried = move_back(&moved, &ROOMY, &nothing, &stop);
    assert!(tried.is_err() && back.join(MARKER).exists(), "{tried:?}");
}

#[test]
fn the_plugin_data_folder_moving_to_another_drive_stopped_part_way_is_cleared_by_the_next_try() {
    let dir = fixtures("mo-space-half-home");
    let (c, d, e) = (dir.join("C"), dir.join("D"), dir.join("E"));
    let presets = c.join("Presets");
    library(&presets);
    let home = d.join("Plugin Data");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&e).unwrap();
    let stop = AtomicBool::new(false);
    let moved = move_folder(&s(&presets), &s(&home), 35, whose(), &ROOMY, &[], &nothing, &stop).unwrap();
    let all = std::slice::from_ref(&moved);
    stopped_part_way(10, |progress| move_home(&s(&home), &s(&e), all, &[], 35, &ROOMY, progress, &stop));
    let half = e.join("Plugin Data");
    assert!(half.join(MARKER).exists());
    assert_eq!(find(&half, all, &stop).unwrap().unwrap().state, State::CutOff);

    let now = move_home(&s(&home), &s(&e), all, &[], 35, &ROOMY, &nothing, &stop).unwrap();
    assert_eq!(now.moved[0].to, s(&half.join("Presets")));
    assert!(!half.join(MARKER).exists());
    assert_eq!(fs::read_to_string(presets.join("Patch.prt")).unwrap(), "patch");
}

#[test]
fn a_marker_at_the_top_of_a_folder_being_moved_is_never_copied_on() {
    let dir = fixtures("mo-space-half-stale");
    let (c, d) = (dir.join("C"), dir.join("D"));
    let presets = c.join("Presets");
    library(&presets);
    // Left there when moving back couldn't take it out, naming the copy it came from.
    plant_marker(&presets, &d.join("Presets"), &c.join("Presets (moving back)"), &presets);
    fs::create_dir_all(&d).unwrap();
    let stop = AtomicBool::new(false);
    let moved: MovedFolder = move_folder(&s(&presets), &s(&d), 35, whose(), &ROOMY, &[], &nothing, &stop).unwrap();
    let copy = d.join("Presets");
    assert!(!copy.join(MARKER).exists());
    // It's the app's, not yours: it doesn't keep the old copy from going.
    assert!(only_in_old_copy(&moved, std::slice::from_ref(&moved), &stop).unwrap().is_empty());
}

#[test]
fn half_copies_are_looked_for_where_moves_go() {
    let dir = fixtures("mo-space-half-places");
    let (c, d, e, f) = (dir.join("C"), dir.join("D"), dir.join("E"), dir.join("F"));
    for n in ["One", "Two", "Three"] {
        library(&c.join(n));
    }
    for drive in [&d, &e, &f] {
        fs::create_dir_all(drive).unwrap();
    }
    let stop = AtomicBool::new(false);
    let one = move_folder(&s(&c.join("One")), &s(&d), 35, whose(), &ROOMY, &[], &nothing, &stop).unwrap();
    // Cut off: Two to where One went, Three to a drive the app remembers.
    stopped_part_way(10, |progress| move_folder(&s(&c.join("Two")), &s(&d), 35, whose(), &ROOMY, &[], progress, &stop));
    stopped_part_way(10, |progress| {
        move_folder(&s(&c.join("Three")), &s(&e), 35, whose(), &ROOMY, &[], progress, &stop)
    });
    // And one where nothing says moves go.
    stopped_part_way(10, |progress| {
        move_folder(&s(&c.join("Three")), &s(&f), 35, whose(), &ROOMY, &[], progress, &stop)
    });
    let all = std::slice::from_ref(&one);

    let mut found: Vec<String> = half_copies(all, &[s(&e)], &stop).unwrap().into_iter().map(|h| h.path).collect();
    found.sort();
    assert_eq!(found, [s(&d.join("Two")), s(&e.join("Three"))]);
    assert!(half_copies(all, &[], &stop).unwrap().iter().all(|h| h.state == State::CutOff));
}
