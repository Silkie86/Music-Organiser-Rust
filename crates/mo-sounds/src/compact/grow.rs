//! The app's own sound library grows again: what a scan of the sample folders found is kept in it (stage 2 of "the
//! sound library grows again", review finding X1-own1). Since the C# app retired, nothing added new sounds, listened
//! to changed ones again, or followed renamed and deleted ones.
//!
//! As the C# app kept them (`SoundLibrary.Save`, `Move` and `Remove`):
//!
//! - **New** sounds get a row of their own, with what listening found (or why the file couldn't be listened to), the
//!   type their name or folders say ([`crate::types::from_path`], else "Unknown"), and no stars, plays or tags yet.
//!   A new sound never takes the number of one forgotten before ([`LAST_ID`]): what's kept elsewhere by that number
//!   (a galaxy's place, a cache) can't be taken for it.
//! - **Changed** sounds are listened to again, keeping everything you gave them (type, stars, heart, plays, tags) and
//!   CLAP's fingerprint (as the C# app did). The duplicate finder's print goes: it was of the file as it was.
//! - **Moved** sounds keep their row, with the type their new name or folders say.
//! - **Gone** sounds are forgotten, with their tags, plays, places and prints.
//!
//! Each call is one transaction, checked before it's kept, one at a time with every other change to the library, and
//! only on the app's own library in this version's format. What to keep comes from [`crate::scan::plan`]; the
//! listening ([`crate::analyse`]) is done before, off the window's thread, so each write is short.

use std::path::Path;
use std::sync::PoisonError;

use rusqlite::types::{Value, ValueRef};
use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params, params_from_iter};

use super::{
    ANALYSIS_COLUMNS, FORMAT, Found, INSERT_SOUND, OWN_SINCE, SYNCING, WAIT, analysis_columns, analysis_values,
    copy_of, dir_of, info_value, look_at, room_left, set_info, write_features, write_wave,
};
use crate::SoundsError;
use crate::analyse::Analysed;

/// The key in `copy_info` that holds the highest number a sound has been given: new ones are numbered after it, so a
/// forgotten sound's number is never given again.
pub const LAST_ID: &str = "last_id";

/// The key in `copy_info` that holds the first number the app gave a sound of its own scan: from it on, CLAP has
/// never heard them (this app has no way to run it), so they aren't counted when deciding whether it has heard enough
/// ([`crate::count_clap_could_hear`]).
pub const NEW_SINCE: &str = "new_since";

/// The key in `copy_info` that counts the changes the app made to its library, but for what's done with sounds
/// ([`super::did`]) and the galaxy's maps ([`super::maps`]). The backup copy holds the count it was made at, so one
/// made in the last day that's behind only by those can wait ([`super::backup::When::Daily`]), and one behind by
/// anything else can't.
pub const CHANGES: &str = "changes";

/// The first number the app gave a sound of its own scan ([`NEW_SINCE`]) in the library at `copy`; `None` when it has
/// added none (or the library can't be read just now).
pub fn new_since(copy: &Path) -> Option<i64> {
    let conn =
        Connection::open_with_flags(copy, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).ok()?;
    conn.busy_timeout(WAIT).ok()?;
    info_value(&conn, NEW_SINCE).ok().flatten()
}

/// One sound file a scan listened to: where it is, its size and time as the folder listing gave them, and what
/// listening found, or why the file couldn't be listened to.
#[derive(Clone, Debug)]
pub struct Listened {
    pub path: String,
    pub size: i64,
    pub mtime: i64,
    pub analysed: Result<Analysed, String>,
}

/// What [`keep`] and [`follow`] changed in the library.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Kept {
    /// New sounds.
    pub added: usize,
    /// Sounds listened to again.
    pub updated: usize,
    /// Sounds whose files were renamed or moved.
    pub moved: usize,
    /// Sounds whose files are gone.
    pub forgotten: usize,
}

impl Kept {
    /// Whether anything changed.
    pub fn any(&self) -> bool {
        *self != Kept::default()
    }
}

