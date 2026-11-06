//! What every clean-up's sending does, whichever clean-up it's from, with everything Windows guards: links never go,
//! one thing reached two ways goes once, and what waits for the administrator step is checked again before Windows
//! asks; and how what went as administrator comes back, all of it in one more step, only as it went. On folders made
//! in the test build's own fixtures folder under fixed names; the bins are folders there too, never the computer's
//! Recycle Bin. Links are junctions on Windows, as the app makes.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use mo_space::admin::{Admin, NOT_AS_IT_WENT, PUT_BACK, run_step_with, write_list};
use mo_space::after_move::{OldHome, remove_old_home};
use mo_space::clean::{
    BACK_NEEDS_ADMIN, Bin, Check, Done, GONE_FROM_BIN, Guard, NAME_TAKEN, NEEDS_ADMIN, Problem, Refused, Removed,
    apply, preview, undo,
};
use mo_space::key;
use mo_space::old_copies::{self, remind};
use mo_space::song::{CompactItem, CompactKind};

mod common;

use common::{folder_link, is_link, kept_at, unlink};

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

/// What went to the bin (where it was, where it is now), seen by both the app and the step.
type Held = Arc<Mutex<Vec<(String, PathBuf)>>>;

/// The bin the administrator step sends to and puts back from: a folder.
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
    fn put_back(&self, removed: &[Removed]) -> Vec<Problem> {
        let mut problems = Vec::new();
        self.put_back_checked(removed, &|_, _| Ok(()), &mut |n, why| {
            problems.extend(why.map(|why| Problem { path: removed[n].path.clone(), why }));
        });
        problems
    }
    fn put_back_checked(&self, removed: &[Removed], check: &Check<'_>, each: &mut dyn FnMut(usize, Option<String>)) {
        for (n, r) in removed.iter().enumerate() {
            let back = match kept_at(r) {
                None => Err(GONE_FROM_BIN.to_owned()),
                Some(at) => check(r, &at).and_then(|()| {
                    if fs::symlink_metadata(&r.real).is_ok() {
                        return Err(NAME_TAKEN.to_owned());
                    }
                    fs::rename(at, &r.real).map_err(|e| e.to_string())
                }),
            };
            each(n, back.err());
        }
    }
}

/// Windows' administrator question: the step runs here, in this process, with the step's bin. `first` runs the first
/// time Windows asks, before the step starts, and never again. You say no while `no` is set.
struct AsAdmin {
    step: StepBin,
    asked: AtomicUsize,
    first: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    no: AtomicBool,
}

impl Admin for AsAdmin {
    fn can_ask(&self) -> bool {
        true
    }
    fn ask(&self, args: &[String]) -> Result<(), String> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        if self.no.load(Ordering::SeqCst) {
            return Err("You said no to Windows' administrator question.".to_owned());
        }
        let first = self.first.lock().unwrap().take();
        if let Some(first) = first {
            first();
        }
        assert!(run_step_with(args, &self.step), "the step didn't know {args:?}");
        Ok(())
    }
}

/// The app's bin: Windows guards everything, so it all waits for the administrator step, and only that step can put
/// anything back. `meanwhile` runs as it's asked, standing in for what changes while the rest of the clean-up runs.
struct GuardedBin {
    admin: AsAdmin,
    meanwhile: Box<dyn Fn(&Path)>,
    /// What it was told of each thing on its way ([`Bin::going`]), with how many times Windows had asked by then.
    told: Mutex<Vec<(Removed, usize)>>,
}

impl GuardedBin {
    fn new(dir: &Path, meanwhile: impl Fn(&Path) + 'static) -> Self {
        let step = StepBin { dir: dir.join("bin"), held: Held::default() };
        let admin = AsAdmin { step, asked: AtomicUsize::new(0), first: Mutex::new(None), no: AtomicBool::new(false) };
        GuardedBin { admin, meanwhile: Box::new(meanwhile), told: Mutex::default() }
    }

