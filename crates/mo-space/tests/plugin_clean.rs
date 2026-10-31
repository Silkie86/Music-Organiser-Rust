//! Cleaning up a plugin's presets and sounds, on folders made in the test build's own fixtures folder under fixed
//! names. The bin is a folder there too, never the computer's Recycle Bin.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use mo_space::clean::{Bin, Problem, Removed, What, undo};
use mo_space::key;
use mo_space::plugin_clean::{Library, PluginUse, Stays, apply, lines};
use mo_space::plugin_space::{PluginSpace, SpaceFolder, SpaceKind};

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

fn folder(path: &Path, kind: SpaceKind, bytes: u64, shared_with: &[&str]) -> SpaceFolder {
    SpaceFolder {
        path: s(path),
        kind,
        bytes,
        shared_with: shared_with.iter().map(|k| (*k).to_owned()).collect(),
        added: false,
    }
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
        held.push((path.to_string_lossy().into_owned(), to));
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

#[test]
fn only_what_no_project_needs_may_go_and_sounds_only_when_no_project_uses_the_plugin() {
    let dir = fixtures("mo-space-plugin-clean");
    let docs = dir.join("Documents");
    write(&docs, "Serum Presets/Lead.fxp", 100);
    write(&docs, "Serum Sounds/Saw.wav", 1000);
    write(&docs, "Serum Used/Kick.wav", 10);
    write(&docs, "Serum Holds/Helper.vst3", 10);
    write(&docs, "Xfer/Settings.xml", 30);
    write(&docs, "Omni Library/Pad.wav", 5000);
    write(&docs, "Omni Presets/a.prt", 50);
    write(&dir, "Samples/Serum Kit/Snare.wav", 20);
    let serum = PluginSpace {
        key: "serum".into(),
        name: "Serum".into(),
        vendor: "Xfer Records".into(),
        folders: vec![
            folder(&dir.join("VST3/Serum.vst3"), SpaceKind::Program, 10, &[]),
            folder(&docs.join("Serum Presets"), SpaceKind::Presets, 100, &[]),
            folder(&docs.join("Serum Sounds"), SpaceKind::Sounds, 1000, &[]),
            folder(&docs.join("Serum Used"), SpaceKind::Sounds, 10, &[]),
            folder(&docs.join("Serum Holds"), SpaceKind::Presets, 10, &[]),
            folder(&docs.join("Xfer"), SpaceKind::Presets, 30, &["nerve"]),
            folder(&dir.join("Samples/Serum Kit"), SpaceKind::Sounds, 20, &[]),
        ],
    };
    let omni = PluginSpace {
        key: "omnisphere".into(),
        name: "Omnisphere".into(),
        vendor: "Spectrasonics".into(),
        folders: vec![
            folder(&docs.join("Omni Library"), SpaceKind::Sounds, 5000, &[]),
            folder(&docs.join("Omni Presets"), SpaceKind::Presets, 50, &[]),
        ],
    };
    let all = vec![serum.clone(), omni.clone()];
    let uses: HashMap<String, PluginUse> = [
        ("serum".to_owned(), PluginUse { projects: 0, unused: true }),
        ("nerve".to_owned(), PluginUse { projects: 3, unused: false }),
        ("omnisphere".to_owned(), PluginUse { projects: 2, unused: true }),
    ]
    .into();
    let protected = vec![s(&dir.join("Samples"))];
    let sample_at = |_: &str| None;
    let sample_named = |name: &str| name.eq_ignore_ascii_case("kick.wav");
    let names = |k: &str| if k == "nerve" { "Nerve".to_owned() } else { k.to_owned() };
    let library = Library {
        protected_roots: &protected,
        system_folders: &[],
        uses: &uses,
        sample_at: &sample_at,
        sample_named: &sample_named,
        names: &names,
    };
    let stop = AtomicBool::new(false);

    let card = lines(&serum, &all, &library, &stop).unwrap();
    let said: Vec<(&str, Option<String>, bool)> = card
        .iter()
        .map(|l| (file_name(&l.folder.path), l.stays.as_ref().map(|w| w.text("Serum", &names)), l.ticked))
        .collect();
    assert_eq!(
        said,
        [
            ("Serum Sounds", None, true),
            ("Serum Presets", None, false),
            ("Xfer", Some("Shared with Nerve, which your projects use.".into()), false),
            ("Serum Kit", Some(format!("Part of your folder {}.", s(&dir.join("Samples")))), false),
            ("Serum Used", Some("A project uses a sample in it (Kick.wav).".into()), false),
            ("Serum Holds", Some("It holds Helper.vst3, a plugin file: Uninstall removes that.".into()), false),
        ]
    );

    // Omnisphere is unused for years but two projects still have it: its presets may go, its sounds stay.
    let card = lines(&omni, &all, &library, &stop).unwrap();
    assert_eq!(card[0].stays, Some(Stays::SoundsInUse { projects: 2 }));
    assert_eq!(card[1].stays, None);

    // Clean up what's ticked, plus a line that must stay (never sent), plus one that changed since.
    let serum_card = lines(&serum, &all, &library, &stop).unwrap();
    let picked: Vec<_> =
        serum_card.iter().filter(|l| l.ticked || file_name(&l.folder.path) != "Serum Presets").cloned().collect();
    let presets = serum_card.iter().find(|l| file_name(&l.folder.path) == "Serum Presets").unwrap().clone();
    write(&docs, "Serum Presets/New.fxp", 5);
    let bin = FolderBin { dir: dir.join("bin"), held: RefCell::new(Vec::new()) };
    let mut all_picked = picked.clone();
    all_picked.push(presets);
    let done = apply(&all_picked, &serum, &all, &library, &bin, &stop);
    assert_eq!(done.removed.len(), 1);
    assert_eq!(done.removed[0].what, What::Plugin(SpaceKind::Sounds));
    assert!(!docs.join("Serum Sounds").exists());
    assert!(
        docs.join("Xfer").exists() && docs.join("Serum Presets").exists() && dir.join("Samples/Serum Kit").exists()
    );
    let changed = done.problems.iter().find(|p| p.path.ends_with("Serum Presets")).unwrap();
    assert_eq!(changed.why, "It changed since you looked, so it stays.");
    assert_eq!(done.problems.len(), 5);

    assert!(undo(&done, &bin).is_empty());
    assert!(docs.join("Serum Sounds/Saw.wav").exists());
}

#[test]
fn a_plugin_the_library_doesnt_know_counts_as_used() {
    let dir = fixtures("mo-space-plugin-clean-unknown");
    write(&dir, "Docs/Thing Presets/a.fxp", 5);
    let space = PluginSpace {
        key: "thing".into(),
        name: "Thing".into(),
        vendor: String::new(),
        folders: vec![folder(&dir.join("Docs/Thing Presets"), SpaceKind::Presets, 5, &[])],
    };
    let uses = HashMap::new();
    let none = |_: &str| None;
    let no = |_: &str| false;
    let names = |k: &str| k.to_owned();
    let library = Library {
        protected_roots: &[],
        system_folders: &[],
        uses: &uses,
        sample_at: &none,
        sample_named: &no,
        names: &names,
    };
    let card = lines(&space, std::slice::from_ref(&space), &library, &AtomicBool::new(false)).unwrap();
    assert_eq!(card[0].stays, Some(Stays::StillUsed));
}

#[cfg(unix)]
#[test]
fn a_folder_reached_through_a_link_stays() {
    let dir = fixtures("mo-space-plugin-clean-link");
    write(&dir, "D Drive/Thing Sounds/a.wav", 5);
    fs::create_dir_all(dir.join("Docs")).unwrap();
    std::os::unix::fs::symlink(dir.join("D Drive"), dir.join("Docs/Moved")).unwrap();
    let space = PluginSpace {
        key: "thing".into(),
        name: "Thing".into(),
        vendor: String::new(),
        folders: vec![folder(&dir.join("Docs/Moved/Thing Sounds"), SpaceKind::Sounds, 5, &[])],
    };
    let uses: HashMap<String, PluginUse> = [("thing".to_owned(), PluginUse { projects: 0, unused: true })].into();
    let none = |_: &str| None;
    let no = |_: &str| false;
    let names = |k: &str| k.to_owned();
    let library = Library {
        protected_roots: &[],
        system_folders: &[],
        uses: &uses,
        sample_at: &none,
        sample_named: &no,
        names: &names,
    };
    let card = lines(&space, std::slice::from_ref(&space), &library, &AtomicBool::new(false)).unwrap();
    assert_eq!(card[0].stays, Some(Stays::Link));
}

/// How projects use each plugin: (name key, projects, unused).
fn uses(list: &[(&str, u32, bool)]) -> HashMap<String, PluginUse> {
    list.iter()
        .map(|(k, projects, unused)| ((*k).to_owned(), PluginUse { projects: *projects, unused: *unused }))
        .collect()
}

fn plugin(key: &str, name: &str, folders: Vec<SpaceFolder>) -> PluginSpace {
    PluginSpace { key: key.into(), name: name.into(), vendor: String::new(), folders }
}

fn names(k: &str) -> String {
    match k {
        "nerve" => "Nerve".to_owned(),
        "omnisphere" => "Omnisphere".to_owned(),
        "serum" => "Serum".to_owned(),
        _ => k.to_owned(),
    }
}

fn no_sample_at(_: &str) -> Option<String> {
    None
}

fn no_sample_named(_: &str) -> bool {
    false
}

fn library<'a>(protected: &'a [String], system: &'a [String], uses: &'a HashMap<String, PluginUse>) -> Library<'a> {
    Library {
        protected_roots: protected,
        system_folders: system,
        uses,
        sample_at: &no_sample_at,
        sample_named: &no_sample_named,
        names: &names,
    }
}

