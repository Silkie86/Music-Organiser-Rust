//! `songs.db`: the app's own song library, in place of the C# app's `library.db` (the C# app is retired, Solomon
//! 2026-11-05).
//!
//! **What's in it.** The tables of `library.db` with their columns as the C# app last made them, so everything that
//! reads the library ([`Library`]) and the writers of its settings and folder lists work on it as they do on
//! `library.db`. Three things differ:
//!
//! - Each save's track details (the C# app's `info`: tens of KB of JSON in the middle of every `projects` row) are in a
//!   table of their own, `project_info`, compressed. The list of saves never shows them, but read out of `library.db`
//!   it had to read through them: most of a 720 MB library, 47-104 s of every start on Solomon's PC, which is why the
//!   saves-list copy ([`crate::list_copy`]) was made. Read out of `songs.db`, the list needs no copy.
//! - Two tables the C# app worked out for itself and the Rust app never reads are left out: `sample_index` (the files
//!   in the sample folders) and `plugin_ids`.
//! - `songs_info` notes what it was made from (`made_from`, the [`crate::list_copy::stamp_of`] of `library.db` then),
//!   when (`made_at`), and since when it's been the app's own library (`own_since`), all in Unix seconds.
//!
//! **How it's made** ([`make`]): once, in the background, from `library.db` read in one go (one snapshot, never
//! changed), into a file of its own beside it (`songs.db.new-<process>`). Then it's checked: every row of every table
//! read back and compared with what was read, by SHA-256 (the track details uncompressed again), then SQLite's own
//! integrity check. Only then is it renamed to `songs.db`.
//!
//! **When it's used** ([`at_start`]): from the next start, when `library.db` hasn't changed since it was made (or has
//! gone); one that's out of step is made again. From then on it's the app's own (`own_since`): the app opens it and
//! never `library.db`, which stays where it is for the archive (step 6 of the new databases).

use std::collections::HashSet;
use std::fs;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use flate2::Compression;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use rusqlite::types::{Value, ValueRef};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params_from_iter};
use sha2::{Digest, Sha256};

use crate::list_copy::{stamp_of, with_ending};
use crate::{Library, LibraryError};

/// The app's own song library, in the app's data folder.
pub const FILE_NAME: &str = "songs.db";
/// The C# app's library it's made from, beside it.
pub const OLD_FILE_NAME: &str = "library.db";

/// SQLite's `application_id` for `songs.db` ("MOSG"): a file without it isn't one.
pub(crate) const APPLICATION_ID: i32 = 0x4D4F_5347;
/// The layout of `songs.db` (SQLite's `user_version`). One of a newer layout, made by a newer version of the app, is
/// left alone, and `library.db` read.
pub const VERSION: i32 = 1;

