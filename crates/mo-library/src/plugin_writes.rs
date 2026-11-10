//! The app keeping its own measure of your plugins in its song library, `songs.db` (review X1-2): which plugins are
//! installed (table `installed_plugins`) and how much room each takes (table `plugin_space`). The C# app measured
//! these and nothing has since it was retired, so a new plugin was never found and a removed one never went.
//!
//! Only a `songs.db` that's the app's own ([`crate::songs_db::own_since`]) is ever written here; never `library.db`.
//! Each write is one SQLite transaction, IMMEDIATE so nothing else writes meanwhile, as [`crate::song_writes`]: both
//! tables are replaced whole, read back and checked, and only then kept. If anything goes wrong nothing is changed,
//! and the old measure stays. The measuring itself is done before, away from the transaction, so the write is short
//! (the Songs watch writes `songs.db` in the background too).

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use chrono::NaiveDateTime;
use mo_formats::PluginRef;
use mo_formats::plugins::name_key;
use rusqlite::{Connection, ErrorCode, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::Serialize;

use crate::LibraryError;
use crate::plugins::{StoredFolder, StoredSpace};
use crate::read::{INSTALLED_RULES, Installed};

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

/// How many measures [`write`] has kept since the app started, so the Songs watch knows to read the installed plugins
/// again ([`measures_kept`]).
static MEASURES_KEPT: AtomicU64 = AtomicU64::new(0);

/// How many plugin measures have been kept since the app started.
pub fn measures_kept() -> u64 {
    MEASURES_KEPT.load(Ordering::Relaxed)
}

/// Keeps `measure` in the app's own `songs.db` at `songs_db` in place of the last one, all of it or none.
pub fn write(songs_db: &Path, measure: &Measure) -> Result<(), LibraryError> {
    let (mut conn, sql) = open_own(songs_db)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(&sql)?;
    own_under_lock(songs_db, &tx)?;
    let wanted = rows(measure);
    put(&tx, &wanted).map_err(&sql)?;
    let back = read_back(&tx).map_err(&sql)?;
    if let Some(why) = differs(&wanted, &back) {
        return Err(write_error(songs_db, format!("what was written didn't read back the same ({why})")));
    }
    tx.commit().map_err(&sql)?;
    MEASURES_KEPT.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

fn write_error(songs_db: &Path, why: String) -> LibraryError {
    LibraryError::Write { path: songs_db.to_owned(), why }
}

/// An SQLite error writing `songs_db`: busy when another copy of the app holds it.
fn sql_error(songs_db: &Path, e: rusqlite::Error) -> LibraryError {
    match e.sqlite_error_code() {
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => LibraryError::Busy { path: songs_db.to_owned() },
        _ => write_error(songs_db, e.to_string()),
    }
}

fn not_own(songs_db: &Path) -> LibraryError {
    write_error(songs_db, "it isn't the app's own song library (yet), so it isn't written".to_owned())
}

/// Opens `songs_db` to write, when it's there and made by the app; with how its SQLite errors are told.
fn open_own(songs_db: &Path) -> Result<(Connection, impl Fn(rusqlite::Error) -> LibraryError + '_), LibraryError> {
    let sql = move |e| sql_error(songs_db, e);
    if !songs_db.is_file() {
        return Err(write_error(songs_db, "it isn't there".to_owned()));
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(songs_db, flags).map_err(sql)?;
    conn.busy_timeout(WAIT).map_err(sql)?;
    let id: i32 = conn.query_row("PRAGMA application_id", [], |r| r.get(0)).map_err(sql)?;
    let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).map_err(sql)?;
    if id != crate::songs_db::APPLICATION_ID || version != crate::songs_db::VERSION {
        return Err(not_own(songs_db));
    }
    Ok((conn, sql))
}

/// Looked at under the write lock: only the app's own library is written.
fn own_under_lock(songs_db: &Path, tx: &Transaction<'_>) -> Result<(), LibraryError> {
    let own: Option<rusqlite::types::Value> = tx
        .query_row("SELECT value FROM songs_info WHERE key = 'own_since'", [], |r| r.get(0))
        .optional()
        .map_err(|e| sql_error(songs_db, e))?;
    if own.is_none() { Err(not_own(songs_db)) } else { Ok(()) }
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

/// The note in `songs_info` of what the saves' plugins were last looked at against: the rules' version
/// ([`INSTALLED_RULES`]) and the installed plugins then.
const RECHECKED_KEY: &str = "installed_rechecked";

/// What [`recheck_installed`] found.
#[derive(Debug, Default)]
pub struct Rechecked {
    /// The installed plugins as the library has them now.
    pub installed: Installed,
    /// The saves that now miss fewer plugins, with how many they miss, by id.
    pub missing: Vec<(i64, u32)>,
    /// Whether the saves' plugins were looked at (not when nothing changed since the last time).
    pub looked: bool,
}

/// Looks again at the saves' plugins marked missing, as a save's own read decides ([`Installed::has`]), and marks the
/// ones found installed now in the app's own `songs.db`: a save is only read when its file changes, so a plugin
/// installed since, or a better rule ([`INSTALLED_RULES`]), would otherwise never reach it. Only ever from missing to
/// installed: the library doesn't keep the id a save found a plugin by, so a plugin marked installed can't be judged
/// again here (one removed shows as missing when its save is read again). Only when the installed plugins or the rules
/// changed since the last look. Asks the disk (each plugin's file), so never on the window's thread; the looking is
/// done before the write, which is one short transaction.
pub fn recheck_installed(songs_db: &Path) -> Result<Rechecked, LibraryError> {
    let (mut conn, sql) = open_own(songs_db)?;
    let installed = crate::db::installed_in(&conn, true, true).map_err(&sql)?;
    let stamp = stamp(&installed);
    let noted: Option<String> = conn
        .query_row("SELECT value FROM songs_info WHERE key = ?1", [RECHECKED_KEY], |r| r.get(0))
        .optional()
        .map_err(&sql)?;
    if noted.as_deref() == Some(stamp.as_str()) {
        return Ok(Rechecked { installed, ..Rechecked::default() });
    }
    // The rows marked missing, read once; each plugin they name judged once.
    let rows: Vec<(i64, i64, PluginRef)> = {
        let mut st = conn
            .prepare(
                "SELECT rowid, project_id, name, vendor, format, path FROM plugins WHERE COALESCE(installed, 0) = 0",
            )
            .map_err(&sql)?;
        let rows = st.query_map([], |r| {
            let text = |i: usize| r.get::<_, Option<String>>(i).map(Option::unwrap_or_default);
            let p = PluginRef {
                name: text(2)?,
                vendor: text(3)?,
                format: text(4)?,
                path: text(5)?,
                ..PluginRef::default()
            };
            Ok((r.get(0)?, r.get::<_, Option<i64>>(1)?.unwrap_or(0), p))
        });
        rows.map_err(&sql)?.collect::<rusqlite::Result<_>>().map_err(&sql)?
    };
    let mut judged: HashMap<(&str, &str, &str, &str), bool> = HashMap::new();
    let found: Vec<&(i64, i64, PluginRef)> = rows
        .iter()
        .filter(|(_, _, p)| *judged.entry((&p.name, &p.vendor, &p.format, &p.path)).or_insert_with(|| installed.has(p)))
        .collect();

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(&sql)?;
    own_under_lock(songs_db, &tx)?;
    let mut changed: BTreeSet<i64> = BTreeSet::new();
    {
        // By its row, but only while it's still the same plugin of the same save, still missing: another copy of the
        // app may have kept that save again meanwhile, and its new rows can take the old row numbers (review G-4).
        let mut set = tx
            .prepare(
                "UPDATE plugins SET installed = 1 WHERE rowid = ?1 AND project_id = ?2 AND COALESCE(name, '') = ?3 \
                 AND COALESCE(vendor, '') = ?4 AND COALESCE(format, '') = ?5 AND COALESCE(path, '') = ?6 \
                 AND COALESCE(installed, 0) = 0",
            )
            .map_err(&sql)?;
        for (row, project, p) in found {
            if set.execute(params![row, project, p.name, p.vendor, p.format, p.path]).map_err(&sql)? > 0 {
                changed.insert(*project);
            }
        }
    }
    let mut missing = Vec::with_capacity(changed.len());
    {
        let mut count =
            tx.prepare("SELECT COUNT(*) FROM plugins WHERE project_id = ?1 AND installed = 0").map_err(&sql)?;
        for id in changed {
            let n: i64 = count.query_row([id], |r| r.get(0)).map_err(&sql)?;
            missing.push((id, u32::try_from(n).unwrap_or(u32::MAX)));
        }
    }
    tx.execute("INSERT OR REPLACE INTO songs_info (key, value) VALUES (?1, ?2)", params![RECHECKED_KEY, stamp])
        .map_err(&sql)?;
    tx.commit().map_err(&sql)?;
    Ok(Rechecked { installed, missing, looked: true })
}

/// What the saves' plugins are looked at against, in short: the rules' version and a fingerprint of everything
/// [`Installed::has`] looks at (FNV-1a over each set sorted, stable from one build to the next).
fn stamp(installed: &Installed) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut add = |b: u8| hash = (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    for (n, set) in [&installed.keys, &installed.names, &installed.ids, &installed.waves].into_iter().enumerate() {
        let mut all: Vec<&String> = set.iter().collect();
        all.sort_unstable();
        for s in all {
            s.bytes().chain([0]).for_each(&mut add);
        }
        add(0xff);
        add(n as u8);
    }
    format!("{INSTALLED_RULES}:{hash:016x}")
}