#[test]
fn a_folder_inside_one_that_goes_goes_with_it_and_a_line_picked_twice_counts_once() {
    let dir = fixtures("mo-space-plugin-clean-nested");
    let docs = dir.join("Docs");
    write(&docs, "Xfer/Settings.xml", 30);
    write(&docs, "Xfer/Serum Presets/Lead.fxp", 100);
    let serum = plugin(
        "serum",
        "Serum",
        vec![
            folder(&docs.join("Xfer"), SpaceKind::Presets, 30, &["nerve"]),
            folder(&docs.join("Xfer/Serum Presets"), SpaceKind::Presets, 100, &[]),
        ],
    );
    let uses = uses(&[("serum", 0, true), ("nerve", 0, true)]);
    let library = library(&[], &[], &uses);
    let stop = AtomicBool::new(false);
    let card = lines(&serum, std::slice::from_ref(&serum), &library, &stop).unwrap();
    assert!(card.iter().all(|l| l.stays.is_none()));
    // The maker's folder frees what's in it too.
    assert_eq!(card.iter().map(|l| l.bytes()).collect::<Vec<_>>(), [100, 130]);

    // The inner folder first, and picked twice: the maker's folder still goes whole, once.
    let picked = vec![card[0].clone(), card[1].clone(), card[0].clone()];
    let bin = FolderBin { dir: dir.join("bin"), held: RefCell::new(Vec::new()) };
    let done = apply(&picked, &serum, std::slice::from_ref(&serum), &library, &bin, &stop);
    assert!(done.problems.is_empty(), "{:?}", done.problems);
    assert_eq!(done.removed.len(), 1);
    assert!(done.removed[0].path.ends_with("Xfer"));
    assert_eq!(done.removed[0].bytes, 130);
    assert!(!docs.join("Xfer").exists());
    assert!(undo(&done, &bin).is_empty());
    assert!(docs.join("Xfer/Serum Presets/Lead.fxp").exists());
}

