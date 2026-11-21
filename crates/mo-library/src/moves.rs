//! Recording a folder moved to another drive in the C# app's `library.db`: one entry added to the `space_moved_folders`
//! setting, the JSON list the C# app keeps its moves in, so moves made by either app are known to both (Space's
//! old-copy reminders and Move back read it). The entry's shape is `mo_space::folder_move::MovedFolder`'s; mo-space
//! sits above this crate, so the caller hands it in as JSON. And the folders moved folders leave behind for you to
//! delete (`space_old_homes`, `mo_space::after_move::OldHome`'s shape): the ones the Plugins page's Link again sets
//! aside ([`keep_old_homes`], Solomon's OK of 21 Nov 2026), for the Space page to remind you of.
//!
//! Each change is [`crate::setting_list`]'s: one checked transaction, and a setting it can't read is left as it is.

use std::path::Path;

use serde_json::Value;

use crate::LibraryError;
use crate::scan::path_key;
use crate::setting_list;

/// The setting the C# app keeps its moved folders in.
pub const MOVED_FOLDERS: &str = "space_moved_folders";

/// The setting the C# app keeps the folders moves left behind in.
pub const OLD_HOMES: &str = "space_old_homes";

/// Adds one moved folder (`entry`, a JSON object in the C# shape, as `mo_space::folder_move::MovedFolder` writes it)
/// to the moved folders. One already there for the same folder (`From`, ignoring letter case and slashes) is replaced,
/// as the C# app does when a folder moves again.
pub fn add_moved_folder(library: &Path, entry: &str) -> Result<(), LibraryError> {
    let write_error = |why: String| LibraryError::Write { path: library.to_owned(), why };
    let entry: Value = serde_json::from_str(entry)
        .ok()
        .filter(Value::is_object)
        .ok_or_else(|| write_error("the move to record isn't one move, so nothing was changed".to_owned()))?;
    let Some(from) = entry.get("From").and_then(Value::as_str).map(path_key) else {
        return Err(write_error("the move to record has no From folder, so nothing was changed".to_owned()));
    };
    setting_list::change(library, MOVED_FOLDERS, |list| {
        list.retain(|m| m.get("From").and_then(Value::as_str).map(path_key).as_deref() != Some(from.as_str()));
        list.push(entry);
        true
    })
    .map(|_| ())
}

