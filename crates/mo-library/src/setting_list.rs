//! Changing one setting that holds a JSON list, as the C# app keeps its lists in `settings`: the one safe way the Rust
//! app's writers ([`crate::moves`], [`crate::plugin_folders`]) change one. As [`crate::folders`]: one transaction that
//! reads the list as it is now (the C# app may have changed it), changes it, reads it back to check, and only then
//! keeps it. A setting that holds something that isn't a JSON list (or isn't text) is refused, never read as empty:
//! adding to an empty list there would wipe what the C# app keeps. It never makes a library or a table.

use std::path::Path;

use rusqlite::types::ValueRef;
use rusqlite::{ErrorCode, OptionalExtension, Transaction, TransactionBehavior};
use serde_json::Value;

use crate::LibraryError;
use crate::folders::open;

/// Changes the list kept in setting `key`: `edit` changes it and says whether it did (when not, nothing is written).
/// Says whether it changed.
pub(crate) fn change(
    library: &Path,
    key: &str,
    edit: impl FnOnce(&mut Vec<Value>) -> bool,
) -> Result<bool, LibraryError> {
    let write_error = |why: String| LibraryError::Write { path: library.to_owned(), why };
    let sql = |e: rusqlite::Error| match e.sqlite_error_code() {
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => LibraryError::Busy { path: library.to_owned() },
        _ => write_error(e.to_string()),
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
    let mut list = read(&tx, key).map_err(write_error)?;
    if !edit(&mut list) {
        return Ok(false);
    }
    let json = serde_json::to_string(&list).map_err(|e| write_error(format!("couldn't write the list ({e})")))?;
    tx.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [key, json.as_str()],
    )
    .map_err(sql)?;
    if read(&tx, key).map_err(write_error)? != list {
        // Dropping the transaction undoes it.
        return Err(write_error(format!("its {key} setting read back wasn't the one written, so nothing was changed")));
    }
    tx.commit().map_err(sql)?;
    Ok(true)
}

/// The list kept now: NULL, missing or blank is none; anything that isn't a JSON list is refused.
fn read(tx: &Transaction<'_>, key: &str) -> Result<Vec<Value>, String> {
    let value = tx
        .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
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
            .map_err(|_| format!("its {key} setting isn't a list the app can read, so it was left as it is")),
        Some(Err(())) => Err(format!("its {key} setting isn't text, so it was left as it is")),
    }
}
