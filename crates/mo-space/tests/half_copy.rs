//! Half copies: what a move leaves when the app or the computer stops part way, on folders made in the test build's own
//! fixtures folder under fixed names. Other folders there stand in for the other drives. A stop is stood in for by a
//! panic from the progress callback, which leaves everything as a crash would: nothing after it runs. Links are
//! junctions on Windows, as the app makes.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, SystemTime};

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

/// Makes the read-only files (and, on a Mac or Linux, folders) a test left writable again, so its folder can be
/// cleared.
fn writable(dir: &Path) {
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(entry.path(), fs::Permissions::from_mode(0o755));
            }
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

/// Each folder in a test's own folder stands for a drive (C, D, E, F…), with this much room.
struct OtherDrive {
    free: Option<u64>,
}

impl Drives for OtherDrive {
    fn same(&self, a: &Path, b: &Path) -> bool {
        matches!((stand_in_drive(a), stand_in_drive(b)), (Some(a), Some(b)) if a == b)
    }
    fn free(&self, _path: &Path) -> Option<u64> {
        self.free
    }
}

/// The stand-in drive a path is on: the test's own folder and the one in it, as [`mo_space::key`] gives them. A path
/// outside them is on a drive of its own.
fn stand_in_drive(path: &Path) -> Option<String> {
    let tests = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let path = mo_space::key(&s(path));
    let rest = [Some(tests.clone()), fs::canonicalize(&tests).ok()]
        .into_iter()
        .flatten()
        .find_map(|tests| path.strip_prefix(&format!("{}/", mo_space::key(&s(&tests)))).map(str::to_owned))?;
    let mut parts = rest.split('/');
    Some(format!("{}/{}", parts.next()?, parts.next()?))
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

/// Makes `copy` the same as `source`, with its time, as the copy makes it.
fn same_as(copy: &Path, source: &Path) {
    fs::copy(source, copy).unwrap();
    let when = fs::metadata(source).unwrap().modified().unwrap();
    fs::File::options().write(true).open(copy).unwrap().set_modified(when).unwrap();
}

/// Writes a file again an hour later than the folder being moved was written.
fn rewrite(file: &Path, text: &str) {
    fs::write(file, text).unwrap();
    let later = SystemTime::now() + Duration::from_secs(3600);
    fs::File::options().write(true).open(file).unwrap().set_modified(later).unwrap();
}

/// A library of six sounds and a patch: 35 bytes.
fn library(at: &Path) {
    for n in 0..6 {
        write(at, &format!("Sounds/{n}.db"), "sound");
    }
    write(at, "Patch.prt", "patch");
}

/// A file kept from being removed until this is dropped: on a Mac or Linux its folder can't be written to (by anyone
/// but the administrator, so a test run as root can't hold it: `holds` says whether it does), and on Windows it's held
/// open, letting nothing else at it.
struct Held {
    #[cfg(windows)]
    _open: fs::File,
    #[cfg(not(windows))]
    folder: PathBuf,
    holds: bool,
}

fn hold(file: &Path) -> Held {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        Held { _open: fs::File::options().read(true).share_mode(0).open(file).unwrap(), holds: true }
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let folder = file.parent().unwrap().to_path_buf();
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o555)).unwrap();
        // Root writes into it anyway.
        let tried = folder.join("can root write here");
        let holds = fs::create_dir(&tried).is_err();
        let _ = fs::remove_dir(&tried);
        Held { folder, holds }
    }
}

#[cfg(not(windows))]
impl Drop for Held {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&self.folder, fs::Permissions::from_mode(0o755));
    }
}

