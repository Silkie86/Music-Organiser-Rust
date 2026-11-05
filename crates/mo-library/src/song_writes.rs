//! The app keeping what it reads of your saves in its own song library, `songs.db` (step 3 of the new databases): a
//! save the folder watch or the start's catch-up read ([`Put`]), one whose file has gone (marked missing), and one the
//! scan now leaves out (dropped, as the C# app's scan drops them). Only a `songs.db` that's the app's own
//! ([`crate::songs_db::own_since`]) is ever written here; never `library.db`, and never a `songs.db` that may yet be
//! made again.
//!
//! Each batch is one SQLite transaction, IMMEDIATE so nothing else writes meanwhile: the rows are written, read back
//! and checked, and only then kept. If anything goes wrong nothing is changed, and the caller tries the batch again
//! with the next one.
//!
//! A save keeps its id: a row with the save's path is the save (whatever id the app gave it), else the row with its
//! id when it isn't new. A new one takes the id the app gave it while that's free, or the next free one, and
//! [`Written::ids`] says so.

use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

use chrono::{Local, NaiveDateTime, TimeZone};
use mo_formats::plugins::name_key;
use mo_formats::{Confidence, PluginKind};
use rusqlite::{Connection, ErrorCode, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::{LibraryError, SaveDetail};

/// A save read (for the first time, again, or in a new place), with what its reader found.
#[derive(Clone, Debug)]
pub struct Put {
    pub detail: SaveDetail,
    /// The app has never kept it: its id is the one the app gave it, which may be taken meanwhile.
    pub new: bool,
}

/// One batch of changes, kept together or not at all.
#[derive(Clone, Debug, Default)]
pub struct Batch {
    pub put: Vec<Put>,
    /// Saves whose file has gone (moved somewhere the app doesn't look, or deleted): they stay, marked missing.
    pub missing: Vec<i64>,
    /// Saves the scan leaves out now (an autosave, or in a folder it skips): they go.
    pub dropped: Vec<i64>,
}

impl Batch {
    pub fn is_empty(&self) -> bool {
        self.put.is_empty() && self.missing.is_empty() && self.dropped.is_empty()
    }
}

/// What a batch did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Written {
    pub put: usize,
    pub missing: usize,
    pub dropped: usize,
    /// Saves kept under another id than the app gave them, as (given, kept): the file was in the library already
    /// under another, or the id was taken meanwhile (another copy of the app).
    pub ids: Vec<(i64, i64)>,
}

/// How long a batch waits for another copy of the app to finish its own: this runs in the background.
const WAIT: Duration = Duration::from_secs(5);

/// Keeps `batch` in the app's own `songs.db` at `songs_db`, all of it or none.
pub fn write(songs_db: &Path, batch: &Batch) -> Result<Written, LibraryError> {
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
    conn.execute_batch("PRAGMA foreign_keys = ON;").map_err(sql)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(sql)?;
    // Looked at again under the write lock: only the app's own library is written.
    let own: Option<rusqlite::types::Value> = tx
        .query_row("SELECT value FROM songs_info WHERE key = 'own_since'", [], |r| r.get(0))
        .optional()
        .map_err(sql)?;
    if own.is_none() {
        return Err(write_error("it isn't the app's own song library, so it isn't written".to_owned()));
    }
    let mut written = Written::default();
    let mut kept = Vec::with_capacity(batch.put.len());
    for put in &batch.put {
        let id = put_one(&tx, put).map_err(sql)?;
        if id != put.detail.save.id {
            written.ids.push((put.detail.save.id, id));
        }
        kept.push((id, &put.detail));
        written.put += 1;
    }
    for id in &batch.missing {
        written.missing += tx.execute("UPDATE projects SET file_missing = 1 WHERE id = ?1", [id]).map_err(sql)?;
    }
    for id in &batch.dropped {
        for table in ["plugins", "samples", "tags", "project_info"] {
            tx.execute(&format!("DELETE FROM {table} WHERE project_id = ?1"), [id]).map_err(sql)?;
        }
        written.dropped += tx.execute("DELETE FROM projects WHERE id = ?1", [id]).map_err(sql)?;
    }
    check(&tx, &kept, batch).map_err(|e| match e {
        Checked::Sql(e) => sql(e),
        Checked::Differs(why) => write_error(format!("what was written didn't read back the same ({why})")),
    })?;
    tx.commit().map_err(sql)?;
    Ok(written)
}