/// The tables made (the C# app's own layout: `LibraryDb.cs`, `.PluginScans.cs`, `.Space.cs`), but for `projects`'
/// `info`, which goes to `project_info`. Their indexes come after they're filled ([`INDEXES`]).
const TABLES: &str = "
    CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
    CREATE TABLE song_choices (kind TEXT, a TEXT, b TEXT, PRIMARY KEY (kind, a));
    CREATE TABLE roots (path TEXT PRIMARY KEY);
    CREATE TABLE projects (
        id INTEGER PRIMARY KEY,
        path TEXT UNIQUE NOT NULL COLLATE NOCASE,
        name TEXT NOT NULL, daw TEXT NOT NULL, folder TEXT, size INTEGER, mtime INTEGER,
        tempo REAL, version TEXT, title TEXT, confidence INTEGER,
        can_midi INTEGER DEFAULT 0, can_presets INTEGER DEFAULT 0,
        error TEXT, file_missing INTEGER DEFAULT 0, hardware TEXT DEFAULT '',
        status TEXT DEFAULT 'Idea', rating INTEGER DEFAULT 0, favourite INTEGER DEFAULT 0,
        musical_key TEXT DEFAULT '', notes TEXT DEFAULT '', last_opened INTEGER,
        detected_key TEXT, created INTEGER, note_count INTEGER, length_bars INTEGER, track_count INTEGER
    );
    CREATE TABLE project_info (
        project_id INTEGER PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
        info BLOB NOT NULL
    );
    CREATE TABLE plugins (
        project_id INTEGER REFERENCES projects(id) ON DELETE CASCADE,
        name TEXT, name_key TEXT, vendor TEXT, kind INTEGER, format TEXT, path TEXT, installed INTEGER
    );
    CREATE TABLE samples (
        project_id INTEGER REFERENCES projects(id) ON DELETE CASCADE,
        path TEXT, found INTEGER, unplayed INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE tags (
        project_id INTEGER REFERENCES projects(id) ON DELETE CASCADE,
        tag TEXT, UNIQUE(project_id, tag)
    );
    CREATE TABLE installed_plugins (name TEXT, name_key TEXT PRIMARY KEY);
    CREATE TABLE plugin_scans (
        path TEXT PRIMARY KEY COLLATE NOCASE, size INTEGER, mtime INTEGER, scanned INTEGER, error TEXT
    );
    CREATE TABLE scanned_plugins (
        path TEXT COLLATE NOCASE, format TEXT, id TEXT, name TEXT, name_key TEXT, vendor TEXT, version TEXT,
        kind TEXT, json TEXT
    );
    CREATE TABLE plugin_space (name_key TEXT PRIMARY KEY, name TEXT, vendor TEXT, folders TEXT, measured INTEGER);
    CREATE TABLE folder_space (folder TEXT PRIMARY KEY COLLATE NOCASE, bytes INTEGER, measured INTEGER);
    CREATE TABLE songs_info (key TEXT PRIMARY KEY, value);
";

/// The C# app's indexes, made once the tables are filled (quicker than keeping them up to date row by row).
const INDEXES: &str = "
    CREATE INDEX ix_plugins_project ON plugins(project_id);
    CREATE INDEX ix_plugins_key ON plugins(name_key);
    CREATE INDEX ix_samples_project ON samples(project_id);
    CREATE INDEX ix_samples_path ON samples(path COLLATE NOCASE);
    CREATE INDEX ix_tags_tag ON tags(tag);
    CREATE INDEX ix_projects_folder ON projects(folder);
    CREATE INDEX ix_scanned_plugins_path ON scanned_plugins(path);
    CREATE INDEX ix_scanned_plugins_key ON scanned_plugins(name_key);
";

/// The tables copied as they are, row for row (`projects` is copied too, its `info` apart).
const WHOLE: [&str; 11] = [
    "settings",
    "song_choices",
    "roots",
    "tags",
    "plugins",
    "samples",
    "installed_plugins",
    "plugin_scans",
    "scanned_plugins",
    "plugin_space",
    "folder_space",
];

/// What the C# app worked out for itself and the Rust app never reads: not copied.
const LEFT_OUT: [&str; 2] = ["sample_index", "plugin_ids"];

/// Which library the app opens at this start ([`at_start`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AtStart {
    /// The file every page reads (and the settings writers write).
    pub library: PathBuf,
    pub state: State,
}

/// Why [`AtStart::library`] is the file it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// `songs.db`, the app's own library.
    Own,
    /// `songs.db`, the app's own library from now on: it was made last time, and `library.db` hasn't changed since.
    Switched,
    /// `library.db`, as before: `songs.db` is to be made from it ([`make`]), because `why`.
    Old { why: String },
    /// `library.db`, as before, and no `songs.db` is made: `why`.
    Kept { why: String },
}

impl AtStart {
    /// Whether `songs.db` should be made from `library.db`, in the background.
    pub fn make_wanted(&self) -> bool {
        matches!(self.state, State::Old { .. })
    }

    /// Whether the app reads its own `songs.db` (rather than `library.db`).
    pub fn is_songs_db(&self) -> bool {
        matches!(self.state, State::Own | State::Switched)
    }

    /// What the log says.
    pub fn line(&self) -> String {
        match &self.state {
            State::Own => "Reading songs.db, the app's own song library".to_owned(),
            State::Switched => {
                "songs.db is the app's own song library from now on: library.db is no longer read".to_owned()
            }
            State::Old { why } => format!("Reading library.db: {why}, so songs.db will be made from it"),
            State::Kept { why } => format!("Reading library.db: {why}"),
        }
    }
}

