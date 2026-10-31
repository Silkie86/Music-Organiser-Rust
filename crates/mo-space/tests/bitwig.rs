//! Bitwig's copies of Library multisamples, on folders made in the test build's own fixtures folder under fixed
//! names. The files are made-up bytes, not anyone's sounds.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

use mo_space::bitwig::{LibraryCopy, LibraryIndex};

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
        copies,
        [LibraryCopy {
            in_project: s(&project.join("multi-samples/Kicks 1 Select-3-2-2-3-2-2-2-2-2.multisample")),
            library_file: s(&library.join("Kicks 1 Select.multisample")),
            bytes: 4096,
        }]
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
    assert_eq!(copies[0].library_file, s(&library.join("Deep/Down/Here/Bass.multisample")));
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
