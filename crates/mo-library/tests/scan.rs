//! Walking real folders for project files. The folders are made in the test build's own fixtures folder, under a
//! fixed name, never a random temp one.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::PathBuf;

use mo_library::scan::{find_projects, plan};

/// The test songs, in a folder of the test's own (tests run at the same time).
fn folder(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    for (path, bytes) in [
        ("Night Drive/Night Drive_14.flp", 14),
        ("Night Drive/Night Drive_2.FLP", 2),
        ("Night Drive/Backup/Night Drive_13.flp", 13),
        ("Night Drive/Night Drive (autosave).flp", 1),
        ("Night Drive/._Night Drive_14.flp", 1),
        ("Night Drive/kick.wav", 1),
        ("Glass Harbor/Glass Harbor.bwproject", 7),
        ("Glass Harbor/auto-backups/Glass Harbor 1.bwproject", 1),
        (".hidden/secret.flp", 1),
        ("Studio/Slow Orbit/Slow Orbit.song", 3),
    ] {
        let p = dir.join(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, vec![0u8; bytes]).unwrap();
    }
    dir
}

#[test]
fn finds_projects_leaving_out_backups_autosaves_and_hidden_folders() {
    let dir = folder("mo-library-scan");
    let root = dir.to_str().unwrap().to_owned();
    let found = find_projects(&[root.clone(), root.to_uppercase(), dir.join("not there").to_str().unwrap().into()]);
    let names: Vec<String> = found.files.iter().map(|f| f.path[root.len() + 1..].replace('\\', "/")).collect();
    assert_eq!(
        names,
        [
            "Glass Harbor/Glass Harbor.bwproject",
            "Night Drive/Night Drive_14.flp",
            "Night Drive/Night Drive_2.FLP",
            "Studio/Slow Orbit/Slow Orbit.song"
        ]
    );
    assert_eq!(found.files[1].size, 14);
    assert!(found.files.iter().all(|f| f.mtime > 0));
    assert!(found.unreadable.is_empty());
    // Nothing known yet: every file is new.
    let p = plan(&found, &[], &[root], &|_| false, false);
    assert_eq!((p.read.len(), p.new), (4, 4));
}

#[test]
fn a_missing_root_keeps_its_saves_and_an_inner_root_is_walked_once() {
    let dir = folder("mo-library-scan-roots");
    let root = dir.to_str().unwrap().to_owned();
    let unplugged = format!("{root} unplugged drive");
    let inner = dir.join("Night Drive").to_str().unwrap().to_owned();
    let found = find_projects(&[inner, root.clone(), unplugged.clone()]);
    assert_eq!(found.files.len(), 4);
    assert_eq!(found.unreadable, std::slice::from_ref(&unplugged));
    let gone = mo_library::scan::KnownFile {
        id: 1,
        path: format!("{unplugged}/Song.flp"),
        size: 1,
        mtime: 1,
        missing: false,
        reread: false,
    };
    assert!(plan(&found, &[gone], &[root, unplugged], &|_| false, false).gone.is_empty());
}

#[cfg(unix)]
#[test]
fn a_folder_link_back_up_its_own_tree_is_walked_once() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-library-scan-link");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("Songs")).unwrap();
    fs::write(dir.join("Songs/Night Drive.flp"), b"x").unwrap();
    std::os::unix::fs::symlink(dir.join("Songs"), dir.join("Songs/loop")).unwrap();
    let elsewhere = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-library-scan-elsewhere");
    let _ = fs::remove_dir_all(&elsewhere);
    fs::create_dir_all(&elsewhere).unwrap();
    fs::write(elsewhere.join("Copper Lines.flp"), b"x").unwrap();
    std::os::unix::fs::symlink(&elsewhere, dir.join("Songs/a")).unwrap();
    std::os::unix::fs::symlink(&elsewhere, dir.join("Songs/b")).unwrap();
    let found = find_projects(&[dir.to_str().unwrap().to_owned()]);
    let names: Vec<&str> = found.files.iter().map(|f| f.path.rsplit('/').next().unwrap()).collect();
    assert_eq!(names, ["Copper Lines.flp", "Night Drive.flp"]);
}

#[test]
fn the_folder_watch_reports_a_new_save_once_things_are_quiet() {
    use std::sync::mpsc;
    use std::time::Duration;

    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-library-watch");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("Night Drive")).unwrap();
    let (tx, rx) = mpsc::channel();
    let watch = mo_library::watch::FolderWatch::start(
        &[dir.to_str().unwrap().to_owned(), dir.join("not there").to_str().unwrap().to_owned()],
        mo_library::scan::is_project_file,
        Duration::from_millis(300),
        move |c| {
            let _ = tx.send(c);
        },
    )
    .unwrap();
    assert_eq!(watch.count(), 1);
    // Some systems start hearing changes a moment after the watch begins, and a busy machine can split the writes
    // over two reports: the save is written again until a report names it, for up to 30 seconds.
    let song = dir.join("Night Drive/Night Drive_15.flp");
    fs::write(dir.join("Night Drive/kick.wav"), b"x").unwrap();
    let mut heard: Vec<String> = Vec::new();
    for round in 0..15 {
        fs::write(&song, format!("save {round}")).unwrap();
        if let Ok(changes) = rx.recv_timeout(Duration::from_secs(2)) {
            heard.extend(changes.files);
            if !heard.is_empty() {
                break;
            }
        }
    }
    let names: Vec<&str> = heard.iter().map(|f| f.rsplit(['/', '\\']).next().unwrap()).collect();
    assert_eq!(names, ["Night Drive_15.flp"]);
}
