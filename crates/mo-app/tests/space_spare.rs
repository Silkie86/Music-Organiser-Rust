//! The Space page's spare plugin copies, from start to end: the look, then Remove and Put back, on plugin folders made
//! in the test build's own fixtures folder under fixed names: never the computer's real plugin folders, nor its
//! Recycle Bin (a folder there stands in for it). The plugin files are made-up bytes.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use mo_app::binned;
use mo_app::space_bin::{self, PutBacks};
use mo_app::space_spare::{Found, Removing, SPARE_COPIES, look, plugin_folders, remove};
use mo_space::clean::{Bin, Problem, Removed, undo};
use mo_space::spare::{Used, Why};

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

/// What a look found, as file names, formats and sizes.
fn summary(found: &Found) -> Vec<(String, String, u64)> {
    let name = |p: &str| Path::new(p).file_name().unwrap().to_string_lossy().into_owned();
    found.copies.iter().map(|c| (name(&c.path), c.format.clone(), c.bytes)).collect()
}

#[test]
fn copies_in_formats_your_saves_dont_use_are_found_and_measured() {
    let dir = fixtures("mo-app-space-spare");
    let (vst3, vst2, avid) = (dir.join("VST3"), dir.join("VSTPlugins"), dir.join("Avid Plug-Ins"));
    // Serum in every format: the saves use its VST3.
    write(&vst3, "Serum.vst3/Contents/x86_64-win/Serum.vst3", 300);
    write(&vst2, "Serum_x64.dll", 120);
    write(&avid, "Serum.aaxplugin/Contents/x64/Serum.aaxplugin", 200);
    write(&avid, "Serum.aaxplugin/Contents/Resources/skin.png", 50);
    // Diva: the saves use its VST2, so its VST3 is the spare one.
    write(&vst3, "Diva.vst3", 80);
    write(&vst2, "Diva(x64).dll", 70);
    // Sylenth1: an old FL Studio save says only "VST", so every copy of it counts as used; and no save uses Vital, so
    // the Plugins page's Never used shows it whole and none of it is a spare copy.
    write(&vst3, "Sylenth1.vst3", 40);
    write(&vst2, "Sylenth1.dll", 30);
    write(&vst3, "Vital.vst3", 20);
    write(&vst2, "Vital.dll", 10);
    // The saves (by their ids in the library) that use each format.
    let saves = |ids: std::ops::Range<i64>| ids.collect::<HashSet<i64>>();
    let used: Used = HashMap::from([
        ("Serum".to_owned(), HashMap::from([("VST3".to_owned(), saves(0..412))])),
        ("Diva".to_owned(), HashMap::from([("VST2".to_owned(), saves(412..419))])),
        ("Sylenth1".to_owned(), HashMap::from([("VST".to_owned(), saves(419..422))])),
    ]);
    let folders = plugin_folders(&[], vec![vst3.clone(), vst2.clone()], vec![avid.clone()]);
    let stop = AtomicBool::new(false);

    // Without Pro Tools, Serum's AAX copy is spare too, measured whole (every file in its bundle).
    let found = look(&folders, &used, false, &stop).unwrap();
    assert_eq!(
        summary(&found),
        [
            ("Serum.aaxplugin".to_owned(), "AAX".to_owned(), 250),
            ("Serum_x64.dll".to_owned(), "VST2".to_owned(), 120),
            ("Diva.vst3".to_owned(), "VST3".to_owned(), 80),
        ]
    );
    assert!(found.copies.iter().all(|c| c.can_remove()));
    assert_eq!(found.bytes(), 450);
    assert_eq!(found.copies[0].why, Why::NoProTools);
    match &found.copies[1].why {
        Why::OtherFormatsUsed { used, projects, .. } => {
            assert_eq!((used.as_slice(), *projects), (&["VST3".to_owned()][..], 412))
        }
        other => panic!("Serum's VST2 is spare because of its VST3, not {other:?}"),
    }

    // With Pro Tools, AAX copies are in use.
    let found = look(&folders, &used, true, &stop).unwrap();
    assert_eq!(summary(&found).iter().map(|s| s.0.as_str()).collect::<Vec<_>>(), ["Serum_x64.dll", "Diva.vst3"]);

    // Your own plugin folders, when Settings lists any, are looked in instead of the standard ones.
    let only_vst2 = plugin_folders(&[vst2.to_string_lossy().into_owned()], vec![vst3], vec![avid]);
    let found = look(&only_vst2, &used, false, &stop).unwrap();
    // With Serum's VST3 not looked at, its VST2 may be the copy the saves load: it stays. So does Diva's VST2, which
    // they use; only the AAX copy is spare.
    assert_eq!(summary(&found).iter().map(|s| s.0.as_str()).collect::<Vec<_>>(), ["Serum.aaxplugin"]);
}

