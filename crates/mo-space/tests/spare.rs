//! Removing spare plugin copies, on files made in the test build's own fixtures folder under fixed names. The bin is
//! a folder there too, never the computer's Recycle Bin.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use mo_space::clean::{Bin, Problem, Removed, What, undo};
use mo_space::key;
use mo_space::plugin_space::SpaceKind;
use mo_space::spare::{SpareCopy, Why, remove};

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

fn spare(path: &Path, format: &str, bytes: u64) -> SpareCopy {
    SpareCopy {
        name: "Serum".to_owned(),
        format: format.to_owned(),
        path: s(path),
        bytes,
        why: Why::OtherFormatsUsed { used: vec!["VST3".to_owned()], projects: 3 },
    }
}

#[test]
fn a_spare_copy_goes_only_while_its_still_spare_and_as_it_was_found() {
    let dir = fixtures("mo-space-spare-remove");
    let plugins = dir.join("Plugins");
    write(&plugins, "VST2/Serum_x64.dll", 100);
    write(&plugins, "CLAP/Serum.clap", 50);
    write(&plugins, "AAX/Serum.aaxplugin/Contents/x64/Serum.aaxplugin", 70);
    write(&plugins, "VST2/Massive.dll", 30);
    write(&plugins, "VST2/Notes.txt", 5);

    let vst2 = spare(&plugins.join("VST2/Serum_x64.dll"), "VST2", 100);
    let clap = spare(&plugins.join("CLAP/Serum.clap"), "CLAP", 50);
    let aax = SpareCopy { why: Why::NoProTools, ..spare(&plugins.join("AAX/Serum.aaxplugin"), "AAX", 70) };
    // Found when it was 20 bytes: an update since.
    let massive = spare(&plugins.join("VST2/Massive.dll"), "VST2", 20);
    // Not a plugin file.
    let notes = spare(&plugins.join("VST2/Notes.txt"), "VST2", 5);
    let picked = vec![vst2.clone(), clap.clone(), aax.clone(), massive.clone(), notes.clone(), vst2.clone()];
    // A song saved since uses Serum's CLAP: found again, it isn't spare now.
    let now = vec![vst2.clone(), aax.clone(), massive.clone(), notes.clone()];

    let bin = FolderBin { dir: dir.join("bin"), held: RefCell::new(Vec::new()) };
    let done = remove(&picked, &now, &bin, &AtomicBool::new(false));
    let gone: Vec<&str> = done.removed.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(gone, [s(&plugins.join("VST2/Serum_x64.dll")), s(&plugins.join("AAX/Serum.aaxplugin"))]);
    assert!(done.removed.iter().all(|r| r.what == What::Plugin(SpaceKind::Program) && r.bin_id.is_some()));
    assert_eq!(done.bytes(), 170);
    let why = |p: &SpareCopy| done.problems.iter().find(|x| x.path == p.path).map(|x| x.why.clone()).unwrap();
    assert!(why(&clap).contains("isn't spare now"), "{}", why(&clap));
    assert!(why(&massive).contains("changed since it was found"), "{}", why(&massive));
    assert!(why(&notes).contains("isn't a VST2 plugin"), "{}", why(&notes));
    assert!(plugins.join("CLAP/Serum.clap").exists() && plugins.join("VST2/Massive.dll").exists());

    // Undo puts them back.
    assert!(undo(&done, &bin).is_empty());
    assert!(plugins.join("VST2/Serum_x64.dll").exists());
    assert!(plugins.join("AAX/Serum.aaxplugin/Contents/x64/Serum.aaxplugin").exists());
}

#[test]
fn a_stop_keeps_the_rest() {
    let dir = fixtures("mo-space-spare-stop");
    write(&dir, "VST2/Serum_x64.dll", 10);
    let copy = spare(&dir.join("VST2/Serum_x64.dll"), "VST2", 10);
    let bin = FolderBin { dir: dir.join("bin"), held: RefCell::new(Vec::new()) };
    let done = remove(std::slice::from_ref(&copy), std::slice::from_ref(&copy), &bin, &AtomicBool::new(true));
    assert!(done.removed.is_empty());
    assert_eq!(done.problems[0].why, "Stopped before it went.");
}

#[cfg(unix)]
#[test]
fn a_copy_thats_a_link_now_stays() {
    let dir = fixtures("mo-space-spare-link");
    write(&dir, "Elsewhere/Serum_x64.dll", 10);
    fs::create_dir_all(dir.join("VST2")).unwrap();
    std::os::unix::fs::symlink(dir.join("Elsewhere/Serum_x64.dll"), dir.join("VST2/Serum_x64.dll")).unwrap();
    let copy = spare(&dir.join("VST2/Serum_x64.dll"), "VST2", 10);
    let bin = FolderBin { dir: dir.join("bin"), held: RefCell::new(Vec::new()) };
    let done = remove(std::slice::from_ref(&copy), std::slice::from_ref(&copy), &bin, &AtomicBool::new(false));
    assert!(done.removed.is_empty());
    assert_eq!(done.problems[0].why, "It's a link now, so it stays.");
    assert!(dir.join("Elsewhere/Serum_x64.dll").exists());
}