/// Keeps what a scan listened to in the app's own sound library beside `sounds_db`: new sounds added, changed ones
/// updated (by path, ignoring letter case). All of `listened` or none of it.
pub fn keep(sounds_db: &Path, listened: &[Listened]) -> Result<Kept, SoundsError> {
    write(sounds_db, |tx| {
        let mut kept = Kept::default();
        let mut last = last_id(tx)?;
        let first_new = last + 1;
        let update = update_sql();
        let gone_with = tables_of(tx, &["sound_waves", "sound_features", "sound_prints"])?;
        for l in listened {
            let analysis = l.analysed.as_ref().ok().map(|a| &a.analysis);
            let features = l.analysed.as_ref().ok().map(|a| features_blob(&a.features)).filter(|b| !b.is_empty());
            let error = l.analysed.as_ref().err();
            let name_type = crate::types::from_path(&l.path);
            let id: Option<i64> = tx
                .prepare_cached("SELECT id FROM sounds WHERE path = ?1 LIMIT 1")?
                .query_row([&l.path], |r| r.get(0))
                .optional()?;
            let id = match id {
                Some(id) => {
                    let mut values: Vec<Value> = vec![
                        Value::Integer(id),
                        Value::Integer(l.size),
                        Value::Integer(l.mtime),
                        error.map_or(Value::Null, |e| Value::Text(e.clone())),
                        name_type.map_or(Value::Null, |t| Value::Text(t.to_owned())),
                    ];
                    values.extend(analysis_values(analysis));
                    tx.prepare_cached(&update)?.execute(params_from_iter(values))?;
                    // What was of the file as it was: listened to again, and the duplicate finder's print goes.
                    for table in &gone_with {
                        tx.prepare_cached(&format!("DELETE FROM {table} WHERE sound_id = ?1"))?.execute([id])?;
                    }
                    kept.updated += 1;
                    id
                }
                None => {
                    last += 1;
                    let text = |t: &str| Value::Text(t.to_owned());
                    let mut values: Vec<Value> = vec![
                        Value::Integer(last),
                        text(&l.path),
                        Value::Integer(l.size),
                        Value::Integer(l.mtime),
                        error.map_or(Value::Null, |e| text(e)),
                        name_type.map_or(Value::Null, text),
                        // Your type, none yet; the type, from the name or "Unknown", with where it came from.
                        Value::Null,
                        text(name_type.unwrap_or("Unknown")),
                        text(if name_type.is_some() { "name" } else { "" }),
                        Value::Real(1.0),
                        // No heart, stars or plays yet, and never played.
                        Value::Integer(0),
                        Value::Integer(0),
                        Value::Integer(0),
                        Value::Null,
                    ];
                    values.extend(analysis_values(analysis));
                    // CLAP hasn't heard it.
                    values.push(Value::Null);
                    tx.prepare_cached(INSERT_SOUND)?.execute(params_from_iter(values))?;
                    kept.added += 1;
                    last
                }
            };
            write_wave(tx, id, analysis)?;
            if let Some(features) = &features {
                write_features(tx, id, ValueRef::Blob(features))?;
            }
        }
        if kept.added > 0 {
            // The first the app numbered itself: CLAP never hears it, nor any after it.
            tx.execute("INSERT OR IGNORE INTO copy_info (key, value) VALUES (?1, ?2)", params![NEW_SINCE, first_new])?;
        }
        set_info(tx, LAST_ID, last)?;
        // Checked before it's kept: each file is there once, as listened to.
        for l in listened {
            let found: i64 = tx
                .prepare_cached("SELECT COUNT(*) FROM sounds WHERE path = ?1 AND size = ?2 AND mtime = ?3")?
                .query_row(params![l.path, l.size, l.mtime], |r| r.get(0))?;
            if found != 1 {
                return Ok(Err(format!("{} read back {found} times", l.path)));
            }
        }
        Ok(Ok(kept))
    })
}

/// Follows sounds whose files were renamed or moved, `(from, to)`, and forgets those whose files are gone, in the app's
/// own sound library beside `sounds_db` (by path, ignoring letter case). All of it or none.
pub fn follow(sounds_db: &Path, moves: &[(String, String)], gone: &[String]) -> Result<Kept, SoundsError> {
    write(sounds_db, |tx| {
        let mut kept = Kept::default();
        // The highest number given so far is noted before any sound goes: it's never given again.
        let last = last_id(tx)?;
        set_info(tx, LAST_ID, last)?;
        let gone_with = tables_of(tx, &["sound_waves", "sound_features", "sound_clap", "sound_prints"])?;
        for (from, to) in moves {
            kept.moved += tx
                .prepare_cached("UPDATE sounds SET path = ?2, name_type = ?3 WHERE path = ?1")?
                .execute(params![from, to, crate::types::from_path(to)])?;
        }
        for path in gone {
            let ids: Vec<i64> = tx
                .prepare_cached("SELECT id FROM sounds WHERE path = ?1")?
                .query_map([path], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            for id in ids {
                // Its tags, plays, places, matches and hidden mark go with it (the C# app's tables say so); these
                // tables of the app's own don't, so they're told.
                for table in &gone_with {
                    tx.prepare_cached(&format!("DELETE FROM {table} WHERE sound_id = ?1"))?.execute([id])?;
                }
                kept.forgotten += tx.prepare_cached("DELETE FROM sounds WHERE id = ?1")?.execute([id])?;
            }
        }
        // Checked before it's kept: nothing is left where it was.
        for path in moves.iter().map(|(from, _)| from).chain(gone) {
            let left: i64 =
                tx.prepare_cached("SELECT COUNT(*) FROM sounds WHERE path = ?1")?.query_row([path], |r| r.get(0))?;
            if left != 0 && !moves.iter().any(|(_, to)| mo_formats::case::eq_ignore_case(to, path)) {
                return Ok(Err(format!("{path} is still there")));
            }
        }
        Ok(Ok(kept))
    })
}

/// Which of `tables` the library has (one made from a `sounds.db` without the duplicate finder's prints has none).
fn tables_of(tx: &Transaction<'_>, tables: &[&'static str]) -> rusqlite::Result<Vec<&'static str>> {
    let mut there = Vec::with_capacity(tables.len());
    for &table in tables {
        let found: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            [table],
            |r| r.get(0),
        )?;
        if found {
            there.push(table);
        }
    }
    Ok(there)
}

