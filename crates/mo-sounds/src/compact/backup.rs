//! The sound library's backup copy: one checked copy of the app's own sound library, kept beside it as
//! `sounds-rust.db.backup` (stage 1 of "the sound library grows again", Solomon 2026-11-06).
//!
//! Once the app writes into its own library (new sounds, plays, stars, tags, the galaxy's maps), what it wrote lives
//! only there: a library found damaged couldn't be made again from `sounds.db` without losing it. So the app keeps
//! this copy, made after the library changes, and mends a damaged library from it ([`super::mend_reading`]).
//!
//! - **Made** ([`back_up`]) by SQLite itself (`VACUUM INTO`: one read of the library at one moment, into a file of its
//!   own beside it), checked whole (`integrity_check`, and the same sounds as the library), put on the disk, and only
//!   then given the backup's name, in place of the one before. There's only ever one (rule 28).
//! - **Up to date** while the library looks just as it did when the backup was made: the backup notes the library's
//!   [`SoundsDb::stamp`] then ([`BACKED_UP_FROM`]), which changes with every save to it (and never with a read), so
//!   nothing that writes to the library has to say so. A backup that hasn't been looked at for a week is checked
//!   again (`quick_check`), and made again if it's damaged. Plays, drags and sends (and the galaxy's maps, favourites,
//!   stars, your own types and tags) change the library at nearly every start, so for them alone it's made again once
//!   a day at most ([`When::Daily`]): the backup holds how many other changes the library had had
//!   ([`super::grow::CHANGES`]), and one behind by any of those is made again at once.
//! - **Mending** ([`restore`]): the backup is checked again (`quick_check`), copied beside the library, and given its
//!   name. The damaged library is set aside as `sounds-rust.db.damaged` (only one: the one before goes), so nothing is
//!   thrown away at once; it goes a week after it was set aside.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::PoisonError;
use std::time::{Duration, Instant, SystemTime};

use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use super::{
    FORMAT, Found, LAYING_OUT, OWN_SINCE, Part, SYNCING, Salvaged, WAIT, dir_of, format_of, info_value, look_at,
    part_of, readable, remove_database, remove_parts, room_for, side, unix_now,
};
use crate::SoundsDb;
use crate::db::is_damage;

/// The backup copy's name ends so, after the library's: `sounds-rust.db.backup`.
const BACKUP_END: &str = ".backup";

/// A damaged library is set aside under its name and this: `sounds-rust.db.damaged`.
const DAMAGED_END: &str = ".damaged";

/// A week: a damaged library set aside goes once it's been there this long, and a backup copy that hasn't been looked
/// at for this long is checked again.
const A_WEEK: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The key in the backup's `copy_info` that holds the library's [`SoundsDb::stamp`] as it was copied: while the
/// library's is the same, the backup is up to date.
pub const BACKED_UP_FROM: &str = "backed_up_from";

/// The key in the backup's `copy_info` that holds when it was made (Unix seconds).
pub(super) const BACKED_UP_AT: &str = "backed_up_at";

/// Where the backup copy of the library at `copy` is kept: beside it.
pub fn backup_of(copy: &Path) -> PathBuf {
    side(copy, BACKUP_END)
}

/// Where the library at `copy` is set aside when it's found damaged and mended.
pub fn damaged_of(copy: &Path) -> PathBuf {
    side(copy, DAMAGED_END)
}

/// What [`back_up`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackedUp {
    /// Made: how big it is, in bytes, and how long making and checking it took.
    Made { bytes: u64, took: Duration },
    /// Not made this time, and why (the disk short of room, the library busy or changed meanwhile…). The backup
    /// copy before, if there is one, stays as it was; a new one is tried again the next time.
    NotNow(String),
    /// Not made: the library itself is damaged where making the copy reads it, somewhere the page's read never looks
    /// (the waveforms, say), and why. It wants mending ([`super::mend_reading`]: review finding S-3); the backup copy
    /// before stays as it was.
    Damaged(String),
}