/// A marker as the app writes one, for a copy of `from` made in `to` that ends up as `settled`; `in_use` once the copy
/// started going into use (just before the swap).
fn plant_marker(folder: &Path, from: &Path, to: &Path, settled: &Path, in_use: bool) {
    let json = serde_json::json!({
        "From": s(from),
        "To": s(to),
        "Settled": s(settled),
        "Started": "2026-11-03T14:03:12.1234567+00:00",
        "InUse": in_use,
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
    stopped_part_way(10, |progress| move_folder(&from, &s(&d), 35, whose(), &ROOMY, &[], &[], progress, &stop));

    // The folder is as it was; what was copied is a half copy, and its marker says so.
    let half = d.join("STEAM");
    assert!(!is_link(&steam) && !c.join("Spectrasonics/STEAM (old copy)").exists());
    let said = fs::read_to_string(half.join(MARKER)).unwrap();
    assert!(said.starts_with("Music Organiser is copying a folder into this one."), "{said}");
    let found = find(&half, &[], &ROOMY, &stop).unwrap().unwrap();
    assert_eq!(found.state, State::CutOff);
    assert_eq!((found.path.as_str(), found.from.as_str()), (s(&half).as_str(), from.as_str()));
    assert!(found.bytes >= 10, "{}", found.bytes);
    assert_eq!(half_copies(&[], &[s(&d)], &ROOMY, &stop).unwrap(), std::slice::from_ref(&found));

    // It isn't in the way: the move clears it first, so the room it takes counts as free.
    assert_eq!(problem(&from, &s(&d), 35, &ROOMY, &[], &[], &stop), None);
    let room = |free: u64| OtherDrive { free: Some(free) };
    assert_eq!(problem(&from, &s(&d), 35, &room(HEADROOM + 35 - found.bytes), &[], &[], &stop), None);
    let short = problem(&from, &s(&d), 35, &room(HEADROOM + 34 - found.bytes), &[], &[], &stop);
    assert!(matches!(short, Some(CantMove::NoRoom { needs: 35, .. })), "{short:?}");

    // Tried again, the move clears it and goes through; in use, the copy has no marker.
    let moved = move_folder(&from, &s(&d), 35, whose(), &ROOMY, &[], &[], &nothing, &stop).unwrap();
    assert!(is_link(&steam));
    assert!(!half.join(MARKER).exists());
    assert_eq!(fs::read_to_string(half.join("Patch.prt")).unwrap(), "patch");
    assert_eq!(fs::read_dir(half.join("Sounds")).unwrap().count(), 6);
    assert!(moved.has_old_copy());
    assert!(only_in_old_copy(&moved, std::slice::from_ref(&moved), &stop).unwrap().is_empty());
    assert!(half_copies(std::slice::from_ref(&moved), &[s(&d)], &ROOMY, &stop).unwrap().is_empty());
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
    stopped_part_way(10, |progress| move_folder(&from, &s(&d), 35, whose(), &ROOMY, &[], &[], progress, &stop));
    let half = d.join("Presets");

    // Something that isn't in the folder being moved (yours, or deleted from it since) keeps the half copy.
    write(&half, "Mine.fxp", "mine");
    // A folder of yours in it, with a big file, counts in its size too.
    write(&half, "Mine/big.bin", &"x".repeat(100_000));
    let found = find(&half, &[], &ROOMY, &stop).unwrap().unwrap();
    let State::Stays(why) = &found.state else { panic!("{found:?}") };
    assert!(why.contains("Mine.fxp"), "{why}");
    assert!(found.bytes > 100_000, "{}", found.bytes);
    assert_eq!(found.bytes, mo_space::size::measure(&half, &stop).unwrap().bytes);
    fs::remove_dir_all(half.join("Mine")).unwrap();
    let said = problem(&from, &s(&d), 35, &ROOMY, &[], &[], &stop);
    assert!(matches!(&said, Some(CantMove::HalfCopy { path, .. }) if *path == s(&half)), "{said:?}");
    assert!(said.unwrap().text().contains("Mine.fxp"));
    let tried = move_folder(&from, &s(&d), 35, whose(), &ROOMY, &[], &[], &nothing, &stop);
    assert!(matches!(tried, Err(CantMove::HalfCopy { .. })), "{tried:?}");
    assert!(clear(&half, &[], &ROOMY, &stop).is_err());
    assert!(half.join("Mine.fxp").exists() && half.join(MARKER).exists() && !is_link(&presets));

    // So does a folder where the folder being moved has a file, and a file where it has a folder.
    fs::remove_file(half.join("Mine.fxp")).unwrap();
    let _ = fs::remove_file(half.join("Patch.prt"));
    fs::create_dir_all(half.join("Patch.prt")).unwrap();
    let State::Stays(why) = find(&half, &[], &ROOMY, &stop).unwrap().unwrap().state else { panic!() };
    assert!(why.contains("Patch.prt"), "{why}");
    fs::remove_dir(half.join("Patch.prt")).unwrap();
    let sounds = half.join("Sounds");
    if sounds.exists() {
        fs::remove_dir_all(&sounds).unwrap();
    }
    fs::write(&sounds, "a file").unwrap();
    let State::Stays(why) = find(&half, &[], &ROOMY, &stop).unwrap().unwrap().state else { panic!() };
    assert!(why.contains("Sounds"), "{why}");
    fs::remove_file(&sounds).unwrap();

    // A file newer than its twin and bigger isn't a copy: it was changed in there since.
    let patch = half.join("Patch.prt");
    same_as(&patch, &presets.join("Patch.prt"));
    assert_eq!(find(&half, &[], &ROOMY, &stop).unwrap().unwrap().state, State::CutOff);
    rewrite(&patch, "patch, changed since");
    let State::Stays(why) = find(&half, &[], &ROOMY, &stop).unwrap().unwrap().state else { panic!() };
    assert!(why.contains("Patch.prt") && why.contains("newer"), "{why}");
    // Newer but no bigger, it's the file the copy was writing when it stopped; but only one file can be that.
    rewrite(&patch, "pat");
    assert_eq!(find(&half, &[], &ROOMY, &stop).unwrap().unwrap().state, State::CutOff);
    let sound = half.join("Sounds").join("0.db");
    fs::create_dir_all(half.join("Sounds")).unwrap();
    rewrite(&sound, "so");
    let State::Stays(why) = find(&half, &[], &ROOMY, &stop).unwrap().unwrap().state else { panic!() };
    assert!(why.contains("0.db") || why.contains("Patch.prt"), "{why}");
    fs::remove_file(&sound).unwrap();
    // Older than its twin, it's a copy from before the folder changed.
    fs::write(&patch, "an older patch").unwrap();
    let older = fs::metadata(presets.join("Patch.prt")).unwrap().modified().unwrap() - Duration::from_secs(3600);
    fs::File::options().write(true).open(&patch).unwrap().set_modified(older).unwrap();
    assert_eq!(find(&half, &[], &ROOMY, &stop).unwrap().unwrap().state, State::CutOff);

    // Once it holds only copies again, it's cleared, for good, with all the room it took.
    let bytes = find(&half, &[], &ROOMY, &stop).unwrap().unwrap().bytes;
    assert_eq!(clear(&half, &[], &ROOMY, &stop), Ok(Cleared::Copy(bytes)));
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
    stopped_part_way(8, |progress| move_folder(&from, &s(&d), 8, whose(), &ROOMY, &[], &[], progress, &stop));
    let half = d.join("Library");
    assert!(is_link(&half.join("Linked")));
    assert!(fs::metadata(half.join("Samples/b.wav")).unwrap().permissions().readonly());

    assert!(matches!(clear(&half, &[], &ROOMY, &stop), Ok(Cleared::Copy(_))));
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
    let moved = move_folder(&s(&presets), &s(&d), 35, whose(), &ROOMY, &[], &[], &nothing, &stop).unwrap();
    let copy = d.join("Presets");
    // The app stopped between the swap and taking the marker out.
    plant_marker(&copy, &presets, &copy, &copy, true);
    let all = std::slice::from_ref(&moved);

    let found = half_copies(all, &[], &ROOMY, &stop).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!((found[0].state.clone(), found[0].bytes), (State::MarkerOnly, 0));
    // Without the move's record too: the folder it copies leads here.
    assert_eq!(find(&copy, &[], &ROOMY, &stop).unwrap().unwrap().state, State::MarkerOnly);
    assert_eq!(clear(&copy, all, &ROOMY, &stop), Ok(Cleared::Marker));
    assert!(!copy.join(MARKER).exists());
    assert_eq!(fs::read_to_string(presets.join("Patch.prt")).unwrap(), "patch");
    assert_eq!(fs::read_dir(copy.join("Sounds")).unwrap().count(), 6);

    // The folder it copies is a link leading elsewhere now (moved on again, say), with no record of this copy: it went
    // into use once, so the plugin may have saved into it. It stays, whole, for you to look at.
    let elsewhere = dir.join("E").join("Presets");
    library(&elsewhere);
    unlink(&presets);
    folder_link(&elsewhere, &presets);
    plant_marker(&copy, &presets, &copy, &copy, true);
    let State::Stays(why) = find(&copy, &[], &ROOMY, &stop).unwrap().unwrap().state else { panic!() };
    assert!(why.contains("went into use"), "{why}");
    assert!(clear(&copy, &[], &ROOMY, &stop).is_err());
    assert_eq!(fs::read_dir(copy.join("Sounds")).unwrap().count(), 6);
    unlink(&presets);
    folder_link(&copy, &presets);

    // An update put a real folder back where the link was: the copy is still the move's, so it isn't touched.
    unlink(&presets);
    library(&presets);
    plant_marker(&copy, &presets, &copy, &copy, true);
    assert_eq!(find(&copy, all, &ROOMY, &stop).unwrap().unwrap().state, State::MarkerOnly);
    // And moving back takes such a marker out first, so the copy left for you never looks like a half copy.
    unlink_or_skip(&presets);
    folder_link(&copy, &presets);
    move_back(&moved, &[], &ROOMY, &nothing, &stop).unwrap();
    assert!(!copy.join(MARKER).exists() && !presets.join(MARKER).exists());
    assert!(find(&copy, &[], &ROOMY, &stop).unwrap().is_none());
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
    stopped_part_way(10, |progress| move_folder(&s(&presets), &s(&d), 35, whose(), &ROOMY, &[], &[], progress, &stop));
    // You renamed the half copy: it isn't where the copy was being made, so it's yours now.
    let renamed = d.join("Presets kept");
    fs::rename(d.join("Presets"), &renamed).unwrap();
    assert_eq!(find(&renamed, &[], &ROOMY, &stop).unwrap().unwrap().state, State::MarkerOnly);
    assert_eq!(clear(&renamed, &[], &ROOMY, &stop), Ok(Cleared::Marker));
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
    stopped_part_way(10, |progress| move_folder(&s(&a), &s(&d), 35, whose(), &ROOMY, &[], &[], progress, &stop));
    let half = d.join("Presets");

    // B's Presets would go to the same place, but the half copy there is A's: it's in the way, as anything is.
    let there = CantMove::TargetThere(s(&half));
    assert_eq!(problem(&s(&b), &s(&d), 35, &ROOMY, &[], &[], &stop), Some(there.clone()));
    assert_eq!(move_folder(&s(&b), &s(&d), 35, whose(), &ROOMY, &[], &[], &nothing, &stop), Err(there));
    assert!(half.join(MARKER).exists());

    // A marker that isn't as the app writes it isn't one: the folder isn't known for a half copy.
    fs::write(half.join(MARKER), "nonsense").unwrap();
    assert_eq!(find(&half, &[], &ROOMY, &stop).unwrap(), None);
    assert_eq!(problem(&s(&a), &s(&d), 35, &ROOMY, &[], &[], &stop), Some(CantMove::TargetThere(s(&half))));
    assert!(clear(&half, &[], &ROOMY, &stop).is_err());
    assert!(half.exists());
    // Nor is a marker that's a folder, or a folder that's a link to a half copy.
    fs::remove_file(half.join(MARKER)).unwrap();
    fs::create_dir(half.join(MARKER)).unwrap();
    assert_eq!(find(&half, &[], &ROOMY, &stop).unwrap(), None);
    fs::remove_dir(half.join(MARKER)).unwrap();
    plant_marker(&half, &a, &half, &half, false);
    let link = d.join("Presets link");
    folder_link(&half, &link);
    assert_eq!(find(&link, &[], &ROOMY, &stop).unwrap(), None);
    assert!(clear(&link, &[], &ROOMY, &stop).is_err());
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
    stopped_part_way(35, |progress| move_folder(&s(&presets), &s(&d), 35, whose(), &ROOMY, &[], &[], progress, &stop));
    let half = d.join("Presets");
    // Cut off before its swap: with an "(old copy)" beside the folder from elsewhere, it's still only a half copy.
    let aside = c.join("Presets (old copy)");
    library(&aside);
    assert_eq!(find(&half, &[], &ROOMY, &stop).unwrap().unwrap().state, State::CutOff);
    fs::remove_dir_all(&aside).unwrap();
    // Stopped just after the swap renamed the folder, before its link was made: the marker said it was going into use.
    plant_marker(&half, &presets, &half, &half, true);
    fs::rename(&presets, &aside).unwrap();
    let State::Stays(why) = find(&half, &[], &ROOMY, &stop).unwrap().unwrap().state else { panic!() };
    assert!(why.contains("Rename it back to \"Presets\""), "{why}");
    assert!(clear(&half, &[], &ROOMY, &stop).is_err());
    // Renamed back, it still stays: the plugin may have saved into it (you may have undone a swap that went through).
    fs::rename(&aside, &presets).unwrap();
    let State::Stays(why) = find(&half, &[], &ROOMY, &stop).unwrap().unwrap().state else { panic!() };
    assert!(why.contains("went into use"), "{why}");
    let tried = move_folder(&s(&presets), &s(&d), 35, whose(), &ROOMY, &[], &[], &nothing, &stop);
    assert!(matches!(tried, Err(CantMove::HalfCopy { .. })), "{tried:?}");
    assert!(half.join("Patch.prt").exists() || half.join("Sounds").exists());
}

#[test]
fn moving_back_the_computer_stopped_leaves_a_half_copy_the_next_try_clears() {
    let dir = fixtures("mo-space-half-back");
    let (c, d) = (dir.join("C"), dir.join("D"));
    let presets = c.join("Presets");
    library(&presets);
    fs::create_dir_all(&d).unwrap();
    let stop = AtomicBool::new(false);
    let moved = move_folder(&s(&presets), &s(&d), 35, whose(), &ROOMY, &[], &[], &nothing, &stop).unwrap();
    // The old copy was deleted once the plugin opened fine, so moving back copies the folder back.
    fs::remove_dir_all(&moved.old_copy).unwrap();
    stopped_part_way(10, |progress| move_back(&moved, &[], &ROOMY, progress, &stop));
    let back = c.join("Presets (moving back)");
    let all = std::slice::from_ref(&moved);
    assert!(back.join(MARKER).exists() && is_link(&presets));
    assert_eq!(find(&back, all, &ROOMY, &stop).unwrap().unwrap().state, State::CutOff);
    // Found beside the moved folder too.
    assert_eq!(half_copies(all, &[], &ROOMY, &stop).unwrap().len(), 1);

    // Tried again, moving back clears it first and goes through; in place, the folder has no marker.
    move_back(&moved, &[], &ROOMY, &nothing, &stop).unwrap();
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
    let moved = move_folder(&s(&presets), &s(&d), 35, whose(), &ROOMY, &[], &[], &nothing, &stop).unwrap();
    fs::remove_dir_all(&moved.old_copy).unwrap();
    stopped_part_way(35, |progress| move_back(&moved, &[], &ROOMY, progress, &stop));
    let back = c.join("Presets (moving back)");
    let all = std::slice::from_ref(&moved);
    // An update put a real folder in the link's place while it copied: the copy may be in use.
    unlink(&presets);
    library(&presets);
    let State::Stays(why) = find(&back, all, &ROOMY, &stop).unwrap().unwrap().state else { panic!() };
    assert!(why.contains("isn't the link"), "{why}");
    let tried = move_back(&moved, &[], &ROOMY, &nothing, &stop);
    assert!(tried.is_err() && back.join(MARKER).exists(), "{tried:?}");
    // Stopped after the link went, before the copy took its name: the marker said it was going into use.
    fs::remove_dir_all(&presets).unwrap();
    plant_marker(&back, Path::new(&moved.to), &back, &presets, true);
    let State::Stays(why) = find(&back, all, &ROOMY, &stop).unwrap().unwrap().state else { panic!() };
    assert!(why.contains("rename \"Presets (moving back)\" to \"Presets\""), "{why}");
    // A folder in the link's place by then: the plugin may have saved into the copy.
    library(&presets);
    let State::Stays(why) = find(&back, all, &ROOMY, &stop).unwrap().unwrap().state else { panic!() };
    assert!(why.contains("went into use"), "{why}");
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
    let moved = move_folder(&s(&presets), &s(&home), 35, whose(), &ROOMY, &[], &[], &nothing, &stop).unwrap();
    let all = std::slice::from_ref(&moved);
    stopped_part_way(10, |progress| move_home(&s(&home), &s(&e), all, &[], 35, &ROOMY, progress, &stop));
    let half = e.join("Plugin Data");
    assert!(half.join(MARKER).exists());
    assert_eq!(find(&half, all, &ROOMY, &stop).unwrap().unwrap().state, State::CutOff);

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
    plant_marker(&presets, &d.join("Presets"), &c.join("Presets (moving back)"), &presets, true);
    fs::create_dir_all(&d).unwrap();
    let stop = AtomicBool::new(false);
    let moved: MovedFolder = move_folder(&s(&presets), &s(&d), 35, whose(), &ROOMY, &[], &[], &nothing, &stop).unwrap();
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
    let one = move_folder(&s(&c.join("One")), &s(&d), 35, whose(), &ROOMY, &[], &[], &nothing, &stop).unwrap();
    // Cut off: Two to where One went, Three to a drive the app remembers.
    stopped_part_way(10, |progress| {
        move_folder(&s(&c.join("Two")), &s(&d), 35, whose(), &ROOMY, &[], &[], progress, &stop)
    });
    stopped_part_way(10, |progress| {
        move_folder(&s(&c.join("Three")), &s(&e), 35, whose(), &ROOMY, &[], &[], progress, &stop)
    });
    // And one where nothing says moves go.
    stopped_part_way(10, |progress| {
        move_folder(&s(&c.join("Three")), &s(&f), 35, whose(), &ROOMY, &[], &[], progress, &stop)
    });
    let all = std::slice::from_ref(&one);

    let mut found: Vec<String> =
        half_copies(all, &[s(&e)], &ROOMY, &stop).unwrap().into_iter().map(|h| h.path).collect();
    found.sort();
    assert_eq!(found, [s(&d.join("Two")), s(&e.join("Three"))]);
    assert!(half_copies(all, &[], &ROOMY, &stop).unwrap().iter().all(|h| h.state == State::CutOff));
}

#[test]
fn a_copy_a_move_is_making_is_never_cleared_nor_made_twice() {
    let dir = fixtures("mo-space-half-busy");
    let (c, d) = (dir.join("C"), dir.join("D"));
    let presets = c.join("Presets");
    library(&presets);
    fs::create_dir_all(&d).unwrap();
    let stop = AtomicBool::new(false);
    let half = d.join("Presets");
    let seen: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let looked = AtomicBool::new(false);
    // Part way through the copy, the Space page looks from another thread, and a second move of the folder starts.
    let progress = |n: u64| {
        if n < 10 || looked.swap(true, Ordering::Relaxed) {
            return;
        }
        let said = thread::scope(|scope| {
            scope
                .spawn(|| {
                    let found = find(&half, &[], &ROOMY, &stop).unwrap().unwrap().state;
                    let listed = half_copies(&[], &[s(&d)], &ROOMY, &stop).unwrap().len();
                    let cleared = clear(&half, &[], &ROOMY, &stop);
                    let looked_first = problem(&s(&presets), &s(&d), 35, &ROOMY, &[], &[], &stop);
                    let again = move_folder(&s(&presets), &s(&d), 35, whose(), &ROOMY, &[], &[], &nothing, &stop);
                    vec![
                        format!("{found:?}"),
                        listed.to_string(),
                        format!("{cleared:?}"),
                        format!("{looked_first:?}"),
                        format!("{again:?}"),
                    ]
                })
                .join()
                .unwrap()
        });
        *seen.lock().unwrap() = said;
    };
    let moved = move_folder(&s(&presets), &s(&d), 35, whose(), &ROOMY, &[], &[], &progress, &stop).unwrap();

    let seen = seen.into_inner().unwrap();
    assert_eq!(seen.len(), 5, "{seen:?}");
    assert!(seen[0].contains("making or clearing a copy here now"), "{seen:?}");
    assert_eq!(seen[1], "0", "{seen:?}");
    assert!(seen[2].contains("making or clearing a copy here now"), "{seen:?}");
    assert!(seen[3].contains("making or clearing a copy at"), "{seen:?}");
    assert!(seen[4].contains("making or clearing a copy at"), "{seen:?}");
    // The move went through, with all of its copy.
    assert!(is_link(&presets) && moved.has_old_copy());
    assert!(!half.join(MARKER).exists());
    assert_eq!(fs::read_dir(half.join("Sounds")).unwrap().count(), 6);
    assert_eq!(fs::read_to_string(half.join("Patch.prt")).unwrap(), "patch");
}

#[test]
fn a_half_copy_whose_folder_moved_elsewhere_since_is_still_cleared() {
    let dir = fixtures("mo-space-half-moved-on");
    let (c, d, e) = (dir.join("C"), dir.join("D"), dir.join("E"));
    let presets = c.join("Presets");
    library(&presets);
    fs::create_dir_all(&d).unwrap();
    fs::create_dir_all(&e).unwrap();
    let stop = AtomicBool::new(false);
    stopped_part_way(10, |progress| move_folder(&s(&presets), &s(&d), 35, whose(), &ROOMY, &[], &[], progress, &stop));
    // Then the folder was moved to another drive, all the way: the half copy never went into use.
    let moved = move_folder(&s(&presets), &s(&e), 35, whose(), &ROOMY, &[], &[], &nothing, &stop).unwrap();
    let half = d.join("Presets");
    let all = std::slice::from_ref(&moved);
    let found = half_copies(all, &[s(&d)], &ROOMY, &stop).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].state, State::CutOff);
    assert!(found[0].bytes >= 10, "{:?}", found[0]);
    assert_eq!(clear(&half, all, &ROOMY, &stop), Ok(Cleared::Copy(found[0].bytes)));
    assert!(!half.exists());
    assert_eq!(fs::read_dir(e.join("Presets").join("Sounds")).unwrap().count(), 6);
}

