//! Installing the FL Studio script: into the right folders, keeping the user's setting, never leaving a part file.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};

use mo_dawlink::scripts::*;

fn fixture(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mo-dawlink-scripts").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn user_data(at: &Path) {
    for d in ["Settings", "Presets", "Projects"] {
        fs::create_dir_all(at.join(d)).unwrap();
    }
}

#[test]
fn finds_fl_studios_folders_from_documents_and_the_project_folders() {
    let root = fixture("find");
    let documents = root.join("Documents");
    user_data(&documents.join("Image-Line").join("FL Studio"));
    let elsewhere = root.join("I").join("Fl Studio Data").join("FL Studio");
    user_data(&elsewhere);
    let projects = elsewhere.join("Projects").display().to_string();
    let found = fl_user_data_folders(&documents, &[projects.clone(), projects, root.display().to_string()]);
    assert_eq!(found, [documents.join("Image-Line").join("FL Studio"), elsewhere]);
}

#[test]
fn installs_and_keeps_the_users_changes_setting() {
    let root = fixture("install");
    let folders = fl_script_folders(&[root.join("A"), root.join("B")]);
    assert_eq!(fl_state(&folders), ScriptState::NotInstalled);
    // B has an older copy that lets the app make changes, with Windows line endings.
    fs::create_dir_all(&folders[1]).unwrap();
    fs::write(folders[1].join(FL_SCRIPT_NAME), "# old\r\nLET_THE_APP_MAKE_CHANGES = True\r\n").unwrap();
    assert_eq!(fl_state(&folders), ScriptState::Outdated);
    let done = install_fl(&folders);
    assert_eq!(done.went, folders);
    assert!(done.failed.is_empty());
    assert_eq!(fl_state(&folders), ScriptState::Installed);
    let a = fs::read_to_string(folders[0].join(FL_SCRIPT_NAME)).unwrap();
    let b = fs::read_to_string(folders[1].join(FL_SCRIPT_NAME)).unwrap();
    assert_eq!(a, FL_SCRIPT);
    assert!(b.contains("\nLET_THE_APP_MAKE_CHANGES = True\n"));
    assert!(!b.contains("LET_THE_APP_MAKE_CHANGES = False"));
    // No part file is left behind, and nothing else is written.
    let names: Vec<_> = fs::read_dir(&folders[1]).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(names, [FL_SCRIPT_NAME]);
}

#[test]
fn a_folder_that_cant_take_it_doesnt_stop_the_others() {
    let root = fixture("blocked");
    // A file where the folder should be.
    fs::write(root.join("A"), "not a folder").unwrap();
    let folders = vec![root.join("A").join("Music Organiser"), root.join("B")];
    let done = install_fl(&folders);
    assert_eq!(done.went, [root.join("B")]);
    assert_eq!(done.failed.len(), 1);
}

#[test]
fn the_shipped_script_has_the_kept_setting_off() {
    assert!(FL_SCRIPT.lines().any(|l| l == "LET_THE_APP_MAKE_CHANGES = False"));
    assert!(FL_SCRIPT.is_ascii());
}