/// How long the start waits for another program (another copy of the app, a virus checker) to let go of `songs.db`
/// before reading `library.db` this time: it holds up the window opening.
const START_WAIT: Duration = Duration::from_millis(250);

/// Which library the app opens at this start, in its data folder `dir`: `songs.db` once it's been made and checked
/// and `library.db` hasn't changed since (it becomes the app's own library then), else `library.db` as before. Looks
/// only at `songs.db`'s own notes and `library.db`'s size and times, so it's quick enough for the start. `app` is this
/// build's version: a `songs.db` that couldn't be made twice from this `library.db` by this build waits a while
/// ([`note_failed`]).
pub fn at_start(dir: &Path, app: &str) -> AtStart {
    let songs = dir.join(FILE_NAME);
    let old = dir.join(OLD_FILE_NAME);
    let old_one = |state| AtStart { library: old.clone(), state };
    // To be made: unless making it failed the same way already.
    let to_make = |why: &str| match stamp_of(&old).and_then(|stamp| failed_before(dir, &stamp, app)) {
        Some(before) => old_one(State::Kept {
            why: format!(
                "songs.db couldn't be made from it ({before}), so it's tried again once library.db or the app changes, \
                 or in a week"
            ),
        }),
        None => old_one(State::Old { why: why.to_owned() }),
    };
    // Made last time and in step: the app's own from now on, once that's written in it. Until then (another program
    // holding it), library.db as before: the app never writes to a songs.db that might yet be made again.
    let switch = |made_from: Option<&str>| match adopt(&songs, made_from) {
        Ok(()) => {
            let _ = fs::remove_file(dir.join(FAILED_NOTE));
            AtStart { library: songs.clone(), state: State::Switched }
        }
        Err(why) => old_one(State::Kept {
            why: format!(
                "songs.db couldn't be noted as the app's own library just now ({why}); tried again next start"
            ),
        }),
    };
    match look(&songs) {
        Ok(Found::Songs { own: Some(_), .. }) => AtStart { library: songs, state: State::Own },
        Ok(Found::Songs { own: None, made_from }) => match stamp_of(&old) {
            Some(now) if made_from.as_deref() == Some(now.as_str()) => switch(Some(&now)),
            // Gone (archived, or left in the old folder when the data folder moved): songs.db is all there is.
            None if !old.exists() => switch(None),
            None => old_one(State::Kept { why: "library.db couldn't be looked at".to_owned() }),
            Some(_) => to_make("library.db has changed since songs.db was made"),
        },
        Ok(Found::Nothing) if old.is_file() => to_make("there's no songs.db yet"),
        Ok(Found::Nothing) => old_one(State::Kept { why: "there's no library yet".to_owned() }),
        Ok(Found::Newer(version)) => old_one(State::Kept {
            why: format!("songs.db was made by a newer version of the app (layout {version}), so it's left as it is"),
        }),
        Err(why) => old_one(State::Kept { why: format!("songs.db couldn't be read ({why}), so it's left as it is") }),
    }
}

/// The note, in the data folder, of a `songs.db` that couldn't be made ([`note_failed`]).
pub const FAILED_NOTE: &str = "songs-db-failed.txt";

/// After how many tries in a row that fail with the same `library.db` and the same build it waits ([`RETRY_AFTER`]):
/// one failure may be a passing thing (another program holding a file), two the same is what happens every time.
const TRIES: u32 = 2;
/// How long it waits then before trying again: each try reads the whole of `library.db`.
const RETRY_AFTER: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Notes in the data folder `dir` that `songs.db` couldn't be made from its `library.db` by the build `app`, because
/// `why`, so that a failure that comes back every time doesn't read the whole of `library.db` at every start
/// ([`at_start`]). A note of one made later, or of a `library.db` or build since changed, has no say; it's removed
/// once `songs.db` is made. Not for a failure that's sure to pass (no room on the drive). Whether the next starts wait
/// now, rather than try again.
pub fn note_failed(dir: &Path, app: &str, why: &str) -> bool {
    let Some(stamp) = stamp_of(&dir.join(OLD_FILE_NAME)) else { return false };
    let tries = read_note(dir).filter(|n| n.library == stamp && n.app == app).map_or(0, |n| n.tries).saturating_add(1);
    let why: String = why.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let text = format!("library: {stamp}\napp: {app}\ntries: {tries}\nlast: {}\nwhy: {why}\n", unix_now());
    fs::write(dir.join(FAILED_NOTE), text).is_ok() && tries >= TRIES
}