#[test]
fn a_half_copy_on_a_drive_with_another_letter_now_is_still_cleared_by_the_next_try() {
    let dir = fixtures("mo-space-half-letter");
    let (c, d, f) = (dir.join("C"), dir.join("D"), dir.join("F"));
    let presets = c.join("Presets");
    library(&presets);
    fs::create_dir_all(&d).unwrap();
    let stop = AtomicBool::new(false);
    stopped_part_way(10, |progress| move_folder(&s(&presets), &s(&d), 35, whose(), &ROOMY, &[], &[], progress, &stop));
    // Plugged in again, the drive got another letter: its folder is called F here.
    fs::rename(&d, &f).unwrap();
    let half = f.join("Presets");
    assert_eq!(find(&half, &[], &ROOMY, &stop).unwrap().unwrap().state, State::CutOff);
    // Another drive has its old letter now: it's still the same half copy.
    fs::create_dir_all(&d).unwrap();
    assert_eq!(find(&half, &[], &ROOMY, &stop).unwrap().unwrap().state, State::CutOff);
    // Also when that drive has a folder at the same place, or can't be looked at (a card reader with no card in it).
    fs::create_dir_all(d.join("Presets")).unwrap();
    assert_eq!(find(&half, &[], &ROOMY, &stop).unwrap().unwrap().state, State::CutOff);
    fs::remove_dir(d.join("Presets")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&d, fs::Permissions::from_mode(0o000)).unwrap();
        let found = find(&half, &[], &ROOMY, &stop).unwrap().unwrap().state;
        fs::set_permissions(&d, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(found, State::CutOff);
    }
    assert_eq!(problem(&s(&presets), &s(&f), 35, &ROOMY, &[], &[], &stop), None);
    move_folder(&s(&presets), &s(&f), 35, whose(), &ROOMY, &[], &[], &nothing, &stop).unwrap();
    assert!(is_link(&presets) && !half.join(MARKER).exists());
    assert_eq!(fs::read_dir(half.join("Sounds")).unwrap().count(), 6);
}

