//! Changing one setting, as the C# app keeps them in `settings`: the one safe way the Rust app's writers
//! ([`crate::moves`], [`crate::plugin_folders`], [`crate::folders::set_export_folder`]) change one, a JSON list
//! ([`change`]) or a line of text ([`change_text`]). As [`crate::folders`]: one transaction that reads the setting as
//! it is now (the C# app may have changed it), changes it, reads it back to check, and only then keeps it. A setting it
//! can't read (a list that isn't a JSON list, or one that isn't text) is refused, never read as empty: adding to an
//! empty list there would wipe what the C# app keeps. It never makes a library or a table.

use std::path::Path;

use rusqlite::types::ValueRef;
use rusqlite::{ErrorCode, OptionalExtension, Transaction};
use serde_json::Value;

use crate::LibraryError;
use crate::folders::open_for_settings;

/// Changes the list kept in setting `key`: `edit` changes it and says whether it did (when not, nothing is written).
/// Says whether it changed.
pub(crate) fn change(
    library: &Path,
    key: &str,
    edit: impl FnOnce(&mut Vec<Value>) -> bool,
) -> Result<bool, LibraryError> {
    change_with(library, key, read, |mut list| {
        if !edit(&mut list) {
            return Ok(None);
        }
        let json = serde_json::to_string(&list).map_err(|e| format!("couldn't write the list ({e})"))?;
        Ok(Some((json, list)))
    })
}

/// Changes the text kept in setting `key`: `edit` is given it as it is now (none when it's missing or NULL) and gives
/// the text to keep in its place, or none to leave it as it is. Says whether it changed.
pub(crate) fn change_text(
    library: &Path,
    key: &str,
    edit: impl FnOnce(Option<&str>) -> Option<String>,
) -> Result<bool, LibraryError> {
    change_with(library, key, read_text, |now| Ok(edit(now.as_deref()).map(|text| (text.clone(), Some(text)))))
}

/// The one transaction every change makes: `read` reads the setting as it is now (refusing what it can't read), and
/// `edit` gives the text to keep in its place with what reading it back must give, or none when there's nothing to do.
fn change_with<T: PartialEq>(
    library: &Path,
    key: &str,
    read: impl Fn(&Transaction<'_>, &str) -> Result<T, String>,
    edit: impl FnOnce(T) -> Result<Option<(String, T)>, String>,
) -> Result<bool, LibraryError> {
    let write_error = |why: String| LibraryError::Write { path: library.to_owned(), why };
    let sql = |e: rusqlite::Error| match e.sqlite_error_code() {
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => LibraryError::Busy { path: library.to_owned() },
        _ => write_error(e.to_string()),
    };
    let mut conn = open_for_settings(library)?;
    let has: bool = conn
        .query_row("SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'settings')", [], |r| {
            r.get(0)
        })
        .map_err(sql)?;
    if !has {
        return Err(write_error("it has no settings table, so nothing was changed".to_owned()));
    }
    // IMMEDIATE takes the write lock at the start, so the setting read is the one written over.
    let tx = conn.begin().map_err(sql)?;
    let now = read(&tx, key).map_err(write_error)?;
    let Some((text, meant)) = edit(now).map_err(write_error)? else {
        return Ok(false);
    };
    tx.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [key, text.as_str()],
    )
    .map_err(sql)?;
    if read(&tx, key).map_err(write_error)? != meant {
        // Dropping the transaction undoes it.
        return Err(write_error(format!("its {key} setting read back wasn't the one written, so nothing was changed")));
    }
    tx.commit().map_err(sql)?;
    Ok(true)
}

/// The text kept now: none when it's missing or NULL; one that isn't text is refused rather than read as none, so a
/// change never writes over what it couldn't read.
fn read_text(tx: &Transaction<'_>, key: &str) -> Result<Option<String>, String> {
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
        None => Ok(None),
        Some(Ok(text)) => Ok(text),
        Some(Err(())) => Err(format!("its {key} setting isn't text, so it was left as it is")),
    }
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