/// A folder standing in for the Recycle Bin: what's sent is moved in, under a number, and can be put back from there.
struct FolderBin {
    dir: PathBuf,
    held: RefCell<Vec<(String, PathBuf)>>,
}

impl FolderBin {
    fn new(dir: PathBuf) -> FolderBin {
        fs::create_dir_all(&dir).unwrap();
        FolderBin { dir, held: RefCell::new(Vec::new()) }
    }

    fn at(&self, path: &str) -> Option<PathBuf> {
        self.held.borrow().iter().find(|(p, _)| p == path).map(|(_, at)| at.clone())
    }
}

impl Bin for FolderBin {
    fn send(&self, path: &Path) -> Result<(), String> {
        let mut held = self.held.borrow_mut();
        let to = self.dir.join(held.len().to_string());
        fs::rename(path, &to).map_err(|e| e.to_string())?;
        held.push((path.to_string_lossy().into_owned(), to));
        Ok(())
    }
    fn find(&self, removed: &mut [Removed]) -> Vec<Problem> {
        for r in removed {
            r.bin_id = self.at(&r.real).map(|at| at.to_string_lossy().into_owned());
        }
        Vec::new()
    }
    fn put_back(&self, removed: &[Removed]) -> Vec<Problem> {
        let mut problems = Vec::new();
        for r in removed {
            match r.bin_id.as_deref().map(PathBuf::from).filter(|at| at.exists()) {
                Some(at) => fs::rename(at, &r.path).unwrap(),
                None => {
                    problems.push(Problem { path: r.path.clone(), why: "It isn't in the bin any more.".to_owned() })
                }
            }
        }
        problems
    }
    fn holds(&self, removed: &[Removed]) -> Option<Vec<bool>> {
        Some(removed.iter().map(|r| r.bin_id.as_deref().is_some_and(|at| Path::new(at).exists())).collect())
    }
}

