//! Adding and removing the folders the app looks in (Settings › Folders), in the C# app's `library.db`: the project
//! folders (`roots`) and the sample, MIDI and plugin folders (JSON lists in `settings`). This, [`crate::moves`]
//! (a folder moved to another drive) and [`crate::plugin_folders`] (your say on a plugin's folders), both from the
//! Plugins page, are the only places the Rust app writes to `library.db`
//! (Solomon's OK: folders 2026-11-03, moves and the Plugins page's folder fixes 2026-11-05); everything else opens it
//! read-only.
//!
//! Each change is one SQLite transaction: it reads the list as it is now (the C# app may have changed it since the
//! page showed it), changes it, reads it back to check, and only then keeps it. If anything goes wrong nothing is
//! changed. It never makes a library or a table: one that isn't there is left alone, with a message.
//!
//! Taking a project folder off the list doesn't take its projects off the Songs list (the C# app asks about that
//! separately): their ratings, notes and tags stay.

use std::path::{Path, PathBuf};
use std::time::Duration;

use mo_formats::case;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior};

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
    let write_error = |why: String| LibraryError::Write { path: library.to_owned(), why };
    let sql = |e: rusqlite::Error| write_error(e.to_string());
    let mut conn = open_for_settings(library)?;
    let table = if list == FolderList::Projects { "roots" } else { "settings" };
    let has: bool = conn
        .query_row("SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)", [table], |r| {
            r.get(0)
        })
        .map_err(sql)?;
    if !has {
        return Err(write_error(format!("it has no {table} table yet; open the C# app once to make it")));
    }
    // IMMEDIATE takes the write lock at the start, so the list read is the one written over.
    let tx = conn.begin().map_err(sql)?;
    let before = read_list(&tx, list).map_err(write_error)?;
    let mut folders = before.clone();
    if list == FolderList::Plugins && folders.is_empty() {
        folders = plugin_defaults.to_vec();
    }
    if !edit(&mut folders) {
        return Ok(Changed::AsItWas);
    }
    match list.setting_key() {
        None => {
            // Only what changed: every other row (and its letter case) stays as the C# app wrote it.
            for gone in before.iter().filter(|b| !folders.iter().any(|f| f == *b)) {
                tx.execute("DELETE FROM roots WHERE path = ?1", [gone]).map_err(sql)?;
            }
            for new in folders.iter().filter(|f| !before.iter().any(|b| b == *f)) {
                tx.execute("INSERT OR IGNORE INTO roots (path) VALUES (?1)", [new]).map_err(sql)?;
            }
        }
        Some(key) => {
            let json =
                serde_json::to_string(&folders).map_err(|e| write_error(format!("couldn't write the list ({e})")))?;
            tx.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [key, json.as_str()],
            )
            .map_err(sql)?;
        }
    }
    // Check before keeping it: the list read back must be the one meant.
    let mut written = read_list(&tx, list).map_err(write_error)?;
    let mut meant = folders.clone();
    if list == FolderList::Projects {
        // The table lists them in its own order.
        written.sort();
        meant.sort();
    }
    if written != meant {
        // Dropping the transaction undoes it.
        return Err(write_error("the folders read back weren't the ones written, so nothing was changed".to_owned()));
    }
    tx.commit().map_err(sql)?;
    Ok(Changed::Done(folders))
}

/// The list as kept now, read the way the page reads it ([`crate::Library::roots`], [`crate::Library::setting`]): a
/// path that isn't text reads as "", a setting that is NULL or missing as no folders. A setting that holds something
/// that isn't a list of text is refused rather than read as empty, so a change never writes over folders it couldn't
/// read.
fn read_list(conn: &Connection, list: FolderList) -> Result<Vec<String>, String> {
    let lossy = |v: ValueRef<'_>| match v {
        ValueRef::Text(t) => Some(String::from_utf8_lossy(t).into_owned()),
        _ => None,
    };
    match list.setting_key() {
        None => {
            let mut st = conn.prepare("SELECT path FROM roots ORDER BY path").map_err(|e| e.to_string())?;
            st.query_map([], |r| Ok(lossy(r.get_ref(0)?).unwrap_or_default()))
                .and_then(|rows| rows.collect())
                .map_err(|e| e.to_string())
        }
        Some(key) => {
            let value = conn
                .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
                    Ok(match r.get_ref(0)? {
                        ValueRef::Null => Ok(None),
                        v => lossy(v).map(Some).ok_or(()),
                    })
                })
                .optional()
                .map_err(|e| e.to_string())?;
            match value {
                None | Some(Ok(None)) => Ok(Vec::new()),
                Some(Ok(Some(json))) => serde_json::from_str::<Vec<String>>(&json)
                    .map_err(|_| format!("its {key} setting isn't a list the app can read, so it was left as it is")),
                Some(Err(())) => Err(format!("its {key} setting isn't text, so it was left as it is")),
            }
        }
    }
}

/// Opens an existing library to change its settings or folder lists: never makes one. Only for writes to what the
/// saves list doesn't hold (the `settings` and `roots` tables, as [`change`] and [`crate::setting_list`] do): once the
/// connection is closed, the app's copy of the list is marked as still fitting ([`crate::list_copy`]), so the next
/// start doesn't read the whole library again. A writer of anything the list shows (a save's rating, notes, tags) must
/// not use this.
pub(crate) fn open_for_settings(library: &Path) -> Result<SettingsWriter, LibraryError> {
    let open_error = |why: String| LibraryError::Open { path: library.to_owned(), why };
    if !library.is_file() {
        return Err(open_error("it isn't there".to_owned()));
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(library, flags).map_err(|e| open_error(e.to_string()))?;
    // The C# app may be saving a scan: wait for it rather than fail.
    conn.busy_timeout(Duration::from_secs(5)).map_err(|e| open_error(e.to_string()))?;
    Ok(SettingsWriter { conn, mark: MarkCopy { library: library.to_owned(), before: None } })
}

/// A connection that changes the library's settings or folder lists ([`open_for_settings`]). Its parts are let go in
/// this order: the connection is closed first (SQLite may tidy its `-wal` then), and then the saves-list copy is marked.
pub(crate) struct SettingsWriter {
    conn: Connection,
    mark: MarkCopy,
}

impl SettingsWriter {
    /// Starts the change's one transaction. IMMEDIATE takes the write lock at the start, so what's read is what's
    /// written over; and the library's files are looked at once the lock is held, when nothing else can write.
    pub(crate) fn begin(&mut self) -> rusqlite::Result<Transaction<'_>> {
        let SettingsWriter { conn, mark } = self;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        mark.before = crate::list_copy::stamp_of(&mark.library);
        Ok(tx)
    }
}

impl std::ops::Deref for SettingsWriter {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.conn
    }
}

/// Marks the saves-list copy once the writer's connection is closed ([`crate::list_copy::after_own_write`]).
struct MarkCopy {
    library: PathBuf,
    /// The library's files when the write lock was taken; none before [`SettingsWriter::begin`].
    before: Option<String>,
}

impl Drop for MarkCopy {
    fn drop(&mut self) {
        if let Some(before) = &self.before {
            crate::list_copy::after_own_write(&self.library, before);
        }
    }
}