/// How soon a library that changed since its backup copy was made is backed up again ([`back_up_when`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum When {
    /// Now: new sounds came in, or the library was made afresh.
    Now,
    /// Once the backup copy is a day old, when the library changed since only by plays, drags and sends, the
    /// galaxy's maps, or your favourites, stars, own types and tags (any other change, now). They change the library
    /// at nearly every start, and making the copy writes about the library's size: a day of them at most is lost if
    /// the library is damaged (the Sounds page looks again every hour while the app stays open: review finding S-6).
    Daily,
}

/// How old a backup copy may be before a library that changed is backed up again, for [`When::Daily`].
const A_DAY: i64 = 24 * 60 * 60;

/// Makes the backup copy of the app's own sound library beside `sounds_db` when there's none, the library has changed
/// since it was made, or it's found damaged. `None` when there's nothing to do: it's up to date, or there's no library
/// that's the app's own in this version's format (one of the format before is laid out anew first). Reads the whole
/// library and writes about its size, so it's for the background, never the window's thread; one at a time with every
/// other change to the library.
pub fn back_up(sounds_db: &Path) -> Option<BackedUp> {
    back_up_when(sounds_db, When::Now)
}

/// As [`back_up`], but with [`When::Daily`] a library that changed only by plays, drags and sends, or the galaxy's
/// maps, since a backup copy made in the last day waits (one that's missing, damaged, of another format, or behind by
/// anything else is still made at once).
pub fn back_up_when(sounds_db: &Path, when: When) -> Option<BackedUp> {
    let copy = super::copy_of(sounds_db);
    let backup = backup_of(&copy);
    let _one_at_a_time = SYNCING.lock().unwrap_or_else(PoisonError::into_inner);
    match look_at(&copy, WAIT) {
        Ok(Found::Copy { format: FORMAT, own: Some(_), .. }) => {}
        Ok(_) => return None,
        Err(why) => return Some(BackedUp::NotNow(format!("the library couldn't be read just now ({why})"))),
    }
    remove_parts(&backup, true);
    let up_to_date = matches!(look_at(&backup, WAIT), Ok(Found::Copy { format: FORMAT, own: Some(_), .. }))
        && (text_note(&backup, BACKED_UP_FROM).is_some_and(|from| from == SoundsDb::stamp(&copy))
            || (when == When::Daily && made_in_the_last_day(&backup) && only_small_changes(&copy, &backup)))
        && still_whole(&backup);
    let made = if up_to_date {
        None
    } else {
        let began = Instant::now();
        Some(match make(&copy, &backup) {
            Ok(bytes) => BackedUp::Made { bytes, took: began.elapsed() },
            Err(not) => not,
        })
    };
    // A library set aside as damaged goes a week after: the library has been whole (and backed up) since.
    let damaged = damaged_of(&copy);
    let old = fs::metadata(&damaged)
        .and_then(|m| m.modified())
        .is_ok_and(|t| SystemTime::now().duration_since(t).is_ok_and(|age| age > A_WEEK));
    if old && backup.exists() {
        let _ = remove_database(&damaged);
    }
    made
}

/// Whether the backup copy at `backup` was made less than a day ago ([`BACKED_UP_AT`]). A time ahead of now (the
/// clock was put back) doesn't count as recent.
fn made_in_the_last_day(backup: &Path) -> bool {
    let at = open_read_only(backup).ok().and_then(|conn| info_value(&conn, BACKED_UP_AT).ok().flatten());
    at.is_some_and(|at| (0..A_DAY).contains(&(unix_now() - at)))
}