/// The highest number a sound has been given ([`LAST_ID`]): the one noted, or the highest there.
fn last_id(tx: &Transaction<'_>) -> rusqlite::Result<i64> {
    let noted = info_value(tx, LAST_ID)?.unwrap_or(0);
    let there: i64 = tx.query_row("SELECT COALESCE(MAX(id), 0) FROM sounds", [], |r| r.get(0))?;
    Ok(noted.max(there))
}

/// The update of a sound listened to again: its file's size and time, why it couldn't be listened to, the type its
/// name says, and what listening found (`?1` is its number).
fn update_sql() -> String {
    let columns = ["size", "mtime", "error", "name_type"].into_iter().chain(analysis_columns!().split(", "));
    let set: Vec<String> = columns.enumerate().map(|(i, c)| format!("{c} = ?{}", i + 2)).collect();
    debug_assert_eq!(set.len(), 4 + ANALYSIS_COLUMNS);
    format!("UPDATE sounds SET {} WHERE id = ?1", set.join(", "))
}

/// The analyser's features as `sounds.db` held them: each number's 4 bytes, little end first.
fn features_blob(features: &[f32]) -> Vec<u8> {
    features.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Runs `change` in one transaction on the app's own sound library beside `sounds_db`, and keeps it only when it says
/// it's whole (`Ok(Ok(_))`); `Ok(Err(why))` drops it. Only the app's own library in this version's format is written,
/// looked at again under the write lock, with room left on its disk. Every change the app makes to its library goes
/// through here or [`write_small`]; each one here is counted ([`CHANGES`]).
pub(super) fn write<T>(
    sounds_db: &Path,
    change: impl FnOnce(&Transaction<'_>) -> rusqlite::Result<Result<T, String>>,
) -> Result<T, SoundsError> {
    write_as(sounds_db, true, change)
}

/// As [`write`], for what's done with sounds ([`super::did`]) and the galaxy's maps ([`super::maps`]): not counted, so
/// the backup copy can wait for it.
pub(super) fn write_small<T>(
    sounds_db: &Path,
    change: impl FnOnce(&Transaction<'_>) -> rusqlite::Result<Result<T, String>>,
) -> Result<T, SoundsError> {
    write_as(sounds_db, false, change)
}

/// [`write`], counted (`counted`) or not.
fn write_as<T>(
    sounds_db: &Path,
    counted: bool,
    change: impl FnOnce(&Transaction<'_>) -> rusqlite::Result<Result<T, String>>,
) -> Result<T, SoundsError> {
    let copy = copy_of(sounds_db);
    let failed = |why: String| SoundsError::Copy { path: copy.clone(), why };
    let _one_at_a_time = SYNCING.lock().unwrap_or_else(PoisonError::into_inner);
    match look_at(&copy, WAIT) {
        Ok(Found::Copy { format: FORMAT, own: Some(_), .. }) => {}
        Ok(_) => {
            return Err(failed(
                "it isn't the app's own sound library in this version's format, so it isn't written".into(),
            ));
        }
        Err(why) => return Err(failed(format!("it couldn't be read just now ({why})"))),
    }
    if let Ok(free) = fs4::available_space(dir_of(&copy)) {
        room_left(free).map_err(failed)?;
    }
    let sql = |e: rusqlite::Error| match e.sqlite_error_code() {
        Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
            failed("something else is writing to it just now".into())
        }
        _ => SoundsError::Read(e),
    };
    let mut conn =
        Connection::open_with_flags(&copy, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX)
            .map_err(sql)?;
    conn.busy_timeout(WAIT).map_err(sql)?;
    // What goes with a sound that's forgotten (its tags, plays, places) is told so by the C# app's own tables.
    conn.pragma_update(None, "foreign_keys", true).map_err(sql)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(sql)?;
    if info_value(&tx, OWN_SINCE).map_err(sql)?.is_none() {
        return Err(failed("it isn't the app's own sound library, so it isn't written".into()));
    }
    let done = change(&tx).map_err(sql)?.map_err(|why| failed(format!("what was written didn't read back ({why})")))?;
    if counted {
        let changes = info_value(&tx, CHANGES).map_err(sql)?.unwrap_or(0);
        set_info(&tx, CHANGES, changes + 1).map_err(sql)?;
    }
    tx.commit().map_err(sql)?;
    Ok(done)
}