#[test]
fn a_half_copy_made_through_a_link_is_still_known_once_its_drive_has_another_letter() {
    let dir = fixtures("mo-space-half-through-link");
    let (c, d, f) = (dir.join("C"), dir.join("D"), dir.join("F"));
    let presets = c.join("Presets");
    library(&presets);
    // The folder picked is a link to another folder on that drive.
    fs::create_dir_all(d.join("Real")).unwrap();
    folder_link(&d.join("Real"), &d.join("Link"));
    let stop = AtomicBool::new(false);
    let link = s(&d.join("Link"));
    stopped_part_way(10, |progress| move_folder(&s(&presets), &link, 35, whose(), &ROOMY, &[], &[], progress, &stop));
    assert!(d.join("Real").join("Presets").join(MARKER).exists());
    // Plugged in again, the drive got another letter: the link leads nowhere now, but the half copy is still known.
    fs::rename(&d, &f).unwrap();
    let half = f.join("Real").join("Presets");
    assert_eq!(find(&half, &[], &ROOMY, &stop).unwrap().unwrap().state, State::CutOff);
    // The next try, to where it really is, clears it first.
    move_folder(&s(&presets), &s(&f.join("Real")), 35, whose(), &ROOMY, &[], &[], &nothing, &stop).unwrap();
    assert!(is_link(&presets) && !half.join(MARKER).exists());
    assert_eq!(fs::read_dir(half.join("Sounds")).unwrap().count(), 6);
}