#[test]
fn a_folder_holding_another_plugins_folder_says_which_plugin_keeps_it() {
    let dir = fixtures("mo-space-plugin-clean-holds");
    let docs = dir.join("Docs");
    write(&docs, "Shared Sounds/Saw.wav", 10);
    write(&docs, "Shared Sounds/Omni Library/Pad.wav", 50);
    let serum = plugin("serum", "Serum", vec![folder(&docs.join("Shared Sounds"), SpaceKind::Sounds, 10, &[])]);
    let omni = plugin(
        "omnisphere",
        "Omnisphere",
        vec![folder(&docs.join("Shared Sounds/Omni Library"), SpaceKind::Sounds, 50, &[])],
    );
    let uses = uses(&[("serum", 0, true), ("omnisphere", 2, true)]);
    let library = library(&[], &[], &uses);
    let card = lines(&serum, &[serum.clone(), omni], &library, &AtomicBool::new(false)).unwrap();
    assert_eq!(
        card[0].stays.as_ref().unwrap().text("Serum", &names),
        "It holds Omni Library, which stays. Sounds stay while a project uses Omnisphere: 2 projects do."
    );
}

#[test]
fn a_plugin_bundle_inside_keeps_the_folder_even_when_it_is_a_folder() {
    let dir = fixtures("mo-space-plugin-clean-bundle");
    let docs = dir.join("Docs");
    write(&docs, "Thing Presets/a.fxp", 5);
    write(&docs, "Thing Presets/More/Helper.component/Contents/MacOS/Helper", 10);
    let thing = plugin("thing", "Thing", vec![folder(&docs.join("Thing Presets"), SpaceKind::Presets, 15, &[])]);
    let uses = uses(&[("thing", 0, true)]);
    let library = library(&[], &[], &uses);
    let card = lines(&thing, std::slice::from_ref(&thing), &library, &AtomicBool::new(false)).unwrap();
    assert_eq!(
        card[0].stays,
        Some(Stays::HoldsPluginFile(s(&docs.join("Thing Presets").join("More").join("Helper.component"))))
    );
}

