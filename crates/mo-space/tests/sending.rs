//! What every clean-up's sending does, whichever clean-up it's from, with everything Windows guards: links never go,
//! one thing reached two ways goes once, and what waits for the administrator step is checked again before Windows
//! asks. On folders made in the test build's own fixtures folder under fixed names; the bins are folders there too,
//! never the computer's Recycle Bin. Links are junctions on Windows, as the app makes.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use mo_space::admin::{Admin, run_step_with};
use mo_space::after_move::{OldHome, remove_old_home};
use mo_space::clean::{Bin, Guard, NEEDS_ADMIN, Problem, Refused, Removed, apply, preview};
use mo_space::key;
use mo_space::song::{CompactItem, CompactKind};

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

fn folder_link(target: &Path, at: &Path) {
    #[cfg(windows)]
    junction::create(target, at).unwrap();
    #[cfg(not(windows))]
    std::os::unix::fs::symlink(target, at).unwrap();
}

fn is_link(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink())
}

/// What went to the bin (where it was, where it is now), seen by both the app and the step.
type Held = Arc<Mutex<Vec<(String, PathBuf)>>>;

/// The bin the administrator step sends to: a folder.
struct StepBin {
    dir: PathBuf,
    held: Held,
}

impl Bin for StepBin {
    fn send(&self, path: &Path) -> Result<(), String> {
        let mut held = self.held.lock().unwrap();
        fs::create_dir_all(&self.dir).unwrap();
        let to = self.dir.join(held.len().to_string());
        fs::rename(path, &to).map_err(|e| e.to_string())?;
        held.push((s(path), to));
        Ok(())
    }
    fn find(&self, _removed: &mut [Removed]) -> Vec<Problem> {
        Vec::new()
    }
    fn put_back(&self, _removed: &[Removed]) -> Vec<Problem> {
        Vec::new()
    }
}

/// Windows' administrator question: the step runs here, in this process, with the step's bin.
struct AsAdmin {
    step: StepBin,
    asked: AtomicUsize,
}

impl Admin for AsAdmin {
    fn can_ask(&self) -> bool {
        true
    }
    fn ask(&self, args: &[String]) -> Result<(), String> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        assert!(run_step_with(args, &self.step), "the step didn't know {args:?}");
        Ok(())
    }
}

/// The app's bin: Windows guards everything, so it all waits for the administrator step. `meanwhile` runs as it's
/// asked, standing in for what changes while the rest of the clean-up runs.
struct GuardedBin {
    admin: AsAdmin,
    meanwhile: Box<dyn Fn(&Path)>,
}

impl GuardedBin {
    fn new(dir: &Path, meanwhile: impl Fn(&Path) + 'static) -> Self {
        let step = StepBin { dir: dir.join("bin"), held: Held::default() };
        GuardedBin { admin: AsAdmin { step, asked: AtomicUsize::new(0) }, meanwhile: Box::new(meanwhile) }
    }

    fn asked(&self) -> usize {
        self.admin.asked.load(Ordering::SeqCst)
    }
}

impl Bin for GuardedBin {
    fn admin(&self) -> &dyn Admin {
        &self.admin
    }
    fn send(&self, path: &Path) -> Result<(), String> {
        (self.meanwhile)(path);
        Err(NEEDS_ADMIN.to_owned())
    }
    fn find(&self, removed: &mut [Removed]) -> Vec<Problem> {
        let held = self.admin.step.held.lock().unwrap();
        for r in removed {
            r.bin_id = held.iter().find(|(p, _)| key(p) == key(&r.real)).map(|(_, at)| s(at));
        }
        Vec::new()
    }
    fn put_back(&self, _removed: &[Removed]) -> Vec<Problem> {
        Vec::new()
    }
}

