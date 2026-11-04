//! Backing up by folder, on folders made in the test build's own fixtures folder under fixed names.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use mo_backup::folders::{Folder, Mode, from_text, gather, to_text};

fn fixtures(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::canonicalize(&dir).unwrap()
}

#[test]
fn folders_are_walked_once_and_keep_their_own_choice() {
    let root = fixtures("backup-folders");
    let music = root.join("Music");
    let bounces = music.join("Bounces");
    fs::create_dir_all(&bounces).unwrap();
    fs::write(music.join("Song.flp"), b"song").unwrap();
    fs::write(bounces.join("Mix.wav"), b"mix").unwrap();
    let list = vec![
        Folder { path: music.clone(), mode: Mode::Slimmed },
        Folder { path: bounces.clone(), mode: Mode::Whole },
        Folder { path: root.join("Unplugged"), mode: Mode::Whole },
    ];
    let got = gather(&list, &root.join("Backup"), &AtomicBool::new(false)).unwrap();
    assert_eq!(got.files.len(), 2, "the inner folder isn't walked twice");
    assert_eq!(got.missing, vec![root.join("Unplugged")]);
}

/// A folder link, as Windows has them (a junction) or a symbolic link elsewhere.
fn folder_link(target: &Path, at: &Path) {
    #[cfg(windows)]
    junction::create(target, at).unwrap();
    #[cfg(not(windows))]
    std::os::unix::fs::symlink(target, at).unwrap();
}

#[test]
fn a_listed_folder_that_is_a_link_is_walked_where_it_really_is() {
    // Review 2026-11-03: a listed junction (Space's folder move leaves one) was walked as empty and not reported.
    let root = fixtures("backup-folders-link");
    let real = root.join("K").join("FL Projects");
    fs::create_dir_all(real.join("Tune")).unwrap();
    fs::write(real.join("Tune").join("Tune.flp"), b"song").unwrap();
    let linked = root.join("Projects");
    folder_link(&real, &linked);
    let gone = root.join("Gone");
    folder_link(&root.join("Nowhere"), &gone);
    let list =
        vec![Folder { path: linked.clone(), mode: Mode::Slimmed }, Folder { path: gone.clone(), mode: Mode::Slimmed }];
    let got = gather(&list, &root.join("Backup"), &AtomicBool::new(false)).unwrap();
    let paths: Vec<&Path> = got.files.iter().map(|f| f.path.as_path()).collect();
    assert_eq!(paths, vec![linked.join("Tune").join("Tune.flp")], "the listed spelling, as projects name it");
    assert_eq!(got.missing, vec![gone], "a link to nowhere is missing");
}

#[test]
fn a_listed_folder_reached_through_a_link_is_walked_too() {
    // The outer folder's walk leaves the link alone, so the folder inside it isn't covered by that walk.
    let root = fixtures("backup-folders-through-link");
    let docs = root.join("Documents");
    let moved = root.join("D").join("Native Instruments");
    fs::create_dir_all(moved.join("Kontakt")).unwrap();
    fs::create_dir_all(&docs).unwrap();
    fs::write(docs.join("Notes.txt"), b"notes").unwrap();
    fs::write(moved.join("Kontakt").join("Pad.nki"), b"pad").unwrap();
    folder_link(&moved, &docs.join("Native Instruments"));
    let kontakt = docs.join("Native Instruments").join("Kontakt");
    let list =
        vec![Folder { path: docs.clone(), mode: Mode::Slimmed }, Folder { path: kontakt.clone(), mode: Mode::Whole }];
    let got = gather(&list, &root.join("Backup"), &AtomicBool::new(false)).unwrap();
    let mut paths: Vec<PathBuf> = got.files.iter().map(|f| f.path.clone()).collect();
    paths.sort();
    let mut want = vec![docs.join("Notes.txt"), kontakt.join("Pad.nki")];
    want.sort();
    assert_eq!(paths, want);
    assert!(got.missing.is_empty());

    // The same folder listed twice, once through the link and once where it is, is walked once.
    let list = vec![
        Folder { path: kontakt.clone(), mode: Mode::Whole },
        Folder { path: moved.join("Kontakt"), mode: Mode::Whole },
    ];
    let got = gather(&list, &root.join("Backup"), &AtomicBool::new(false)).unwrap();
    assert_eq!(got.files.len(), 1);
    assert_eq!(got.files[0].path, kontakt.join("Pad.nki"), "named by the first line");

    // The outer folder listed where it really is: the inner one's files keep the inner one's spelling.
    let list =
        vec![Folder { path: root.join("D"), mode: Mode::Slimmed }, Folder { path: kontakt.clone(), mode: Mode::Whole }];
    let got = gather(&list, &root.join("Backup"), &AtomicBool::new(false)).unwrap();
    assert_eq!(got.files.len(), 1, "walked once");
    assert_eq!(got.files[0].path, kontakt.join("Pad.nki"));
}

#[test]
fn the_backup_is_left_out_when_a_listed_link_leads_into_it() {
    let root = fixtures("backup-folders-backup-inside");
    let real = root.join("E").join("Projects");
    fs::create_dir_all(real.join("Backup").join("E")).unwrap();
    fs::write(real.join("Song.flp"), b"song").unwrap();
    fs::write(real.join("Backup").join("E").join("Song.flp"), b"song").unwrap();
    let linked = root.join("Projects");
    folder_link(&real, &linked);
    let list = vec![Folder { path: linked.clone(), mode: Mode::Slimmed }];
    let got = gather(&list, &real.join("Backup"), &AtomicBool::new(false)).unwrap();
    let paths: Vec<&Path> = got.files.iter().map(|f| f.path.as_path()).collect();
    assert_eq!(paths, vec![linked.join("Song.flp")]);
}

#[test]
fn the_list_keeps_through_text() {
    let list = vec![
        Folder { path: PathBuf::from(r"I:\FL Studio Data"), mode: Mode::Slimmed },
        Folder { path: PathBuf::from(r"K:\Bitwig Studio"), mode: Mode::Whole },
    ];
    assert_eq!(from_text(&to_text(&list)), list);
    assert!(from_text("odd line\nwhole\t\n").is_empty());
}
