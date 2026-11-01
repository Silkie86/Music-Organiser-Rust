//! Removing spare plugin copies, on files made in the test build's own fixtures folder under fixed names. The bin is
//! a folder there too, never the computer's Recycle Bin.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

use mo_space::clean::{Bin, Problem, Removed, What, undo};
use mo_space::key;
use mo_space::plugin_space::SpaceKind;
use mo_space::spare::{SpareCopy, Why, look, remove};

mod common;

use common::{folder_link, unlink};

fn fixtures(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &Path, rel: &str, bytes: usize) {
    let p = dir.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, vec![0u8; bytes]).unwrap();
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
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
        held.push((s(path), to));
        Ok(())
    }
    fn find(&self, removed: &mut [Removed]) -> Vec<Problem> {
        for r in removed {
            r.bin_id = self.held.borrow().iter().find(|(p, _)| key(p) == key(&r.real)).map(|(_, at)| s(at));
        }
        Vec::new()
    }
    fn put_back(&self, removed: &[Removed]) -> Vec<Problem> {
        for r in removed {
            let at =
                self.held.borrow().iter().find(|(_, at)| Some(s(at)) == r.bin_id).map(|(_, at)| at.clone()).unwrap();
            fs::rename(at, &r.path).unwrap();
        }
        Vec::new()
    }
}

/// A spare VST2-or-other copy as `find` gives it, looked at now; the projects use `used_copy`'s format.
fn spare(path: &Path, format: &str, used_copy: &Path) -> SpareCopy {
    let (bytes, stamp) = look(&s(path), &AtomicBool::new(false));
    SpareCopy {
        name: "Serum".to_owned(),
        format: format.to_owned(),
        path: s(path),
        bytes,
        stamp,
        why: Why::OtherFormatsUsed { used: vec!["VST3".to_owned()], projects: 3, used_copies: vec![s(used_copy)] },
    }
}

fn folder_bin(dir: &Path) -> FolderBin {
    FolderBin { dir: dir.join("bin"), held: RefCell::new(Vec::new()) }
}

/// Removes one copy, found again just now as it was found: what it says, if it stays.
fn remove_one(copy: &SpareCopy, bin: &FolderBin) -> Option<String> {
    let done = remove(std::slice::from_ref(copy), std::slice::from_ref(copy), bin, &AtomicBool::new(false));
    done.problems.first().map(|p| p.why.clone())
}