#[test]
fn a_half_copy_you_moved_elsewhere_on_its_drive_keeps_everything_but_its_marker() {
    let dir = fixtures("mo-space-half-moved-by-you");
    let (c, d) = (dir.join("C"), dir.join("D"));
    let presets = c.join("Presets");
    library(&presets);
    let data = d.join("Plugin Data");
    fs::create_dir_all(&data).unwrap();
    let stop = AtomicBool::new(false);
    stopped_part_way(10, |progress| {
        move_folder(&s(&presets), &s(&data), 35, whose(), &ROOMY, &[], &[], progress, &stop)
    });
    let files = |folder: &Path| fs::read_dir(folder.join("Sounds")).map_or(0, |sounds| sounds.count());
    let copied = files(&data.join("Presets"));
    assert!(copied > 0);
    // You moved it beside the folder it was made in, to look at it: the same name, as deep, on the same drive.
    let beside = d.join("Kept").join("Presets");
    fs::create_dir_all(d.join("Kept")).unwrap();
    fs::rename(data.join("Presets"), &beside).unwrap();
    assert_eq!(find(&beside, &[], &ROOMY, &stop).unwrap().unwrap().state, State::MarkerOnly);
    // Then to the top of the drive.
    let top = d.join("Presets");
    fs::rename(&beside, &top).unwrap();
    assert_eq!(find(&top, &[], &ROOMY, &stop).unwrap().unwrap().state, State::MarkerOnly);
    assert_eq!(half_copies(&[], &[s(&d)], &ROOMY, &stop).unwrap()[0].state, State::MarkerOnly);
    assert_eq!(clear(&top, &[], &ROOMY, &stop), Ok(Cleared::Marker));
    assert!(!top.join(MARKER).exists());
    assert_eq!(files(&top), copied);
}

