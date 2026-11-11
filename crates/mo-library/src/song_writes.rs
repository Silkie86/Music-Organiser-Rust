//! The app keeping what it reads of your saves in its own song library, `songs.db` (step 3 of the new databases): a
//! save the folder watch or the start's catch-up read ([`Put`]), and one whose file has gone ([`Missing`]). Only a
//! `songs.db` that's the app's own ([`crate::songs_db::own_since`]) is ever written here; never `library.db`, and never
//! a `songs.db` that may yet be made again. No save's row is ever removed: one the scan now leaves out (an autosave, or
//! in a folder it skips) stays, with what you gave it, and is only left off the list ([`crate::scan::is_ignored`]).
//!
//! Each batch is one SQLite transaction, IMMEDIATE so nothing else writes meanwhile: the rows are written, read back
//! and checked, and only then kept. If anything goes wrong nothing is changed, and the caller tries again.
//!
//! A save keeps its id: a row with the save's path is the save (whatever id the app gave it), else the row with its
//! id while it still has the path the library knew the save by ([`Put::known_as`]). A new one takes the id the app
//! gave it while that's free, or the next free one, and [`Written::ids`] says so.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

use chrono::{Local, NaiveDateTime, TimeZone};
use mo_formats::plugins::name_key;
use mo_formats::{Confidence, PluginKind};
use rusqlite::{Connection, ErrorCode, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::song_fields::same_tag;
use crate::{LibraryError, SaveDetail};

/// A save read (for the first time, again, or in a new place), with what its reader found.
#[derive(Clone, Debug)]
pub struct Put {
    pub detail: SaveDetail,
    /// The path the library knows the save by, as it was last kept (the one it moved from, for a save in a new place);
    /// `None` when the app has never kept it (its id is the one the app gave it, which may be taken meanwhile). The row
    /// with its id is this save only while it still has that path: otherwise it's another save's by now (another copy
    /// of the app kept one under that id), and this one is kept as new (review finding A-fp1).
    pub known_as: Option<String>,
}

/// A save the library has, whose file has gone (moved somewhere the app doesn't look, or deleted): it stays, marked
/// missing. Only the row with both its id and its path is marked, so a row that's another save's by now never is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Missing {
    pub id: i64,
    pub path: String,
}

/// One batch of changes, kept together or not at all.
#[derive(Clone, Debug, Default)]
pub struct Batch {
    pub put: Vec<Put>,
    pub missing: Vec<Missing>,
    /// The saves [`to_mend`] gave have been read again (they're in this batch or one kept before it): noted, so
    /// they aren't read again at the next start.
    pub mended: bool,
    /// With `mended`: those of them that couldn't be read this time (in a folder that couldn't be read, a drive
    /// that's away), noted with it so [`to_mend`] gives them, and only them, at the next start.
    pub owed: Vec<i64>,
}

impl Batch {
    pub fn is_empty(&self) -> bool {
        self.put.is_empty() && self.missing.is_empty() && !self.mended
    }
}

/// The note in `songs_info` that the saves [`to_mend`] gives were read again, with when.
const MENDED_KEY: &str = "muted_marks_mended";

/// The note in `songs_info` of the saves still to be read again when the rest were ([`Batch::owed`]), by id, with a
/// comma between each.
const OWED_KEY: &str = "muted_marks_owed";

/// When `library.db` has gone: how long before the library became the app's own a save the app kept since may have
/// last changed (the C# app kept the library up to date until about then).
const MEND_SINCE: i64 = 30 * 24 * 60 * 60;