#[test]
fn a_spare_copy_goes_only_while_its_still_spare_and_as_it_was_found() {
    let dir = fixtures("mo-space-spare-remove");
    let plugins = dir.join("Plugins");
    write(&plugins, "VST3/Serum.vst3", 40);
    write(&plugins, "VST3/Massive.vst3", 40);
    write(&plugins, "VST2/Serum_x64.dll", 100);
    write(&plugins, "CLAP/Serum.clap", 50);
    write(&plugins, "AAX/Serum.aaxplugin/Contents/x64/Serum.aaxplugin", 70);
    write(&plugins, "VST2/Massive.dll", 20);
    write(&plugins, "VST2/Notes.txt", 5);

    let vst3 = plugins.join("VST3/Serum.vst3");
    let vst2 = spare(&plugins.join("VST2/Serum_x64.dll"), "VST2", &vst3);
    let clap = spare(&plugins.join("CLAP/Serum.clap"), "CLAP", &vst3);
    let aax = SpareCopy { why: Why::NoProTools, ..spare(&plugins.join("AAX/Serum.aaxplugin"), "AAX", &vst3) };
    let massive = spare(&plugins.join("VST2/Massive.dll"), "VST2", &plugins.join("VST3/Massive.vst3"));
    // Not a plugin file.
    let notes = spare(&plugins.join("VST2/Notes.txt"), "VST2", &vst3);
    assert_eq!((vst2.bytes, aax.bytes), (100, 70));
    // Updated since it was found.
    write(&plugins, "VST2/Massive.dll", 30);
    let picked = vec![vst2.clone(), clap.clone(), aax.clone(), massive.clone(), notes.clone(), vst2.clone()];
    // A song saved since uses Serum's CLAP: found again, it isn't spare now.
    let now = vec![vst2.clone(), aax.clone(), massive.clone(), notes.clone()];

    let bin = folder_bin(&dir);
    let done = remove(&picked, &now, &bin, &AtomicBool::new(false));
    let gone: Vec<&str> = done.removed.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(gone, [s(&plugins.join("VST2/Serum_x64.dll")), s(&plugins.join("AAX/Serum.aaxplugin"))]);
    assert!(done.removed.iter().all(|r| r.what == What::Plugin(SpaceKind::Program) && r.bin_id.is_some()));
    assert_eq!(done.bytes(), 170);
    let why = |p: &SpareCopy| done.problems.iter().find(|x| x.path == p.path).map(|x| x.why.clone()).unwrap();
    assert!(why(&clap).contains("isn't spare any more (a project saved since may use its CLAP"), "{}", why(&clap));
    assert!(why(&massive).contains("changed since it was found"), "{}", why(&massive));
    assert!(why(&notes).contains("isn't a VST2 plugin"), "{}", why(&notes));
    assert_eq!(done.problems.len(), 3);
    assert!(plugins.join("CLAP/Serum.clap").exists() && plugins.join("VST2/Massive.dll").exists());

    // Undo puts them back.
    assert!(undo(&done, &bin).is_empty());
    assert!(plugins.join("VST2/Serum_x64.dll").exists());
    assert!(plugins.join("AAX/Serum.aaxplugin/Contents/x64/Serum.aaxplugin").exists());
}

#[test]
fn an_update_of_the_same_size_keeps_it() {
    let dir = fixtures("mo-space-spare-same-size");
    write(&dir, "VST3/Serum.vst3", 10);
    write(&dir, "VST2/Serum_x64.dll", 10);
    let copy = spare(&dir.join("VST2/Serum_x64.dll"), "VST2", &dir.join("VST3/Serum.vst3"));
    // The same size, written an hour later.
    fs::write(dir.join("VST2/Serum_x64.dll"), [1u8; 10]).unwrap();
    let later = SystemTime::now() + Duration::from_secs(3600);
    fs::File::options().write(true).open(dir.join("VST2/Serum_x64.dll")).unwrap().set_modified(later).unwrap();
    let why = remove_one(&copy, &folder_bin(&dir)).unwrap();
    assert!(why.contains("changed since it was found"), "{why}");
    assert!(dir.join("VST2/Serum_x64.dll").exists());
}

#[test]
fn it_stays_when_the_copy_your_projects_use_has_gone() {
    let dir = fixtures("mo-space-spare-used-gone");
    write(&dir, "VST2/Serum_x64.dll", 10);
    let copy = spare(&dir.join("VST2/Serum_x64.dll"), "VST2", &dir.join("VST3/Serum.vst3"));
    let why = remove_one(&copy, &folder_bin(&dir)).unwrap();
    assert!(why.starts_with("The VST3 copy your projects use isn't there now"), "{why}");
    // An empty bundle left behind doesn't count.
    fs::create_dir_all(dir.join("VST3/Serum.vst3/Contents")).unwrap();
    let why = remove_one(&copy, &folder_bin(&dir)).unwrap();
    assert!(why.contains("isn't there now"), "{why}");
    assert!(dir.join("VST2/Serum_x64.dll").exists());
    // A link to it whose file has gone doesn't count either, but one whose file is there does.
    #[cfg(unix)]
    {
        fs::remove_dir_all(dir.join("VST3/Serum.vst3")).unwrap();
        std::os::unix::fs::symlink(dir.join("Unplugged/Serum.vst3"), dir.join("VST3/Serum.vst3")).unwrap();
        let why = remove_one(&copy, &folder_bin(&dir)).unwrap();
        assert!(why.contains("isn't there now"), "{why}");
        write(&dir, "Unplugged/Serum.vst3", 10);
        assert_eq!(remove_one(&copy, &folder_bin(&dir)), None);
        assert!(!dir.join("VST2/Serum_x64.dll").exists());
    }
}