#[test]
fn a_file_link_copied_as_its_file_is_still_a_copy() {
    let dir = fixtures("mo-space-half-file-link");
    let (c, d) = (dir.join("C"), dir.join("D"));
    let presets = c.join("Presets");
    library(&presets);
    write(&dir, "Shared/Init.fxp", "init");
    // Where a file link can't be made (Windows without Developer Mode), there's nothing to check.
    if !common::file_link(&dir.join("Shared").join("Init.fxp"), &presets.join("Init.fxp")) {
        return;
    }
    fs::create_dir_all(&d).unwrap();
    let stop = AtomicBool::new(false);
    stopped_part_way(10, |progress| move_folder(&s(&presets), &s(&d), 39, whose(), &ROOMY, &[], &[], progress, &stop));
    let half = d.join("Presets");
    // Windows copies the file a link leads to when it won't make the link again.
    let copied = half.join("Init.fxp");
    if fs::symlink_metadata(&copied).is_ok() {
        fs::remove_file(&copied).unwrap();
    }
    same_as(&copied, &dir.join("Shared").join("Init.fxp"));
    assert_eq!(find(&half, &[], &ROOMY, &stop).unwrap().unwrap().state, State::CutOff);
    assert!(matches!(clear(&half, &[], &ROOMY, &stop), Ok(Cleared::Copy(_))));
    assert_eq!(fs::read_to_string(dir.join("Shared").join("Init.fxp")).unwrap(), "init");
    assert!(is_link(&presets.join("Init.fxp")));
}