/// The FL Studio saves to read again once so their "muted" sample marks come back, by id: until this version the app
/// kept a sample whose notes are all muted as one that plays (review finding A-1). Only the rows the app wrote itself
/// were kept so: those that differ from the C# app's `library.db` beside it (as it was when the C# app stopped), or
/// aren't in it. A row differs by its file (path, size and time), or, for one read whole, by its samples (how many,
/// and how many muted): the app may have read a save again whose file hadn't changed, one marked missing that came
/// back, say (review findings G-1 and X1-4). When `library.db` has gone (or can't be read), those of the saves changed
/// since a month before the library became the app's own. Saves whose file is missing are left: one that comes back
/// is read again anyway. Once they've been read again ([`Batch::mended`]), only those that couldn't be then
/// ([`Batch::owed`]). `None` when there's nothing to do: they've all been read again already, or the library at
/// `songs_db` isn't the app's own.
///
/// Of `library.db`, only what comes before each save's details is read, and the samples: the details are tens of KB a
/// save, most of the file (review finding G-2).
pub fn to_mend(songs_db: &Path) -> Result<Option<Vec<i64>>, LibraryError> {
    let Some(own_since) = crate::songs_db::own_since(songs_db) else {
        return Ok(None);
    };
    let conn = open_read_only(songs_db)?;
    let noted: Option<rusqlite::types::Value> =
        conn.query_row("SELECT value FROM songs_info WHERE key = ?1", [MENDED_KEY], |r| r.get(0)).optional()?;
    if noted.is_some() {
        let owed: Option<String> =
            conn.query_row("SELECT value FROM songs_info WHERE key = ?1", [OWED_KEY], |r| r.get(0)).optional()?;
        let owed: Vec<i64> = owed.iter().flat_map(|o| o.split(',')).filter_map(|id| id.trim().parse().ok()).collect();
        if owed.is_empty() {
            return Ok(None);
        }
        // Those still FL Studio saves whose file isn't missing, as the first time.
        let mut st = conn
            .prepare("SELECT 1 FROM projects WHERE id = ?1 AND daw = 'FL Studio' AND COALESCE(file_missing, 0) = 0")?;
        let mut still = Vec::with_capacity(owed.len());
        for id in owed {
            if st.exists([id])? {
                still.push(id);
            }
        }
        return Ok(Some(still));
    }
    let library_db = songs_db.with_file_name(crate::songs_db::OLD_FILE_NAME);
    let before = library_db.is_file().then(|| before(&library_db).ok()).flatten();
    // Muted marks are compared only where the C# app kept them.
    let samples_now = match &before {
        Some(b) => samples_of(&conn, b.muted)?,
        None => HashMap::new(),
    };
    let mut st = conn.prepare(
        "SELECT id, path, size, mtime, error IS NULL FROM projects
         WHERE daw = 'FL Studio' AND COALESCE(file_missing, 0) = 0",
    )?;
    let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, fl_row(r)?, r.get::<_, bool>(4)?)))?;
    let mut ids = Vec::new();
    for row in rows {
        let (id, row, whole) = row?;
        let wanted = match &before {
            Some(b) => b.rows.get(&id) != Some(&row) || (whole && b.samples.get(&id) != samples_now.get(&id)),
            None => row.mtime.is_some_and(|t| t >= own_since - MEND_SINCE),
        };
        if wanted {
            ids.push(id);
        }
    }
    Ok(Some(ids))
}

/// Whether the library at `songs_db` notes that the saves [`to_mend`] gave were read again: from then on `to_mend`
/// never looks at the C# app's `library.db` beside it, so that may be archived (`mo_app::old_databases`). `false`
/// when it can't be told (no library there, one that can't be read just now).
pub fn mend_noted(songs_db: &Path) -> bool {
    open_read_only(songs_db)
        .and_then(|conn| {
            Ok(conn.query_row("SELECT COUNT(*) > 0 FROM songs_info WHERE key = ?1", [MENDED_KEY], |r| r.get(0))?)
        })
        .unwrap_or(false)
}

/// What [`to_mend`] compares of a save's row: its file, as a read sets it.
#[derive(Debug, PartialEq, Eq)]
struct FlRow {
    path: String,
    size: Option<i64>,
    mtime: Option<i64>,
}

/// An [`FlRow`] from the id, path, size and time columns of a query.
fn fl_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<FlRow> {
    Ok(FlRow { path: r.get(1)?, size: r.get(2)?, mtime: r.get(3)? })
}