/// Whether the library at `copy` changed since its backup copy at `backup` was made only by what's done with sounds,
/// the galaxy's maps and your own say about sounds: the same library (its [`OWN_SINCE`]) with no other change since
/// ([`super::grow::CHANGES`]).
fn only_small_changes(copy: &Path, backup: &Path) -> bool {
    let notes = |path: &Path| {
        let conn = open_read_only(path).ok()?;
        Some((info_value(&conn, OWN_SINCE).ok()?, info_value(&conn, super::grow::CHANGES).ok()?))
    };
    notes(copy).is_some_and(|copy| notes(backup) == Some(copy))
}

/// A note in `copy_info` of the database at `path` that's text; `None` when there's none, or it can't be read.
fn text_note(path: &Path, key: &str) -> Option<String> {
    let conn = open_read_only(path).ok()?;
    conn.query_row("SELECT value FROM copy_info WHERE key = ?1", [key], |r| {
        Ok(match r.get_ref(0)? {
            ValueRef::Text(t) => std::str::from_utf8(t).ok().map(str::to_owned),
            _ => None,
        })
    })
    .optional()
    .ok()
    .flatten()
    .flatten()
}

/// Whether the backup copy at `backup` is still whole, as far as can be told: one not looked at for a week is checked
/// again (`quick_check`), and its time set to now when it is. One that can't be checked just now counts as whole, till
/// the next time.
fn still_whole(backup: &Path) -> bool {
    let looked_at = fs::metadata(backup).and_then(|m| m.modified());
    if looked_at.is_ok_and(|t| SystemTime::now().duration_since(t).is_ok_and(|age| age <= A_WEEK)) {
        return true;
    }
    match check(backup, None, false) {
        Ok(()) => {
            let _ = fs::File::options().write(true).open(backup).and_then(|f| f.set_modified(SystemTime::now()));
            true
        }
        Err(NotRestored::NoBackup(_)) => false,
        Err(NotRestored::NotNow(_)) => true,
    }
}

pub(super) fn open_read_only(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    conn.busy_timeout(WAIT)?;
    Ok(conn)
}

/// The size of the database at `path`, with SQLite's log beside it.
fn size_of(path: &Path) -> u64 {
    ["", "-wal"].iter().filter_map(|end| fs::metadata(side(path, end)).ok()).map(|m| m.len()).sum()
}

