//! The Plugins page's Clean up, from the page's rows to the Recycle Bin and back, on folders made in the test build's
//! own fixtures folder under fixed names. The bin is a folder there too, never the computer's Recycle Bin.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use chrono::{NaiveDate, NaiveDateTime};
use mo_app::plugins::clean::{Inputs, done_text, freed_text, intro, picked, rows, undone_text};
use mo_app::plugins::{Plugins, Read, SaveBrief, SongBrief};
use mo_library::PluginKind;
use mo_library::plugins::{PluginUse, SampleIndex, ScanCounts, StoredFolder, StoredSpace};
use mo_space::clean::{Bin, Problem, Removed, undo};
use mo_space::key;
use mo_space::plugin_clean::{apply, lines};

fn at(y: i32, m: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(y, m, 1).unwrap().and_hms_opt(12, 0, 0).unwrap()
}

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

fn space(name: &str, folders: &[(&Path, u8, u64)]) -> StoredSpace {
    StoredSpace {
        key: mo_library::read::name_key(name),
        name: name.into(),
        vendor: String::new(),
        folders: folders
            .iter()
            .map(|(p, k, b)| StoredFolder { path: s(p), kind: *k, bytes: *b, shared_with: Vec::new(), added: false })
            .collect(),
        measured: Some(at(2026, 10)),
    }
}

fn uses(save: i64, name: &str) -> PluginUse {
    PluginUse {
        save,
        name: name.into(),
        key: mo_library::read::name_key(name),
        vendor: String::new(),
        kind: PluginKind::Instrument,
        format: "VST3".into(),
        installed: true,
        daw: "FL Studio".into(),
    }
}