/// What [`to_mend`] compares of the C# app's `library.db`.
struct Before {
    /// The FL Studio saves' rows, by id.
    rows: HashMap<i64, FlRow>,
    /// Each save's samples, by id ([`samples_of`]).
    samples: HashMap<i64, (i64, i64)>,
    /// Whether it kept which samples are muted (an older C# app's didn't).
    muted: bool,
}

/// The C# app's `library.db` at `library_db`, as [`to_mend`] compares it: only columns before each save's details,
/// which hold most of the file (an older C# app's has no `samples` table, or no muted marks in it).
fn before(library_db: &Path) -> Result<Before, LibraryError> {
    let conn = open_read_only(library_db)?;
    let mut st = conn.prepare("SELECT id, path, size, mtime FROM projects WHERE daw = 'FL Studio'")?;
    let rows = st.query_map([], |r| Ok((r.get(0)?, fl_row(r)?)))?.collect::<rusqlite::Result<_>>()?;
    let mut st = conn.prepare("SELECT name FROM pragma_table_info('samples')")?;
    let columns: Vec<String> = st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    let muted = columns.iter().any(|c| c == "unplayed");
    let samples = if columns.is_empty() { HashMap::new() } else { samples_of(&conn, muted)? };
    Ok(Before { rows, samples, muted })
}

/// How many samples each save in the library on `conn` has, by id, and how many of them are muted (counted only with
/// `muted`: 0 otherwise). A save with none isn't there, nor a sample of no save.
fn samples_of(conn: &Connection, muted: bool) -> rusqlite::Result<HashMap<i64, (i64, i64)>> {
    let muted = if muted { "COALESCE(SUM(unplayed = 2), 0)" } else { "0" };
    let mut st = conn.prepare(&format!(
        "SELECT project_id, COUNT(*), {muted} FROM samples WHERE project_id IS NOT NULL GROUP BY project_id"
    ))?;
    st.query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?.collect()
}

fn open_read_only(path: &Path) -> Result<Connection, LibraryError> {
    let open_error = |e: rusqlite::Error| LibraryError::Open { path: path.to_owned(), why: e.to_string() };
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(path, flags).map_err(open_error)?;
    conn.busy_timeout(WAIT).map_err(open_error)?;
    crate::db::sort_in_memory(&conn).map_err(open_error)?;
    Ok(conn)
}

/// What a batch did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Written {
    pub put: usize,
    /// How many saves were marked missing (one the library no longer has as it was isn't).
    pub missing: usize,
    /// Saves kept under another id than the app gave them, as (given, kept): the file was in the library already
    /// under another, or the id was taken meanwhile (another copy of the app).
    pub ids: Vec<(i64, i64)>,
    /// The note that the saves [`to_mend`] gave were read again was kept ([`Batch::mended`]).
    pub mended: bool,
}

/// How long a batch waits for another copy of the app to finish its own: this runs in the background.
const WAIT: Duration = Duration::from_secs(5);

/// An SQLite error writing `songs_db`, as the library's: busy when another write holds it, else that it wasn't
/// changed and why.
pub(crate) fn write_failed(songs_db: &Path, e: rusqlite::Error) -> LibraryError {
    match e.sqlite_error_code() {
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked) => LibraryError::Busy { path: songs_db.to_owned() },
        _ => LibraryError::Write { path: songs_db.to_owned(), why: e.to_string() },
    }
}

fn not_own(songs_db: &Path) -> LibraryError {
    let why = "it isn't the app's own song library (yet), so it isn't written".to_owned();
    LibraryError::Write { path: songs_db.to_owned(), why }
}

