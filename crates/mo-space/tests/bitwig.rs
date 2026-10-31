//! Bitwig's copies of Library multisamples, on folders made in the test build's own fixtures folder under fixed
//! names. The files are made-up bytes, not anyone's sounds.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

use mo_space::bitwig::{FixedProject, LibraryCopy, LibraryIndex, remove_copies};
use mo_space::clean::{Bin, Problem, Removed, What, undo};
use mo_space::key;

fn fixtures(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &Path, rel: &str, bytes: &[u8]) {
    let p = dir.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, bytes).unwrap();
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// A found copy with its paths as Space compares them (on Windows, a path written with `/` is found with `\`).
fn keyed(c: &LibraryCopy) -> (String, String, u64) {
    (key(&c.in_project), key(&c.library_file), c.bytes)
}

#[test]
fn a_projects_copy_is_found_only_when_its_the_same_as_a_library_file_byte_for_byte() {
    let dir = fixtures("mo-space-bitwig-copies");
    let (library, project) = (dir.join("Library/Multisamples"), dir.join("Projects/Garage"));
    let kicks = [1u8; 4096];
    let mut hats = [2u8; 4096];
    write(&library, "Kicks 1 Select.multisample", &kicks);
    // The same bytes under another name in the Library: the copy's own name wins.
    write(&library, "Older/Kicks Backup.multisample", &kicks);
    write(&library, "OHH 1 Select.multisample", &hats);
    write(&library, "Snare.multisample", b"snare");

    write(&project, "multi-samples/Kicks 1 Select-3-2-2-3-2-2-2-2-2.multisample", &kicks);
    // Same size as the Library's, one byte different (edited in the project): it stays the project's own.
    hats[100] = 9;
    write(&project, "multi-samples/OHH 1 Select-2-2-2-3-2-2-2.multisample", &hats);
    // A size no Library file has.
    write(&project, "multi-samples/Pad-2.multisample", b"pad");
    // Not a multisample, and not in the multisample folder.
    write(&project, "multi-samples/notes.txt", &kicks);
    write(&project, "samples/Kicks 1 Select.multisample", &kicks);

    let stop = AtomicBool::new(false);
    let index = LibraryIndex::new(&[dir.join("Library")], &stop).unwrap();
    let copies = index.copies_in(&project, &stop).unwrap();
    assert_eq!(
        copies.iter().map(keyed).collect::<Vec<_>>(),
        [keyed(&LibraryCopy {
            in_project: s(&project.join("multi-samples/Kicks 1 Select-3-2-2-3-2-2-2-2-2.multisample")),
            library_file: s(&library.join("Kicks 1 Select.multisample")),
            bytes: 4096,
        })]
    );

    // A project without the folder has none.
    assert!(index.copies_in(&dir.join("Projects"), &stop).unwrap().is_empty());
}

#[test]
fn a_copy_named_otherwise_takes_the_library_file_with_the_shortest_name() {
    let dir = fixtures("mo-space-bitwig-renamed");
    let (library, project) = (dir.join("Library"), dir.join("Project"));
    let bass = [7u8; 1000];
    write(&library, "Bass Long Name.multisample", &bass);
    // The shortest name, though not the shortest path.
    write(&library, "Deep/Down/Here/Bass.multisample", &bass);
    write(&project, "multi-samples/My Bass-2.multisample", &bass);
    let stop = AtomicBool::new(false);
    let copies = LibraryIndex::new(std::slice::from_ref(&library), &stop).unwrap().copies_in(&project, &stop).unwrap();
    assert_eq!(copies.len(), 1);
    assert_eq!(key(&copies[0].library_file), key(&s(&library.join("Deep/Down/Here/Bass.multisample"))));
}

