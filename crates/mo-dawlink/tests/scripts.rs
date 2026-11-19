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
    // The script's own line endings stay (a Windows checkout has CRLF), with their setting in place of the app's.
    assert!(b.lines().any(|l| l.trim_end_matches('\r') == "LET_THE_APP_MAKE_CHANGES = True"));
    assert!(!b.contains("LET_THE_APP_MAKE_CHANGES = False"));
    assert_eq!(b.lines().count(), FL_SCRIPT.lines().count());
    // No part file is left behind, and nothing else is written.
    let names: Vec<_> = fs::read_dir(&folders[1]).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(names, [FL_SCRIPT_NAME]);
}

/// The 19 Nov review's LK-6: the measure's switch, turned on by hand, isn't "out of date", and an update keeps it.
#[test]
fn a_copy_with_the_measure_turned_on_is_the_apps_own_and_keeps_it_through_an_update() {
    let root = fixture("measure");
    let folders = fl_script_folders(&[root.join("A")]);
    fs::create_dir_all(&folders[0]).unwrap();
    let on = FL_SCRIPT.replace("MEASURE_IN_LIVE_TEST = False", "MEASURE_IN_LIVE_TEST = True");
    assert_ne!(on, FL_SCRIPT);
    fs::write(folders[0].join(FL_SCRIPT_NAME), &on).unwrap();
    assert_eq!(fl_state(&folders), ScriptState::Installed);
    // An older copy with both settings on: the update keeps both.
    fs::write(folders[0].join(FL_SCRIPT_NAME), "# old\nLET_THE_APP_MAKE_CHANGES = True\nMEASURE_IN_LIVE_TEST = True\n")
        .unwrap();
    assert_eq!(fl_state(&folders), ScriptState::Outdated);
    assert!(install_fl(&folders).failed.is_empty());
    assert_eq!(fl_state(&folders), ScriptState::Installed);
    let now = fs::read_to_string(folders[0].join(FL_SCRIPT_NAME)).unwrap();
    assert!(now.lines().any(|l| l == "LET_THE_APP_MAKE_CHANGES = True"));
    assert!(now.lines().any(|l| l == "MEASURE_IN_LIVE_TEST = True"));
    assert_eq!(now.lines().count(), FL_SCRIPT.lines().count());
}

#[test]
fn a_folder_that_cant_take_it_doesnt_stop_the_others() {
    let root = fixture("blocked");
    // A file where the folder should be.
    fs::write(root.join("A"), "not a folder").unwrap();
    let folders = vec![root.join("A").join("DAWmate"), root.join("B")];
    let done = install_fl(&folders);
    assert_eq!(done.went, [root.join("B")]);
    assert_eq!(done.failed.len(), 1);
}

#[test]
fn the_shipped_script_has_the_kept_setting_off() {
    assert!(FL_SCRIPT.lines().any(|l| l == "LET_THE_APP_MAKE_CHANGES = False"));
    assert!(FL_SCRIPT.is_ascii());
}

/// A tiny stand-in for the built extension: a zip with a manifest.
fn extension(at: &Path, version: &str) {
    use std::io::Write;
    let mut zip = zip::ZipWriter::new(fs::File::create(at).unwrap());
    zip.start_file("META-INF/MANIFEST.MF", zip::write::SimpleFileOptions::default()).unwrap();
    write!(zip, "Manifest-Version: 1.0\r\nImplementation-Version: {version}\r\n").unwrap();
    zip.finish().unwrap();
}

#[test]
fn finds_the_built_extension_beside_the_app_or_in_the_repo() {
    let root = fixture("source");
    let app = root.join("run").join("music-organiser.exe");
    fs::create_dir_all(root.join("run").join("extensions")).unwrap();
    assert_eq!(bitwig_extension_source(&app), None);
    let repo_built = root.join("extensions").join("bitwig-java").join("target");
    fs::create_dir_all(&repo_built).unwrap();
    extension(&repo_built.join(BITWIG_EXTENSION_NAME), "3.18");
    assert_eq!(bitwig_extension_source(&app), Some(repo_built.join(BITWIG_EXTENSION_NAME)));
    let beside = root.join("run").join("extensions").join(BITWIG_EXTENSION_NAME);
    extension(&beside, "3.19");
    assert_eq!(bitwig_extension_source(&app), Some(beside.clone()));
    assert_eq!(extension_version(&beside).as_deref(), Some("3.19"));
}

#[test]
fn installs_the_extension_and_takes_out_the_old_script() {
    let root = fixture("bitwig");
    let documents = root.join("Documents");
    fs::create_dir_all(documents.join("Bitwig Studio")).unwrap();
    let k = root.join("K").join("Bitwig Studio");
    let users = bitwig_user_folders(Some(&k.display().to_string()), std::slice::from_ref(&documents));
    assert_eq!(users, [k.clone(), documents.join("Bitwig Studio")]);
    // Bitwig was seen loading it from a third Extensions folder.
    let seen = root.join("Elsewhere").join("Extensions");
    fs::create_dir_all(&seen).unwrap();
    let folders = bitwig_extension_folders(&users, Some(&seen.join(BITWIG_EXTENSION_NAME).display().to_string()));
    assert_eq!(folders, [k.join("Extensions"), documents.join("Bitwig Studio").join("Extensions"), seen.clone()]);
    // Something Bitwig loaded that isn't in an Extensions folder isn't written to.
    assert_eq!(bitwig_extension_folders(&users, Some("C:\\Temp\\x.bwextension")).len(), 2);

    let source = root.join(BITWIG_EXTENSION_NAME);
    extension(&source, "3.18");
    assert_eq!(bitwig_state(&source, &folders), ScriptState::NotInstalled);
    let old = k.join("Controller Scripts").join("Music Organiser");
    fs::create_dir_all(&old).unwrap();
    fs::write(old.join(BITWIG_SCRIPT_NAME), "// old").unwrap();
    let done = install_bitwig(&source, &folders, &users);
    assert_eq!(done.installed.went, folders);
    assert!(done.installed.failed.is_empty());
    assert_eq!(done.removed, [old.join(BITWIG_SCRIPT_NAME)]);
    assert!(!old.exists() && k.join("Controller Scripts").exists());
    assert_eq!(bitwig_state(&source, &folders), ScriptState::Installed);
    extension(&source, "3.19");
    assert_eq!(bitwig_state(&source, &folders), ScriptState::Outdated);
}