/// Why making `songs.db` from the `library.db` at `stamp` failed with the build `app` when it's to wait still
/// ([`TRIES`], [`RETRY_AFTER`]).
fn failed_before(dir: &Path, stamp: &str, app: &str) -> Option<String> {
    let note = read_note(dir)?;
    let waited = u64::try_from(unix_now().saturating_sub(note.last)).unwrap_or(0);
    (note.library == stamp && note.app == app && note.tries >= TRIES && waited < RETRY_AFTER.as_secs())
        .then_some(note.why)
}

/// What a [`FAILED_NOTE`] says.
struct Note {
    library: String,
    app: String,
    tries: u32,
    last: i64,
    why: String,
}

fn read_note(dir: &Path) -> Option<Note> {
    let text = fs::read_to_string(dir.join(FAILED_NOTE)).ok()?;
    let line = |key: &str| text.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix(": ")).map(str::to_owned);
    Some(Note {
        library: line("library")?,
        app: line("app")?,
        tries: line("tries")?.parse().ok()?,
        last: line("last")?.parse().ok()?,
        why: line("why").unwrap_or_default(),
    })
}

/// Since when (Unix seconds) the `songs.db` at `songs` has been the app's own library; `None` while it isn't (or isn't
/// there, or can't be read).
pub fn own_since(songs: &Path) -> Option<i64> {
    match look(songs) {
        Ok(Found::Songs { own, .. }) => own,
        _ => None,
    }
}

/// What's at `songs.db`'s place.
enum Found {
    Nothing,
    /// A `songs.db` of this layout: the [`stamp_of`] of the `library.db` it was made from, and since when it's been the
    /// app's own library.
    Songs {
        made_from: Option<String>,
        own: Option<i64>,
    },
    /// One of a newer layout.
    Newer(i32),
}

fn look(songs: &Path) -> Result<Found, String> {
    if !songs.exists() {
        return Ok(Found::Nothing);
    }
    let found = (|| -> rusqlite::Result<Result<Found, String>> {
        let conn = open_read_only(songs)?;
        let id: i32 = conn.query_row("PRAGMA application_id", [], |r| r.get(0))?;
        if id != APPLICATION_ID {
            return Ok(Err("it isn't the app's song library".to_owned()));
        }
        let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version > VERSION {
            return Ok(Ok(Found::Newer(version)));
        }
        if version < VERSION {
            return Ok(Err(format!("it's of an unknown layout ({version})")));
        }
        let info = |key: &str| -> rusqlite::Result<Option<Value>> {
            conn.query_row("SELECT value FROM songs_info WHERE key = ?1", [key], |r| r.get(0)).optional()
        };
        let made_from = match info("made_from")? {
            Some(Value::Text(stamp)) => Some(stamp),
            _ => None,
        };
        let own = match info("own_since")? {
            Some(Value::Integer(since)) => Some(since),
            _ => None,
        };
        Ok(Ok(Found::Songs { made_from, own }))
    })();
    found.map_err(|e| e.to_string())?
}

fn open_read_only(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    conn.busy_timeout(START_WAIT)?;
    Ok(conn)
}

