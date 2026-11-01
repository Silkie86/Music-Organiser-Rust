//! Backing up by folder, on folders made in the test build's own fixtures folder under fixed names.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::PathBuf;
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
    let got = gather(&list, &AtomicBool::new(false)).unwrap();
    assert_eq!(got.files.len(), 2, "the inner folder isn't walked twice");
    assert_eq!(got.always, vec![bounces]);
    assert_eq!(got.missing, vec![root.join("Unplugged")]);
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