#[test]
fn clean_up_sends_what_may_go_to_the_bin_and_puts_it_back() {
    let dir = fixtures("mo-app-plugins-clean");
    let docs = dir.join("Documents");
    // Arcade: no song uses it. Its presets and its sound library may go; one sound folder holds a sample a song uses,
    // so it stays.
    write(&docs, "Arcade Presets/a.fxp", 1000);
    write(&docs, "Arcade Library/loop.wav", 5000);
    write(&docs, "Arcade Kits/kick.wav", 3000);
    // Serum: a song uses it now. Its presets may only go once none has for two years, and its sounds while any uses it.
    write(&docs, "Serum Presets/b.fxp", 1000);
    let read = Read {
        uses: vec![uses(2, "Serum")],
        saves: vec![SaveBrief {
            id: 2,
            name: "New".into(),
            path: "C:/beats/New.flp".into(),
            daw: "FL Studio".into(),
            modified: at(2026, 9),
        }],
        song_of: HashMap::from([(2, 0)]),
        songs: vec![SongBrief { name: "New".into(), latest: 2 }],
        spaces: vec![
            space(
                "Arcade",
                &[
                    (&docs.join("Arcade Presets"), 1, 1000),
                    (&docs.join("Arcade Library"), 2, 5000),
                    (&docs.join("Arcade Kits"), 2, 3000),
                ],
            ),
            space("Serum", &[(&docs.join("Serum Presets"), 1, 1000)]),
        ],
        installed_names: vec!["Arcade".into(), "Serum".into()],
        scans: ScanCounts::default(),
        looked_for_installed: true,
        unused_years: 2,
        needed_count: 6,
    };
    let mut plugins = Plugins::new(read, at(2026, 11));
    let arcade = plugins.rows.iter().position(|r| r.name == "Arcade").unwrap();
    let serum = plugins.rows.iter().position(|r| r.name == "Serum").unwrap();
    assert!(plugins.rows[arcade].unused && !plugins.rows[serum].unused);

    let mut samples = SampleIndex::default();
    // Saved on the old PC, on another drive letter: the name still holds the folder.
    samples.add(r"F:\Old PC\Arcade Kits\kick.wav");
    let stop = AtomicBool::new(false);

    // Serum: still used, so nothing may go.
    let ask = plugins.clean_ask(serum).unwrap();
    let inputs = Inputs { ask, protected: vec![], kept: vec![], samples: samples.clone() };
    let found = inputs.with(|lib| lines(&inputs.ask.space, &inputs.ask.all, lib, &stop)).unwrap();
    let shown = rows(&found, &inputs);
    assert_eq!(shown.len(), 1);
    assert!(!shown[0].can && shown[0].why.contains("still use Serum"), "{}", shown[0].why);
    assert_eq!(freed_text(&found, &[false]), "Nothing here can go: each line says why.");
    assert!(intro(&inputs, &found).contains("1 project uses Serum"));

    // Arcade: the library is ticked (sounds named after it), the presets wait for a tick, the kits stay.
    let ask = plugins.clean_ask(arcade).unwrap();
    let inputs = Inputs { ask, protected: vec![], kept: vec![], samples };
    let found = inputs.with(|lib| lines(&inputs.ask.space, &inputs.ask.all, lib, &stop)).unwrap();
    let shown = rows(&found, &inputs);
    let line = |name: &str| shown.iter().position(|r| r.path.ends_with(name)).unwrap();
    let (library, presets, kits) = (line("Arcade Library"), line("Arcade Presets"), line("Arcade Kits"));
    assert!(shown[library].can && shown[library].ticked && shown[library].kind == "Sounds", "{:?}", shown);
    assert!(shown[presets].can && !shown[presets].ticked);
    assert!(!shown[kits].can && shown[kits].why.contains("kick.wav"), "{}", shown[kits].why);

    // Ticking the presets too: both go, the kits can't be ticked in.
    let mut ticks: Vec<bool> = shown.iter().map(|r| r.ticked).collect();
    ticks[presets] = true;
    ticks[kits] = true;
    assert_eq!(picked(&found, &ticks).len(), 2);
    assert!(freed_text(&found, &ticks).starts_with("Frees "));
    let bin = FolderBin { dir: dir.join("bin"), held: RefCell::new(Vec::new()) };
    let done = inputs.with(|lib| apply(&picked(&found, &ticks), &inputs.ask.space, &inputs.ask.all, lib, &bin, &stop));
    assert!(done.problems.is_empty(), "{:?}", done.problems);
    assert!(!docs.join("Arcade Library").exists() && !docs.join("Arcade Presets").exists());
    assert!(docs.join("Arcade Kits/kick.wav").exists());
    assert!(done_text(&done).0.starts_with("Freed "));

    // The page forgets them until the old app measures again: only the kits are left on its card.
    plugins.forget_removed(&done);
    let left = &plugins.rows[arcade].room.as_ref().unwrap().folders;
    assert_eq!(left.len(), 1);
    assert!(left[0].path.ends_with("Arcade Kits"));

    // Put back.
    let problems = undo(&done, &bin);
    assert!(problems.is_empty());
    assert!(docs.join("Arcade Library/loop.wav").exists() && docs.join("Arcade Presets/a.fxp").exists());
    assert_eq!(undone_text(&done, &problems).0, "Put back 2 folders.");
}

#[test]
fn a_missing_or_unmeasured_plugin_has_nothing_to_clean() {
    let read = Read {
        uses: vec![PluginUse { installed: false, ..uses(1, "Gone Synth") }],
        saves: vec![SaveBrief {
            id: 1,
            name: "A".into(),
            path: "a".into(),
            daw: "FL Studio".into(),
            modified: at(2020, 1),
        }],
        song_of: HashMap::from([(1, 0)]),
        songs: vec![SongBrief { name: "A".into(), latest: 1 }],
        spaces: vec![],
        installed_names: vec![],
        scans: ScanCounts::default(),
        looked_for_installed: true,
        unused_years: 2,
        needed_count: 6,
    };
    let plugins = Plugins::new(read, at(2026, 11));
    assert!(plugins.clean_ask(0).is_none());
    assert!(plugins.clean_ask(99).is_none());
}