/// Notes in `songs` that it's the app's own library from now on (kept as it was, if it was already), when it's still
/// the one made from the `library.db` at `made_from` (`None`: `library.db` has gone, so it's taken as it is). Looked at
/// again inside the write: another copy of the app may have made it again meanwhile.
fn adopt(songs: &Path, made_from: Option<&str>) -> Result<(), String> {
    let noted = (|| -> rusqlite::Result<Result<(), String>> {
        let conn =
            Connection::open_with_flags(songs, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
        conn.busy_timeout(START_WAIT)?;
        let tx = rusqlite::Transaction::new_unchecked(&conn, rusqlite::TransactionBehavior::Immediate)?;
        let now: Option<Value> =
            tx.query_row("SELECT value FROM songs_info WHERE key = 'made_from'", [], |r| r.get(0)).optional()?;
        if let Some(wanted) = made_from
            && !matches!(&now, Some(Value::Text(stamp)) if stamp == wanted)
        {
            return Ok(Err("it was made again meanwhile".to_owned()));
        }
        tx.execute("INSERT OR IGNORE INTO songs_info (key, value) VALUES ('own_since', ?1)", [unix_now()])?;
        tx.commit()?;
        Ok(Ok(()))
    })();
    noted.map_err(|e| e.to_string())?
}

/// What making `songs.db` did ([`make`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Made {
    /// How many saves it holds.
    pub saves: u64,
    /// How many rows were copied, in all the tables.
    pub rows: u64,
    /// Its size, and that of the `library.db` (with its `-wal`) it was made from.
    pub bytes: u64,
    pub from_bytes: u64,
}

/// Makes `songs_db` from the C# app's library at `library_db`, which is only read, as one snapshot: in a file of its
/// own, checked, then renamed (see the module notes). `progress` hears how far it's got (0 to 1) now and then. A
/// `songs.db` that's the app's own library already (or one a newer version of the app made) is never replaced.
///
/// Reads the whole of `library.db`, so call it in the background, at low priority.
pub fn make(library_db: &Path, songs_db: &Path, progress: &mut dyn FnMut(f32)) -> Result<Made, LibraryError> {
    let failed = |why: String| LibraryError::Write { path: songs_db.to_owned(), why };
    refuse_to_replace(songs_db).map_err(failed)?;
    // Before reading: a write during the read makes `library.db` differ from this, so it's made again next time.
    let stamp = stamp_of(library_db).ok_or_else(|| failed("library.db isn't there".to_owned()))?;
    let old = Library::open_read_only(library_db)?;
    for table in &old.tables {
        if !table.starts_with("sqlite_")
            && table != "projects"
            && !WHOLE.contains(&table.as_str())
            && !LEFT_OUT.contains(&table.as_str())
        {
            return Err(failed(format!("library.db has a table the app doesn't know ({table}), so nothing was made")));
        }
    }
    // One out of step goes first, with SQLite's files beside it: it's never read (the app reads library.db until
    // songs.db is its own), and its room is wanted for the new one.
    remove_with_sides(songs_db).map_err(failed)?;
    let new = new_path(songs_db);
    let mut part = Part::new(new.clone());
    let made = (|| -> Result<(u64, u64), String> {
        let conn = Connection::open(&new).map_err(|e| e.to_string())?;
        // Only a whole file, checked, is ever put in place, so it needs no journal while it's made.
        conn.execute_batch("PRAGMA journal_mode = OFF; PRAGMA synchronous = OFF; PRAGMA foreign_keys = OFF;")
            .map_err(|e| e.to_string())?;
        conn.execute_batch(TABLES).map_err(|e| e.to_string())?;
        let read = fill(&old, &conn, &stamp, progress)?;
        drop(old);
        let back = read_back(&conn, &read.tables)?;
        if back != read.hashes {
            let which: Vec<&str> =
                read.hashes.iter().zip(&back).filter(|(a, b)| a != b).map(|(a, _)| a.0.as_str()).collect();
            return Err(format!("what was written didn't read back the same ({})", which.join(", ")));
        }
        let check: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0)).map_err(|e| e.to_string())?;
        if check != "ok" {
            return Err(format!("SQLite's check of it failed ({check})"));
        }
        conn.execute_batch(&format!(
            "PRAGMA application_id = {APPLICATION_ID}; PRAGMA user_version = {VERSION}; PRAGMA journal_mode = WAL;"
        ))
        .map_err(|e| e.to_string())?;
        // Closed before it's synced and moved: Windows can't move a file that's open.
        conn.close().map_err(|(_, e)| e.to_string())?;
        // Windows only flushes a file for a program that may write to it.
        fs::OpenOptions::new().write(true).open(&new).and_then(|f| f.sync_all()).map_err(|e| e.to_string())?;
        Ok((read.saves, read.rows))
    })();
    let (saves, rows) = made.map_err(failed)?;
    // Looked at again: another copy of the app may have made one meanwhile, and switched to it.
    refuse_to_replace(songs_db).map_err(failed)?;
    // Whatever SQLite kept beside a songs.db made meanwhile would be played onto the new one: it goes first.
    remove_with_sides(songs_db).map_err(failed)?;
    fs::rename(&new, songs_db).map_err(|e| failed(format!("it couldn't be put in place ({e})")))?;
    part.kept();
    let _ = fs::remove_file(songs_db.with_file_name(FAILED_NOTE));
    let size = |p: &Path| fs::metadata(p).map_or(0, |m| m.len());
    Ok(Made {
        saves,
        rows,
        bytes: size(songs_db),
        from_bytes: size(library_db) + size(&with_ending(library_db, "-wal")),
    })
}