#[test]
fn what_turns_into_a_link_while_it_waits_for_windows_isnt_asked_about() {
    let dir = fixtures("mo-space-sending-link");
    let (d, e) = (dir.join("D"), dir.join("E"));
    write(&d, "Plugin Data/STEAM/Pad.db", "pad");
    write(&e, "Plugin Data/STEAM/Pad.db", "pad");
    let home = d.join("Plugin Data");
    let old = OldHome {
        path: s(&home),
        bytes: 3,
        note: "its old place".to_owned(),
        copy: s(&e.join("Plugin Data")),
        kind: 2,
    };
    // While it waits, the folder is put aside and a link to it takes its place.
    let bin = GuardedBin::new(&dir, |path| {
        let aside = path.with_file_name("Plugin Data (aside)");
        fs::rename(path, &aside).unwrap();
        folder_link(&aside, path);
    });
    let done = remove_old_home(&old, &[], true, &bin, &AtomicBool::new(false));
    assert!(done.removed.is_empty());
    assert_eq!(done.problems, [Problem { path: s(&home), why: Refused::Link.text().to_owned() }]);
    // Windows wasn't asked, and the folder the link leads to is whole.
    assert_eq!(bin.asked(), 0);
    assert!(is_link(&home));
    assert_eq!(fs::read_to_string(d.join("Plugin Data (aside)/STEAM/Pad.db")).unwrap(), "pad");
}

#[test]
fn one_folder_reached_two_ways_goes_once_as_administrator() {
    let dir = fixtures("mo-space-sending-twice");
    let projects = dir.join("Projects");
    write(&projects, "Song/Song.song", "song");
    write(&projects, "Song/Cache/a.peak", "peak");
    // A link inside your project folder, to a song in it: the same Cache folder, spelt another way.
    folder_link(&projects.join("Song"), &projects.join("Song (link)"));
    let roots = vec![s(&projects)];
    let keep = HashSet::new();
    let no_sample = |_: &str| false;
    let guard = Guard { project_roots: &roots, sample_folders: &[], keep: &keep, sample_in_use: &no_sample };
    let cache = |song: &str| CompactItem {
        kind: CompactKind::Cache,
        path: s(&projects.join(song).join("Cache")),
        bytes: 4,
        modified: None,
        same_as: None,
    };
    let plan = preview(&[cache("Song"), cache("Song (link)")], &guard);
    assert_eq!(plan.lines.len(), 2);

    let bin = GuardedBin::new(&dir, |_| {});
    let done = apply(&plan, &guard, &bin, &AtomicBool::new(false));
    assert_eq!(bin.asked(), 1);
    assert_eq!(done.removed.len(), 1);
    assert_eq!(done.bytes(), 4);
    assert_eq!(done.problems.len(), 1);
    assert_eq!(done.problems[0].path, cache("Song (link)").path);
    assert!(done.problems[0].why.contains("reached another way, so it's only sent once"), "{}", done.problems[0].why);
    assert!(!projects.join("Song/Cache").exists());
}

#[test]
fn a_stop_while_things_wait_for_windows_keeps_them() {
    let dir = fixtures("mo-space-sending-stop");
    let (d, e) = (dir.join("D"), dir.join("E"));
    write(&d, "Plugin Data/STEAM/Pad.db", "pad");
    write(&e, "Plugin Data/STEAM/Pad.db", "pad");
    let home = d.join("Plugin Data");
    let old = OldHome {
        path: s(&home),
        bytes: 3,
        note: "its old place".to_owned(),
        copy: s(&e.join("Plugin Data")),
        kind: 2,
    };
    // Stop is pressed while it waits.
    let stop = Arc::new(AtomicBool::new(false));
    let pressed = Arc::clone(&stop);
    let bin = GuardedBin::new(&dir, move |_| pressed.store(true, Ordering::Relaxed));
    let done = remove_old_home(&old, &[], true, &bin, &stop);
    assert!(done.removed.is_empty());
    assert_eq!(done.problems, [Problem { path: s(&home), why: "Stopped before it went.".to_owned() }]);
    assert_eq!(bin.asked(), 0);
    assert!(home.join("STEAM/Pad.db").exists());
}