/// Opens the app's own `songs.db` at `songs_db` to be written, waiting [`WAIT`] for another write: never one that
/// isn't the app's song library, or is of another layout. Whether it's the app's *own* is looked at once the write
/// has begun ([`begin_own`]).
pub(crate) fn open_to_write(songs_db: &Path) -> Result<Connection, LibraryError> {
    let sql = |e| write_failed(songs_db, e);
    if !songs_db.is_file() {
        return Err(LibraryError::Write { path: songs_db.to_owned(), why: "it isn't there".to_owned() });
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(songs_db, flags).map_err(sql)?;
    conn.busy_timeout(WAIT).map_err(sql)?;
    conn.execute_batch("PRAGMA foreign_keys = ON;").map_err(sql)?;
    let id: i32 = conn.query_row("PRAGMA application_id", [], |r| r.get(0)).map_err(sql)?;
    let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).map_err(sql)?;
    if id != crate::songs_db::APPLICATION_ID || version != crate::songs_db::VERSION {
        return Err(not_own(songs_db));
    }
    Ok(conn)
}

/// Begins a write on `conn` ([`open_to_write`]), IMMEDIATE so nothing else writes meanwhile, and looks under that
/// lock that the library is the app's own: only that one is written.
pub(crate) fn begin_own<'c>(conn: &'c mut Connection, songs_db: &Path) -> Result<Transaction<'c>, LibraryError> {
    let sql = |e| write_failed(songs_db, e);
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(sql)?;
    let own: Option<rusqlite::types::Value> = tx
        .query_row("SELECT value FROM songs_info WHERE key = 'own_since'", [], |r| r.get(0))
        .optional()
        .map_err(sql)?;
    if own.is_none() {
        return Err(not_own(songs_db));
    }
    Ok(tx)
}

/// Keeps `batch` in the app's own `songs.db` at `songs_db`, all of it or none.
pub fn write(songs_db: &Path, batch: &Batch) -> Result<Written, LibraryError> {
    let write_error = |why: String| LibraryError::Write { path: songs_db.to_owned(), why };
    let sql = |e| write_failed(songs_db, e);
    let mut conn = open_to_write(songs_db)?;
    let tx = begin_own(&mut conn, songs_db)?;
    let mut written = Written::default();
    let mut kept = Vec::with_capacity(batch.put.len());
    for put in &batch.put {
        let (id, new) = put_one(&tx, put).map_err(sql)?;
        if id != put.detail.save.id {
            written.ids.push((put.detail.save.id, id));
        }
        kept.push((id, new, &put.detail));
        written.put += 1;
    }
    for m in &batch.missing {
        written.missing += tx
            .execute("UPDATE projects SET file_missing = 1 WHERE id = ?1 AND path = ?2", params![m.id, m.path])
            .map_err(sql)?;
    }
    if batch.mended {
        tx.execute(
            "INSERT OR REPLACE INTO songs_info (key, value) VALUES (?1, ?2)",
            params![MENDED_KEY, Local::now().timestamp()],
        )
        .map_err(sql)?;
        if batch.owed.is_empty() {
            tx.execute("DELETE FROM songs_info WHERE key = ?1", [OWED_KEY]).map_err(sql)?;
        } else {
            let owed: Vec<String> = batch.owed.iter().map(i64::to_string).collect();
            tx.execute(
                "INSERT OR REPLACE INTO songs_info (key, value) VALUES (?1, ?2)",
                params![OWED_KEY, owed.join(",")],
            )
            .map_err(sql)?;
        }
        written.mended = true;
    }
    check(&tx, &kept, batch).map_err(|e| match e {
        Checked::Sql(e) => sql(e),
        Checked::Differs(why) => write_error(format!("what was written didn't read back the same ({why})")),
    })?;
    tx.commit().map_err(sql)?;
    Ok(written)
}