/// Why `songs_db` mustn't be replaced, if it mustn't: it's the app's own library, or a newer app's.
fn refuse_to_replace(songs_db: &Path) -> Result<(), String> {
    match look(songs_db) {
        Ok(Found::Songs { own: Some(_), .. }) => Err("songs.db is the app's own library already".to_owned()),
        Ok(Found::Newer(version)) => Err(format!("songs.db was made by a newer version of the app (layout {version})")),
        // Not the app's own yet, or none: made (again).
        Ok(_) => Ok(()),
        // Can't be read: it may be the app's own, damaged. Left for Solomon, never replaced.
        Err(why) => Err(format!("the songs.db there couldn't be read ({why})")),
    }
}

/// Removes the `songs.db` at `songs_db` that's to be made again, and SQLite's files beside it.
fn remove_with_sides(songs_db: &Path) -> Result<(), String> {
    for end in ["", "-wal", "-shm", "-journal"] {
        match fs::remove_file(with_ending(songs_db, end)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("the old songs.db{end} couldn't be removed ({e})")),
        }
    }
    Ok(())
}

/// Where a new `songs.db` is made: `songs.db.new-<process>`, which the start-up sweep clears if a crash leaves it.
fn new_path(songs_db: &Path) -> PathBuf {
    with_ending(songs_db, &format!(".new-{}", std::process::id()))
}

/// A new `songs.db` being made: removed (with SQLite's files beside it) unless it's [`Part::kept`].
struct Part {
    path: PathBuf,
    keep: bool,
}

impl Part {
    fn new(path: PathBuf) -> Part {
        let part = Part { path, keep: false };
        part.remove();
        part
    }

    fn kept(&mut self) {
        self.keep = true;
    }

    fn remove(&self) {
        for end in ["", "-journal", "-wal", "-shm"] {
            let _ = fs::remove_file(with_ending(&self.path, end));
        }
    }
}

impl Drop for Part {
    fn drop(&mut self) {
        if !self.keep {
            self.remove();
        }
    }
}

/// What was read out of `library.db`: how many saves and rows, the tables it had with the columns copied (but for
/// `info`), and each one's SHA-256 (the track details apart), in the order [`read_back`] gives them.
struct Copied {
    saves: u64,
    rows: u64,
    tables: Vec<(String, Vec<String>)>,
    hashes: Vec<(String, [u8; 32])>,
}

/// The name `project_info`'s check goes by: the track details, as `library.db` holds them.
const INFO: &str = "projects.info";