/// Makes the backup copy of `copy` at `backup`: under another name, noted with what it was made from and when,
/// checked, put on the disk, and then given the backup's name. How big it is, or why not ([`BackedUp::NotNow`], or
/// [`BackedUp::Damaged`] for damage met reading the library).
fn make(copy: &Path, backup: &Path) -> Result<u64, BackedUp> {
    let not_now = BackedUp::NotNow;
    if let Ok(free) = fs4::available_space(dir_of(copy)) {
        room_for(free, size_of(copy), "making its backup copy").map_err(not_now)?;
    }
    let part = part_of(backup);
    remove_database(&part).map_err(|e| not_now(format!("a half-made backup copy couldn't be removed ({e})")))?;
    let _part = Part(part.clone());
    let name = part.to_str().ok_or_else(|| not_now("its folder's name can't be given to SQLite".into()))?;
    // It waits its turn behind the long reads of the library under way or waiting (every sound, every fingerprint),
    // so it never starts in the middle of one on a slow disk (review finding S-4). It doesn't keep the turn: `VACUUM
    // INTO` can't give way between rows, so the waveforms of the tiles on show would wait for the whole copy (tens of
    // seconds for a big library), which is worse than sharing the disk with them for that while.
    drop(crate::turns::long_turn());
    // How the library looks before it's read: the same after, so nothing was written to it in between.
    let from = SoundsDb::stamp(copy);
    let sounds = (|| -> rusqlite::Result<i64> {
        let conn = open_read_only(copy)?;
        // The sounds counted and the copy made on one read of the library, at one moment.
        let sounds = conn.query_row("SELECT COUNT(*) FROM sounds", [], |r| r.get(0))?;
        conn.execute("VACUUM INTO ?1", [name])?;
        Ok(sounds)
    })()
    .map_err(|e| {
        if is_damage(&e) { BackedUp::Damaged(format!("the library is damaged ({e})")) } else { not_now(e.to_string()) }
    })?;
    if SoundsDb::stamp(copy) != from {
        return Err(not_now("the library changed while it was copied".into()));
    }
    (|| -> rusqlite::Result<()> {
        let conn =
            Connection::open_with_flags(&part, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
        conn.execute(
            "INSERT OR REPLACE INTO copy_info (key, value) VALUES (?1, ?2), (?3, ?4)",
            params![BACKED_UP_FROM, from, BACKED_UP_AT, unix_now()],
        )?;
        conn.close().map_err(|(_, e)| e)
    })()
    .map_err(|e| not_now(format!("it couldn't be noted ({e})")))?;
    check(&part, Some(sounds), true).map_err(|(NotRestored::NoBackup(why) | NotRestored::NotNow(why))| not_now(why))?;
    fs::OpenOptions::new().write(true).open(&part).and_then(|f| f.sync_all()).map_err(|e| not_now(e.to_string()))?;
    fs::rename(&part, backup).map_err(|e| not_now(format!("it couldn't be given its name ({e})")))?;
    Ok(size_of(backup))
}

/// Checks the backup copy at `path`: whole (SQLite's `integrity_check`, or with `whole` false its quicker
/// `quick_check`), the app's own library in a format this version reads, with `sounds` sounds when that's known.
/// [`NotRestored::NoBackup`] when it isn't; [`NotRestored::NotNow`] when it couldn't be read just now.
fn check(path: &Path, sounds: Option<i64>, whole: bool) -> Result<(), NotRestored> {
    let checked = (|| -> rusqlite::Result<Result<(), String>> {
        let conn = open_read_only(path)?;
        let pragma = if whole { "PRAGMA integrity_check" } else { "PRAGMA quick_check" };
        let said: String = conn.query_row(pragma, [], |r| r.get(0))?;
        if said != "ok" {
            return Ok(Err(format!("it's damaged ({said})")));
        }
        if !format_of(&conn)?.is_some_and(readable) || info_value(&conn, OWN_SINCE)?.is_none() {
            return Ok(Err("it isn't a whole copy of the app's own sound library".into()));
        }
        let held: i64 = conn.query_row("SELECT COUNT(*) FROM sounds", [], |r| r.get(0))?;
        if let Some(sounds) = sounds.filter(|&s| s != held) {
            return Ok(Err(format!("it holds {held} sounds, not {sounds}")));
        }
        Ok(Ok(()))
    })();
    match checked {
        Ok(checked) => checked.map_err(NotRestored::NoBackup),
        Err(e) if is_damage(&e) => Err(NotRestored::NoBackup(format!("it's damaged ({e})"))),
        Err(e) => Err(NotRestored::NotNow(format!("it couldn't be read just now ({e})"))),
    }
}

/// Whether the library at `copy` has a backup copy it could be put back from now ([`restore`] looks the same way):
/// there, the app's own library in a format this version reads, and whole by SQLite's `quick_check`. Why not
/// otherwise. Reads the whole backup copy, so for the background, and not at every start: the old `sounds.db` is
/// only put away once this says yes (`mo_app::old_databases`), as a damaged library with no usable backup is made
/// again from that.
pub fn usable(copy: &Path) -> Result<(), String> {
    let backup = backup_of(copy);
    match look_at(&backup, WAIT) {
        Ok(Found::Copy { format, own: Some(_), .. }) if readable(format) => {}
        Ok(Found::Nothing) => return Err("the app's own sound library has no backup copy yet".into()),
        Ok(_) => return Err("the sound library's backup copy isn't a whole copy of it".into()),
        Err(why) => return Err(format!("the sound library's backup copy couldn't be read just now ({why})")),
    }
    check(&backup, None, false).map_err(|(NotRestored::NoBackup(why) | NotRestored::NotNow(why))| {
        format!("the sound library's backup copy can't be used: {why}")
    })
}

/// Why [`restore`] didn't put the library back from its backup copy. Either way, nothing has changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum NotRestored {
    /// There's no backup copy to put it back from: none there, or it's damaged too, or not a whole copy.
    NoBackup(String),
    /// There's one, but it couldn't be put back just now (the disk short of room, something else holding the library
    /// or its backup…): it's tried again the next time.
    NotNow(String),
}

