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

#[test]
fn an_older_bitwig_copy_that_cant_be_moved_aside_stops_the_update_in_every_folder() {
    let root = fixture("bitwig-all-or-nothing");
    let (a, b) = (root.join("A").join("Extensions"), root.join("B").join("Extensions"));
    let source = root.join(BITWIG_EXTENSION_NAME);
    extension(&source, "3.31");
    for folder in [&a, &b] {
        fs::create_dir_all(folder).unwrap();
        extension(&folder.join(FIRST_BITWIG_EXTENSION_NAME), "3.30");
    }
    // In B something is in the way where the older copy would be moved aside (as Bitwig holding it open would be).
    let blocked = b.join(format!("{FIRST_BITWIG_EXTENSION_NAME}.mo-old"));
    fs::create_dir_all(blocked.join("in the way")).unwrap();
    let folders = vec![a.clone(), b.clone()];
    let done = install_bitwig(&source, &folders, &[]);
    assert!(done.installed.went.is_empty() && done.removed.is_empty(), "{done:?}");
    assert_eq!(done.installed.failed.len(), 2);
    // Neither folder has the new copy, and each still has its older one, so Bitwig never has two.
    for folder in [&a, &b] {
        assert!(folder.join(FIRST_BITWIG_EXTENSION_NAME).is_file() && !folder.join(BITWIG_EXTENSION_NAME).exists());
    }
    assert!(!a.join(format!("{FIRST_BITWIG_EXTENSION_NAME}.mo-old")).exists());
    assert_eq!(bitwig_state(&source, &folders), ScriptState::Outdated);

    // Once out of the way, it goes through.
    fs::remove_dir_all(&blocked).unwrap();
    let done = install_bitwig(&source, &folders, &[]);
    assert_eq!(done.installed.went, folders);
    assert_eq!(bitwig_state(&source, &folders), ScriptState::Installed);
}

#[test]
fn an_older_bitwig_copy_left_aside_is_taken_out_by_the_next_install() {
    let root = fixture("bitwig-left-aside");
    let extensions = root.join("Extensions");
    fs::create_dir_all(&extensions).unwrap();
    let source = root.join(BITWIG_EXTENSION_NAME);
    extension(&source, "3.31");
    let folders = vec![extensions.clone()];
    install_bitwig(&source, &folders, &[]);
    assert_eq!(bitwig_state(&source, &folders), ScriptState::Installed);
    // An earlier install couldn't delete the copy it had moved aside.
    let aside = extensions.join(format!("{FIRST_BITWIG_EXTENSION_NAME}.mo-old"));
    extension(&aside, "3.30");
    assert_eq!(bitwig_state(&source, &folders), ScriptState::Outdated, "so the row offers Update");
    let done = install_bitwig(&source, &folders, &[]);
    assert!(done.installed.failed.is_empty() && done.installed.left.is_empty());
    let names: Vec<_> = fs::read_dir(&extensions).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(names, [BITWIG_EXTENSION_NAME]);
    assert_eq!(bitwig_state(&source, &folders), ScriptState::Installed);
}

#[test]
fn a_new_copy_one_folder_cant_take_stops_the_update_everywhere() {
    let root = fixture("bitwig-write-blocked");
    let (a, b) = (root.join("A").join("Extensions"), root.join("B").join("Extensions"));
    let source = root.join(BITWIG_EXTENSION_NAME);
    extension(&source, "3.31");
    for folder in [&a, &b] {
        fs::create_dir_all(folder).unwrap();
        extension(&folder.join(FIRST_BITWIG_EXTENSION_NAME), "3.30");
    }
    // B can't take the new copy: something is in the way of the file it's first written as.
    fs::create_dir_all(b.join(format!("{BITWIG_EXTENSION_NAME}.mo-part")).join("in the way")).unwrap();
    let folders = vec![a.clone(), b.clone()];
    let done = install_bitwig(&source, &folders, &[]);
    assert!(done.installed.went.is_empty() && done.removed.is_empty() && done.installed.failed.len() == 2);
    for folder in [&a, &b] {
        assert!(folder.join(FIRST_BITWIG_EXTENSION_NAME).is_file() && !folder.join(BITWIG_EXTENSION_NAME).exists());
    }
    assert!(!a.join(format!("{BITWIG_EXTENSION_NAME}.mo-part")).exists(), "no part file stays");
}