#[test]
fn a_copy_whose_swap_failed_goes_and_what_couldnt_go_is_cleared_by_the_next_try() {
    let dir = fixtures("mo-space-half-swap-failed");
    let (c, d) = (dir.join("C"), dir.join("D"));
    let presets = c.join("Presets");
    library(&presets);
    fs::create_dir_all(&d).unwrap();
    let stop = AtomicBool::new(false);
    let copy = d.join("Presets");
    let aside = c.join("Presets (old copy)");
    let held: Mutex<Option<Held>> = Mutex::new(None);
    // Once all of it has copied, a folder takes the name the folder is set aside under, so the swap fails; and a file
    // in the copy is held, so the copy can't all go then.
    let progress = |n: u64| {
        let mut held = held.lock().unwrap();
        if n >= 35 && held.is_none() {
            fs::create_dir_all(&aside).unwrap();
            *held = Some(hold(&copy.join("Sounds").join("0.db")));
        }
    };
    let tried = move_folder(&s(&presets), &s(&d), 35, whose(), &ROOMY, &[], &[], &progress, &stop);
    assert_eq!(tried, Err(CantMove::OldCopyThere));
    let held = held.into_inner().unwrap().expect("a file of the copy was held");
    assert!(!is_link(&presets));
    if held.holds {
        // The swap didn't happen, so the copy never went into use: what's left of it is a half copy like any other.
        assert!(copy.join("Sounds").join("0.db").exists() && copy.join(MARKER).exists());
        assert_eq!(find(&copy, &[], &ROOMY, &stop).unwrap().unwrap().state, State::CutOff);
    } else {
        // Run as root, nothing holds the file, so all of the copy went: what's left can't be checked.
        assert!(!copy.exists());
        eprintln!("Run as root: the file couldn't be held, so a copy left by a failed swap wasn't checked.");
    }
    drop(held);
    fs::remove_dir_all(&aside).unwrap();

    // Tried again, the move clears it and goes through.
    let moved = move_folder(&s(&presets), &s(&d), 35, whose(), &ROOMY, &[], &[], &nothing, &stop).unwrap();
    assert!(is_link(&presets) && moved.has_old_copy());
    assert!(!copy.join(MARKER).exists());
    assert_eq!(fs::read_dir(copy.join("Sounds")).unwrap().count(), 6);
    assert_eq!(fs::read_to_string(copy.join("Patch.prt")).unwrap(), "patch");
}