/// Puts the library at `copy` back from its backup copy, for a library found damaged: the backup checked again,
/// copied beside the library, given everything that can still be read of the damaged library (it's newer:
/// [`super::salvage`]), and given the library's name; the damaged library set aside ([`damaged_of`]), with its log.
/// When the backup was made and what was kept of the damaged library, or why it wasn't put back.
pub(crate) fn restore(copy: &Path) -> Result<(SystemTime, Salvaged), NotRestored> {
    use NotRestored::{NoBackup, NotNow};
    let backup = backup_of(copy);
    let _one_at_a_time = SYNCING.lock().unwrap_or_else(PoisonError::into_inner);
    match look_at(&backup, WAIT) {
        Ok(Found::Copy { format, own: Some(_), .. }) if readable(format) => {}
        Ok(Found::Nothing) => return Err(NoBackup("there's no backup copy".into())),
        Ok(_) => return Err(NoBackup("the backup copy isn't a whole copy of the library".into())),
        Err(why) => return Err(NotNow(format!("the backup copy couldn't be read just now ({why})"))),
    }
    check(&backup, None, false).map_err(|not| match not {
        NoBackup(why) => NoBackup(format!("the backup copy can't be used: {why}")),
        NotNow(why) => NotNow(format!("the backup copy: {why}")),
    })?;
    let made = open_read_only(&backup)
        .and_then(|conn| info_value(&conn, BACKED_UP_AT))
        .ok()
        .flatten()
        .and_then(|at| u64::try_from(at).ok())
        .map(|at| SystemTime::UNIX_EPOCH + Duration::from_secs(at))
        .or_else(|| fs::metadata(&backup).and_then(|m| m.modified()).ok())
        .unwrap_or_else(SystemTime::now);
    if let Ok(free) = fs4::available_space(dir_of(copy)) {
        room_for(free, size_of(&backup), "putting it back from its backup copy").map_err(NotNow)?;
    }
    let part = part_of(copy);
    remove_database(&part).map_err(|e| NotNow(format!("a half-made copy couldn't be removed ({e})")))?;
    let _part = Part(part.clone());
    let put = (|| -> Result<Salvaged, String> {
        fs::copy(&backup, &part).map_err(|e| e.to_string())?;
        let conn =
            Connection::open_with_flags(&part, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX)
                .map_err(|e| e.to_string())?;
        // The library isn't a backup: what this one was made from goes with it.
        conn.execute("DELETE FROM copy_info WHERE key IN (?1, ?2)", [BACKED_UP_FROM, BACKED_UP_AT])
            .map_err(|e| e.to_string())?;
        // What the app wrote since the backup was made, where it can still be read (review finding S-3).
        let salvaged = super::salvage::salvage(&conn, copy, dir_of(copy)).map_err(|f| f.why)?;
        // Read and written as the library is: in WAL mode, so the page's readers never wait for a writer.
        conn.pragma_update(None, "journal_mode", "WAL").map_err(|e| e.to_string())?;
        conn.close().map_err(|(_, e)| e.to_string())?;
        if ["-wal", "-shm", "-journal"].iter().any(|end| side(&part, end).exists()) {
            return Err("its log was left beside it".into());
        }
        fs::OpenOptions::new().write(true).open(&part).and_then(|f| f.sync_all()).map_err(|e| e.to_string())?;
        Ok(salvaged)
    })();
    let salvaged = put.map_err(|why| NotNow(format!("it couldn't be copied from its backup copy ({why})")))?;
    // No reader in this app opens the library while it's swapped.
    let _readers_wait = LAYING_OUT.lock().unwrap_or_else(PoisonError::into_inner);
    set_aside(copy).map_err(NotNow)?;
    if let Err(e) = fs::rename(&part, copy) {
        let undone = match put_back(copy) {
            Ok(()) => String::new(),
            Err(e) => format!("; the damaged one is left as {} ({e})", damaged_of(copy).display()),
        };
        return Err(NotNow(format!("the copy of its backup couldn't be given its name ({e}){undone}")));
    }
    Ok((made, salvaged))
}