/// Writes one save, and says which id it's kept under and whether its row is new.
///
/// A save the library has already keeps what you gave it there (its status, stars, heart, key, notes and tags:
/// [`YOUR_COLUMNS`]): a read sets what the file says, and what's yours is set by [`crate::song_fields`] alone. So a
/// read handed over with those as they were a moment ago (the folder watch reads while you click) can't undo a change
/// of yours. Only a new row takes them from the read (a new save of a song starts with the song's). A tag for
/// hardware the save uses is still put on as it's read, as the C# app did.
fn put_one(tx: &Transaction<'_>, put: &Put) -> rusqlite::Result<(i64, bool)> {
    let save = &put.detail.save;
    let at_path: Option<i64> =
        tx.query_row("SELECT id FROM projects WHERE path = ?1", [&save.path], |r| r.get(0)).optional()?;
    let id_taken = || -> rusqlite::Result<bool> {
        tx.query_row("SELECT EXISTS (SELECT 1 FROM projects WHERE id = ?1)", [save.id], |r| r.get(0))
    };
    // Moved: the row with its id, while that's still the save the library knew (by its path then).
    let known_row = || -> rusqlite::Result<bool> {
        let Some(known) = &put.known_as else { return Ok(false) };
        tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM projects WHERE id = ?1 AND path = ?2)",
            params![save.id, known],
            |r| r.get(0),
        )
    };
    let (id, new) = match at_path {
        Some(id) => {
            update(tx, id, put)?;
            (id, false)
        }
        None if known_row()? => {
            update(tx, save.id, put)?;
            (save.id, false)
        }
        None if !id_taken()? => (insert(tx, Some(save.id), put)?, true),
        None => (insert(tx, None, put)?, true),
    };
    for table in ["plugins", "samples"] {
        tx.execute(&format!("DELETE FROM {table} WHERE project_id = ?1"), [id])?;
    }
    if new {
        // Whatever an older library left under this id (tags of a save the C# app removed) isn't this save's.
        tx.execute("DELETE FROM tags WHERE project_id = ?1", [id])?;
    }
    let mut plugin = tx.prepare_cached(
        "INSERT INTO plugins (project_id, name, name_key, vendor, kind, format, path, installed)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )?;
    for p in &put.detail.plugins {
        plugin.execute(params![
            id,
            p.name,
            name_key(&p.name),
            p.vendor,
            kind_number(p.kind),
            p.format,
            p.path,
            p.installed
        ])?;
    }
    let mut sample =
        tx.prepare_cached("INSERT INTO samples (project_id, path, found, unplayed) VALUES (?1, ?2, ?3, ?4)")?;
    for s in &put.detail.samples {
        sample.execute(params![id, s.path, s.found, s.use_number()])?;
    }
    let mut tag = tx.prepare_cached("INSERT OR IGNORE INTO tags (project_id, tag) VALUES (?1, ?2)")?;
    for t in tags_to_add(save, new) {
        // Not beside the same tag in other letters (an older library's "Digitakt").
        if new || crate::song_fields::tags_like(tx, id, &same_tag(t))?.is_empty() {
            tag.execute(params![id, t])?;
        }
    }
    match info_json(&put.detail) {
        Some(json) => {
            let blob = crate::songs_db::deflate(json.as_bytes())
                .map_err(|why| rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(why))))?;
            tx.execute("INSERT OR REPLACE INTO project_info (project_id, info) VALUES (?1, ?2)", params![id, blob])?;
        }
        None => {
            tx.execute("DELETE FROM project_info WHERE project_id = ?1", [id])?;
        }
    }
    Ok((id, new))
}

/// The tags a read puts on a save's row: all it comes with for a new row, and for one the library has already only
/// those for the hardware it uses (the rest are yours: [`put_one`]).
fn tags_to_add(save: &crate::Save, new: bool) -> impl Iterator<Item = &String> {
    let hardware: Vec<String> = save.hardware.iter().map(|h| same_tag(h)).collect();
    save.tags.iter().filter(move |t| new || hardware.contains(&same_tag(t)))
}

/// The columns a read sets, in the order [`update`] and [`insert`] give them (`?2` on).
const READ_COLUMNS: &str = "path, name, daw, folder, size, mtime, tempo, version, title, confidence, can_midi, \
    can_presets, error, file_missing, hardware, status, rating, favourite, musical_key, notes, detected_key, created, \
    note_count, length_bars, track_count";

