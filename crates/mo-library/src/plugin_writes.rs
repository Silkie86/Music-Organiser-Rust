//! The app keeping its own measure of your plugins in its song library, `songs.db` (review X1-2): which plugins are
//! installed (table `installed_plugins`) and how much room each takes (table `plugin_space`). The C# app measured
//! these and nothing has since it was retired, so a new plugin was never found and a removed one never went.
//!
//! Only a `songs.db` that's the app's own ([`crate::songs_db::own_since`]) is ever written here; never `library.db`.
//! Each write is one SQLite transaction, IMMEDIATE so nothing else writes meanwhile, as [`crate::song_writes`]: both
//! tables are replaced whole, read back and checked, and only then kept. If anything goes wrong nothing is changed,
//! and the old measure stays. The measuring itself is done before, away from the transaction, so the write is short
//! (the Songs watch writes `songs.db` in the background too).

use std::path::Path;
use std::time::Duration;

use chrono::NaiveDateTime;
use mo_formats::plugins::name_key;
use rusqlite::{Connection, ErrorCode, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::Serialize;

use crate::LibraryError;
use crate::plugins::{StoredFolder, StoredSpace};

/// What a measure found: the installed plugins' names, and each one's room on disk.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Measure {
    /// The installed plugins' names, as their files name them. Two with one name key are one plugin: the first
    /// name is kept.
    pub installed: Vec<String>,
    /// Each installed plugin's room, by its name key (the first of a key is kept). Its `measured` is when.
    pub spaces: Vec<StoredSpace>,
}

/// How long a write waits for another one to finish its own: this runs in the background.
const WAIT: Duration = Duration::from_secs(5);

/// The folder list as the C# app wrote it (System.Text.Json), so [`crate::Library::plugin_spaces`] reads both alike.
#[derive(Serialize)]
struct Row<'a> {
    #[serde(rename = "Path")]
    path: &'a str,
    #[serde(rename = "Kind")]
    kind: u8,
    #[serde(rename = "Bytes")]
    bytes: i64,
    #[serde(rename = "SharedWith")]
    shared_with: &'a [String],
    #[serde(rename = "Added")]
    added: bool,
}

fn folders_json(folders: &[StoredFolder]) -> String {
    let rows: Vec<Row<'_>> = folders
        .iter()
        .map(|f| Row {
            path: &f.path,
            kind: f.kind.min(2),
            bytes: i64::try_from(f.bytes).unwrap_or(i64::MAX),
            shared_with: &f.shared_with,
            added: f.added,
        })
        .collect();
    serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_owned())
}

fn seconds(t: Option<NaiveDateTime>) -> Option<i64> {
    use chrono::TimeZone;
    t.and_then(|t| chrono::Local.from_local_datetime(&t).earliest()).map(|t| t.timestamp())
}

/// The rows `measure` makes, one per name key, as they're written and read back: (name, name key) and (name key,
/// name, maker, folders, measured).
type Rows = (Vec<(String, String)>, Vec<(String, String, String, String, Option<i64>)>);

fn rows(measure: &Measure) -> Rows {
    let mut seen = std::collections::HashSet::new();
    let installed = measure
        .installed
        .iter()
        .map(|n| (n.clone(), name_key(n)))
        .filter(|(_, k)| !k.is_empty() && seen.insert(k.clone()))
        .collect();
    let mut seen = std::collections::HashSet::new();
    let spaces = measure
        .spaces
        .iter()
        .filter(|s| !s.key.is_empty() && seen.insert(s.key.clone()))
        .map(|s| (s.key.clone(), s.name.clone(), s.vendor.clone(), folders_json(&s.folders), seconds(s.measured)))
        .collect();
    (installed, spaces)
}

/// Keeps `measure` in the app's own `songs.db` at `songs_db` in place of the last one, all of it or none.
pub fn write(songs_db: &Path, measure: &Measure) -> Result<(), LibraryError> {
    let write_error = |why: String| LibraryError::Write { path: songs_db.to_owned(), why };
    let sql = |e: rusqlite::Error| match e.sqlite_error_code() {
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => LibraryError::Busy { path: songs_db.to_owned() },
        _ => write_error(e.to_string()),
    };
    let not_own = || write_error("it isn't the app's own song library (yet), so it isn't written".to_owned());
    if !songs_db.is_file() {
        return Err(write_error("it isn't there".to_owned()));
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let mut conn = Connection::open_with_flags(songs_db, flags).map_err(sql)?;
    conn.busy_timeout(WAIT).map_err(sql)?;
    let id: i32 = conn.query_row("PRAGMA application_id", [], |r| r.get(0)).map_err(sql)?;
    let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).map_err(sql)?;
    if id != crate::songs_db::APPLICATION_ID || version != crate::songs_db::VERSION {
        return Err(not_own());
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(sql)?;
    // Looked at under the write lock: only the app's own library is written.
    let own: Option<rusqlite::types::Value> = tx
        .query_row("SELECT value FROM songs_info WHERE key = 'own_since'", [], |r| r.get(0))
        .optional()
        .map_err(sql)?;
    if own.is_none() {
        return Err(not_own());
    }
    let wanted = rows(measure);
    put(&tx, &wanted).map_err(sql)?;
    let back = read_back(&tx).map_err(sql)?;
    if let Some(why) = differs(&wanted, &back) {
        return Err(write_error(format!("what was written didn't read back the same ({why})")));
    }
    tx.commit().map_err(sql)
}

fn put(tx: &Transaction<'_>, (installed, spaces): &Rows) -> rusqlite::Result<()> {
    tx.execute("DELETE FROM installed_plugins", [])?;
    tx.execute("DELETE FROM plugin_space", [])?;
    let mut st = tx.prepare_cached("INSERT INTO installed_plugins (name, name_key) VALUES (?1, ?2)")?;
    for (name, key) in installed {
        st.execute(params![name, key])?;
    }
    let mut st = tx.prepare_cached(
        "INSERT INTO plugin_space (name_key, name, vendor, folders, measured) VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    for (key, name, vendor, folders, measured) in spaces {
        st.execute(params![key, name, vendor, folders, measured])?;
    }
    Ok(())
}

fn read_back(tx: &Transaction<'_>) -> rusqlite::Result<Rows> {
    let mut st = tx.prepare("SELECT name, name_key FROM installed_plugins ORDER BY name_key")?;
    let installed = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
    let mut st = tx.prepare("SELECT name_key, name, vendor, folders, measured FROM plugin_space ORDER BY name_key")?;
    let spaces = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok((installed, spaces))
}

/// What's different between the rows meant and the rows read back, if anything.
fn differs(wanted: &Rows, back: &Rows) -> Option<String> {
    let mut installed = wanted.0.clone();
    installed.sort_by(|a, b| a.1.cmp(&b.1));
    let mut spaces = wanted.1.clone();
    spaces.sort_by(|a, b| a.0.cmp(&b.0));
    if installed != back.0 {
        return Some(format!("{} installed plugins meant, {} read back", installed.len(), back.0.len()));
    }
    if spaces != back.1 {
        return Some(format!("{} plugins' room meant, {} read back", spaces.len(), back.1.len()));
    }
    None
}