/// Sets the damaged library at `copy` aside ([`damaged_of`]), with its log, so the two can still be opened together;
/// the one set aside before goes. Not when something else has it open: then nothing is moved. A log left beside the
/// copy put in its place would be read into it as if it were its own, so nothing is put there unless it has gone.
pub(super) fn set_aside(copy: &Path) -> Result<(), String> {
    let held = || "the damaged library couldn't be set aside: something else has it open".to_owned();
    let busy =
        |e: std::io::Error| format!("the damaged library couldn't be set aside ({e}): something else may have it open");
    if held_elsewhere(copy) {
        return Err(held());
    }
    let damaged = damaged_of(copy);
    remove_database(&damaged).map_err(busy)?;
    // Windows won't remove the shared-memory file of a library something has open: then nothing is moved.
    for end in ["-shm", "-journal"] {
        match fs::remove_file(side(copy, end)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(busy(e)),
            _ => {}
        }
    }
    // The library first, then its log: if the log can't follow, the library is put back, so the two stay together.
    match fs::rename(copy, &damaged) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(busy(e)),
        _ => {}
    }
    match fs::rename(side(copy, "-wal"), side(&damaged, "-wal")) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            let _ = fs::rename(&damaged, copy);
            return Err(busy(e));
        }
        _ => {}
    }
    if ["", "-wal", "-shm", "-journal"].iter().any(|end| side(copy, end).exists()) {
        let _ = put_back(copy);
        return Err(held());
    }
    // It goes a week from now: a file keeps the time it was last written when it's moved.
    let _ = fs::File::options().write(true).open(&damaged).and_then(|f| f.set_modified(SystemTime::now()));
    Ok(())
}

/// Undoes [`set_aside`]: the damaged library at `copy` given its name again, then its log (never a log without its
/// library, which would be read into another put there).
pub(super) fn put_back(copy: &Path) -> std::io::Result<()> {
    let damaged = damaged_of(copy);
    if copy.exists() {
        return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "something else is there in its place"));
    }
    fs::rename(&damaged, copy)?;
    match fs::rename(side(&damaged, "-wal"), side(copy, "-wal")) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Whether something else (another copy of the app) has the library at `copy` open. SQLite lets go of the library's
/// log and shared-memory file as the last connection to it closes, having put what the log held into it: opened and
/// closed here, they're gone unless something else holds it. Windows won't move or remove a file that's open anyway;
/// elsewhere one can be, so this is how it's told there. A library so damaged SQLite can't open it at all counts as
/// held by nothing (its files are as they are).
fn held_elsewhere(copy: &Path) -> bool {
    let Ok(conn) =
        Connection::open_with_flags(copy, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX)
    else {
        return false;
    };
    // Read, so SQLite opens it and its log: one that's never read is closed without touching them. Busy is held.
    match conn.query_row("PRAGMA schema_version", [], |r| r.get::<_, i64>(0)) {
        Err(e) if is_damage(&e) => return false,
        Err(_) => return true,
        Ok(_) => {}
    }
    if conn.close().is_err() {
        return true;
    }
    ["-wal", "-shm"].iter().any(|end| side(copy, end).exists())
}