#[test]
fn ticked_copies_go_to_the_bin_each_checked_again_and_come_back() {
    let dir = fixtures("mo-app-space-spare-remove");
    let (vst3, vst2, avid) = (dir.join("VST3"), dir.join("VSTPlugins"), dir.join("Avid Plug-Ins"));
    write(&vst3, "Serum.vst3", 300);
    write(&vst2, "Serum_x64.dll", 120);
    write(&avid, "Serum.aaxplugin/Contents/x64/Serum.aaxplugin", 200);
    write(&vst3, "Diva.vst3", 80);
    write(&vst2, "Diva(x64).dll", 70);
    write(&vst3, "Massive.vst3", 60);
    write(&vst2, "Massive.dll", 50);
    let saves = |ids: std::ops::Range<i64>| ids.collect::<HashSet<i64>>();
    let used: Used = HashMap::from([
        ("Serum".to_owned(), HashMap::from([("VST3".to_owned(), saves(0..10))])),
        ("Diva".to_owned(), HashMap::from([("VST2".to_owned(), saves(10..12))])),
        ("Massive".to_owned(), HashMap::from([("VST3".to_owned(), saves(12..13))])),
    ]);
    let folders = plugin_folders(&[], vec![vst3.clone(), vst2.clone()], vec![avid.clone()]);
    let stop = AtomicBool::new(false);
    let found = look(&folders, &used, false, &stop).unwrap();
    let name = |p: &str| Path::new(p).file_name().unwrap().to_string_lossy().into_owned();
    let names: Vec<String> = found.copies.iter().map(|c| name(&c.path)).collect();
    assert_eq!(names, ["Serum.aaxplugin", "Serum_x64.dll", "Diva.vst3", "Massive.dll"]);

    // Every copy that can go is ticked out of the box: Diva's VST3 is unticked. Then Serum's VST2 changes (an update)
    // and Massive's VST3, the copy the saves use, is uninstalled, after the look and before Remove.
    let mut removing = Removing::default();
    removing.tick(&[PathBuf::from(&found.copies[2].path)], false);
    let picked = removing.ticked_copies(Some(&found));
    assert_eq!(picked.len(), 3);
    write(&vst2, "Serum_x64.dll", 121);
    fs::remove_file(vst3.join("Massive.vst3")).unwrap();
    removing.start(picked.clone());
    let bin = FolderBin::new(dir.join("Bin"));
    let when = binned::now();
    let done = remove(&picked, &folders, &used, false, &bin, &stop);

    // Only the AAX copy went; the other two stayed, saying why.
    assert_eq!(done.removed.iter().map(|r| name(&r.path)).collect::<Vec<_>>(), ["Serum.aaxplugin"]);
    assert!(!avid.join("Serum.aaxplugin").exists());
    assert!(vst2.join("Serum_x64.dll").exists() && vst2.join("Massive.dll").exists());
    let why = |file: &str| done.problems.iter().find(|p| name(&p.path) == file).map(|p| p.why.clone()).unwrap();
    assert!(why("Serum_x64.dll").contains("changed since it was found"), "{}", why("Serum_x64.dll"));
    assert!(why("Massive.dll").contains("isn't spare any more"), "{}", why("Massive.dll"));
    assert_eq!(done.bytes(), 200);

    // What went is kept in the app's folder, so the Recycle Bin line and Put back have it after a restart too.
    let app = dir.join("App");
    let (kept, kept_at) = binned::record(&app, &bin, SPARE_COPIES, when, &done);
    assert!(kept.noted && kept_at == when);
    assert_eq!((kept.in_bin.items, kept.in_bin.bytes), (1, 200));
    assert_eq!(space_bin::line(Some(&kept)).unwrap().size, "200 bytes");
    let card = space_bin::card(Some(&kept), &PutBacks::default(), true, false);
    assert_eq!(card.parts[0].put_back, Some((SPARE_COPIES.to_owned(), when)));
    assert_eq!(binned::read(&app).unwrap(), kept.list);
    removing.sent(when, done);
    let sent = removing.to_put_back().unwrap();
    assert_eq!(sent.when, when);
    // Put back puts back what went to the bin: here, all that went.
    assert_eq!(sent.coming_back(), kept.list[0].done);

    // Put back: it's where it was, and nothing of the app's is left in the bin, nor kept.
    let problems = undo(&sent.coming_back(), &bin);
    assert!(problems.is_empty(), "{problems:?}");
    assert!(avid.join("Serum.aaxplugin/Contents/x64/Serum.aaxplugin").exists());
    removing.put_back(when, problems);
    let now = binned::refresh(&app, &bin);
    assert!(now.list.is_empty() && now.in_bin.is_empty());
    assert_eq!(space_bin::line(Some(&now)), None);
    assert!(!app.join(binned::FILE).exists());
    // Put back again finds nothing there.
    assert_eq!(undo(&kept.list[0].done, &bin).len(), 1);
}