#[test]
fn only_sharers_your_projects_use_keep_a_folder_and_the_plugins_folders_are_checked_as_they_are_now() {
    let dir = fixtures("mo-space-plugin-clean-now");
    let docs = dir.join("Docs");
    write(&docs, "Thing Presets/a.fxp", 5);
    write(&docs, "Shared Presets/b.fxp", 5);
    let thing = plugin(
        "thing",
        "Thing",
        vec![
            folder(&docs.join("Thing Presets"), SpaceKind::Presets, 5, &[]),
            folder(&docs.join("Shared Presets"), SpaceKind::Presets, 5, &["nerve", "serum2"]),
        ],
    );
    let uses = uses(&[("thing", 0, true), ("nerve", 3, false), ("serum2", 0, true)]);
    let library = library(&[], &[], &uses);
    let stop = AtomicBool::new(false);
    let card = lines(&thing, std::slice::from_ref(&thing), &library, &stop).unwrap();
    assert_eq!(card[1].stays.as_ref().unwrap().text("Thing", &names), "Shared with Nerve, which your projects use.");
    assert_eq!(card[0].stays, None);

    // Measured again since: Nerve shares the folder now.
    let mut now = thing.clone();
    now.folders[0].shared_with = vec!["nerve".into()];
    let bin = FolderBin { dir: dir.join("bin"), held: RefCell::new(Vec::new()) };
    let done = apply(&card[..1], &now, std::slice::from_ref(&now), &library, &bin, &stop);
    assert!(done.removed.is_empty());
    assert_eq!(done.problems[0].why, "Shared with Nerve, which your projects use.");

    // No longer one of the plugin's folders.
    let mut now = thing.clone();
    now.folders.remove(0);
    let done = apply(&card[..1], &now, std::slice::from_ref(&now), &library, &bin, &stop);
    assert!(done.removed.is_empty());
    assert_eq!(done.problems[0].why, "It changed since you looked, so it stays.");
    assert!(docs.join("Thing Presets/a.fxp").exists());
}

#[test]
fn a_file_saved_again_at_the_same_size_or_renamed_counts_as_a_change() {
    let dir = fixtures("mo-space-plugin-clean-same-size");
    let docs = dir.join("Docs");
    write(&docs, "Thing Presets/Lead.fxp", 100);
    write(&docs, "Thing Sounds/a.wav", 100);
    write(&docs, "Thing More/b.fxp", 10);
    let thing = plugin(
        "thing",
        "Thing",
        vec![
            folder(&docs.join("Thing Presets"), SpaceKind::Presets, 100, &[]),
            folder(&docs.join("Thing Sounds"), SpaceKind::Sounds, 100, &[]),
            folder(&docs.join("Thing More"), SpaceKind::Presets, 10, &[]),
        ],
    );
    let uses = uses(&[("thing", 0, true)]);
    let library = library(&[], &[], &uses);
    let stop = AtomicBool::new(false);
    let card = lines(&thing, std::slice::from_ref(&thing), &library, &stop).unwrap();
    assert!(card.iter().all(|l| l.stays.is_none()));

    let lead = fs::File::options().write(true).open(docs.join("Thing Presets/Lead.fxp")).unwrap();
    lead.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000)).unwrap();
    drop(lead);
    fs::rename(docs.join("Thing Sounds/a.wav"), docs.join("Thing Sounds/b.wav")).unwrap();

    let bin = FolderBin { dir: dir.join("bin"), held: RefCell::new(Vec::new()) };
    let done = apply(&card, &thing, std::slice::from_ref(&thing), &library, &bin, &stop);
    assert_eq!(done.removed.len(), 1);
    assert!(done.removed[0].path.ends_with("Thing More"));
    assert_eq!(done.problems.len(), 2);
    assert!(done.problems.iter().all(|p| p.why == "It changed since you looked, so it stays."));
}

