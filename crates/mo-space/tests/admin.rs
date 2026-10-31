//! The administrator step, on folders made in the test build's own fixtures folder under fixed names. Windows'
//! administrator question is stood in for: "as administrator" here means a folder the test locked (so the test's own
//! user can't change what's in it, as Windows guards a plugin's folder) is unlocked while the step runs, in this same
//! process. Locking only works for a normal user, so those tests are checked as one (as CI runs); as root nothing is
//! locked and they stop early. Links are junctions on Windows, as the app makes.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};

use mo_space::admin::{MOVE_FOLDER, RECYCLE_BIN, run_step, write_list};

fn fixtures(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    if dir.exists() {
        unlock_all(&dir);
        fs::remove_dir_all(&dir).unwrap();
    }
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Makes every folder in a fixture writable again, so the next run can clear it.
fn unlock_all(dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::symlink_metadata(dir).is_ok_and(|m| m.is_dir()) {
            let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o755));
            for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
                unlock_all(&entry.path());
            }
        }
    }
    #[cfg(not(unix))]
    let _ = dir;
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

fn real(p: &Path) -> PathBuf {
    fs::canonicalize(p).unwrap()
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| (*a).to_owned()).collect()
}

#[test]
fn only_the_apps_own_flags_start_a_step_and_a_step_not_as_the_app_starts_it_does_nothing() {
    assert!(!run_step(&[]));
    assert!(!run_step(&strings(&["--open", "x.flp"])));
    assert!(run_step(&strings(&[MOVE_FOLDER])));
    assert!(run_step(&strings(&[MOVE_FOLDER, "swap", "list"])));
    assert!(run_step(&strings(&[MOVE_FOLDER, "rename", "a", "b", "c"])));
    assert!(run_step(&strings(&[RECYCLE_BIN, "a"])));
    assert!(run_step(&strings(&[RECYCLE_BIN, "no list here", "result", "AB"])));
}

#[test]
fn a_list_changed_since_it_was_written_or_a_result_already_there_does_nothing() {
    let dir = fixtures("mo-space-admin-list");
    let (a, b) = (dir.join("A"), dir.join("B"));
    fs::create_dir_all(&a).unwrap();
    fs::create_dir_all(&b).unwrap();
    let at = dir.join("Link");
    folder_link(&a, &at);
    let (list, result) = (dir.join("list.txt"), dir.join("result.txt"));
    let fingerprint = write_list(&list, &[format!("{}\t{}", s(&at), s(&b))]).unwrap();
    // The list is only ever written as a new file.
    assert!(write_list(&list, &["x".to_owned()]).is_err());
    let step = |fingerprint: &str| strings(&[MOVE_FOLDER, "relink", &s(&list), &s(&result), fingerprint]);

    // Something changed the list while Windows asked: the step does nothing at all.
    fs::write(&list, format!("{}\t{}\n", s(&at), s(&dir))).unwrap();
    assert!(run_step(&step(&fingerprint)));
    assert!(!result.exists());
    assert_eq!(real(&at), real(&a));

    // The list as written, but something is where the result goes: nothing is done.
    fs::remove_file(&list).unwrap();
    let fingerprint = write_list(&list, &[format!("{}\t{}", s(&at), s(&b))]).unwrap();
    fs::write(&result, "there already").unwrap();
    assert!(run_step(&step(&fingerprint)));
    assert_eq!(fs::read_to_string(&result).unwrap(), "there already");
    assert_eq!(real(&at), real(&a));

    // As the app starts it, the link is changed and the result says so.
    fs::remove_file(&result).unwrap();
    assert!(run_step(&step(&fingerprint.to_lowercase())));
    assert_eq!(real(&at), real(&b));
    assert_eq!(fs::read_to_string(&result).unwrap(), format!("{}\tdone\t\n", s(&at)));
}