/// Writes one save, and says which id it's kept under.
fn put_one(tx: &Transaction<'_>, put: &Put) -> rusqlite::Result<i64> {
    let save = &put.detail.save;
    let at_path: Option<i64> =
        tx.query_row("SELECT id FROM projects WHERE path = ?1", [&save.path], |r| r.get(0)).optional()?;
    let id_taken = || -> rusqlite::Result<bool> {
        tx.query_row("SELECT EXISTS (SELECT 1 FROM projects WHERE id = ?1)", [save.id], |r| r.get(0))
    };
    let id = match at_path {
        Some(id) => {
            update(tx, id, put)?;
            id
        }
        None if !put.new && id_taken()? => {
            update(tx, save.id, put)?;
            save.id
        }
        None if !id_taken()? => insert(tx, Some(save.id), put)?,
        None => insert(tx, None, put)?,
    };
    for table in ["plugins", "samples", "tags"] {
        tx.execute(&format!("DELETE FROM {table} WHERE project_id = ?1"), [id])?;
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
        sample.execute(params![id, s.path, s.found, s.unplayed])?;
    }
    let mut tag = tx.prepare_cached("INSERT OR IGNORE INTO tags (project_id, tag) VALUES (?1, ?2)")?;
    for t in &save.tags {
        tag.execute(params![id, t])?;
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
    Ok(id)
}

/// The columns a read sets, in the order [`update`] and [`insert`] give them (`?2` on).
const READ_COLUMNS: &str = "path, name, daw, folder, size, mtime, tempo, version, title, confidence, can_midi, \
    can_presets, error, file_missing, hardware, status, rating, favourite, musical_key, notes, detected_key, created, \
    note_count, length_bars, track_count";

/// Sets a save's row as read. When it was last opened is left as it is: that's the app's to note, not the file's.
fn update(tx: &Transaction<'_>, id: i64, put: &Put) -> rusqlite::Result<()> {
    let set: Vec<String> =
        READ_COLUMNS.split(", ").enumerate().map(|(i, c)| format!("{} = ?{}", c.trim(), i + 2)).collect();
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
/// missing, missing; each one dropped, gone.
fn check(tx: &Transaction<'_>, kept: &[(i64, &SaveDetail)], batch: &Batch) -> Result<(), Checked> {
    let mut row = tx.prepare(
        "SELECT path, size, mtime, file_missing,
             (SELECT COUNT(*) FROM plugins WHERE project_id = p.id),
             (SELECT COUNT(*) FROM samples WHERE project_id = p.id),
             (SELECT COUNT(*) FROM tags WHERE project_id = p.id)
         FROM projects p WHERE id = ?1",
    )?;
    for (id, d) in kept {
        let back: Option<(String, i64, i64, i64, i64, i64, i64)> = row
            .query_row([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)))
            .optional()?;
        let tags = {
            let mut t: Vec<&str> = d.save.tags.iter().map(String::as_str).collect();
            t.sort_unstable();
            t.dedup();
            t.len()
        };
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
    let gone = |id: &i64| -> rusqlite::Result<Option<i64>> {
        tx.query_row("SELECT file_missing FROM projects WHERE id = ?1", [id], |r| r.get(0)).optional()
    };
    for id in &batch.missing {
        if let Some(missing) = gone(id)?
            && missing != 1
        {
            return Err(Checked::Differs(format!("save {id} marked missing")));
        }
    }
    for id in &batch.dropped {
        if gone(id)?.is_some() {
            return Err(Checked::Differs(format!("save {id} left out")));
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