    /// What changes the first time Windows asks, before the step starts.
    fn once_asked(mut self, first: impl FnOnce() + Send + 'static) -> Self {
        self.admin.first = Mutex::new(Some(Box::new(first)));
        self
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
            r.bin_id = held.iter().rev().find(|(p, _)| key(p) == key(&r.real)).map(|(_, at)| s(at));
        }
        Vec::new()
    }
    fn put_back(&self, removed: &[Removed]) -> Vec<Problem> {
        removed.iter().map(|r| Problem { path: r.path.clone(), why: BACK_NEEDS_ADMIN.to_owned() }).collect()
    }
    fn holds(&self, removed: &[Removed]) -> Option<Vec<bool>> {
        Some(removed.iter().map(|r| kept_at(r).is_some()).collect())
    }
    fn going(&self, removed: &Removed) {
        self.told.lock().unwrap().push((removed.clone(), self.asked()));
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
fn what_waits_for_windows_stays_when_a_folder_on_the_way_leads_elsewhere_now() {
    let dir = fixtures("mo-space-sending-folder-swapped");
    let (d, e) = (dir.join("D"), dir.join("E"));
    write(&d, "Data/Plugin Data/STEAM/Pad.db", "pad");
    write(&e, "Plugin Data/STEAM/Pad.db", "pad");
    let home = d.join("Data/Plugin Data");
    let old = OldHome {
        path: s(&home),
        bytes: 3,
        note: "its old place".to_owned(),
        copy: s(&e.join("Plugin Data")),
        kind: 2,
    };
    // While it waits, the folder it's in is put aside and a link to that takes its place: the same files, but not
    // where it was really.
    let bin = GuardedBin::new(&dir, |path| {
        let folder = path.parent().unwrap();
        let aside = folder.with_file_name("Data (aside)");
        fs::rename(folder, &aside).unwrap();
        folder_link(&aside, folder);
    });
    let done = remove_old_home(&old, &[], true, &bin, &AtomicBool::new(false));
    assert!(done.removed.is_empty());
    assert_eq!(done.problems.len(), 1);
    assert!(done.problems[0].why.contains("leads somewhere else now"), "{}", done.problems[0].why);
    assert_eq!(bin.asked(), 0);
    assert_eq!(fs::read_to_string(d.join("Data (aside)/Plugin Data/STEAM/Pad.db")).unwrap(), "pad");
}

#[test]
fn the_step_leaves_alone_what_a_link_on_the_way_now_leads_to_even_when_it_holds_the_same() {
    let dir = fixtures("mo-space-sending-swapped-at-the-step");
    let (d, e) = (dir.join("D"), dir.join("E"));
    write(&d, "Data/Plugin Data/STEAM/Pad.db", "pad");
    write(&e, "Plugin Data/STEAM/Pad.db", "pad");
    let home = d.join("Data/Plugin Data");
    let old = OldHome {
        path: s(&home),
        bytes: 3,
        note: "its old place".to_owned(),
        copy: s(&e.join("Plugin Data")),
        kind: 2,
    };
    // Once Windows has asked, the folder it's in is put aside and a link to it takes its place: the same files, so
    // what the app checked looks the same, but it isn't where the app checked it.
    let data = d.join("Data");
    let bin = GuardedBin::new(&dir, |_| {}).once_asked(move || {
        let aside = data.with_file_name("Data (aside)");
        fs::rename(&data, &aside).unwrap();
        folder_link(&aside, &data);
    });
    let done = remove_old_home(&old, &[], true, &bin, &AtomicBool::new(false));
    assert_eq!(bin.asked(), 1);
    assert!(done.removed.is_empty());
    assert_eq!(done.problems.len(), 1);
    assert!(done.problems[0].why.contains("reached through a link"), "{}", done.problems[0].why);
    assert_eq!(fs::read_to_string(d.join("Data (aside)/Plugin Data/STEAM/Pad.db")).unwrap(), "pad");
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

/// Three songs' caches, gone to the bin as administrator (Windows guards them all here) in one step: what Put back
/// puts back. Each cache's folder, in the same order as the songs.
fn three_caches_gone(dir: &Path) -> (GuardedBin, Done, Vec<PathBuf>) {
    let projects = dir.join("Projects");
    let caches: Vec<PathBuf> = ["A", "B", "C"]
        .iter()
        .map(|song| {
            write(&projects, &format!("{song}/{song}.song"), "song");
            write(&projects, &format!("{song}/Cache/a.peak"), "peak");
            projects.join(song).join("Cache")
        })
        .collect();
    let roots = vec![s(&projects)];
    let keep = HashSet::new();
    let no_sample = |_: &str| false;
    let guard = Guard { project_roots: &roots, sample_folders: &[], keep: &keep, sample_in_use: &no_sample };
    let lines: Vec<CompactItem> = caches
        .iter()
        .map(|c| CompactItem { kind: CompactKind::Cache, path: s(c), bytes: 4, modified: None, same_as: None })
        .collect();
    let bin = GuardedBin::new(dir, |_| {});
    let done = apply(&preview(&lines, &guard), &guard, &bin, &AtomicBool::new(false));
    assert!(done.problems.is_empty(), "{:?}", done.problems);
    assert_eq!(done.removed.len(), 3);
    assert!(done.removed.iter().all(|r| r.stamp.is_some() && kept_at(r).is_some()), "{:?}", done.removed);
    assert_eq!(bin.asked(), 1);
    (bin, done, caches)
}

#[test]
fn what_waits_for_windows_is_told_before_it_asks_as_it_will_be_kept() {
    // The step goes on once you say yes, even if the app closes meanwhile: what it may bin is told first, so the app
    // can keep it for Put back; what the step leaves, the bin tells apart later.
    let dir = fixtures("mo-space-sending-told");
    let (bin, done, _) = three_caches_gone(&dir);
    let told = bin.told.lock().unwrap();
    assert_eq!(told.len(), 3);
    assert!(told.iter().all(|(_, asked)| *asked == 0), "told after Windows asked");
    // As it's kept once it went: the same time and the stamp the step checks. Only what the bin calls it comes later.
    let told: Vec<Removed> = told.iter().map(|(r, _)| r.clone()).collect();
    let unfound: Vec<Removed> = done.removed.iter().map(|r| Removed { bin_id: None, ..r.clone() }).collect();
    assert_eq!(told, unfound);
    // Each is told once, whether or not the step bins it.
    let left = fixtures("mo-space-sending-told-no");
    let projects = left.join("Projects");
    write(&projects, "A/A.song", "song");
    write(&projects, "A/Cache/a.peak", "peak");
    let roots = vec![s(&projects)];
    let keep = HashSet::new();
    let no_sample = |_: &str| false;
    let guard = Guard { project_roots: &roots, sample_folders: &[], keep: &keep, sample_in_use: &no_sample };
    let cache = CompactItem {
        kind: CompactKind::Cache,
        path: s(&projects.join("A/Cache")),
        bytes: 4,
        modified: None,
        same_as: None,
    };
    let bin = GuardedBin::new(&left, |_| {});
    bin.admin.no.store(true, Ordering::SeqCst);
    let done = apply(&preview(&[cache], &guard), &guard, &bin, &AtomicBool::new(false));
    assert!(done.removed.is_empty());
    assert_eq!(bin.told.lock().unwrap().len(), 1);
    assert!(projects.join("A/Cache/a.peak").exists());
}

/// What went from `path`.
fn gone<'a>(done: &'a Done, path: &Path) -> &'a Removed {
    done.removed.iter().find(|r| r.path == s(path)).unwrap()
}