/// Those of [`READ_COLUMNS`] that are yours, not the file's: a read sets them only on a new row ([`put_one`]), and
/// [`crate::song_fields::Change`] has a change for each (as `mo_app::live` carries each over to a new save of a song).
pub(crate) const YOUR_COLUMNS: [&str; 5] = ["status", "rating", "favourite", "musical_key", "notes"];

/// Sets a save's row as read. When it was last opened is left as it is: that's the app's to note, not the file's.
/// What you gave it is left as it is too ([`YOUR_COLUMNS`]).
fn update(tx: &Transaction<'_>, id: i64, put: &Put) -> rusqlite::Result<()> {
    let set: Vec<String> = READ_COLUMNS
        .split(", ")
        .enumerate()
        .filter(|(_, c)| !YOUR_COLUMNS.contains(&c.trim()))
        .map(|(i, c)| format!("{} = ?{}", c.trim(), i + 2))
        .collect();
    let mut st = tx.prepare_cached(&format!("UPDATE projects SET {} WHERE id = ?1", set.join(", ")))?;
    st.execute(rusqlite::params_from_iter(std::iter::once(id.into()).chain(values(put))))?;
    Ok(())
}

/// Adds a save's row, under `id` (or the next free one), and says which.
fn insert(tx: &Transaction<'_>, id: Option<i64>, put: &Put) -> rusqlite::Result<i64> {
    let count = READ_COLUMNS.split(", ").count();
    let marks: Vec<String> = (2..count + 2).map(|i| format!("?{i}")).collect();
    let mut st = tx.prepare_cached(&format!(
        "INSERT INTO projects (id, {READ_COLUMNS}, last_opened) VALUES (?1, {}, ?{})",
        marks.join(", "),
        count + 2
    ))?;
    let last_opened = put.detail.save.last_opened.and_then(unix);
    let id_value = id.map_or(rusqlite::types::Value::Null, rusqlite::types::Value::Integer);
    st.execute(rusqlite::params_from_iter(
        std::iter::once(id_value).chain(values(put)).chain(std::iter::once(last_opened.into())),
    ))?;
    Ok(tx.last_insert_rowid())
}

/// The values of [`READ_COLUMNS`] for a save, as the library keeps them (the reader's [`crate::db`] the other way).
fn values(put: &Put) -> impl Iterator<Item = rusqlite::types::Value> {
    use rusqlite::types::Value;
    let d = &put.detail;
    let s = &d.save;
    let text = |t: &str| Value::Text(t.to_owned());
    let known = |n: i32| if n < 0 { Value::Null } else { Value::Integer(i64::from(n)) };
    let flag = |b: bool| Value::Integer(i64::from(b));
    vec![
        text(&s.path),
        text(&s.name),
        text(&s.daw),
        text(&s.folder),
        Value::Integer(s.size),
        Value::Integer(s.mtime),
        s.tempo.map_or(Value::Null, Value::Real),
        text(&s.version),
        text(&d.title),
        Value::Integer(confidence_number(s.confidence)),
        flag(d.can_extract_midi),
        flag(d.can_extract_presets),
        s.error.as_deref().map_or(Value::Null, text),
        flag(s.file_missing),
        text(&s.hardware.join(", ")),
        text(&s.status),
        Value::Integer(i64::from(s.rating)),
        flag(s.favourite),
        text(&s.musical_key),
        text(&s.notes),
        if s.detected_key.is_empty() { Value::Null } else { text(&s.detected_key) },
        s.created.and_then(unix).map_or(Value::Null, Value::Integer),
        known(s.note_count),
        known(s.length_bars),
        known(s.track_count),
    ]
    .into_iter()
}

/// What went wrong reading a batch back.
enum Checked {
    Sql(rusqlite::Error),
    Differs(String),
}

impl From<rusqlite::Error> for Checked {
    fn from(e: rusqlite::Error) -> Checked {
        Checked::Sql(e)
    }
}

