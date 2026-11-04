//! The Space page's look for Bitwig copies, from start to end, on folders made in the test build's own fixtures folder
//! under fixed names: never a real Bitwig folder. The files are made-up bytes, not anyone's sounds.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use mo_app::space_bitwig::{Copies, look};
use mo_space::key;

fn fixtures(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::canonicalize(&dir).unwrap()
}

fn write(dir: &Path, rel: &str, bytes: &[u8]) {
    let p = dir.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, bytes).unwrap();
}

/// A project as a test compares it: its folder's name, the room its copies take, and each copy's file name with the
/// Library file it's the same as.
type Seen = (String, u64, Vec<(String, String)>);

/// What a look found, as names and sizes.
fn summary(found: &Copies) -> Vec<Seen> {
    let name = |p: &str| Path::new(p).file_name().unwrap().to_string_lossy().into_owned();
    found
        .projects
        .iter()
        .map(|p| {
            let copies = p.copies.iter().map(|c| (name(&c.in_project), name(&c.library_file))).collect();
            (p.name(), p.bytes(), copies)
        })
        .collect()
}

#[test]
fn the_copies_in_each_project_are_found_and_listed_biggest_first() {
    let dir = fixtures("mo-app-space-bitwig");
    let studio = dir.join("K/Bitwig Studio");
    let (kicks, pads, own, bass) = ([1u8; 4096], [2u8; 8192], [3u8; 100], [4u8; 2048]);
    write(&studio, "Library/Multisamples/Kicks 1 Select.multisample", &kicks);
    write(&studio, "Library/Multisamples/Pads.multisample", &pads);
    // Bitwig's own place for the Library, in Documents.
    let documents = dir.join("Documents");
    write(&documents, "Bitwig Studio/Library/Multisamples/Bass.multisample", &bass);

    let projects = studio.join("Projects");
    write(&projects, "GARAGE/GARAGE.bwproject", b"project");
    write(&projects, "GARAGE/multi-samples/Kicks 1 Select-3-2.multisample", &kicks);
    write(&projects, "GARAGE/multi-samples/Pads-2.multisample", &pads);
    // The project's own multisample, the same as no Library file: it stays the project's.
    write(&projects, "GARAGE/multi-samples/Own.multisample", &own);
    write(&projects, "Night/Night.bwproject", b"project");
    write(&projects, "Night/multi-samples/Kicks 1 Select-2.multisample", &kicks);
    write(&projects, "Sketch/Sketch.bwproject", b"project");
    write(&projects, "Sketch/multi-samples/Own.multisample", &own);
    // A project kept outside Bitwig Studio is compared with the Library too.
    let elsewhere = dir.join("I/Music");
    write(&elsewhere, "Song/Song.bwproject", b"project");
    write(&elsewhere, "Song/multi-samples/Bass-2.multisample", &bass);
    write(&elsewhere, "Song/multi-samples/Kicks 1 Select-2.multisample", &kicks);

    let roots = [projects.clone(), elsewhere.clone()];
    let stop = AtomicBool::new(false);
    let mut index = None;
    let mut heard = Vec::new();
    let found = look(&roots, Some(&documents), &mut index, &stop, |done, of| heard.push((done, of))).unwrap();
    assert_eq!(heard, [(0, 4), (1, 4), (2, 4), (3, 4), (4, 4)]);
    assert_eq!(found.folders, 4);
    assert_eq!(
        found.library.iter().map(|l| key(&l.to_string_lossy())).collect::<Vec<_>>(),
        [studio.join("Library"), documents.join("Bitwig Studio/Library")].map(|l| key(&l.to_string_lossy()))
    );
    let pair = |copy: &str, library: &str| (copy.to_owned(), library.to_owned());
    assert_eq!(
        summary(&found),
        [
            (
                "GARAGE".to_owned(),
                4096 + 8192,
                vec![
                    pair("Kicks 1 Select-3-2.multisample", "Kicks 1 Select.multisample"),
                    pair("Pads-2.multisample", "Pads.multisample")
                ]
            ),
            (
                "Song".to_owned(),
                2048 + 4096,
                vec![
                    pair("Bass-2.multisample", "Bass.multisample"),
                    pair("Kicks 1 Select-2.multisample", "Kicks 1 Select.multisample")
                ]
            ),
            ("Night".to_owned(), 4096, vec![pair("Kicks 1 Select-2.multisample", "Kicks 1 Select.multisample")]),
        ]
    );
    assert_eq!((found.bytes(), found.count()), (4096 + 8192 + 2048 + 4096 + 4096, 5));

    // The next look keeps the Library it read: a multisample added since is found, and one taken out isn't.
    write(&studio, "Library/Multisamples/Own.multisample", &own);
    fs::remove_file(studio.join("Library/Multisamples/Pads.multisample")).unwrap();
    let again = look(&roots, Some(&documents), &mut index, &stop, |_, _| {}).unwrap();
    let names: Vec<(String, usize)> = again.projects.iter().map(|p| (p.name(), p.copies.len())).collect();
    assert_eq!(
        names,
        [("Song".to_owned(), 2), ("GARAGE".to_owned(), 2), ("Night".to_owned(), 1), ("Sketch".to_owned(), 1)]
    );
}

#[test]
fn without_projects_or_a_library_nothing_is_read() {
    let dir = fixtures("mo-app-space-bitwig-none");
    let stop = AtomicBool::new(false);
    // No project keeps copies of multisamples.
    write(&dir, "Projects/Sketch/Sketch.bwproject", b"project");
    let found = look(&[dir.join("Projects")], None, &mut None, &stop, |_, _| {}).unwrap();
    assert_eq!(found, Copies::default());
    // Copies, but no Bitwig Studio folder above them and none in Documents: there's no Library to compare them with.
    write(&dir, "Projects/Song/Song.bwproject", b"project");
    write(&dir, "Projects/Song/multi-samples/Kicks-2.multisample", b"kicks");
    let mut index = None;
    let found = look(&[dir.join("Projects")], Some(&dir.join("No documents")), &mut index, &stop, |_, _| {}).unwrap();
    assert_eq!((found.folders, found.library.len(), found.projects.len()), (1, 0, 0));
    assert!(index.is_none(), "no Library was read");
}

#[test]
fn a_stop_stops_it_and_the_next_look_starts_again() {
    let dir = fixtures("mo-app-space-bitwig-stop");
    let studio = dir.join("Bitwig Studio");
    write(&studio, "Library/Kicks.multisample", b"kicks");
    write(&studio, "Projects/Song/Song.bwproject", b"project");
    write(&studio, "Projects/Song/multi-samples/Kicks-2.multisample", b"kicks");
    let roots = [studio.join("Projects")];
    let mut index = None;
    assert!(look(&roots, None, &mut index, &AtomicBool::new(true), |_, _| {}).is_err());
    let found = look(&roots, None, &mut index, &AtomicBool::new(false), |_, _| {}).unwrap();
    assert_eq!(found.count(), 1);
}