/// Why what went from `path` didn't come back, or `None` when it did.
fn why_not<'a>(problems: &'a [Problem], path: &Path) -> Option<&'a str> {
    problems.iter().find(|p| p.path == s(path)).map(|p| p.why.as_str())
}

#[test]
fn what_went_as_administrator_comes_back_in_one_step_and_only_as_it_went() {
    let dir = fixtures("mo-space-sending-put-back");
    let (bin, done, caches) = three_caches_gone(&dir);
    // In the bin, something is added to B's cache: it isn't what went any more.
    let b_in_bin = kept_at(gone(&done, &caches[1])).unwrap();
    write(&b_in_bin, "planted.dll", "not what went");
    // Something new has C's cache's name.
    write(&caches[2], "new.peak", "new");

    let problems = undo(&done, &bin);
    // Windows is asked once more, for all three.
    assert_eq!(bin.asked(), 2);
    assert_eq!(problems.len(), 2, "{problems:?}");
    assert_eq!(why_not(&problems, &caches[0]), None);
    assert_eq!(fs::read_to_string(caches[0].join("a.peak")).unwrap(), "peak");
    assert_eq!(why_not(&problems, &caches[1]), Some(NOT_AS_IT_WENT));
    assert!(!caches[1].exists() && b_in_bin.join("planted.dll").exists());
    assert_eq!(why_not(&problems, &caches[2]), Some(NAME_TAKEN));
    assert_eq!(fs::read_to_string(caches[2].join("new.peak")).unwrap(), "new");
    assert!(!caches[2].join("a.peak").exists() && kept_at(gone(&done, &caches[2])).is_some());
}