/// Copies `old` into the new file `new` in one transaction, reading `old` as one snapshot.
fn fill(old: &Library, new: &Connection, stamp: &str, progress: &mut dyn FnMut(f32)) -> Result<Copied, String> {
    let e = |e: rusqlite::Error| e.to_string();
    // One read for all of it: a write meanwhile (the app's own settings writers) can't mix two states.
    let snapshot = old.conn.unchecked_transaction().map_err(e)?;
    let tables: Vec<&str> = std::iter::once("projects").chain(WHOLE).filter(|t| old.has(t)).collect();
    let mut total = 0u64;
    for table in &tables {
        total +=
            old.conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get::<_, i64>(0)).map_err(e)?.max(0)
                as u64;
    }
    let mut done = 0u64;
    let mut step = |done: &mut u64| {
        *done += 1;
        if (*done).is_multiple_of(2_000) {
            progress(*done as f32 / total.max(1) as f32);
        }
    };
    new.execute_batch("BEGIN").map_err(e)?;
    let mut insert_info = new.prepare("INSERT INTO project_info (project_id, info) VALUES (?1, ?2)").map_err(e)?;
    let mut hashes = Vec::new();
    let mut copied = Vec::new();
    let mut saves = 0u64;
    for table in &tables {
        let columns = columns_to_copy(old, new, table)?;
        let keep_rowid = !has_rowid_column(new, table).map_err(e)?;
        let mut rows = Sha256::new();
        let mut info = Sha256::new();
        // `info` is read last, so the other columns sit at the same places in the row whichever library it is.
        let wanted: Vec<&str> = columns.iter().map(String::as_str).filter(|c| *c != "info").collect();
        let has_info = *table == "projects" && columns.iter().any(|c| c == "info");
        let mut select: Vec<String> = vec!["rowid".to_owned()];
        select.extend(wanted.iter().map(|c| quote(c)));
        if has_info {
            select.push("info".to_owned());
        }
        let mut read =
            old.conn.prepare(&format!("SELECT {} FROM {table} ORDER BY rowid", select.join(", "))).map_err(e)?;
        let mut into: Vec<String> = Vec::new();
        if keep_rowid {
            into.push("rowid".to_owned());
        }
        into.extend(wanted.iter().map(|c| quote(c)));
        let mut insert = new
            .prepare(&format!(
                "INSERT INTO {table} ({}) VALUES ({})",
                into.join(", "),
                vec!["?"; into.len()].join(", ")
            ))
            .map_err(e)?;
        let mut found = read.query([]).map_err(e)?;
        while let Some(row) = found.next().map_err(e)? {
            let rowid: i64 = row.get(0).map_err(e)?;
            let mut values: Vec<ValueRef<'_>> = Vec::with_capacity(into.len());
            if keep_rowid {
                values.push(ValueRef::Integer(rowid));
            }
            hash_value(&mut rows, ValueRef::Integer(rowid));
            for i in 0..wanted.len() {
                let value = row.get_ref(i + 1).map_err(e)?;
                hash_value(&mut rows, value);
                values.push(value);
            }
            insert
                .execute(params_from_iter(values.iter().map(|v| rusqlite::types::ToSqlOutput::Borrowed(*v))))
                .map_err(e)?;
            // The track details as the app reads them: text, or none (anything else reads as none there too).
            if has_info && let ValueRef::Text(text) = row.get_ref(wanted.len() + 1).map_err(e)? {
                hash_value(&mut info, ValueRef::Integer(rowid));
                hash_value(&mut info, ValueRef::Text(text));
                insert_info.execute(rusqlite::params![rowid, deflate(text)?]).map_err(e)?;
            }
            if *table == "projects" {
                saves += 1;
            }
            step(&mut done);
        }
        hashes.push(((*table).to_owned(), rows.finalize().into()));
        if *table == "projects" {
            hashes.push((INFO.to_owned(), info.finalize().into()));
        }
        copied.push(((*table).to_owned(), wanted.iter().map(|c| (*c).to_owned()).collect()));
    }
    drop(insert_info);
    drop(snapshot);
    new.execute_batch(INDEXES).map_err(e)?;
    let mut note = new.prepare("INSERT INTO songs_info (key, value) VALUES (?1, ?2)").map_err(e)?;
    note.execute(rusqlite::params!["made_from", stamp]).map_err(e)?;
    note.execute(rusqlite::params!["made_at", unix_now()]).map_err(e)?;
    drop(note);
    new.execute_batch("COMMIT").map_err(e)?;
    progress(1.0);
    Ok(Copied { saves, rows: done, tables: copied, hashes })
}

/// The columns of `table` to copy from `old`: all it has, each of which the new file must have too (`projects`' `info`
/// goes to `project_info`). A column the app doesn't know stops the copy rather than be left behind.
fn columns_to_copy(old: &Library, new: &Connection, table: &str) -> Result<Vec<String>, String> {
    let names = |conn: &Connection| -> rusqlite::Result<Vec<String>> {
        let mut st = conn.prepare("SELECT name FROM pragma_table_info(?1)")?;
        st.query_map([table], |r| r.get(0))?.collect()
    };
    let have: HashSet<String> = names(new).map_err(|e| e.to_string())?.into_iter().collect();
    let columns = names(&old.conn).map_err(|e| e.to_string())?;
    if let Some(unknown) = columns.iter().find(|c| !have.contains(*c) && !(table == "projects" && *c == "info")) {
        return Err(format!("library.db's {table} has a column the app doesn't know ({unknown}), so nothing was made"));
    }
    Ok(columns)
}