/// Keeps folders left behind for you to delete: each of `entries` is the folder as it is now (a JSON object in the C#
/// shape, as `mo_space::after_move::OldHome` writes it) with the path it was kept under before, when it was (it moved
/// with the drive it's on, say). One kept under that path, or under its path now (`Path`, ignoring letter case and
/// slashes), is replaced; the rest are added. All are kept, or none is.
pub fn keep_old_homes(library: &Path, entries: &[(String, Option<String>)]) -> Result<(), LibraryError> {
    let write_error = |why: String| LibraryError::Write { path: library.to_owned(), why };
    let mut keep: Vec<(Vec<String>, Value)> = Vec::new();
    for (entry, before) in entries {
        let entry: Value = serde_json::from_str(entry)
            .ok()
            .filter(Value::is_object)
            .ok_or_else(|| write_error("a folder to keep isn't one folder, so nothing was changed".to_owned()))?;
        let Some(now) = entry.get("Path").and_then(Value::as_str).map(path_key) else {
            return Err(write_error("a folder to keep has no Path, so nothing was changed".to_owned()));
        };
        keep.push((before.iter().map(|b| path_key(b)).chain([now]).collect(), entry));
    }
    if keep.is_empty() {
        return Ok(());
    }
    setting_list::change(library, OLD_HOMES, |list| {
        for (paths, entry) in keep {
            list.retain(|h| h.get("Path").and_then(Value::as_str).map(path_key).is_none_or(|p| !paths.contains(&p)));
            list.push(entry);
        }
        true
    })
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use rusqlite::{Connection, OptionalExtension};
    use serde_json::json;

    use super::*;

    /// A file in a folder of this run's own, so two runs at once don't share files; cleared first.
    fn place(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("mo-library-moves-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        for end in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{end}", path.display()));
        }
        path
    }

    fn library(name: &str, value: Option<&str>) -> std::path::PathBuf {
        let path = place(name);
        let conn = Connection::open(&path).unwrap();
        // As the C# app keeps it.
        conn.execute_batch("PRAGMA journal_mode = WAL; CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);")
            .unwrap();
        if let Some(v) = value {
            conn.execute("INSERT INTO settings VALUES (?1, ?2)", [MOVED_FOLDERS, v]).unwrap();
        }
        path
    }

    fn kept(path: &Path) -> Option<String> {
        Connection::open(path)
            .unwrap()
            .query_row("SELECT value FROM settings WHERE key = ?1", [MOVED_FOLDERS], |r| r.get(0))
            .optional()
            .unwrap()
    }

    #[test]
    fn a_move_is_added_beside_the_ones_there_and_replaces_one_of_the_same_folder() {
        let csharp =
            r#"[{"From":"C:\\Old\\Kits","To":"E:\\Kits","OldCopy":"","Bytes":5,"Moved":"2026-10-26T14:03:12+01:00"}]"#;
        let path = library("add.db", Some(csharp));
        let entry = json!({"From": "C:\\Users\\me\\Documents\\Arcade", "To": "E:\\Arcade", "Bytes": 10});
        add_moved_folder(&path, &entry.to_string()).unwrap();
        let list: Vec<Value> = serde_json::from_str(&kept(&path).unwrap()).unwrap();
        assert_eq!(list.len(), 2);
        // The C# app's entry is kept as it was, every field.
        assert_eq!(list[0]["Moved"], "2026-10-26T14:03:12+01:00");
        // The same folder moving again, written another way.
        add_moved_folder(&path, &json!({"From": "c:/users/me/documents/arcade/", "To": "F:\\Arcade"}).to_string())
            .unwrap();
        let list: Vec<Value> = serde_json::from_str(&kept(&path).unwrap()).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[1]["To"], "F:\\Arcade");
    }

    #[test]
    fn a_setting_it_cant_read_is_left_as_it_is() {
        let path = library("bad.db", Some("not json"));
        assert!(add_moved_folder(&path, &json!({"From": "C:\\A", "To": "E:\\A"}).to_string()).is_err());
        assert_eq!(kept(&path).as_deref(), Some("not json"));
        // None yet: the first one.
        let path = library("none.db", None);
        add_moved_folder(&path, &json!({"From": "C:\\A", "To": "E:\\A"}).to_string()).unwrap();
        assert!(kept(&path).unwrap().contains("E:\\\\A"));
        // Something that isn't one move is refused.
        assert!(add_moved_folder(&path, "[1]").is_err());
        // An entry without its folder is refused.
        assert!(add_moved_folder(&path, &json!({"To": "E:\\B"}).to_string()).is_err());
        // A library that isn't there isn't made.
        let gone = place("not-there.db");
        assert!(add_moved_folder(&gone, &json!({"From": "C:\\A"}).to_string()).is_err() && !gone.exists());
    }

    fn homes_kept(path: &Path) -> Option<String> {
        Connection::open(path)
            .unwrap()
            .query_row("SELECT value FROM settings WHERE key = ?1", [OLD_HOMES], |r| r.get(0))
            .optional()
            .unwrap()
    }

    #[test]
    fn folders_left_behind_are_kept_beside_the_ones_there_and_replace_the_same_folder() {
        let path = library("homes.db", None);
        let csharp = r#"[{"Path":"C:\\Old\\Kits","Bytes":5,"Note":"its old place"}]"#;
        Connection::open(&path).unwrap().execute("INSERT INTO settings VALUES (?1, ?2)", [OLD_HOMES, csharp]).unwrap();
        let aside = json!({"Path": "C:\\Data\\Arcade (after update)", "Bytes": 7, "Copy": "E:\\Arcade"});
        let from_before = json!({"Path": "E:\\Arcade (kept from before)", "Bytes": 2});
        keep_old_homes(&path, &[(aside.to_string(), None), (from_before.to_string(), None)]).unwrap();
        let list: Vec<Value> = serde_json::from_str(&homes_kept(&path).unwrap()).unwrap();
        assert_eq!(list.len(), 3);
        // The C# app's entry is kept as it was.
        assert_eq!(list[0]["Note"], "its old place");
        assert_eq!(list[1]["Copy"], "E:\\Arcade");
        // The same folder again, written another way, replaces it; one that moved with its drive replaces the one
        // under its path before.
        let again = json!({"Path": "c:/data/arcade (after update)/", "Bytes": 9});
        let followed = json!({"Path": "F:\\Arcade (kept from before)", "Bytes": 2});
        let before = Some("e:\\arcade (kept from before)".to_owned());
        keep_old_homes(&path, &[(again.to_string(), None), (followed.to_string(), before)]).unwrap();
        let list: Vec<Value> = serde_json::from_str(&homes_kept(&path).unwrap()).unwrap();
        assert_eq!(list.len(), 3, "{list:?}");
        assert_eq!(list[1]["Bytes"], 9);
        assert_eq!(list[2]["Path"], "F:\\Arcade (kept from before)");
        // The moved folders are left as they were.
        assert!(kept(&path).is_none());
    }

    #[test]
    fn folders_left_behind_that_cant_be_read_are_left_as_they_are() {
        let path = library("homes-bad.db", None);
        Connection::open(&path).unwrap().execute("INSERT INTO settings VALUES (?1, 'oops')", [OLD_HOMES]).unwrap();
        let aside = json!({"Path": "C:\\A (after update)"}).to_string();
        assert!(keep_old_homes(&path, &[(aside.clone(), None)]).is_err());
        assert_eq!(homes_kept(&path).as_deref(), Some("oops"));
        // One that isn't a folder, or has no Path, is refused, and the rest with it.
        let path = library("homes-none.db", None);
        assert!(keep_old_homes(&path, &[(aside.clone(), None), ("[1]".to_owned(), None)]).is_err());
        assert!(keep_old_homes(&path, &[(aside.clone(), None), (json!({"Bytes": 1}).to_string(), None)]).is_err());
        assert!(homes_kept(&path).is_none());
        // Nothing to keep writes nothing.
        keep_old_homes(&path, &[]).unwrap();
        assert!(homes_kept(&path).is_none());
        // None yet: the first one.
        keep_old_homes(&path, &[(aside, None)]).unwrap();
        assert!(homes_kept(&path).unwrap().contains("after update"));
    }

    #[test]
    fn a_setting_that_isnt_text_is_left_as_it_is() {
        let path = library("blob.db", None);
        Connection::open(&path).unwrap().execute("INSERT INTO settings VALUES (?1, x'00')", [MOVED_FOLDERS]).unwrap();
        assert!(add_moved_folder(&path, &json!({"From": "C:\\A"}).to_string()).is_err());
    }
}