#[test]
fn a_path_reached_through_a_link_is_left_alone() {
    let dir = fixtures("mo-space-admin-through");
    let (a, b) = (dir.join("A"), dir.join("B"));
    fs::create_dir_all(&a).unwrap();
    fs::create_dir_all(&b).unwrap();
    // "Plugins" is a link to the real folder the plugin's link is in.
    let real_folder = dir.join("Real");
    fs::create_dir_all(&real_folder).unwrap();
    folder_link(&a, &real_folder.join("Link"));
    folder_link(&real_folder, &dir.join("Plugins"));
    let through = dir.join("Plugins").join("Link");
    let (list, result) = (dir.join("list.txt"), dir.join("result.txt"));
    let fingerprint = write_list(&list, &[format!("{}\t{}", s(&through), s(&b))]).unwrap();
    assert!(run_step(&strings(&[MOVE_FOLDER, "relink", &s(&list), &s(&result), &fingerprint])));
    assert_eq!(real(&through), real(&a));
    let said = fs::read_to_string(&result).unwrap();
    assert!(said.contains("\tkept\t") && said.contains("reached through a link"), "{said}");

    // Where it says how it went is reached through a link: nothing is done, and nothing written there.
    let (list, through_result) = (dir.join("list 2.txt"), dir.join("Plugins").join("result.txt"));
    let at = real_folder.join("Link");
    let fingerprint = write_list(&list, &[format!("{}\t{}", s(&at), s(&b))]).unwrap();
    assert!(run_step(&strings(&[MOVE_FOLDER, "relink", &s(&list), &s(&through_result), &fingerprint])));
    assert!(!through_result.exists());
    assert_eq!(real(&at), real(&a));
}

/// Folders Windows guards, stood in for by folders the test locks (only Unix has the locks the test uses).
#[cfg(unix)]
mod guarded {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use mo_space::admin::{Admin, NoAdmin, run_step_with};
    use mo_space::clean::{Bin, NEEDS_ADMIN, Problem, Removed};
    use mo_space::folder_move::Drives;
    use mo_space::key;

    use super::{fixtures, folder_link, real, s};