/// Whether `table` has a column that is its row id (an `INTEGER PRIMARY KEY`), which then keeps it.
fn has_rowid_column(conn: &Connection, table: &str) -> rusqlite::Result<bool> {
    let keys: Vec<String> = {
        let mut st = conn.prepare("SELECT type FROM pragma_table_info(?1) WHERE pk > 0")?;
        st.query_map([table], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
    };
    Ok(keys.len() == 1 && keys[0].eq_ignore_ascii_case("INTEGER"))
}

/// Reads the new file back as [`fill`] read `library.db` (the tables it had, the columns it copied, in the same
/// order): each table's SHA-256, the track details uncompressed again.
fn read_back(conn: &Connection, tables: &[(String, Vec<String>)]) -> Result<Vec<(String, [u8; 32])>, String> {
    let e = |e: rusqlite::Error| e.to_string();
    let mut hashes = Vec::new();
    for (table, columns) in tables {
        let mut rows = Sha256::new();
        let select: Vec<String> = std::iter::once("rowid".to_owned()).chain(columns.iter().map(|c| quote(c))).collect();
        let mut st = conn.prepare(&format!("SELECT {} FROM {table} ORDER BY rowid", select.join(", "))).map_err(e)?;
        let mut found = st.query([]).map_err(e)?;
        while let Some(row) = found.next().map_err(e)? {
            for i in 0..select.len() {
                hash_value(&mut rows, row.get_ref(i).map_err(e)?);
            }
        }
        hashes.push((table.clone(), rows.finalize().into()));
        if table == "projects" {
            let mut info = Sha256::new();
            let mut st = conn.prepare("SELECT project_id, info FROM project_info ORDER BY project_id").map_err(e)?;
            let mut found = st.query([]).map_err(e)?;
            while let Some(row) = found.next().map_err(e)? {
                hash_value(&mut info, row.get_ref(0).map_err(e)?);
                let text = inflate(row.get_ref(1).map_err(e)?.as_blob().map_err(|e| e.to_string())?)?;
                hash_value(&mut info, ValueRef::Text(&text));
            }
            hashes.push((INFO.to_owned(), info.finalize().into()));
        }
    }
    Ok(hashes)
}

/// Adds one value to `hash`, its kind first, so no two different rows hash the same way.
fn hash_value(hash: &mut Sha256, value: ValueRef<'_>) {
    match value {
        ValueRef::Null => hash.update([0]),
        ValueRef::Integer(i) => {
            hash.update([1]);
            hash.update(i.to_le_bytes());
        }
        ValueRef::Real(f) => {
            hash.update([2]);
            hash.update(f.to_bits().to_le_bytes());
        }
        ValueRef::Text(t) => {
            hash.update([3]);
            hash.update((t.len() as u64).to_le_bytes());
            hash.update(t);
        }
        ValueRef::Blob(b) => {
            hash.update([4]);
            hash.update((b.len() as u64).to_le_bytes());
            hash.update(b);
        }
    }
}

/// A column's name, quoted for SQL.
fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// A save's track details, compressed as `project_info` keeps them.
pub(crate) fn deflate(text: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = DeflateEncoder::new(Vec::with_capacity(text.len() / 4), Compression::default());
    out.write_all(text).map_err(|e| e.to_string())?;
    out.finish().map_err(|e| e.to_string())
}

/// A save's track details from `project_info`, uncompressed again (the text's bytes).
pub(crate) fn inflate(blob: &[u8]) -> Result<Vec<u8>, String> {
    let mut text = Vec::with_capacity(blob.len() * 4);
    DeflateDecoder::new(blob).read_to_end(&mut text).map_err(|e| e.to_string())?;
    Ok(text)
}

/// The time now, in Unix seconds.
fn unix_now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).ok().and_then(|d| i64::try_from(d.as_secs()).ok()).unwrap_or(0)
}