/// Reads back what was written: each save's row and how many plugins, samples and tags it has; each save marked
/// missing, missing.
fn check(tx: &Transaction<'_>, kept: &[(i64, bool, &SaveDetail)], batch: &Batch) -> Result<(), Checked> {
    let mut row = tx.prepare(
        "SELECT path, size, mtime, file_missing,
             (SELECT COUNT(*) FROM plugins WHERE project_id = p.id),
             (SELECT COUNT(*) FROM samples WHERE project_id = p.id),
             (SELECT COUNT(*) FROM tags WHERE project_id = p.id)
         FROM projects p WHERE id = ?1",
    )?;
    for (id, new, d) in kept {
        let mut back: Option<(String, i64, i64, i64, i64, i64, i64)> = row
            .query_row([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)))
            .optional()?;
        // A new row has just the tags it came with. One the library had keeps its own, however many, with one for
        // each piece of hardware the save uses: those are looked for by name, and the count left out.
        let mut tags: Vec<&str> = d.save.tags.iter().map(String::as_str).collect();
        tags.sort_unstable();
        tags.dedup();
        let tags = if *new { tags.len() } else { 0 };
        if !*new {
            for hardware in tags_to_add(&d.save, false) {
                if crate::song_fields::tags_like(tx, *id, &same_tag(hardware))?.is_empty() {
                    return Err(Checked::Differs(format!("the save {} (its tag {hardware})", d.save.path)));
                }
            }
            if let Some(back) = back.as_mut() {
                back.6 = 0;
            }
        }
        let wanted = (
            d.save.path.clone(),
            d.save.size,
            d.save.mtime,
            i64::from(d.save.file_missing),
            d.plugins.len() as i64,
            d.samples.len() as i64,
            tags as i64,
        );
        if back.as_ref() != Some(&wanted) {
            return Err(Checked::Differs(format!("the save {}", d.save.path)));
        }
    }
    for m in &batch.missing {
        let back: Option<i64> = tx
            .query_row("SELECT file_missing FROM projects WHERE id = ?1 AND path = ?2", params![m.id, m.path], |r| {
                r.get(0)
            })
            .optional()?;
        if back.is_some_and(|missing| missing != 1) {
            return Err(Checked::Differs(format!("{} marked missing", m.path)));
        }
    }
    Ok(())
}

/// What the reader found, as the C# app kept it (and [`crate::db`] reads it): `{"Tracks": [...], "Details": {...},
/// "Warnings": [...]}`, the details in their order. None when it found nothing to say.
fn info_json(d: &SaveDetail) -> Option<String> {
    if d.tracks.is_empty() && d.details.is_empty() && d.warnings.is_empty() {
        return None;
    }
    let quote = |s: &str| serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_owned());
    let list = |items: &[String]| items.iter().map(|t| quote(t)).collect::<Vec<_>>().join(",");
    let mut json = String::with_capacity(64 + d.tracks.len() * 24);
    let _ = write!(json, "{{\"Tracks\":[{}],\"Details\":{{", list(&d.tracks));
    let pairs: Vec<String> = d.details.iter().map(|(k, v)| format!("{}:{}", quote(k), quote(v))).collect();
    let _ = write!(json, "{}}},\"Warnings\":[{}]}}", pairs.join(","), list(&d.warnings));
    Some(json)
}

/// The library's number for a plugin's kind (`db::plugin_kind` the other way).
fn kind_number(kind: PluginKind) -> i64 {
    match kind {
        PluginKind::Unknown => 0,
        PluginKind::Instrument => 1,
        PluginKind::Effect => 2,
        PluginKind::NoteEffect => 3,
    }
}

/// The library's number for how sure a reader was (`db::confidence` the other way).
fn confidence_number(c: Confidence) -> i64 {
    match c {
        Confidence::Full => 0,
        Confidence::Partial => 1,
        Confidence::Heuristic => 2,
        Confidence::Catalogue => 3,
    }
}

/// A local time as Unix seconds, as the library keeps times (the first of the two, in the hour the clocks go back).
fn unix(t: NaiveDateTime) -> Option<i64> {
    Local.from_local_datetime(&t).earliest().map(|t| t.timestamp())
}
