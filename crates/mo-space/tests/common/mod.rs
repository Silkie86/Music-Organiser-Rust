//! Folder links for the tests, made and removed the way each system has them. One copy, shared by every test file
//! (`mod common;`), so a change to how tests make links is made once.

// Each test file uses only some of these.
#![allow(dead_code)]

use std::fs;
use std::path::Path;

/// A folder link, as Windows has them (a junction, as Space makes) or a symbolic link elsewhere.
pub fn folder_link(target: &Path, at: &Path) {
    #[cfg(windows)]
    junction::create(target, at).unwrap();
    #[cfg(not(windows))]
    std::os::unix::fs::symlink(target, at).unwrap();
}

/// Removes a link [`folder_link`] made, not what it leads to (Windows removes a junction as a folder).
pub fn unlink(at: &Path) {
    if cfg!(windows) { fs::remove_dir(at).unwrap() } else { fs::remove_file(at).unwrap() }
}

/// A link (a junction on Windows too), not a real file or folder.
pub fn is_link(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink())
}