#[test]
fn what_went_as_administrator_stays_in_the_bin_when_you_say_no_or_the_app_never_noted_what_it_held() {
    let dir = fixtures("mo-space-sending-put-back-no");
    let (bin, done, caches) = three_caches_gone(&dir);
    // You say no: all of it stays in the bin, and each says why.
    bin.admin.no.store(true, Ordering::SeqCst);
    let problems = undo(&done, &bin);
    assert_eq!(bin.asked(), 2);
    assert_eq!(problems.len(), 3);
    assert!(problems.iter().all(|p| p.why == "You said no to Windows' administrator question."), "{problems:?}");
    assert!(caches.iter().all(|c| !c.exists()));
    assert!(done.removed.iter().all(|r| kept_at(r).is_some()));

    // What the app kept of them before it noted what each held: they come back as you, which Windows doesn't allow
    // where it guards, and Windows isn't asked.
    bin.admin.no.store(false, Ordering::SeqCst);
    let before = Done { removed: done.removed.iter().map(|r| Removed { stamp: None, ..r.clone() }).collect(), ..done };
    let problems = undo(&before, &bin);
    assert_eq!(bin.asked(), 2);
    assert_eq!(problems.len(), 3);
    assert!(problems.iter().all(|p| p.why == BACK_NEEDS_ADMIN), "{problems:?}");
    assert!(caches.iter().all(|c| !c.exists()));
}

#[test]
fn windows_isnt_asked_when_nothing_can_come_back() {
    let dir = fixtures("mo-space-sending-put-back-nothing");
    let (bin, done, caches) = three_caches_gone(&dir);
    // You emptied the bin of A's and B's, and something new has C's name.
    for cache in &caches[..2] {
        fs::remove_dir_all(kept_at(gone(&done, cache)).unwrap()).unwrap();
    }
    write(&caches[2], "new.peak", "new");
    let problems = undo(&done, &bin);
    assert_eq!(bin.asked(), 1);
    assert_eq!(why_not(&problems, &caches[0]), Some(GONE_FROM_BIN));
    assert_eq!(why_not(&problems, &caches[1]), Some(GONE_FROM_BIN));
    assert_eq!(why_not(&problems, &caches[2]), Some(NAME_TAKEN));
}

#[test]
fn what_the_step_leaves_in_the_bin_isnt_counted_back_whatever_is_there_now() {
    let dir = fixtures("mo-space-sending-put-back-meanwhile");
    let (bin, done, caches) = three_caches_gone(&dir);
    // While Windows asks, you empty the bin of A's and something new takes its place (the plugin installed again).
    let (a, a_in_bin) = (caches[0].clone(), kept_at(gone(&done, &caches[0])).unwrap());
    *bin.admin.first.lock().unwrap() = Some(Box::new(move || {
        fs::remove_dir_all(&a_in_bin).unwrap();
        write(&a, "new.peak", "new");
    }));
    let problems = undo(&done, &bin);
    assert_eq!(bin.asked(), 2);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert_eq!(why_not(&problems, &caches[0]), Some(NAME_TAKEN));
    assert_eq!(fs::read_to_string(caches[0].join("new.peak")).unwrap(), "new");
    assert!(caches[1].join("a.peak").exists() && caches[2].join("a.peak").exists());
}