#[cfg(unix)]
#[test]
fn your_folders_and_the_computers_own_are_known_by_where_they_really_are() {
    use std::os::unix::fs::symlink;
    let dir = fixtures("mo-space-plugin-clean-real-paths");
    write(&dir, "Area/Samples/Kit/a.wav", 5);
    write(&dir, "Area/Other/x.fxp", 5);
    write(&dir, "Home/Music/Thing Sounds/a.wav", 5);
    write(&dir, "Home/Stuff/y.fxp", 5);
    fs::create_dir_all(dir.join("Elsewhere")).unwrap();
    symlink(dir.join("Area/Samples"), dir.join("Elsewhere/Samples Link")).unwrap();
    symlink(dir.join("Home/Music"), dir.join("Elsewhere/Music Link")).unwrap();
    let protected = vec![s(&dir.join("Elsewhere/Samples Link"))];
    let system = vec![s(&dir.join("Elsewhere/Music Link"))];
    let thing = plugin(
        "thing",
        "Thing",
        vec![
            folder(&dir.join("Area/Samples/Kit"), SpaceKind::Presets, 6, &[]),
            folder(&dir.join("Area"), SpaceKind::Presets, 5, &[]),
            folder(&dir.join("Area/Other"), SpaceKind::Presets, 4, &[]),
            folder(&dir.join("Home/Music"), SpaceKind::Presets, 3, &[]),
            folder(&dir.join("Home"), SpaceKind::Presets, 2, &[]),
            folder(&dir.join("Home/Music/Thing Sounds"), SpaceKind::Presets, 1, &[]),
        ],
    );
    let uses = uses(&[("thing", 0, true)]);
    let library = library(&protected, &system, &uses);
    let card = lines(&thing, std::slice::from_ref(&thing), &library, &AtomicBool::new(false)).unwrap();
    let stays: Vec<Option<Stays>> = card.into_iter().map(|l| l.stays).collect();
    assert_eq!(
        stays,
        [
            Some(Stays::InYourFolder(protected[0].clone())),
            Some(Stays::HoldsYourFolder(protected[0].clone())),
            None,
            Some(Stays::SystemFolder),
            Some(Stays::SystemFolder),
            None,
        ]
    );
}

#[cfg(unix)]
#[test]
fn a_part_that_cant_be_read_keeps_the_folder() {
    use std::os::unix::fs::PermissionsExt;
    let dir = fixtures("mo-space-plugin-clean-unreadable");
    let docs = dir.join("Docs");
    write(&docs, "Thing Presets/a.fxp", 5);
    write(&docs, "Thing Presets/Locked/b.fxp", 5);
    let locked = docs.join("Thing Presets/Locked");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    // An administrator can read it anyway: nothing to see then.
    let readable = fs::read_dir(&locked).is_ok();
    let thing = plugin("thing", "Thing", vec![folder(&docs.join("Thing Presets"), SpaceKind::Presets, 10, &[])]);
    let uses = uses(&[("thing", 0, true)]);
    let library = library(&[], &[], &uses);
    let card = lines(&thing, std::slice::from_ref(&thing), &library, &AtomicBool::new(false));
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    if !readable {
        assert_eq!(card.unwrap()[0].stays, Some(Stays::Unreadable(s(&locked))));
    }
}

fn file_name(p: &str) -> &str {
    Path::new(p).file_name().unwrap().to_str().unwrap()
}