#[test]
fn a_name_with_its_own_number_is_matched_before_a_shorter_one() {
    let dir = fixtures("mo-space-bitwig-own-number");
    let (library, project) = (dir.join("Library"), dir.join("Project"));
    let pad = [5u8; 300];
    write(&library, "Pad.multisample", &pad);
    write(&library, "Pad-10.multisample", &pad);
    write(&project, "multi-samples/Pad-10-2.multisample", &pad);
    let stop = AtomicBool::new(false);
    let copies = LibraryIndex::new(std::slice::from_ref(&library), &stop).unwrap().copies_in(&project, &stop).unwrap();
    assert_eq!(copies[0].library_file, s(&library.join("Pad-10.multisample")));
}

#[test]
fn another_projects_copies_never_count_as_library_files() {
    let dir = fixtures("mo-space-bitwig-other-project");
    let projects = dir.join("Projects");
    let kicks = [1u8; 512];
    write(&projects, "A/multi-samples/Kicks-2.multisample", &kicks);
    write(&projects, "B/multi-samples/Kicks-2.multisample", &kicks);
    // A template keeps its copies in a multi-samples folder too.
    write(&projects, "Keys.bwtemplate/project/multi-samples/Kicks-3.multisample", &kicks);
    let stop = AtomicBool::new(false);
    // The folder holding the projects given as a Library folder.
    let index = LibraryIndex::new(std::slice::from_ref(&projects), &stop).unwrap();
    assert!(index.copies_in(&projects.join("A"), &stop).unwrap().is_empty());
    assert!(index.copies_in(&projects.join("B"), &stop).unwrap().is_empty());
}

#[test]
fn a_library_file_changed_since_it_was_read_is_read_again() {
    let dir = fixtures("mo-space-bitwig-library-changed");
    let library = dir.join("Library");
    let old = [1u8; 256];
    write(&library, "Kicks.multisample", &old);
    write(&dir, "First/multi-samples/Kicks-2.multisample", &old);
    write(&dir, "Second/multi-samples/Kicks-2.multisample", &old);
    let stop = AtomicBool::new(false);
    let index = LibraryIndex::new(std::slice::from_ref(&library), &stop).unwrap();
    assert_eq!(index.copies_in(&dir.join("First"), &stop).unwrap().len(), 1);
    // Saved again in Bitwig: the same size, other bytes, a later time.
    fs::write(library.join("Kicks.multisample"), [2u8; 256]).unwrap();
    let later = SystemTime::now() + Duration::from_secs(3600);
    fs::File::options().write(true).open(library.join("Kicks.multisample")).unwrap().set_modified(later).unwrap();
    assert!(index.copies_in(&dir.join("Second"), &stop).unwrap().is_empty());
}

#[test]
fn a_stop_stops_it() {
    let dir = fixtures("mo-space-bitwig-stop");
    write(&dir, "Library/A.multisample", b"a");
    write(&dir, "Project/multi-samples/A-2.multisample", b"a");
    let stopped = AtomicBool::new(true);
    assert!(LibraryIndex::new(&[dir.join("Library")], &stopped).is_err());
    let index = LibraryIndex::new(&[dir.join("Library")], &AtomicBool::new(false)).unwrap();
    assert!(index.copies_in(&dir.join("Project"), &stopped).is_err());
}

#[cfg(unix)]
#[test]
fn a_copy_that_is_a_link_or_a_multisample_folder_that_is_a_link_is_left_out() {
    let dir = fixtures("mo-space-bitwig-links");
    let a = [3u8; 64];
    write(&dir, "Library/A.multisample", &a);
    write(&dir, "Elsewhere/A-2.multisample", &a);
    fs::create_dir_all(dir.join("Project/multi-samples")).unwrap();
    std::os::unix::fs::symlink(dir.join("Library/A.multisample"), dir.join("Project/multi-samples/A-3.multisample"))
        .unwrap();
    fs::create_dir_all(dir.join("Linked")).unwrap();
    std::os::unix::fs::symlink(dir.join("Elsewhere"), dir.join("Linked/multi-samples")).unwrap();
    let stop = AtomicBool::new(false);
    let index = LibraryIndex::new(&[dir.join("Library")], &stop).unwrap();
    assert!(index.copies_in(&dir.join("Project"), &stop).unwrap().is_empty());
    assert!(index.copies_in(&dir.join("Linked"), &stop).unwrap().is_empty());
}