    fn write(dir: &Path, rel: &str, text: &str) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }

    /// A Recycle Bin that's a folder in the fixtures. A file it isn't allowed to move (in a locked folder) needs
    /// administrator rights, as Windows says of one it guards.
    struct FolderBin {
        dir: PathBuf,
        held: Arc<Mutex<Vec<(String, PathBuf)>>>,
        admin: Option<Box<Unlock>>,
    }

    impl FolderBin {
        fn new(dir: &Path) -> Self {
            FolderBin { dir: dir.join("bin"), held: Arc::new(Mutex::new(Vec::new())), admin: None }
        }

        /// The same bin, as the step sees it.
        fn twin(&self) -> Self {
            FolderBin { dir: self.dir.clone(), held: Arc::clone(&self.held), admin: None }
        }
    }

    impl Bin for FolderBin {
        fn send(&self, path: &Path) -> Result<(), String> {
            let mut held = self.held.lock().unwrap();
            fs::create_dir_all(&self.dir).unwrap();
            let to = self.dir.join(held.len().to_string());
            match fs::rename(path, &to) {
                Ok(()) => {
                    held.push((s(path), to));
                    Ok(())
                }
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Err(NEEDS_ADMIN.to_owned()),
                Err(e) => Err(e.to_string()),
            }
        }
        fn admin(&self) -> &dyn Admin {
            match &self.admin {
                Some(admin) => &**admin,
                None => &NoAdmin,
            }
        }
        fn find(&self, removed: &mut [Removed]) -> Vec<Problem> {
            for r in removed {
                r.bin_id = self.held.lock().unwrap().iter().find(|(p, _)| key(p) == key(&r.real)).map(|(_, at)| s(at));
            }
            Vec::new()
        }
        fn put_back(&self, _removed: &[Removed]) -> Vec<Problem> {
            Vec::new()
        }
    }

    /// Stands in for Windows' administrator question: unlocks the locked folders while the step runs here, then locks
    /// them again. With `no`, you said no.
    struct Unlock {
        locked: Vec<PathBuf>,
        asked: AtomicUsize,
        no: bool,
        bin: Option<FolderBin>,
        /// A file something saves while Windows asks.
        saved_meanwhile: Option<PathBuf>,
    }

    impl Unlock {
        fn new(locked: &[&Path]) -> Self {
            Unlock {
                locked: locked.iter().map(|p| p.to_path_buf()).collect(),
                asked: AtomicUsize::new(0),
                no: false,
                bin: None,
                saved_meanwhile: None,
            }
        }

        fn asked(&self) -> usize {
            self.asked.load(Ordering::SeqCst)
        }
    }

    impl Admin for Unlock {
        fn can_ask(&self) -> bool {
            true
        }
        fn ask(&self, args: &[String]) -> Result<(), String> {
            self.asked.fetch_add(1, Ordering::SeqCst);
            if self.no {
                return Err("You said no to Windows' administrator question.".to_owned());
            }
            if let Some(saved) = &self.saved_meanwhile {
                fs::write(saved, "saved meanwhile").unwrap();
            }
            self.locked.iter().for_each(|d| lock(d, false));
            let ran = match &self.bin {
                Some(bin) => run_step_with(args, bin),
                None => run_step_with(args, &FolderBin::new(Path::new("unused"))),
            };
            self.locked.iter().for_each(|d| lock(d, true));
            assert!(ran, "the step didn't know {args:?}");
            Ok(())
        }
    }

    /// Locks a folder so its files and folders can't be renamed, removed or added by this user.
    fn lock(dir: &Path, locked: bool) {
        fs::set_permissions(dir, fs::Permissions::from_mode(if locked { 0o555 } else { 0o755 })).unwrap();
    }

    /// Whether locking keeps this user out (not when running as root).
    fn locking_works(dir: &Path) -> bool {
        let probe = dir.join("probe");
        fs::create_dir_all(&probe).unwrap();
        lock(&probe, true);
        let works = fs::write(probe.join("x"), "x").is_err();
        lock(&probe, false);
        fs::remove_dir_all(&probe).unwrap();
        works
    }

    /// Another drive, with Windows' question stood in for.
    struct OtherDrive<'a>(&'a dyn Admin);

    impl Drives for OtherDrive<'_> {
        fn same(&self, _a: &Path, _b: &Path) -> bool {
            false
        }
        fn free(&self, _path: &Path) -> Option<u64> {
            None
        }
        fn admin(&self) -> &dyn Admin {
            self.0
        }
    }

    /// The same drive.
    struct SameDrive;

    impl Drives for SameDrive {
        fn same(&self, _a: &Path, _b: &Path) -> bool {
            true
        }
        fn free(&self, _path: &Path) -> Option<u64> {
            None
        }
    }

    fn nothing(_: u64) {}

    #[test]
    fn a_folder_windows_guards_moves_and_comes_back_as_administrator() {
        let dir = fixtures("mo-space-admin-move");
        if !locking_works(&dir) {
            return;
        }
        let (guarded, d) = (dir.join("ProgramData"), dir.join("D"));
        write(&guarded, "Spectrasonics/Pad.db", "pad");
        fs::create_dir_all(&d).unwrap();
        let from = guarded.join("Spectrasonics");
        lock(&guarded, true);
        let stop = AtomicBool::new(false);
        let unlock = Unlock::new(&[&guarded]);
        let drives = OtherDrive(&unlock);
        let moved = mo_space::folder_move::move_folder(
            &s(&from),
            &s(&d),
            3,
            ("omnisphere", mo_space::plugin_space::SpaceKind::Sounds, &[]),
            &drives,
            &[],
            &nothing,
            &stop,
        );
        let moved = moved.unwrap();
        // Asked once, at the swap: the copy was made as you.
        assert_eq!(unlock.asked(), 1);
        assert!(fs::symlink_metadata(&from).unwrap().file_type().is_symlink());
        assert_eq!(real(&from), real(&d.join("Spectrasonics")));
        assert_eq!(fs::read_to_string(guarded.join("Spectrasonics (old copy)/Pad.db")).unwrap(), "pad");

        // Saved through the link since; moving back asks again.
        write(&d, "Spectrasonics/User.db", "mine");
        let back = mo_space::folder_move::move_back(&moved, &drives, &nothing, &stop).unwrap();
        assert!(back.kept.is_empty());
        assert_eq!(unlock.asked(), 2);
        assert!(!fs::symlink_metadata(&from).unwrap().file_type().is_symlink());
        assert_eq!(fs::read_to_string(from.join("User.db")).unwrap(), "mine");
        assert!(!guarded.join("Spectrasonics (old copy)").exists());
        lock(&guarded, false);
    }

    #[test]
    fn when_you_say_no_or_nobody_can_be_asked_nothing_moves() {
        let dir = fixtures("mo-space-admin-no");
        if !locking_works(&dir) {
            return;
        }
        let (guarded, d) = (dir.join("ProgramData"), dir.join("D"));
        write(&guarded, "Spectrasonics/Pad.db", "pad");
        fs::create_dir_all(&d).unwrap();
        let from = guarded.join("Spectrasonics");
        lock(&guarded, true);
        let stop = AtomicBool::new(false);
        let move_it = |drives: &dyn Drives| {
            mo_space::folder_move::move_folder(
                &s(&from),
                &s(&d),
                3,
                ("omnisphere", mo_space::plugin_space::SpaceKind::Sounds, &[]),
                drives,
                &[],
                &nothing,
                &stop,
            )
        };
        // You said no: the copy made meanwhile goes, and the folder is as it was.
        let no = Unlock { no: true, ..Unlock::new(&[&guarded]) };
        let said = move_it(&OtherDrive(&no)).unwrap_err().text();
        assert_eq!(said, "You said no to Windows' administrator question. Nothing was moved.");
        assert_eq!(no.asked(), 1);
        assert!(!d.join("Spectrasonics").exists());
        assert_eq!(fs::read_to_string(from.join("Pad.db")).unwrap(), "pad");

        // Nobody to ask (a Mac, or Linux): it stops before copying anything.
        let said = move_it(&OtherDrive(&NoAdmin)).unwrap_err();
        assert_eq!(said, mo_space::folder_move::CantMove::NeedsAdmin);
        assert!(!d.join("Spectrasonics").exists());

        // Allowed, but Windows says no even then (the DAW has a file in it open): it says to close it.
        let in_use = Unlock::new(&[]);
        let said = move_it(&OtherDrive(&in_use)).unwrap_err().text();
        assert!(said.contains("even as administrator") && said.contains("close your DAW"), "{said}");
        assert!(said.ends_with("Nothing was moved."), "{said}");
        assert_eq!(in_use.asked(), 1);
        assert!(!d.join("Spectrasonics").exists());
        assert_eq!(fs::read_to_string(from.join("Pad.db")).unwrap(), "pad");
        lock(&guarded, false);
    }

    #[test]
    fn links_windows_guards_are_pointed_again_in_one_step() {
        let dir = fixtures("mo-space-admin-relink");
        if !locking_works(&dir) {
            return;
        }
        let (guarded, d) = (dir.join("ProgramData"), dir.join("D"));
        let data = d.join("Plugin Data");
        write(&data, "Spectrasonics/Pad.db", "pad");
        write(&data, "Toontrack/Kit.db", "kit");
        fs::create_dir_all(&guarded).unwrap();
        // Toontrack's move was made through a link to the folder Windows guards: the step is handed the real place.
        let through = dir.join("PD");
        folder_link(&guarded, &through);
        let mut all = Vec::new();
        for name in ["Spectrasonics", "Toontrack"] {
            folder_link(&data.join(name), &guarded.join(name));
            let from = if name == "Toontrack" { through.join(name) } else { guarded.join(name) };
            all.push(mo_space::folder_move::MovedFolder {
                from: s(&from),
                to: s(&data.join(name)),
                old_copy: s(&guarded.join(format!("{name} (old copy)"))),
                bytes: 3,
                moved: String::new(),
                key: name.to_lowercase(),
                kind: 2,
                shared_with: Vec::new(),
            });
        }
        let by_hand = d.join("Data");
        fs::rename(&data, &by_hand).unwrap();
        lock(&guarded, true);
        let unlock = Unlock::new(&[&guarded]);
        let now = mo_space::after_move::repoint(&s(&data), &s(&by_hand), &all, &[], &unlock);
        lock(&guarded, false);
        assert!(now.problems.is_empty(), "{:?}", now.problems);
        // One question for both.
        assert_eq!(unlock.asked(), 1);
        assert_eq!(fs::read_to_string(guarded.join("Spectrasonics/Pad.db")).unwrap(), "pad");
        assert_eq!(fs::read_to_string(guarded.join("Toontrack/Kit.db")).unwrap(), "kit");
        assert_eq!(now.moved[1].to, s(&by_hand.join("Toontrack")));
    }

    #[test]
    fn what_windows_guards_goes_to_the_bin_in_one_step_and_stays_when_you_say_no() {
        let dir = fixtures("mo-space-admin-bin");
        if !locking_works(&dir) {
            return;
        }
        let (guarded, d) = (dir.join("ProgramData"), dir.join("D"));
        write(&guarded, "Spectrasonics/Pad.db", "pad");
        fs::create_dir_all(&d).unwrap();
        let stop = AtomicBool::new(false);
        let moved = mo_space::folder_move::move_folder(
            &s(&guarded.join("Spectrasonics")),
            &s(&d),
            3,
            ("omnisphere", mo_space::plugin_space::SpaceKind::Sounds, &[]),
            &OtherDrive(&NoAdmin),
            &[],
            &nothing,
            &stop,
        )
        .unwrap();
        let all = vec![moved.clone()];
        lock(&guarded, true);
        let old_copy = Path::new(&moved.old_copy);

        // You said no: it stays, and says why.
        let mut bin = FolderBin::new(&dir);
        bin.admin = Some(Box::new(Unlock { no: true, bin: Some(bin.twin()), ..Unlock::new(&[&guarded]) }));
        let kept = mo_space::folder_move::remove_old_copy(&moved, &all, false, &bin, &stop);
        assert!(kept.removed.is_empty());
        assert_eq!(kept.problems[0].why, "You said no to Windows' administrator question.");
        assert!(old_copy.exists());

        // Something saves into it while Windows asks: the step sees it isn't what was checked, and leaves it.
        let saved = old_copy.join("User.db");
        let meanwhile =
            Unlock { bin: Some(bin.twin()), saved_meanwhile: Some(saved.clone()), ..Unlock::new(&[&guarded]) };
        bin.admin = Some(Box::new(meanwhile));
        let kept = mo_space::folder_move::remove_old_copy(&moved, &all, true, &bin, &stop);
        assert!(kept.removed.is_empty());
        assert!(kept.problems[0].why.contains("isn't as it was when the app checked it"), "{:?}", kept.problems);
        assert!(saved.exists());
        fs::remove_file(&saved).unwrap();

        // Asked and allowed: it goes to the bin, and the bin knows it.
        bin.admin = Some(Box::new(Unlock { bin: Some(bin.twin()), ..Unlock::new(&[&guarded]) }));
        let done = mo_space::folder_move::remove_old_copy(&moved, &all, false, &bin, &stop);
        assert!(done.problems.is_empty(), "{:?}", done.problems);
        assert_eq!(done.removed.len(), 1);
        assert!(done.removed[0].bin_id.is_some());
        assert!(!old_copy.exists());
        assert_eq!(bin.admin.as_deref().map(Unlock::asked), Some(1));
        lock(&guarded, false);
    }

    #[test]
    fn a_folder_inside_one_that_stays_after_all_is_tried_on_its_own() {
        use mo_space::plugin_clean::{Library, PluginUse, apply, lines};
        use mo_space::plugin_space::{PluginSpace, SpaceFolder, SpaceKind};
        use std::collections::HashMap;

        let dir = fixtures("mo-space-admin-plugin-clean");
        if !locking_works(&dir) {
            return;
        }
        // Xfer can't be moved by you (its folder is guarded); Serum Presets, inside it, can.
        let docs = dir.join("Docs");
        let make = || {
            write(&docs, "Xfer/Settings.xml", "x");
            write(&docs, "Xfer/Serum Presets/Lead.fxp", "lead");
        };
        make();
        let folder = |rel: &str, bytes| SpaceFolder {
            path: s(&docs.join(rel)),
            kind: SpaceKind::Presets,
            bytes,
            shared_with: Vec::new(),
            added: false,
        };
        let serum = PluginSpace {
            key: "serum".into(),
            name: "Serum".into(),
            vendor: String::new(),
            folders: vec![folder("Xfer", 1), folder("Xfer/Serum Presets", 4)],
        };
        let uses: HashMap<String, PluginUse> =
            [("serum".to_owned(), PluginUse { projects: 0, unused: true })].into_iter().collect();
        let names = |k: &str| k.to_owned();
        let library = Library {
            protected_roots: &[],
            kept_folders: &[],
            uses: &uses,
            sample_at: &|_| None,
            sample_named: &|_| false,
            names: &names,
        };
        let stop = AtomicBool::new(false);
        let all = std::slice::from_ref(&serum);

        // You say no: Xfer stays, and Serum Presets, which would have gone with it, goes on its own.
        let card = lines(&serum, all, &library, &stop).unwrap();
        lock(&docs, true);
        let mut bin = FolderBin::new(&dir);
        bin.admin = Some(Box::new(Unlock { no: true, ..Unlock::new(&[&docs]) }));
        let done = apply(&card, &serum, all, &library, &bin, &stop);
        lock(&docs, false);
        assert_eq!(done.removed.len(), 1, "{done:?}");
        assert!(done.removed[0].path.ends_with("Serum Presets"));
        assert_eq!(done.problems.len(), 1);
        assert!(done.problems[0].path.ends_with("Xfer"));
        assert_eq!(done.problems[0].why, "You said no to Windows' administrator question.");
        assert!(done.went_with.is_empty());
        assert!(docs.join("Xfer/Settings.xml").exists());

        // You say yes, but even the administrator is refused (the DAW has a file in Xfer open), and Windows guards
        // Serum Presets too: it stays as well, and Windows isn't asked a second time.
        make();
        let card = lines(&serum, all, &library, &stop).unwrap();
        lock(&docs, true);
        lock(&docs.join("Xfer"), true);
        let mut bin = FolderBin::new(&dir.join("refused"));
        bin.admin = Some(Box::new(Unlock { bin: Some(bin.twin()), ..Unlock::new(&[]) }));
        let done = apply(&card, &serum, all, &library, &bin, &stop);
        lock(&docs.join("Xfer"), false);
        lock(&docs, false);
        assert!(done.removed.is_empty(), "{done:?}");
        assert_eq!(done.problems.len(), 2);
        assert!(done.problems[0].why.contains("even as administrator"), "{:?}", done.problems);
        assert!(done.problems[1].why.contains("asked once already"), "{:?}", done.problems);
        assert_eq!(bin.admin.as_deref().map(Unlock::asked), Some(1));

        // You say yes: Xfer goes as administrator, and Serum Presets with it.
        let card = lines(&serum, all, &library, &stop).unwrap();
        lock(&docs, true);
        let mut bin = FolderBin::new(&dir.join("second"));
        bin.admin = Some(Box::new(Unlock { bin: Some(bin.twin()), ..Unlock::new(&[&docs]) }));
        let done = apply(&card, &serum, all, &library, &bin, &stop);
        lock(&docs, false);
        assert!(done.problems.is_empty(), "{:?}", done.problems);
        assert_eq!(done.removed.len(), 1);
        assert!(done.removed[0].path.ends_with("Xfer"));
        assert_eq!(done.went_with.len(), 1);
        assert!(!docs.join("Xfer").exists());
        assert_eq!(bin.admin.as_deref().map(Unlock::asked), Some(1));
    }

    #[test]
    fn the_folder_moved_folders_live_in_says_to_rename_it_in_file_explorer_when_windows_says_no() {
        let dir = fixtures("mo-space-admin-home");
        if !locking_works(&dir) {
            return;
        }
        let guarded = dir.join("Guarded");
        write(&guarded, "Plugin Data/Spectrasonics/Pad.db", "pad");
        lock(&guarded, true);
        let stop = AtomicBool::new(false);
        let home = guarded.join("Plugin Data");
        let said = mo_space::after_move::move_home(&s(&home), &s(&dir), &[], &[], 3, &SameDrive, &nothing, &stop);
        lock(&guarded, false);
        let said = said.unwrap_err().text();
        assert!(said.contains("rename the folder in File Explorer"), "{said}");
        assert!(home.join("Spectrasonics/Pad.db").exists());
    }

    #[test]
    fn spare_plugin_copies_in_program_files_go_in_one_step() {
        use mo_space::spare::{SpareCopy, Why, look, remove};

        let dir = fixtures("mo-space-admin-spare");
        if !locking_works(&dir) {
            return;
        }
        let program_files = dir.join("Program Files");
        write(&program_files, "VST2/Serum_x64.dll", "serum");
        write(&program_files, "VST2/Massive.dll", "massive");
        write(&program_files, "VST3/Serum.vst3", "serum");
        let copy = |name: &str| {
            let path = s(&program_files.join("VST2").join(name));
            let (bytes, stamp) = look(&path, &AtomicBool::new(false));
            let used_copies = vec![s(&program_files.join("VST3/Serum.vst3"))];
            SpareCopy {
                name: name.to_owned(),
                format: "VST2".to_owned(),
                path,
                bytes,
                stamp,
                why: Why::OtherFormatsUsed { used: vec!["VST3".to_owned()], projects: 1, used_copies },
            }
        };
        let picked = vec![copy("Serum_x64.dll"), copy("Massive.dll")];
        let vst2 = program_files.join("VST2");
        lock(&vst2, true);
        let mut bin = FolderBin::new(&dir);
        bin.admin = Some(Box::new(Unlock { bin: Some(bin.twin()), ..Unlock::new(&[&vst2]) }));
        let done = remove(&picked, &picked, &bin, &AtomicBool::new(false));
        lock(&vst2, false);
        assert!(done.problems.is_empty(), "{:?}", done.problems);
        assert_eq!(done.removed.len(), 2);
        assert!(done.removed.iter().all(|r| r.bin_id.is_some()));
        assert_eq!(bin.admin.as_deref().map(Unlock::asked), Some(1));
        assert!(!vst2.join("Serum_x64.dll").exists() && !vst2.join("Massive.dll").exists());
    }

    #[test]
    fn bitwig_copies_windows_guards_go_in_one_step_and_are_checked_again_before_it_asks() {
        use mo_space::bitwig::{FixedProject, remove_copies};

        let dir = fixtures("mo-space-admin-bitwig");
        if !locking_works(&dir) {
            return;
        }
        let library = dir.join("Library");
        let project = dir.join("Project");
        write(&library, "Kicks.multisample", "kicks");
        write(&project, "P (fixed).bwproject", "fixed");
        write(&project, "multi-samples/Kicks-2.multisample", "kicks");
        let ms = project.join("multi-samples");
        let changed = [(ms.join("Kicks-2.multisample"), library.join("Kicks.multisample"))];
        let libraries = [library.clone()];
        let file = project.join("P (fixed).bwproject");
        let go = |still_named: &(dyn Fn(&Path, &Path) -> Option<String> + Sync)| {
            let fix = FixedProject { file: &file, changed: &changed, library_folders: &libraries, still_named };
            lock(&ms, true);
            let mut bin = FolderBin::new(&dir);
            bin.admin = Some(Box::new(Unlock { bin: Some(bin.twin()), ..Unlock::new(&[&ms]) }));
            let done = remove_copies(&fix, &bin, &AtomicBool::new(false));
            lock(&ms, false);
            (done, bin.admin.as_deref().map(Unlock::asked))
        };
        // A save made while the other lines were checked names it: it stays, and Windows isn't asked.
        let calls = AtomicUsize::new(0);
        let named_later = |_: &Path, _: &Path| {
            (calls.fetch_add(1, Ordering::SeqCst) > 0).then(|| "A save made since uses it.".to_owned())
        };
        let (done, asked) = go(&named_later);
        assert!(done.removed.is_empty());
        assert_eq!(done.problems[0].why, "A save made since uses it.");
        assert_eq!(asked, Some(0));
        assert!(ms.join("Kicks-2.multisample").exists());
        // Otherwise it goes, Windows asked once.
        let (done, asked) = go(&|_: &Path, _: &Path| None);
        assert!(done.problems.is_empty(), "{:?}", done.problems);
        assert_eq!(done.removed.len(), 1);
        assert_eq!(asked, Some(1));
        assert!(!ms.join("Kicks-2.multisample").exists() && library.join("Kicks.multisample").exists());
    }
}