#[test]
fn a_copy_thats_gone_or_not_spare_now_says_why() {
    let dir = fixtures("mo-space-spare-why");
    write(&dir, "VST3/Serum.vst3", 10);
    write(&dir, "AAX/Serum.aaxplugin/Contents/x64/Serum.aaxplugin", 10);
    let vst3 = dir.join("VST3/Serum.vst3");
    let aax = SpareCopy { why: Why::NoProTools, ..spare(&dir.join("AAX/Serum.aaxplugin"), "AAX", &vst3) };
    // Uninstalled since: found again, it isn't in the list, but that's not why it stays.
    let gone = spare(&dir.join("VST2/Serum_x64.dll"), "VST2", &vst3);
    let bin = folder_bin(&dir);
    let done = remove(&[aax.clone(), gone.clone()], &[], &bin, &AtomicBool::new(false));
    assert!(done.removed.is_empty());
    let why: Vec<&str> = done.problems.iter().map(|p| p.why.as_str()).collect();
    assert_eq!(
        why,
        ["It isn't spare any more (Pro Tools may be on this computer now), so it stays.", "It isn't there any more."]
    );
}

#[test]
fn a_stop_keeps_the_rest() {
    let dir = fixtures("mo-space-spare-stop");
    write(&dir, "VST3/Serum.vst3", 10);
    write(&dir, "VST2/Serum_x64.dll", 10);
    let copy = spare(&dir.join("VST2/Serum_x64.dll"), "VST2", &dir.join("VST3/Serum.vst3"));
    let done =
        remove(std::slice::from_ref(&copy), std::slice::from_ref(&copy), &folder_bin(&dir), &AtomicBool::new(true));
    assert!(done.removed.is_empty());
    assert_eq!(done.problems[0].why, "Stopped before it went.");
}

#[test]
fn a_bundle_deeper_than_the_walks_go_can_t_go() {
    let dir = fixtures("mo-space-spare-deep");
    write(&dir, "VST3/Pad.vst3/Contents/x86_64-win/Pad.vst3", 10);
    // Folders 130 deep inside: more than any walk reads (128), so what's at the bottom can't be checked.
    let deep: PathBuf = std::iter::repeat_n("d", 130).collect();
    write(&dir.join("VST3/Deep.vst3/Contents"), &format!("{}/x.bin", s(&deep)), 10);
    let stop = AtomicBool::new(false);
    assert!(look(&s(&dir.join("VST3/Pad.vst3")), &stop).1.is_some());
    assert_eq!(look(&s(&dir.join("VST3/Deep.vst3")), &stop).1, None);
}

#[cfg(unix)]
#[test]
fn a_copy_thats_a_link_stays() {
    let dir = fixtures("mo-space-spare-link");
    write(&dir, "VST3/Serum.vst3", 10);
    write(&dir, "Elsewhere/Serum_x64.dll", 10);
    fs::create_dir_all(dir.join("VST2")).unwrap();
    std::os::unix::fs::symlink(dir.join("Elsewhere/Serum_x64.dll"), dir.join("VST2/Serum_x64.dll")).unwrap();
    let copy = spare(&dir.join("VST2/Serum_x64.dll"), "VST2", &dir.join("VST3/Serum.vst3"));
    // A link takes no room, and it's listed only so you see it.
    assert_eq!(copy.bytes, 0);
    assert!(!copy.can_remove());
    let why = remove_one(&copy, &folder_bin(&dir)).unwrap();
    assert_eq!(why, "It's a link to another place, so removing it frees nothing. It stays.");
    assert!(dir.join("Elsewhere/Serum_x64.dll").exists());
}

