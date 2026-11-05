//! Your say on a plugin's folders, from the Plugins page's On disk tab (Solomon's OK, 2026-11-05): a folder you added
//! to a plugin (a sound library on another drive the guess missed), and a guessed folder you said isn't its. Kept in the
//! C# app's own settings (`plugin_space_added:<key>` and `plugin_space_not:<key>`, JSON lists of folders), so the C#
//! app's next measure follows them too. Each change is [`crate::setting_list`]'s: one checked transaction, and a
//! setting it can't read is left as it is.

use std::path::Path;

use serde_json::Value;

use crate::LibraryError;
use crate::plugins::{added_folders_key, ruled_out_key};
use crate::scan::path_key;
use crate::setting_list;

/// Adds `folder` to the folders you added to the plugin `key` (once, whatever its letter case or slashes).
pub fn add_plugin_folder(library: &Path, key: &str, folder: &str) -> Result<(), LibraryError> {
    add_to(library, &added_folders_key(key), folder)
}

/// Says the guessed `folder` isn't the plugin `key`'s: its measures leave it out from now on.
pub fn rule_out_plugin_folder(library: &Path, key: &str, folder: &str) -> Result<(), LibraryError> {
    add_to(library, &ruled_out_key(key), folder)
}

/// Takes `folder` off the folders you added to the plugin `key`.
pub fn take_off_added_folder(library: &Path, key: &str, folder: &str) -> Result<(), LibraryError> {
    let gone = path_key(folder);
    setting_list::change(library, &added_folders_key(key), |list| {
        let before = list.len();
        list.retain(|f| f.as_str().map(path_key).as_deref() != Some(gone.as_str()));
        list.len() != before
    })
}

fn add_to(library: &Path, setting: &str, folder: &str) -> Result<(), LibraryError> {
    let folder = folder.trim();
    if folder.is_empty() {
        return Err(LibraryError::Write { path: library.to_owned(), why: "no folder was given".to_owned() });
    }
    let new = path_key(folder);
    setting_list::change(library, setting, |list| {
        if list.iter().any(|f| f.as_str().map(path_key).as_deref() == Some(new.as_str())) {
            return false;
        }
        list.push(Value::String(folder.to_owned()));
        true
    })
}

#[cfg(test)]
mod tests {
    use rusqlite::{Connection, OptionalExtension};

    use super::*;

    fn library(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("mo-library-plugin-folders-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        for end in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{end}", path.display()));
        }
        Connection::open(&path)
            .unwrap()
            .execute_batch("PRAGMA journal_mode = WAL; CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);")
            .unwrap();
        path
    }

    fn kept(path: &Path, key: &str) -> Option<String> {
        Connection::open(path)
            .unwrap()
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0))
            .optional()
            .unwrap()
    }

    #[test]
    fn folders_are_added_once_ruled_out_and_taken_off() {
        let path = library("folders.db");
        add_plugin_folder(&path, "kontakt", "E:\\Kontakt Libraries").unwrap();
        add_plugin_folder(&path, "kontakt", "e:/kontakt libraries/").unwrap();
        assert_eq!(kept(&path, "plugin_space_added:kontakt").as_deref(), Some(r#"["E:\\Kontakt Libraries"]"#));
        rule_out_plugin_folder(&path, "kontakt", "C:\\Users\\me\\Documents\\Native Instruments\\Massive").unwrap();
        assert!(kept(&path, "plugin_space_not:kontakt").unwrap().contains("Massive"));
        take_off_added_folder(&path, "kontakt", "E:/Kontakt Libraries").unwrap();
        assert_eq!(kept(&path, "plugin_space_added:kontakt").as_deref(), Some("[]"));
        // Another plugin's lists aren't touched.
        assert_eq!(kept(&path, "plugin_space_added:massive"), None);
    }

    #[test]
    fn a_list_it_cant_read_is_left_as_it_is() {
        let path = library("bad.db");
        Connection::open(&path)
            .unwrap()
            .execute("INSERT INTO settings VALUES ('plugin_space_not:arcade', 'oops')", [])
            .unwrap();
        assert!(rule_out_plugin_folder(&path, "arcade", "C:\\A").is_err());
        assert_eq!(kept(&path, "plugin_space_not:arcade").as_deref(), Some("oops"));
        assert!(add_plugin_folder(&path, "arcade", "  ").is_err());
    }
}