#[test]
fn a_library_folder_inside_the_project_never_counts() {
    let dir = fixtures("mo-space-bitwig-library-in-project");
    let project = dir.join("Projects/Garage");
    write(&project, "multi-samples/Kicks 1 Select-2.multisample", &[1u8; 64]);
    write(&project, "multi-samples/Kicks 1 Select.multisample", &[1u8; 64]);
    let stop = AtomicBool::new(false);
    // The project (or its multisample folder) given as a Library folder by mistake.
    for library in [project.clone(), project.join("multi-samples")] {
        let index = LibraryIndex::new(&[library], &stop).unwrap();
        assert!(index.copies_in(&project, &stop).unwrap().is_empty());
    }
}

/// A folder standing in for the Recycle Bin.
struct FolderBin {
    dir: PathBuf,
    held: RefCell<Vec<(String, PathBuf)>>,
}

impl FolderBin {
    fn new(dir: &Path) -> Self {
        FolderBin { dir: dir.join("bin"), held: RefCell::new(Vec::new()) }
    }
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

/// Nothing else in the project folder names a copy.
fn named_nowhere(_: &Path, _: &Path) -> Option<String> {
    None
}

fn fixed<'a>(file: &'a Path, changed: &'a [(PathBuf, PathBuf)], library: &'a [PathBuf]) -> FixedProject<'a> {
    FixedProject { file, changed, library_folders: library, still_named: &named_nowhere }
}

#[test]
fn a_copy_the_fixed_project_no_longer_uses_goes_only_while_its_still_the_library_files_twin() {
    let dir = fixtures("mo-space-bitwig-remove");
    let (library, project) = (dir.join("Library"), dir.join("Projects/Garage"));
    let kicks = [1u8; 2048];
    write(&library, "Kicks 1 Select.multisample", &kicks);
    write(&library, "OHH 1 Select.multisample", &[2u8; 2048]);
    write(&project, "Garage.bwproject", b"original");
    write(&project, "Garage (fixed).bwproject", b"fixed");
    write(&project, "multi-samples/Kicks 1 Select-3-2.multisample", &kicks);
    // Edited in the project since it was found: the same size, other bytes.
    write(&project, "multi-samples/OHH 1 Select-2.multisample", &[3u8; 2048]);
    // Not in the multisample folder.
    write(&project, "samples/Kicks 1 Select.multisample", &kicks);
    // What it'd use instead is another project's copy, outside the Library.
    write(&dir, "Projects/Other/multi-samples/Kicks.multisample", &kicks);
    write(&project, "multi-samples/Kicks-4.multisample", &kicks);
    // Its Library file has gone.
    write(&project, "multi-samples/Snare-2.multisample", b"snare");
    // Another save in the folder still names it.
    write(&project, "multi-samples/Kicks-5.multisample", &kicks);

    let ms = project.join("multi-samples");
    let changed = vec![
        (ms.join("Kicks 1 Select-3-2.multisample"), library.join("Kicks 1 Select.multisample")),
        (ms.join("OHH 1 Select-2.multisample"), library.join("OHH 1 Select.multisample")),
        (project.join("samples/Kicks 1 Select.multisample"), library.join("Kicks 1 Select.multisample")),
        (ms.join("Kicks-4.multisample"), dir.join("Projects/Other/multi-samples/Kicks.multisample")),
        (ms.join("Snare-2.multisample"), library.join("Snare.multisample")),
        (ms.join("Kicks-5.multisample"), library.join("Kicks 1 Select.multisample")),
        // Twice: once is enough.
        (ms.join("Kicks 1 Select-3-2.multisample"), library.join("Kicks 1 Select.multisample")),
    ];
    let libraries = [library.clone()];
    let still_named = |copy: &Path, _: &Path| {
        copy.ends_with("Kicks-5.multisample").then(|| "Garage 19.bwproject still uses it, so it stays.".to_owned())
    };
    let file = project.join("Garage (fixed).bwproject");
    let fix = FixedProject { still_named: &still_named, ..fixed(&file, &changed, &libraries) };
    let bin = FolderBin::new(&dir);
    let done = remove_copies(&fix, &bin, &AtomicBool::new(false));
    let gone: Vec<&str> = done.removed.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(gone, [s(&ms.join("Kicks 1 Select-3-2.multisample"))]);
    assert_eq!(done.removed[0].what, What::BitwigCopy);
    assert_eq!(done.bytes(), 2048);
    let why: Vec<&str> = done.problems.iter().map(|p| p.why.as_str()).collect();
    assert_eq!(
        why,
        [
            "It isn't the same as the Library file now, so it stays.",
            "It isn't in the fixed project's own multi-samples folder, so it stays.",
            "What the fixed project uses instead isn't in the Library, so the copy stays.",
            "The Library file the fixed project uses isn't there now, so the copy stays.",
            "Garage 19.bwproject still uses it, so it stays.",
        ]
    );
    // The Library file stays, and undo puts the copy back.
    assert!(library.join("Kicks 1 Select.multisample").exists());
    assert!(undo(&done, &bin).is_empty());
    assert!(ms.join("Kicks 1 Select-3-2.multisample").exists());
}