#[test]
fn the_put_back_step_checks_each_line_again_before_it_happens() {
    let dir = fixtures("mo-space-sending-put-back-checks");
    let (bin, done, caches) = three_caches_gone(&dir);
    let runs = AtomicUsize::new(0);
    // One run of the step, as the app starts it, with these lines: what it said.
    let step = |rows: &[[String; 2]]| {
        let n = runs.fetch_add(1, Ordering::SeqCst);
        let (list, result) = (dir.join(format!("list {n}.txt")), dir.join(format!("result {n}.txt")));
        let fingerprint = write_list(&list, &rows.iter().map(|r| r.join("\t")).collect::<Vec<_>>()).unwrap();
        let args = vec![PUT_BACK.to_owned(), s(&list), s(&result), fingerprint];
        assert!(run_step_with(&args, &bin.admin.step));
        fs::read_to_string(&result).unwrap()
    };
    let line = |real: &Path, r: &Removed| [s(real), serde_json::to_string(r).unwrap()];
    let not_as_written = |said: &str| said.ends_with("\tkept\tThe list wasn't as the app writes it.\n");
    let a = gone(&done, &caches[0]);

    // Where it would go back to isn't where it went from.
    let said = step(&[line(&caches[1], a)]);
    assert!(not_as_written(&said), "{said}");
    // What the app keeps of it doesn't say what it held, or can't be read.
    let said = step(&[line(&caches[0], &Removed { stamp: None, ..a.clone() })]);
    assert!(not_as_written(&said), "{said}");
    let said = step(&[[s(&caches[0]), "{".to_owned()]]);
    assert!(not_as_written(&said), "{said}");
    // The folder it goes back into is reached through a link now.
    let song = caches[0].parent().unwrap();
    let aside = song.with_file_name("A (aside)");
    fs::rename(song, &aside).unwrap();
    folder_link(&aside, song);
    let said = step(&[line(&caches[0], a)]);
    assert!(said.contains("\tkept\t") && said.contains("reached through a link"), "{said}");
    unlink(song);
    fs::rename(&aside, song).unwrap();
    // The bin doesn't hold it any more (you emptied it).
    let c = gone(&done, &caches[2]);
    fs::remove_dir_all(kept_at(c).unwrap()).unwrap();
    let said = step(&[line(&caches[2], c)]);
    assert_eq!(said, format!("{}\tkept\t{GONE_FROM_BIN}\n", s(&caches[2])));
    assert!(caches.iter().all(|c| !c.exists()));

    // As the app writes them, both still in the bin come back, in one run.
    let said = step(&[line(&caches[0], a), line(&caches[1], gone(&done, &caches[1]))]);
    assert_eq!(said, format!("{}\tdone\t\n{}\tdone\t\n", s(&caches[0]), s(&caches[1])));
    assert!(caches[0].join("a.peak").exists() && caches[1].join("a.peak").exists());
}

#[test]
fn old_copies_windows_guards_go_in_one_step_and_come_back_in_one_more() {
    let dir = fixtures("mo-space-sending-old-copies");
    let (c, d) = (dir.join("C"), dir.join("D"));
    // Two folders left behind whose files are on the other drive now: Windows guards both.
    let homes: Vec<OldHome> = ["STEAM", "Kontakt"]
        .iter()
        .map(|name| {
            write(&c, &format!("{name} (after update)/a.db"), "a");
            write(&d, &format!("{name}/a.db"), "a");
            OldHome {
                path: s(&c.join(format!("{name} (after update)"))),
                bytes: 1,
                note: "left by an update".to_owned(),
                copy: s(&d.join(name)),
                kind: 2,
            }
        })
        .collect();
    let stop = AtomicBool::new(false);
    let look = remind(&[], &homes, &[], &stop).unwrap();
    assert!(look.leftovers.iter().all(|l| l.can_go()), "{:?}", look.leftovers);
    let bin = GuardedBin::new(&dir, |_| {});

    let done = old_copies::remove(&look.leftovers, (&[], &homes), (&[], &homes), &bin, &stop);
    assert!(done.problems.is_empty(), "{:?}", done.problems);
    assert_eq!(done.removed.len(), 2);
    assert!(done.removed.iter().all(|r| r.stamp.is_some() && kept_at(r).is_some()), "{:?}", done.removed);
    assert_eq!(bin.asked(), 1);
    assert!(homes.iter().all(|h| !Path::new(&h.path).exists()));
    // Put back asks once more, for both.
    assert!(undo(&done, &bin).is_empty());
    assert_eq!(bin.asked(), 2);
    assert!(homes.iter().all(|h| Path::new(&h.path).join("a.db").exists()));
}
