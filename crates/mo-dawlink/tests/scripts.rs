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