#[test]
fn a_folder_on_the_way_that_leads_elsewhere_now_keeps_it() {
    let dir = fixtures("mo-space-spare-repointed");
    write(&dir, "VST3/Serum.vst3", 10);
    write(&dir, "Mine/Serum_x64.dll", 10);
    write(&dir, "Other rig/Serum_x64.dll", 10);
    folder_link(&dir.join("Mine"), &dir.join("VST2"));
    let copy = spare(&dir.join("VST2/Serum_x64.dll"), "VST2", &dir.join("VST3/Serum.vst3"));
    // The folder now leads to another rig's copy: the same name, size and time.
    let time = fs::metadata(dir.join("Mine/Serum_x64.dll")).unwrap().modified().unwrap();
    fs::File::options().write(true).open(dir.join("Other rig/Serum_x64.dll")).unwrap().set_modified(time).unwrap();
    unlink(&dir.join("VST2"));
    folder_link(&dir.join("Other rig"), &dir.join("VST2"));
    let why = remove_one(&copy, &folder_bin(&dir)).unwrap();
    assert!(why.contains("changed since it was found"), "{why}");
    assert!(dir.join("Other rig/Serum_x64.dll").exists());
}

/// A path written with a server's name is on a network drive (on Linux, `//` at the start is the same as `/`).
#[cfg(target_os = "linux")]
#[test]
fn a_copy_on_a_network_drive_stays() {
    let dir = fixtures("mo-space-spare-network");
    write(&dir, "VST3/Serum.vst3", 10);
    write(&dir, "VST2/Serum_x64.dll", 10);
    let copy =
        spare(Path::new(&format!("/{}", s(&dir.join("VST2/Serum_x64.dll")))), "VST2", &dir.join("VST3/Serum.vst3"));
    let why = remove_one(&copy, &folder_bin(&dir)).unwrap();
    assert_eq!(why, "It's on a network drive, which has no Recycle Bin.");
    assert!(dir.join("VST2/Serum_x64.dll").exists());
}

#[test]
fn one_file_picked_by_two_paths_goes_once() {
    let dir = fixtures("mo-space-spare-two-paths");
    write(&dir, "VST3/Serum.vst3", 10);
    write(&dir, "VST2/Serum_x64.dll", 10);
    folder_link(&dir.join("VST2"), &dir.join("Old VST2"));
    let used = dir.join("VST3/Serum.vst3");
    let first = spare(&dir.join("VST2/Serum_x64.dll"), "VST2", &used);
    let again = spare(&dir.join("Old VST2/Serum_x64.dll"), "VST2", &used);
    let picked = [first.clone(), again.clone(), first.clone()];
    let done = remove(&picked, &picked, &folder_bin(&dir), &AtomicBool::new(false));
    assert_eq!(done.removed.len(), 1);
    assert_eq!(done.bytes(), 10);
    assert_eq!(done.problems.len(), 1);
    assert!(done.problems[0].why.starts_with("It's the same file as "), "{}", done.problems[0].why);
    assert_eq!(done.problems[0].path, again.path);
}

#[test]
fn a_good_second_path_still_goes_when_the_first_stays() {
    let dir = fixtures("mo-space-spare-second-path");
    write(&dir, "VST3/Serum.vst3", 10);
    write(&dir, "VST2/Serum_x64.dll", 10);
    folder_link(&dir.join("VST2"), &dir.join("Old VST2"));
    let used = dir.join("VST3/Serum.vst3");
    let old = spare(&dir.join("Old VST2/Serum_x64.dll"), "VST2", &used);
    let good = spare(&dir.join("VST2/Serum_x64.dll"), "VST2", &used);
    // Found again just now, only the second path is listed: the first stays, as it isn't spare now.
    let done =
        remove(&[old.clone(), good.clone()], std::slice::from_ref(&good), &folder_bin(&dir), &AtomicBool::new(false));
    assert_eq!(done.removed.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(), [good.path.as_str()]);
    assert_eq!(done.problems.len(), 1);
    assert_eq!(done.problems[0].path, old.path);
    assert!(done.problems[0].why.contains("isn't spare any more"), "{}", done.problems[0].why);
}