#[test]
fn a_link_where_the_copy_goes_is_in_the_way_as_anything_is() {
    let dir = fixtures("mo-space-half-link-there");
    let (c, d) = (dir.join("C"), dir.join("D"));
    let presets = c.join("Presets");
    library(&presets);
    fs::create_dir_all(&d).unwrap();
    let stop = AtomicBool::new(false);
    stopped_part_way(10, |progress| move_folder(&s(&presets), &s(&d), 35, whose(), &ROOMY, &[], &[], progress, &stop));
    // The half copy was renamed, and a link to it took its name.
    let half = d.join("Presets");
    let renamed = d.join("Presets kept");
    fs::rename(&half, &renamed).unwrap();
    folder_link(&renamed, &half);
    let there = CantMove::TargetThere(s(&half));
    assert_eq!(problem(&s(&presets), &s(&d), 35, &ROOMY, &[], &[], &stop), Some(there.clone()));
    // The move says the same: its own hold on the place doesn't count as the app making a copy there.
    assert_eq!(move_folder(&s(&presets), &s(&d), 35, whose(), &ROOMY, &[], &[], &nothing, &stop), Err(there));
    assert!(renamed.join(MARKER).exists() && is_link(&half) && !is_link(&presets));

    // The renamed half copy isn't where the copy was made, though the link of that name leads to it: it's yours now,
    // so only its marker goes, and the link stays.
    let found = half_copies(&[], &[s(&d)], &ROOMY, &stop).unwrap();
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!((found[0].path.as_str(), &found[0].state), (s(&renamed).as_str(), &State::MarkerOnly));
    assert_eq!(clear(&renamed, &[], &ROOMY, &stop), Ok(Cleared::Marker));
    assert!(!renamed.join(MARKER).exists());
    assert!(renamed.join("Patch.prt").exists() || renamed.join("Sounds").exists());
    assert!(is_link(&half) && fs::canonicalize(&half).unwrap() == fs::canonicalize(&renamed).unwrap());
}
