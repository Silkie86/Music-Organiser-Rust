//! The app keeping its own measure of your plugins in its song library, `songs.db` (review X1-2): which plugins are
//! installed (table `installed_plugins`) and how much room each takes (table `plugin_space`). The C# app measured
//! these and nothing has since it was retired, so a new plugin was never found and a removed one never went.
//!
//! Only a `songs.db` that's the app's own ([`crate::songs_db::own_since`]) is ever written here; never `library.db`.
//! Each write is one SQLite transaction, IMMEDIATE so nothing else writes meanwhile, as [`crate::song_writes`]: both
//! tables are replaced whole, read back and checked, and only then kept. If anything goes wrong nothing is changed,
//! and the old measure stays. The measuring itself is done before, away from the transaction, so the write is short
//! (the Songs watch writes `songs.db` in the background too).

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use chrono::NaiveDateTime;
use mo_formats::PluginRef;
use mo_formats::plugins::name_key;
use rusqlite::{Connection, ErrorCode, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::Serialize;

use crate::LibraryError;
use crate::plugins::{StoredFolder, StoredSpace};
use crate::read::Installed;

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

/// Bump when what counts as installed changes ([`crate::read::Installed::has`]), so every save's plugins are looked at
/// again at the next start. 2: a Waves plugin named plainly with Waves as its vendor (Bitwig's "Smack Attack Stereo").
pub const INSTALLED_RULES: u32 = 2;

/// The note in `songs_info` of what the saves' plugins were last looked at against: [`INSTALLED_RULES`] and the
/// installed plugins then.
const RECHECKED_KEY: &str = "installed_rechecked";

/// What [`recheck_installed`] found.
#[derive(Debug, Default)]
pub struct Rechecked {
    /// The installed plugins as the library has them now.
    pub installed: Installed,
    /// The saves whose count of missing plugins changed, with the new count, by id.
    pub missing: Vec<(i64, u32)>,
    /// Whether the saves' plugins were looked at (not when nothing changed since the last time).
    pub looked: bool,
}

/// Looks again at whether each save's plugins are installed, as a save's own read decides, and keeps any that changed
/// in the app's own `songs.db`: a save is only read when its file changes, so a plugin installed (or removed) since, or
/// a better rule ([`INSTALLED_RULES`]), would otherwise never reach it. Only when the installed plugins or the rules
/// changed since the last look. Asks the disk (each plugin's file), so never on the window's thread; the looking is
/// done before the write, which is one short transaction.
pub fn recheck_installed(songs_db: &Path) -> Result<Rechecked, LibraryError> {
    let write_error = |why: String| LibraryError::Write { path: songs_db.to_owned(), why };
    let sql = |e: rusqlite::Error| match e.sqlite_error_code() {
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => LibraryError::Busy { path: songs_db.to_owned() },
        _ => write_error(e.to_string()),
    };
    if crate::songs_db::own_since(songs_db).is_none() {
        return Err(write_error("it isn't the app's own song library (yet), so it isn't written".to_owned()));
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let mut conn = Connection::open_with_flags(songs_db, flags).map_err(sql)?;
    conn.busy_timeout(WAIT).map_err(sql)?;
    let installed = crate::db::installed_in(&conn, true, true).map_err(sql)?;
    let stamp = stamp(&installed);
    let noted: Option<String> = conn
        .query_row("SELECT value FROM songs_info WHERE key = ?1", [RECHECKED_KEY], |r| r.get(0))
        .optional()
        .map_err(sql)?;
    if noted.as_deref() == Some(stamp.as_str()) {
        return Ok(Rechecked { installed, ..Rechecked::default() });
    }
    // Each plugin as the saves name it, once: far fewer than the saves' rows.
    let plugins: Vec<PluginRef> = {
        let mut st = conn.prepare("SELECT DISTINCT name, vendor, format, path FROM plugins").map_err(sql)?;
        let rows = st.query_map([], |r| {
            let text = |i: usize| r.get::<_, Option<String>>(i).map(Option::unwrap_or_default);
            Ok(PluginRef { name: text(0)?, vendor: text(1)?, format: text(2)?, path: text(3)?, ..PluginRef::default() })
        });
        rows.map_err(sql)?.collect::<rusqlite::Result<_>>().map_err(sql)?
    };
    let flags: Vec<(PluginRef, bool)> = plugins
        .into_iter()
        .map(|p| {
            let has = installed.has(&p);
            (p, has)
        })
        .collect();

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(sql)?;
    let mut changed: BTreeSet<i64> = BTreeSet::new();
    {
        let same = "COALESCE(name, '') = ?1 AND COALESCE(vendor, '') = ?2 AND COALESCE(format, '') = ?3 \
                    AND COALESCE(path, '') = ?4 AND COALESCE(installed, -1) != ?5";
        let mut which = tx.prepare(&format!("SELECT DISTINCT project_id FROM plugins WHERE {same}")).map_err(sql)?;
        let mut set = tx.prepare(&format!("UPDATE plugins SET installed = ?5 WHERE {same}")).map_err(sql)?;
        for (p, has) in &flags {
            let args = params![p.name, p.vendor, p.format, p.path, i64::from(*has)];
            let ids = which.query_map(args, |r| r.get::<_, i64>(0)).map_err(sql)?;
            let ids: Vec<i64> = ids.collect::<rusqlite::Result<_>>().map_err(sql)?;
            if ids.is_empty() {
                continue;
            }
            set.execute(args).map_err(sql)?;
            changed.extend(ids);
        }
    }
    let mut missing = Vec::with_capacity(changed.len());
    {
        let mut count =
            tx.prepare("SELECT COUNT(*) FROM plugins WHERE project_id = ?1 AND installed = 0").map_err(sql)?;
        for id in changed {
            let n: i64 = count.query_row([id], |r| r.get(0)).map_err(sql)?;
            missing.push((id, u32::try_from(n).unwrap_or(u32::MAX)));
        }
    }
    tx.execute("INSERT OR REPLACE INTO songs_info (key, value) VALUES (?1, ?2)", params![RECHECKED_KEY, stamp])
        .map_err(sql)?;
    tx.commit().map_err(sql)?;
    Ok(Rechecked { installed, missing, looked: true })
}

/// What the saves' plugins are looked at against, in short: the rules' version and a fingerprint of the installed
/// plugins (FNV-1a over their sorted keys and ids, stable from one build to the next).
fn stamp(installed: &Installed) -> String {
    let mut all: Vec<&str> = installed.keys.iter().chain(&installed.ids).map(String::as_str).collect();
    all.sort_unstable();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for s in all {
        for b in s.bytes().chain([0]) {
            hash = (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!("{INSTALLED_RULES}:{}:{hash:016x}", installed.keys.len() + installed.ids.len())
}