#[test]
fn a_library_folder_that_holds_projects_never_gives_a_projects_copy() {
    let dir = fixtures("mo-space-bitwig-remove-wide-library");
    let project = dir.join("Projects/Garage");
    write(&project, "Garage (fixed).bwproject", b"fixed");
    write(&project, "multi-samples/A-2.multisample", b"a");
    write(&project, "multi-samples/Self-2.multisample", b"self");
    write(&dir, "Projects/Other/multi-samples/A.multisample", b"a");
    let ms = project.join("multi-samples");
    let changed = [
        (ms.join("A-2.multisample"), dir.join("Projects/Other/multi-samples/A.multisample")),
        // Pointed at itself.
        (ms.join("Self-2.multisample"), ms.join("Self-2.multisample")),
    ];
    // The whole Bitwig folder given as the Library.
    let libraries = [dir.clone()];
    let done = remove_copies(
        &fixed(&project.join("Garage (fixed).bwproject"), &changed, &libraries),
        &FolderBin::new(&dir),
        &AtomicBool::new(false),
    );
    assert!(done.removed.is_empty());
    assert!(done.problems.iter().all(|p| p.why.contains("is a project's copy too")), "{:?}", done.problems);
}

#[test]
fn without_a_fixed_project_or_after_a_stop_every_copy_stays() {
    let dir = fixtures("mo-space-bitwig-remove-no-fix");
    let (library, project) = (dir.join("Library"), dir.join("Project"));
    write(&library, "A.multisample", b"a");
    write(&project, "multi-samples/A-2.multisample", b"a");
    write(&project, "Project.bwproject", b"original");
    let changed = [(project.join("multi-samples/A-2.multisample"), library.join("A.multisample"))];
    let libraries = [library.clone()];
    let bin = FolderBin::new(&dir);
    let go = |file: &str, stop: bool| {
        let done = remove_copies(&fixed(&project.join(file), &changed, &libraries), &bin, &AtomicBool::new(stop));
        assert!(done.removed.is_empty());
        done.problems[0].why.clone()
    };
    assert_eq!(
        go("Project (fixed).bwproject", false),
        "The fixed project isn't there now, so the copies it was made for stay."
    );
    // The original project, given by mistake.
    assert_eq!(go("Project.bwproject", false), "The project given isn't a \"(fixed)\" one, so its copies stay.");
    write(&project, "Project (fixed 2).bwproject", b"fixed");
    assert_eq!(go("Project (fixed 2).bwproject", true), "Stopped before it went.");
    assert!(project.join("multi-samples/A-2.multisample").exists());
}