#[test]
fn the_fl_script_from_before_the_rename_comes_out_keeping_its_setting() {
    let root = fixture("fl-renamed");
    user_data(&root);
    let folders = fl_script_folders(std::slice::from_ref(&root));
    let hardware = root.join("Settings").join("Hardware");
    assert_eq!(folders, [hardware.join(FL_FOLDER)]);
    // Only the old copy is there, in the old folder, with changes on, and a file of the user's beside it.
    let first = hardware.join(FIRST_FL_FOLDER);
    fs::create_dir_all(&first).unwrap();
    fs::write(first.join(FIRST_FL_SCRIPT_NAME), "# old\nLET_THE_APP_MAKE_CHANGES = True\n").unwrap();
    fs::write(first.join("notes.txt"), "mine").unwrap();
    assert_eq!(fl_state(&folders), ScriptState::Outdated);
    let done = install_fl(&folders);
    assert_eq!((done.went.as_slice(), done.failed.as_slice()), (folders.as_slice(), &[][..]));
    assert_eq!(done.removed, [first.join(FIRST_FL_SCRIPT_NAME)]);
    let new = fs::read_to_string(folders[0].join(FL_SCRIPT_NAME)).unwrap();
    assert!(new.lines().any(|l| l == "LET_THE_APP_MAKE_CHANGES = True"), "the old copy's setting came over");
    // Only the old script went: the user's file, and so its folder, stay.
    assert!(!first.join(FIRST_FL_SCRIPT_NAME).exists() && first.join("notes.txt").exists());
    assert_eq!(fl_state(&folders), ScriptState::Installed);

    // With nothing else in it, the old folder goes too.
    fs::remove_file(first.join("notes.txt")).unwrap();
    fs::write(first.join(FIRST_FL_SCRIPT_NAME), "# old\n").unwrap();
    assert_eq!(fl_state(&folders), ScriptState::Outdated);
    let done = install_fl(&folders);
    assert_eq!(done.removed.len(), 1);
    assert!(!first.exists() && folders[0].join(FL_SCRIPT_NAME).exists());
}

#[test]
fn a_folder_that_cant_take_the_new_script_keeps_its_old_one() {
    let root = fixture("fl-renamed-blocked");
    user_data(&root);
    let hardware = root.join("Settings").join("Hardware");
    let first = hardware.join(FIRST_FL_FOLDER);
    fs::create_dir_all(&first).unwrap();
    fs::write(first.join(FIRST_FL_SCRIPT_NAME), "# old\n").unwrap();
    // A file where the new folder should be.
    fs::write(hardware.join(FL_FOLDER), "not a folder").unwrap();
    let done = install_fl(&fl_script_folders(std::slice::from_ref(&root)));
    assert!(done.went.is_empty() && done.removed.is_empty() && done.failed.len() == 1);
    assert!(first.join(FIRST_FL_SCRIPT_NAME).exists(), "the link it has keeps working");
}

#[test]
fn the_bitwig_extension_from_before_the_rename_comes_out() {
    let root = fixture("bitwig-renamed");
    let user = root.join("Bitwig Studio");
    let extensions = user.join("Extensions");
    fs::create_dir_all(&extensions).unwrap();
    let source = root.join(BITWIG_EXTENSION_NAME);
    extension(&source, "3.31");
    // Bitwig has the old file (same ID), and was seen loading it.
    extension(&extensions.join(FIRST_BITWIG_EXTENSION_NAME), "3.30");
    let seen = extensions.join(FIRST_BITWIG_EXTENSION_NAME).display().to_string();
    let folders = bitwig_extension_folders(std::slice::from_ref(&user), Some(&seen));
    assert_eq!(folders, std::slice::from_ref(&extensions));
    assert_eq!(bitwig_state(&source, &folders), ScriptState::Outdated);
    let done = install_bitwig(&source, &folders, std::slice::from_ref(&user));
    assert!(done.installed.failed.is_empty());
    assert_eq!(done.removed, [extensions.join(FIRST_BITWIG_EXTENSION_NAME)]);
    let names: Vec<_> = fs::read_dir(&extensions).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(names, [BITWIG_EXTENSION_NAME], "one copy, so Bitwig lists the controller once");
    assert_eq!(bitwig_state(&source, &folders), ScriptState::Installed);
    // A folder Bitwig was seen loading the old file from, outside the user folders, is updated too.
    let elsewhere = root.join("Elsewhere").join("Extensions");
    fs::create_dir_all(&elsewhere).unwrap();
    let seen = elsewhere.join(FIRST_BITWIG_EXTENSION_NAME).display().to_string();
    assert_eq!(bitwig_extension_folders(std::slice::from_ref(&user), Some(&seen)), [extensions, elsewhere]);
}