#[test]
fn an_older_copy_left_aside_by_a_cut_off_install_goes_back_if_this_one_stops() {
    let root = fixture("bitwig-cut-off");
    let extensions = root.join("Extensions");
    fs::create_dir_all(&extensions).unwrap();
    let source = root.join(BITWIG_EXTENSION_NAME);
    extension(&source, "3.31");
    let aside = extensions.join(format!("{FIRST_BITWIG_EXTENSION_NAME}.mo-old"));
    extension(&aside, "3.30");
    fs::create_dir_all(extensions.join(format!("{BITWIG_EXTENSION_NAME}.mo-part")).join("in the way")).unwrap();
    let folders = vec![extensions.clone()];
    let done = install_bitwig(&source, &folders, &[]);
    assert_eq!(done.installed.failed.len(), 1);
    assert!(extensions.join(FIRST_BITWIG_EXTENSION_NAME).is_file() && !aside.exists(), "Bitwig keeps its link");
}

#[cfg(unix)]
#[test]
fn a_folder_listed_twice_through_a_link_is_done_once() {
    let root = fixture("bitwig-twice");
    let k = root.join("K").join("Extensions");
    fs::create_dir_all(&k).unwrap();
    std::os::unix::fs::symlink(root.join("K"), root.join("Docs")).unwrap();
    let docs = root.join("Docs").join("Extensions");
    let source = root.join(BITWIG_EXTENSION_NAME);
    extension(&source, "3.31");
    extension(&k.join(FIRST_BITWIG_EXTENSION_NAME), "3.30");
    let folders = vec![k.clone(), docs];
    let done = install_bitwig(&source, &folders, &[]);
    assert_eq!(done.installed.went, folders);
    assert!(done.installed.failed.is_empty() && done.installed.left.is_empty(), "{done:?}");
    assert_eq!(done.removed, [k.join(FIRST_BITWIG_EXTENSION_NAME)]);
    let names: Vec<_> = fs::read_dir(&k).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(names, [BITWIG_EXTENSION_NAME]);
}

#[test]
fn taking_the_links_out_removes_only_the_apps_own_files_and_folders() {
    let root = fixture("take-out");
    // FL: the script in its own folder, and the one from before the rename beside a file of the user's.
    user_data(&root);
    let fl_folders = fl_script_folders(std::slice::from_ref(&root));
    install_fl(&fl_folders);
    let hardware = root.join("Settings").join("Hardware");
    let first = hardware.join(FIRST_FL_FOLDER);
    fs::create_dir_all(&first).unwrap();
    fs::write(first.join(FIRST_FL_SCRIPT_NAME), "# old\n").unwrap();
    fs::write(first.join("notes.txt"), "mine").unwrap();
    let fl = fl_installed_files(&fl_folders);
    assert_eq!(fl, [fl_folders[0].join(FL_SCRIPT_NAME), first.join(FIRST_FL_SCRIPT_NAME)]);
    let done = take_out(&fl);
    assert_eq!(done.removed, fl);
    assert!(done.failed.is_empty());
    assert!(!fl_folders[0].exists(), "the app's own folder, left empty, goes");
    assert!(first.join("notes.txt").exists(), "the user's file, and so its folder, stay");
    assert_eq!(fl_state(&fl_folders), ScriptState::NotInstalled);
    assert!(fl_installed_files(&fl_folders).is_empty());

    // Bitwig: the extension, the old one, the old one moved aside, and the old script; nothing else of Bitwig's.
    let user = root.join("Bitwig Studio");
    let extensions = user.join("Extensions");
    fs::create_dir_all(&extensions).unwrap();
    for name in [BITWIG_EXTENSION_NAME, FIRST_BITWIG_EXTENSION_NAME, "Another.bwextension"] {
        extension(&extensions.join(name), "1");
    }
    fs::write(extensions.join(format!("{FIRST_BITWIG_EXTENSION_NAME}.mo-old")), "old").unwrap();
    let scripts = user.join("Controller Scripts").join("Music Organiser");
    fs::create_dir_all(&scripts).unwrap();
    fs::write(scripts.join(BITWIG_SCRIPT_NAME), "// old").unwrap();
    let folders = vec![extensions.clone()];
    let users = vec![user.clone()];
    let bw = bitwig_installed_files(&folders, &users);
    assert_eq!(bw.len(), 4);
    let done = take_out(&bw);
    assert_eq!(done.removed.len(), 4);
    let names: Vec<_> = fs::read_dir(&extensions).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(names, ["Another.bwextension"], "Bitwig's own folder and other extensions stay");
    assert!(!scripts.exists() && user.join("Controller Scripts").exists());
    assert!(bitwig_installed_files(&folders, &users).is_empty());

    // Only the app's own names, whatever list it's given.
    let theirs = extensions.join("Another.bwextension");
    assert!(take_out(std::slice::from_ref(&theirs)).removed.is_empty() && theirs.exists());
}
