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

/// A file link, where the system lets one be made (Windows only does with Developer Mode on, or as administrator).
/// Says whether it was made.
pub fn file_link(target: &Path, at: &Path) -> bool {
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_file(target, at);
    #[cfg(not(windows))]
    let made = std::os::unix::fs::symlink(target, at);
    made.is_ok()
}

/// Makes the read-only files a test left writable again, and on a Mac or Linux opens again the folders it locked (also
/// when it stopped before it could), so its folder can be cleared. Never through a link.
pub fn writable(dir: &Path) {
    // A link (or nothing) isn't looked in: only a real folder is.
    if !fs::symlink_metadata(dir).is_ok_and(|m| m.is_dir()) {
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o755));
    }
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            writable(&entry.path());
        } else if kind.is_file()
            && let Ok(meta) = entry.metadata()
            && meta.permissions().readonly()
        {
            let mut permissions = meta.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
            let _ = fs::set_permissions(entry.path(), permissions);
        }
    }
}
