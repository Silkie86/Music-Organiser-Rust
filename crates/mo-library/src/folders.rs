//! Adding and removing the folders the app looks in (Settings › Folders), in the C# app's `library.db`: the project
//! folders (`roots`) and the sample, MIDI and plugin folders (JSON lists in `settings`). This is the only place the
//! Rust app writes to `library.db` (Solomon's OK, 2026-11-03); everything else opens it read-only.
//!
//! Each change is one SQLite transaction: it reads the list as it is now (the C# app may have changed it since the
//! page showed it), changes it, reads it back to check, and only then keeps it. If anything goes wrong nothing is
//! changed. It never makes a library or a table: one that isn't there is left alone, with a message.
//!
//! Taking a project folder off the list doesn't take its projects off the Songs list (the C# app asks about that
//! separately): their ratings, notes and tags stay.

use std::path::Path;
use std::time::Duration;

use mo_formats::case;
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior};

use crate::LibraryError;

/// One of the folder lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FolderList {
    /// The project folders, the `roots` table.
    Projects,
    /// `sample_folders` in `settings`.
    Samples,
    /// `midi_folders` in `settings`.
    Midi,
    /// `plugin_folders` in `settings`. Empty means the standard folders, which the caller passes in.
    Plugins,
}

impl FolderList {
    /// The setting it's kept in; none for the project folders, which have their own table.
    pub fn setting_key(self) -> Option<&'static str> {
        match self {
            FolderList::Projects => None,
            FolderList::Samples => Some("sample_folders"),
            FolderList::Midi => Some("midi_folders"),
            FolderList::Plugins => Some("plugin_folders"),
        }
    }
}

/// What a change did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Changed {
    /// The list now, as kept.
    Done(Vec<String>),
    /// Nothing to do: the folder was already there (adding), or wasn't (removing).
    AsItWas,
}

/// Adds `folder` to `list`. A folder already there (whatever its letter case, as Windows sees it) is left as it is.
/// `plugin_defaults` are the standard plugin folders, which an empty plugin list stands for: adding to an empty plugin
/// list keeps them, as the C# app does.
pub fn add_folder(
    library: &Path,
    list: FolderList,
    folder: &str,
    plugin_defaults: &[String],
) -> Result<Changed, LibraryError> {
    let folder = folder.trim();
    if folder.is_empty() {
        return Ok(Changed::AsItWas);
    }
    change(library, list, plugin_defaults, |folders| {
        if folders.iter().any(|f| same_folder(f, folder)) {
            return false;
        }
        folders.push(folder.to_owned());
        true
    })
}

/// Takes `folder` off `list`. The folder itself and everything in it stay where they are. Taking the last plugin
/// folder off goes back to the standard ones.
pub fn remove_folder(
    library: &Path,
    list: FolderList,
    folder: &str,
    plugin_defaults: &[String],
) -> Result<Changed, LibraryError> {
    change(library, list, plugin_defaults, |folders| {
        let before = folders.len();
        folders.retain(|f| !same_folder(f, folder));
        folders.len() != before
    })
}

/// Whether two folders are one: equal but for letter case and a separator at the end.
fn same_folder(a: &str, b: &str) -> bool {
    let trim = |s: &str| s.trim_end_matches(['\\', '/']).to_owned();
    case::key(&trim(a)) == case::key(&trim(b))
}

/// Reads the list, lets `edit` change it, and keeps it, all in one transaction. `edit` says whether it changed it.
fn change(
    library: &Path,
    list: FolderList,
    plugin_defaults: &[String],
    edit: impl FnOnce(&mut Vec<String>) -> bool,
) -> Result<Changed, LibraryError> {
    let mut conn = open(library)?;
    let table = if list == FolderList::Projects { "roots" } else { "settings" };
    let has: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [table],
        |r| r.get(0),
    )?;
    if !has {
        return Err(LibraryError::Open {
            path: library.to_owned(),
            why: format!("it has no {table} table yet; open the C# app once to make it"),
        });
    }
    // IMMEDIATE takes the write lock at the start, so the list read is the one written over.
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut folders = read_list(&tx, list)?;
    if list == FolderList::Plugins && folders.is_empty() {
        folders = plugin_defaults.to_vec();
    }
    if !edit(&mut folders) {
        return Ok(Changed::AsItWas);
    }
    match list.setting_key() {
        None => {
            // Only what changed: every other row (and its letter case) stays as the C# app wrote it.
            let before = read_list(&tx, list)?;
            for gone in before.iter().filter(|b| !folders.iter().any(|f| f == *b)) {
                tx.execute("DELETE FROM roots WHERE path = ?1", [gone])?;
            }
            for new in folders.iter().filter(|f| !before.iter().any(|b| b == *f)) {
                tx.execute("INSERT OR IGNORE INTO roots VALUES (?1)", [new])?;
            }
        }
        Some(key) => {
            let json = serde_json::to_string(&folders).map_err(|e| LibraryError::Open {
                path: library.to_owned(),
                why: format!("couldn't write the list ({e})"),
            })?;
            tx.execute(
                "INSERT INTO settings VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [key, json.as_str()],
            )?;
        }
    }
    // Check before keeping it: the list read back must be the one meant.
    let mut written = read_list(&tx, list)?;
    let mut meant = folders.clone();
    if list == FolderList::Projects {
        // The table lists them in its own order.
        written.sort();
        meant.sort();
    }
    if written != meant {
        // Dropping the transaction undoes it.
        return Err(LibraryError::Open {
            path: library.to_owned(),
            why: "the folders read back weren't the ones written, so nothing was changed".to_owned(),
        });
    }
    tx.commit()?;
    Ok(Changed::Done(folders))
}

/// The list as kept now. A setting that isn't a list of text reads as empty, as in the C# app.
fn read_list(conn: &Connection, list: FolderList) -> Result<Vec<String>, LibraryError> {
    Ok(match list.setting_key() {
        None => {
            let mut st = conn.prepare("SELECT path FROM roots ORDER BY path")?;
            st.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<_, _>>()?
        }
        Some(key) => conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get::<_, Option<String>>(0))
            .optional()?
            .flatten()
            .and_then(|json| serde_json::from_str::<Vec<String>>(&json).ok())
            .unwrap_or_default(),
    })
}

/// Opens an existing library to change it: never makes one.
fn open(library: &Path) -> Result<Connection, LibraryError> {
    let open_error = |why: String| LibraryError::Open { path: library.to_owned(), why };
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(library, flags).map_err(|e| open_error(e.to_string()))?;
    // The C# app may be saving a scan: wait for it rather than fail.
    conn.busy_timeout(Duration::from_secs(5)).map_err(|e| open_error(e.to_string()))?;
    Ok(conn)
}