#[cfg(unix)]
#[test]
fn a_link_a_linked_folder_or_a_hard_link_to_the_library_file_stays() {
    let dir = fixtures("mo-space-bitwig-remove-links");
    let library = dir.join("Library");
    write(&library, "A.multisample", b"a");
    write(&dir, "Elsewhere/A-2.multisample", b"a");
    let libraries = [library.clone()];
    let bin = FolderBin::new(&dir);
    let go = |project: &Path| {
        write(project, "P (fixed).bwproject", b"fixed");
        let changed = [(project.join("multi-samples/A-2.multisample"), library.join("A.multisample"))];
        let done = remove_copies(
            &fixed(&project.join("P (fixed).bwproject"), &changed, &libraries),
            &bin,
            &AtomicBool::new(false),
        );
        assert!(done.removed.is_empty());
        done.problems[0].why.clone()
    };
    // The copy is a link to a file elsewhere.
    let one = dir.join("One");
    fs::create_dir_all(one.join("multi-samples")).unwrap();
    std::os::unix::fs::symlink(dir.join("Elsewhere/A-2.multisample"), one.join("multi-samples/A-2.multisample"))
        .unwrap();
    assert_eq!(go(&one), "It isn't a multisample file of the project's own, so it stays.");
    // The multisample folder is a link to a folder elsewhere.
    let two = dir.join("Two");
    fs::create_dir_all(&two).unwrap();
    std::os::unix::fs::symlink(dir.join("Elsewhere"), two.join("multi-samples")).unwrap();
    assert_eq!(go(&two), "It isn't in the fixed project's own multi-samples folder, so it stays.");
    // The copy is the Library file itself, by a hard link.
    let three = dir.join("Three");
    fs::create_dir_all(three.join("multi-samples")).unwrap();
    fs::hard_link(library.join("A.multisample"), three.join("multi-samples/A-2.multisample")).unwrap();
    assert!(go(&three).contains("a hard link"));
    assert!(dir.join("Elsewhere/A-2.multisample").exists() && library.join("A.multisample").exists());
}

#[cfg(unix)]
#[test]
fn one_copy_given_two_ways_goes_once_and_a_good_second_way_still_counts() {
    let dir = fixtures("mo-space-bitwig-remove-two-ways");
    let (library, project) = (dir.join("Library"), dir.join("Project"));
    write(&library, "A.multisample", b"a");
    write(&project, "P (fixed).bwproject", b"fixed");
    write(&project, "multi-samples/A-2.multisample", b"a");
    write(&project, "multi-samples/B-2.multisample", b"a");
    std::os::unix::fs::symlink(project.join("multi-samples"), project.join("Old name")).unwrap();
    let ms = project.join("multi-samples");
    let changed = [
        // B: first with a Library file that isn't there, then as it should be.
        (ms.join("B-2.multisample"), library.join("Gone.multisample")),
        (ms.join("B-2.multisample"), library.join("A.multisample")),
        // A: then again through a linked folder's name.
        (ms.join("A-2.multisample"), library.join("A.multisample")),
        (project.join("Old name/A-2.multisample"), library.join("A.multisample")),
    ];
    let libraries = [library.clone()];
    let file = project.join("P (fixed).bwproject");
    let done = remove_copies(&fixed(&file, &changed, &libraries), &FolderBin::new(&dir), &AtomicBool::new(false));
    let gone: Vec<&str> = done.removed.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(gone, [s(&ms.join("B-2.multisample")), s(&ms.join("A-2.multisample"))]);
    let why: Vec<&str> = done.problems.iter().map(|p| p.why.as_str()).collect();
    assert_eq!(why.len(), 2);
    assert!(why[0].contains("isn't there now") && why[1].starts_with("It's the same file as "), "{why:?}");
}
