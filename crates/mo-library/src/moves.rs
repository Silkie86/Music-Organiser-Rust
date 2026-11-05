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
use std::time::Duration;

use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior};
use serde_json::Value;

use crate::LibraryError;

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
    let entry = &entry;
    let sql = |e: rusqlite::Error| write_error(e.to_string());
    let Some(from) = entry.get("From").and_then(Value::as_str).map(same_place) else {
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
    list.retain(|m| m.get("From").and_then(Value::as_str).map(same_place).as_deref() != Some(from.as_str()));
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

/// A folder as moves compare them: `/` between folders, no `/` at the end, letter case ignored.
fn same_place(path: &str) -> String {
    mo_formats::case::key(path.replace('\\', "/").trim_end_matches('/'))
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

/// Opens an existing library to change it: never makes one.
fn open(library: &Path) -> Result<Connection, LibraryError> {
    let open_error = |why: String| LibraryError::Open { path: library.to_owned(), why };
    if !library.is_file() {
        return Err(open_error("it isn't there".to_owned()));
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(library, flags).map_err(|e| open_error(e.to_string()))?;
    // The C# app may be saving a scan: wait for it rather than fail.
    conn.busy_timeout(Duration::from_secs(5)).map_err(|e| open_error(e.to_string()))?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn library(name: &str, value: Option<&str>) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(name);
        let _ = std::fs::remove_file(&path);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);").unwrap();
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
        let path = library("mo-library-moves-add.db", Some(csharp));
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
        let path = library("mo-library-moves-bad.db", Some("not json"));
        assert!(add_moved_folder(&path, &json!({"From": "C:\\A", "To": "E:\\A"}).to_string()).is_err());
        assert_eq!(kept(&path).as_deref(), Some("not json"));
        // None yet: the first one.
        let path = library("mo-library-moves-none.db", None);
        add_moved_folder(&path, &json!({"From": "C:\\A", "To": "E:\\A"}).to_string()).unwrap();
        assert!(kept(&path).unwrap().contains("E:\\\\A"));
        // Something that isn't one move is refused.
        assert!(add_moved_folder(&path, "[1]").is_err());
        // An entry without its folder is refused.
        assert!(add_moved_folder(&path, &json!({"To": "E:\\B"}).to_string()).is_err());
        // A library that isn't there isn't made.
        let gone = std::env::temp_dir().join("mo-library-moves-not-there.db");
        let _ = std::fs::remove_file(&gone);
        assert!(add_moved_folder(&gone, &json!({"From": "C:\\A"}).to_string()).is_err() && !gone.exists());
    }
}
