//! Recording a folder moved to another drive in the C# app's `library.db`: one entry added to the `space_moved_folders`
//! setting, the JSON list the C# app keeps its moves in, so moves made by either app are known to both (Space's
//! old-copy reminders and Move back read it). The entry's shape is `mo_space::folder_move::MovedFolder`'s; mo-space
//! sits above this crate, so the caller hands it in as JSON.
//!
//! As [`crate::folders`]: one transaction that reads the setting as it is now (the C# app may have changed it), adds
//! the entry, reads it back to check, and only then keeps it. A setting that holds something that isn't a JSON list is
//! refused, never read as empty: adding to an empty list there would wipe the C# app's moves. It never makes a library
//! or a table.

use std::path::Path;

use rusqlite::types::ValueRef;
use rusqlite::{ErrorCode, OptionalExtension, Transaction, TransactionBehavior};
use serde_json::Value;

use crate::LibraryError;
use crate::folders::open;
use crate::scan::path_key;

/// The setting the C# app keeps its moved folders in.
pub const MOVED_FOLDERS: &str = "space_moved_folders";

/// Adds one moved folder (`entry`, a JSON object in the C# shape, as `mo_space::folder_move::MovedFolder` writes it)
/// to the moved folders. One already there for the same folder (`From`, ignoring letter case and slashes) is replaced,
/// as the C# app does when a folder moves again.
pub fn add_moved_folder(library: &Path, entry: &str) -> Result<(), LibraryError> {
    let write_error = |why: String| LibraryError::Write { path: library.to_owned(), why };
    let entry: Value = serde_json::from_str(entry)
        .ok()
        .filter(Value::is_object)
        .ok_or_else(|| write_error("the move to record isn't one move, so nothing was changed".to_owned()))?;
    let sql = |e: rusqlite::Error| match e.sqlite_error_code() {
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => LibraryError::Busy { path: library.to_owned() },
        _ => write_error(e.to_string()),
    };
    let Some(from) = entry.get("From").and_then(Value::as_str).map(path_key) else {
        return Err(write_error("the move to record has no From folder, so nothing was changed".to_owned()));
    };
    let mut conn = open(library)?;
    let has: bool = conn
        .query_row("SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'settings')", [], |r| {
            r.get(0)
        })
        .map_err(sql)?;
    if !has {
        return Err(write_error("it has no settings table yet; open the C# app once to make it".to_owned()));
    }
    // IMMEDIATE takes the write lock at the start, so the list read is the one written over.
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(sql)?;
    let mut list = read(&tx).map_err(write_error)?;
    list.retain(|m| m.get("From").and_then(Value::as_str).map(path_key).as_deref() != Some(from.as_str()));
    list.push(entry.clone());
    let json = serde_json::to_string(&list).map_err(|e| write_error(format!("couldn't write the list ({e})")))?;
    tx.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [MOVED_FOLDERS, json.as_str()],
    )
    .map_err(sql)?;
    if read(&tx).map_err(write_error)? != list {
        // Dropping the transaction undoes it.
        return Err(write_error("the moves read back weren't the ones written, so nothing was changed".to_owned()));
    }
    tx.commit().map_err(sql)?;
    Ok(())
}

/// The moved folders as kept now: NULL or missing is none; anything that isn't a JSON list is refused.
fn read(tx: &Transaction<'_>) -> Result<Vec<Value>, String> {
    let value = tx
        .query_row("SELECT value FROM settings WHERE key = ?1", [MOVED_FOLDERS], |r| {
            Ok(match r.get_ref(0)? {
                ValueRef::Null => Ok(None),
                ValueRef::Text(t) => Ok(Some(String::from_utf8_lossy(t).into_owned())),
                _ => Err(()),
            })
        })
        .optional()
        .map_err(|e| e.to_string())?;
    match value {
        None | Some(Ok(None)) => Ok(Vec::new()),
        Some(Ok(Some(json))) if json.trim().is_empty() => Ok(Vec::new()),
        Some(Ok(Some(json))) => serde_json::from_str::<Vec<Value>>(&json)
            .map_err(|_| format!("its {MOVED_FOLDERS} setting isn't a list the app can read, so it was left as it is")),
        Some(Err(())) => Err(format!("its {MOVED_FOLDERS} setting isn't text, so it was left as it is")),
    }
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;
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

    #[test]
    fn a_setting_that_isnt_text_is_left_as_it_is() {
        let path = library("blob.db", None);
        Connection::open(&path).unwrap().execute("INSERT INTO settings VALUES (?1, x'00')", [MOVED_FOLDERS]).unwrap();
        assert!(add_moved_folder(&path, &json!({"From": "C:\\A"}).to_string()).is_err());
    }
}
