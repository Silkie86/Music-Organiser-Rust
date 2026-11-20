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
//!   thrown away at once; it goes a week after it was set aside. One that had nothing to mend it from, with parts that
//!   couldn't be read, is kept for good instead, as `sounds-rust.db.damaged-kept` ([`kept_of`]): those parts are
//!   nowhere else.
//! - **Damaged too** ([`NotRestored::Damaged`]): what can still be read of it is kept all the same, under what can be
//!   read of the library ([`super::mend_reading`]), and it's set aside as `sounds-rust.db.backup-damaged`
//!   ([`backup_damaged_of`]) before the mended library takes the damaged one's place, so no backup copy is ever made
//!   over it (the whole-app review of 19 Nov, SN-3).

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

/// A damaged library kept for good is set aside under its name and this: `sounds-rust.db.damaged-kept` ([`kept_of`]).
const KEPT_END: &str = ".damaged-kept";

/// A damaged backup copy is set aside under the library's name and this: `sounds-rust.db.backup-damaged`
/// ([`backup_damaged_of`]).
const BACKUP_DAMAGED_END: &str = ".backup-damaged";

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

/// Where the library at `copy` is set aside for good when it's found damaged with nothing to mend it from (no backup
/// copy, and no `sounds.db`) and parts of it couldn't be read: they're nowhere else, so it's never removed, nor
/// replaced by one set aside after it (the whole-app review of 14 Nov: [`super::mend_reading`]).
pub fn kept_of(copy: &Path) -> PathBuf {
    side(copy, KEPT_END)
}

/// Where the backup copy of the library at `copy` is set aside when it's found damaged too as the library is mended:
/// what could still be read of it went into the mended library, and no backup copy is made over it (the whole-app
/// review of 19 Nov, SN-3). It goes a week after it was set aside once there's a whole backup copy again, but never
/// while a damaged library kept for good is there ([`kept_of`]): it may hold some of what that one couldn't give.
pub fn backup_damaged_of(copy: &Path) -> PathBuf {
    side(copy, BACKUP_DAMAGED_END)
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
    let week_old = |path: &Path| {
        fs::metadata(path)
            .and_then(|m| m.modified())
            .is_ok_and(|t| SystemTime::now().duration_since(t).is_ok_and(|age| age > A_WEEK))
    };
    // A library set aside as damaged goes a week after: the library has been whole (and backed up) since.
    let damaged = damaged_of(&copy);
    if week_old(&damaged) && backup.exists() {
        let _ = remove_database(&damaged);
    }
    // So does a damaged backup copy set aside, once a whole one has been made or found since; never while a damaged
    // library kept for good is there, as it may hold some of what that one couldn't give.
    let whole_now = up_to_date || matches!(made, Some(BackedUp::Made { .. }));
    let backup_damaged = backup_damaged_of(&copy);
    if whole_now && week_old(&backup_damaged) && database_there(&kept_of(&copy)).is_none() {
        let _ = remove_database(&backup_damaged);
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
        Err(NotRestored::NoBackup(_) | NotRestored::Damaged(_)) => false,
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
    check(&part, Some(sounds), true)
        .map_err(|(NotRestored::NoBackup(why) | NotRestored::Damaged(why) | NotRestored::NotNow(why))| not_now(why))?;
    fs::OpenOptions::new().write(true).open(&part).and_then(|f| f.sync_all()).map_err(|e| not_now(e.to_string()))?;
    fs::rename(&part, backup).map_err(|e| not_now(format!("it couldn't be given its name ({e})")))?;
    Ok(size_of(backup))
}

/// Checks the backup copy at `path`: whole (SQLite's `integrity_check`, or with `whole` false its quicker
/// `quick_check`), the app's own library in a format this version reads, with `sounds` sounds when that's known.
/// [`NotRestored::Damaged`] when SQLite finds it damaged, [`NotRestored::NoBackup`] when it isn't such a copy;
/// [`NotRestored::NotNow`] when it couldn't be read just now.
fn check(path: &Path, sounds: Option<i64>, whole: bool) -> Result<(), NotRestored> {
    use NotRestored::{Damaged, NoBackup};
    let checked = (|| -> rusqlite::Result<Result<(), NotRestored>> {
        let conn = open_read_only(path)?;
        let pragma = if whole { "PRAGMA integrity_check" } else { "PRAGMA quick_check" };
        let said: String = conn.query_row(pragma, [], |r| r.get(0))?;
        if said != "ok" {
            return Ok(Err(Damaged(format!("it's damaged ({said})"))));
        }
        if !format_of(&conn)?.is_some_and(readable) || info_value(&conn, OWN_SINCE)?.is_none() {
            return Ok(Err(NoBackup("it isn't a whole copy of the app's own sound library".into())));
        }
        let held: i64 = conn.query_row("SELECT COUNT(*) FROM sounds", [], |r| r.get(0))?;
        if let Some(sounds) = sounds.filter(|&s| s != held) {
            return Ok(Err(NoBackup(format!("it holds {held} sounds, not {sounds}"))));
        }
        Ok(Ok(()))
    })();
    match checked {
        Ok(checked) => checked,
        Err(e) if is_damage(&e) => Err(Damaged(format!("it's damaged ({e})"))),
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
    check(&backup, None, false).map_err(
        |(NotRestored::NoBackup(why) | NotRestored::Damaged(why) | NotRestored::NotNow(why))| {
            format!("the sound library's backup copy can't be used: {why}")
        },
    )
}

/// Why [`restore`] didn't put the library back from its backup copy. Either way, nothing has changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum NotRestored {
    /// There's no backup copy to put it back from: none there, or not a whole copy of the app's own library.
    NoBackup(String),
    /// There's one, but it's damaged too: nothing can be read where it's looked at, or SQLite's `quick_check` finds a
    /// fault. What can still be read of it is kept all the same ([`super::mend_reading`]).
    Damaged(String),
    /// There's one, but it couldn't be put back just now (the disk short of room, something else holding the library
    /// or its backup…): it's tried again the next time.
    NotNow(String),
}

/// Puts the library at `copy` back from its backup copy, for a library found damaged: the backup checked again,
/// copied beside the library, given everything that can still be read of the damaged library (it's newer:
/// [`super::salvage`]), and given the library's name; the damaged library set aside ([`damaged_of`]), with its log.
/// When the backup was made and what was kept of the damaged library, or why it wasn't put back.
pub(crate) fn restore(copy: &Path) -> Result<(SystemTime, Salvaged), NotRestored> {
    use NotRestored::NotNow;
    let backup = backup_of(copy);
    let _one_at_a_time = SYNCING.lock().unwrap_or_else(PoisonError::into_inner);
    // A copy left half made by a mend cut off before (the app closed as it ran) goes first: it would take the room this
    // one needs (the whole-app review of 19 Nov, SN-15).
    remove_parts(copy, true);
    let made = checked_backup(&backup)?;
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
        let salvaged = super::salvage::salvage(&conn, copy, dir_of(copy), false).map_err(|f| f.why)?;
        finish_part(conn, &part)?;
        Ok(salvaged)
    })();
    let salvaged = put.map_err(|why| NotNow(format!("it couldn't be copied from its backup copy ({why})")))?;
    swap_in(&part, copy, &damaged_of(copy), "the copy of its backup").map_err(NotNow)?;
    Ok((made, salvaged))
}

/// Checks the backup copy at `backup` before it's put back ([`restore`], [`bring_back`]): the app's own library in a
/// format this version reads, and whole by SQLite's `quick_check`. When it was made ([`made_at`]), or why it can't be
/// put back.
fn checked_backup(backup: &Path) -> Result<SystemTime, NotRestored> {
    use NotRestored::{Damaged, NoBackup, NotNow};
    match look_at(backup, WAIT) {
        Ok(Found::Copy { format, own: Some(_), .. }) if readable(format) => {}
        Ok(Found::Nothing) => return Err(NoBackup("there's no backup copy".into())),
        Ok(Found::Unreadable) => {
            return Err(Damaged(
                "the backup copy can't be put back: it's damaged (its first part can't be read)".into(),
            ));
        }
        Ok(_) => return Err(NoBackup("the backup copy isn't a whole copy of the library".into())),
        Err(why) => return Err(NotNow(format!("the backup copy couldn't be read just now ({why})"))),
    }
    check(backup, None, false).map_err(|not| match not {
        NoBackup(why) => NoBackup(format!("the backup copy can't be used: {why}")),
        Damaged(why) => Damaged(format!("the backup copy can't be put back: {why}")),
        NotNow(why) => NotNow(format!("the backup copy: {why}")),
    })?;
    Ok(made_at(backup))
}

/// When the backup copy at `backup` was made, as it notes it ([`BACKED_UP_AT`]); `None` when that can't be read (a
/// damaged one may not say).
fn noted_made_at(backup: &Path) -> Option<SystemTime> {
    let at = open_read_only(backup).and_then(|conn| info_value(&conn, BACKED_UP_AT)).ok().flatten()?;
    u64::try_from(at).ok().map(|at| SystemTime::UNIX_EPOCH + Duration::from_secs(at))
}

/// When the backup copy at `backup` was made, for a card: as it notes it, else when its file was last written, else
/// now.
fn made_at(backup: &Path) -> SystemTime {
    noted_made_at(backup)
        .or_else(|| fs::metadata(backup).and_then(|m| m.modified()).ok())
        .unwrap_or_else(SystemTime::now)
}

/// What [`bring_back`] put in the place of the app's own sound library, which wasn't there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BroughtBack {
    /// The library itself, set aside at `from` as it was being mended ([`damaged_of`], or [`kept_of`] for one kept
    /// for good), after its backup copy was made or with none: it holds what the backup doesn't, so it's put back as it
    /// was, to be mended again as it's read.
    SetAside { from: PathBuf },
    /// Its backup copy, made at `made`, copied into its place (it stays, as the backup): what changed in the library
    /// since is lost.
    Backup { made: SystemTime },
    /// Its backup copy, which is damaged, moved into its place (it isn't a backup copy any more), with nothing newer to
    /// put back: it's mended as it's read, with what can still be read of it, and then set aside as a damaged library
    /// is (kept for good when parts of it couldn't be read). `set_aside`: the one set aside as it was found damaged
    /// before ([`backup_damaged_of`]).
    DamagedBackup { set_aside: bool },
}

/// Puts the app's own sound library at `copy` back when it isn't there, but something it could be mended from is (the
/// whole-app review of 19 Nov, SN-14): the library itself, set aside as it was being mended (the app stopped between
/// the two renames that put the mended one in its place), or its backup copy (something else took the library away).
/// Nothing else does: no library is made empty in its place ([`super::make_empty`]: it would be backed up over the
/// last of yours), so the Sounds page would say there's none at every start.
///
/// The newest of them is put back: the library set aside after its backup copy was made (or with none), as it was;
/// else the backup copy, copied (it stays the backup); else a damaged backup copy, moved, to be mended as it's read
/// (copied, it would be kept twice: the library made of it is set aside as it's mended, beside it). Which is newest
/// goes by when each was set aside and when the backup copy was made, as far as that can be told: never by a time
/// ahead of now, and a library set aside is put back over a whole backup copy only when it's sure to be newer, as it
/// would undo what's in that (the mend takes the library's word over the backup copy's). Over a damaged one that was
/// set aside as the library was mended, it's put back unless it's sure to be older: that mend set the library aside
/// after it.
///
/// What was put back, or `None` when the library is there or there's nothing to put back. Why not, when it couldn't
/// be: its log left without it (SQLite would read it into whatever is put there), the backup copy unreadable just
/// now, or not one this version can put back, the disk short of room. Reads the backup copy and writes about its size:
/// for the background, never the window's thread; one at a time with every other change to the library.
pub fn bring_back(copy: &Path) -> Result<Option<BroughtBack>, String> {
    use NotRestored::{Damaged, NoBackup, NotNow};
    let _one_at_a_time = SYNCING.lock().unwrap_or_else(PoisonError::into_inner);
    // A file that can't be looked at counts as there: nothing is put over what may be the library.
    let there = |path: &Path| !matches!(path.try_exists(), Ok(false));
    if there(copy) {
        return Ok(None);
    }
    let (backup, backup_damaged) = (backup_of(copy), backup_damaged_of(copy));
    let asides: Vec<PathBuf> = [damaged_of(copy), kept_of(copy)].into_iter().filter(|aside| there(aside)).collect();
    if asides.is_empty() && !there(&backup) && !there(&backup_damaged) {
        return Ok(None);
    }
    for end in ["-wal", "-journal"] {
        let left = side(copy, end);
        if there(&left) {
            return Err(format!(
                "{} is there without the library it's part of, so nothing is put back beside it until it's moved away \
                 (it would be read into it)",
                left.display()
            ));
        }
    }
    // A shared-memory file left without it holds none of it (Windows won't let one go that something has open).
    match fs::remove_file(side(copy, "-shm")) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            return Err(format!("something else may have it open ({e})"));
        }
        _ => {}
    }
    let looked = there(&backup).then(|| checked_backup(&backup));
    if let Some(Err(NotNow(why))) = looked {
        return Err(why);
    }
    // A time ahead of now isn't trusted: the clock was wrong as it was written, or is now.
    let now = SystemTime::now();
    let trusted = |at: SystemTime| (at <= now).then_some(at);
    let file_time = |path: &Path| fs::metadata(path).and_then(|m| m.modified()).ok();
    // When each was set aside (a file set aside is given that time), and when a backup copy was made at the latest: as
    // it notes it, else when its file was last written, as it's never made after that (a whole one's time is set again
    // when it's checked, a damaged one set aside has the time it was set aside).
    let set_at = |aside: &PathBuf| file_time(aside);
    let made_by =
        |backup: &Path| noted_made_at(backup).and_then(trusted).or_else(|| file_time(backup).and_then(trusted));
    let put_back = match &looked {
        // The backup copy beside it, whole or damaged: a library set aside after it was made holds what it doesn't
        // (the app stopped as that library was being mended), so it's put back as it was, to be mended with what the
        // backup has. One set aside before, or one it can't be told of, would undo what's in the backup copy (the mend
        // takes the library's word): the backup copy is put back in its place, and the library set aside stays as it
        // is.
        Some(Ok(_) | Err(Damaged(_))) => {
            let made = made_by(&backup);
            let newer =
                |aside: &PathBuf| made.is_some_and(|made| set_at(aside).and_then(trusted).is_some_and(|at| at > made));
            asides.into_iter().filter(newer).max_by_key(set_at)
        }
        // Only a damaged backup copy set aside: a mend sets that aside before the library, so the library set aside is
        // put back unless it's sure to be older (set aside before the backup copy was made).
        _ if there(&backup_damaged) => {
            let made = made_by(&backup_damaged);
            let older = |aside: &PathBuf| made.is_some_and(|made| set_at(aside).is_some_and(|at| at < made));
            asides.into_iter().filter(|aside| !older(aside)).max_by_key(set_at)
        }
        // No backup copy: the newest set aside.
        _ => asides.into_iter().max_by_key(set_at),
    };
    if let Some(aside) = put_back {
        move_in(&aside, copy)?;
        return Ok(Some(BroughtBack::SetAside { from: aside }));
    }
    match looked {
        Some(Ok(made)) => {
            copy_in(&backup, copy)?;
            Ok(Some(BroughtBack::Backup { made }))
        }
        Some(Err(Damaged(_))) => {
            move_in(&backup, copy)?;
            Ok(Some(BroughtBack::DamagedBackup { set_aside: false }))
        }
        _ if there(&backup_damaged) => {
            move_in(&backup_damaged, copy)?;
            Ok(Some(BroughtBack::DamagedBackup { set_aside: true }))
        }
        // Only a backup copy this version can't put back (not the app's own library, or of another format, which a
        // newer version may have made): it stays as it is, and no library is made beside it either
        // ([`super::make_empty`]: it would be backed up over it).
        Some(Err(NoBackup(why) | NotNow(why))) => Err(format!(
            "{} is there, but {why}, so it's left as it is (a newer version of the app may have made it), and no \
             library is put back or made beside it until it's moved away",
            backup.display()
        )),
        None => Ok(None),
    }
}

/// Moves the file at `from` (the library set aside as it was being mended, or a damaged backup copy) into the place of
/// the library at `copy`, which isn't there ([`bring_back`]), with its log ([`put_back_from`]), while no reader in this
/// app opens the library. A shared-memory file left beside it holds none of it: it goes first. Why not, when it
/// couldn't be: then it's where it was.
fn move_in(from: &Path, copy: &Path) -> Result<(), String> {
    let _readers_wait = LAYING_OUT.lock().unwrap_or_else(PoisonError::into_inner);
    let failed = |e: std::io::Error| format!("{} couldn't be put in its place ({e})", from.display());
    match fs::remove_file(side(from, "-shm")) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(failed(e)),
        _ => {}
    }
    put_back_from(copy, from).map_err(failed)
}

/// Copies the whole backup copy at `from` into the place of the library at `copy`, which isn't there ([`bring_back`]):
/// beside it first, readied as [`restore`] readies one (what it was made from goes: it isn't a backup any more), then
/// given its name, never over one that's there. Why not, when it couldn't be.
fn copy_in(from: &Path, copy: &Path) -> Result<(), String> {
    // A copy left half made by a mend cut off before goes first: it would take the room this one needs (SN-15).
    remove_parts(copy, true);
    if let Ok(free) = fs4::available_space(dir_of(copy)) {
        room_for(free, size_of(from), "putting it back from its backup copy")?;
    }
    let part = part_of(copy);
    remove_database(&part).map_err(|e| format!("a half-made copy couldn't be removed ({e})"))?;
    let _part = Part(part.clone());
    let copied = (|| -> Result<(), String> {
        fs::copy(from, &part).map_err(|e| e.to_string())?;
        let conn =
            Connection::open_with_flags(&part, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX)
                .map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM copy_info WHERE key IN (?1, ?2)", [BACKED_UP_FROM, BACKED_UP_AT])
            .map_err(|e| e.to_string())?;
        finish_part(conn, &part)
    })();
    copied.map_err(|why| format!("it couldn't be copied from its backup copy ({why})"))?;
    let _readers_wait = LAYING_OUT.lock().unwrap_or_else(PoisonError::into_inner);
    // A second name for the file, which is never put over one that's there; where the disk can't give a file two (a
    // FAT drive), renamed while nothing is. The half-made name goes as it's let go of.
    match fs::hard_link(&part, copy) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists || copy.exists() => {
            Err("another library was put in its place meanwhile".into())
        }
        Err(_) => fs::rename(&part, copy).map_err(|e| format!("it couldn't be given its name ({e})")),
    }
}

/// Readies the new library made at `part`, open on `conn`, to take the library's name ([`swap_in`]): read and written
/// in WAL mode as the library is, so the page's readers never wait for a writer; closed, with no log left beside it
/// (it would be read into the library as if it were its own); and on the disk, so a library with its name is whole even
/// after a power cut.
pub(super) fn finish_part(conn: Connection, part: &Path) -> Result<(), String> {
    conn.pragma_update(None, "journal_mode", "WAL").map_err(|e| e.to_string())?;
    conn.close().map_err(|(_, e)| e.to_string())?;
    if ["-wal", "-shm", "-journal"].iter().any(|end| side(part, end).exists()) {
        return Err("its log was left beside it".into());
    }
    fs::OpenOptions::new().write(true).open(part).and_then(|f| f.sync_all()).map_err(|e| e.to_string())
}

/// Puts the new library made at `part` ([`finish_part`]) in the place of the damaged one at `copy`, which is set aside
/// as `aside` first ([`set_aside_as`]), and put back when the new one can't be given its name. Why not, when it wasn't
/// (`what` names the new one there).
pub(super) fn swap_in(part: &Path, copy: &Path, aside: &Path, what: &str) -> Result<(), String> {
    // No reader in this app opens the library while it's swapped.
    let _readers_wait = LAYING_OUT.lock().unwrap_or_else(PoisonError::into_inner);
    set_aside_as(copy, aside)?;
    if let Err(e) = fs::rename(part, copy) {
        let undone = match put_back_from(copy, aside) {
            Ok(()) => String::new(),
            Err(e) => format!("; the damaged one is left as {} ({e})", aside.display()),
        };
        return Err(format!("{what} couldn't be given its name ({e}){undone}"));
    }
    Ok(())
}

/// Sets the damaged library at `copy` aside ([`damaged_of`]), with its log, so the two can still be opened together;
/// the one set aside before goes. Not when something else has it open: then nothing is moved. A log left beside the
/// copy put in its place would be read into it as if it were its own, so nothing is put there unless it has gone.
pub(super) fn set_aside(copy: &Path) -> Result<(), String> {
    set_aside_as(copy, &damaged_of(copy))
}

/// [`set_aside`], as `aside`. One kept for good there ([`kept_of`]) is never replaced: then nothing is moved.
pub(super) fn set_aside_as(copy: &Path, aside: &Path) -> Result<(), String> {
    let held = || "the damaged library couldn't be set aside: something else has it open".to_owned();
    let busy =
        |e: std::io::Error| format!("the damaged library couldn't be set aside ({e}): something else may have it open");
    if held_elsewhere(copy) {
        return Err(held());
    }
    let damaged = aside;
    if damaged == kept_of(copy) {
        if let Some(there) = database_there(damaged) {
            return Err(format!("{} is there, kept for good, so it isn't replaced", there.display()));
        }
    } else {
        remove_database(damaged).map_err(busy)?;
    }
    // Windows won't remove the shared-memory file of a library something has open: then nothing is moved.
    for end in ["-shm", "-journal"] {
        match fs::remove_file(side(copy, end)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(busy(e)),
            _ => {}
        }
    }
    // The library first, then its log: if the log can't follow, the library is put back, so the two stay together.
    match fs::rename(copy, damaged) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(busy(e)),
        _ => {}
    }
    match fs::rename(side(copy, "-wal"), side(damaged, "-wal")) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            let _ = fs::rename(damaged, copy);
            return Err(busy(e));
        }
        _ => {}
    }
    if ["", "-wal", "-shm", "-journal"].iter().any(|end| side(copy, end).exists()) {
        let _ = put_back_from(copy, damaged);
        return Err(held());
    }
    // One set aside for a week goes a week from now: a file keeps the time it was last written when it's moved.
    let _ = fs::File::options().write(true).open(damaged).and_then(|f| f.set_modified(SystemTime::now()));
    Ok(())
}

/// The first of the files of a database at `path` that's there (the database, or a part SQLite keeps beside it).
pub(super) fn database_there(path: &Path) -> Option<PathBuf> {
    ["", "-wal", "-shm", "-journal"].iter().map(|end| side(path, end)).find(|p| !matches!(p.try_exists(), Ok(false)))
}

/// Undoes [`set_aside`]: the damaged library at `copy` given its name again, then its log (never a log without its
/// library, which would be read into another put there).
pub(super) fn put_back(copy: &Path) -> std::io::Result<()> {
    put_back_from(copy, &damaged_of(copy))
}

/// [`put_back`], from where [`set_aside_as`] set it aside: `aside`. Never beside a log that isn't its own; if its own
/// can't follow it, it goes back where it was, so the two stay together.
pub(super) fn put_back_from(copy: &Path, aside: &Path) -> std::io::Result<()> {
    if copy.exists() || side(copy, "-wal").exists() {
        return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "something else is there in its place"));
    }
    fs::rename(aside, copy)?;
    match fs::rename(side(aside, "-wal"), side(copy, "-wal")) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            let _ = fs::rename(copy, aside);
            Err(e)
        }
        _ => Ok(()),
    }
}

/// Sets the damaged backup copy of the library at `copy` aside ([`backup_damaged_of`]), with any log SQLite left
/// beside it, once what can still be read of it is in the library mended from it: no backup copy is ever made over it
/// (the whole-app review of 19 Nov, SN-3). The one set aside before goes, but never while a damaged library kept for
/// good is there ([`kept_of`]): it may hold some of what that one couldn't give, so nothing is set aside meanwhile.
/// Nothing to do when there's none. Its time is set to now, as it goes a week from now. Why not, when it couldn't be:
/// then it's where it was.
pub(super) fn set_backup_aside(copy: &Path) -> Result<(), String> {
    let (backup, aside) = (backup_of(copy), backup_damaged_of(copy));
    let failed = |e: std::io::Error| format!("its damaged backup copy couldn't be set aside ({e})");
    if database_there(&backup).is_none() {
        return Ok(());
    }
    if let (Some(before), Some(kept)) = (database_there(&aside), database_there(&kept_of(copy))) {
        return Err(format!(
            "its damaged backup copy couldn't be set aside: the one set aside before ({}) is kept while the damaged \
             library kept for good ({}) is there, so put those somewhere safe",
            before.display(),
            kept.display()
        ));
    }
    remove_database(&aside).map_err(failed)?;
    for end in ["-shm", "-journal"] {
        match fs::remove_file(side(&backup, end)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(failed(e)),
            _ => {}
        }
    }
    // The copy first, then its log: if the log can't follow, the copy is put back, so the two stay together.
    match fs::rename(&backup, &aside) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(failed(e)),
        _ => {}
    }
    match fs::rename(side(&backup, "-wal"), side(&aside, "-wal")) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            let _ = fs::rename(&aside, &backup);
            return Err(failed(e));
        }
        _ => {}
    }
    let _ = fs::File::options().write(true).open(&aside).and_then(|f| f.set_modified(SystemTime::now()));
    Ok(())
}

/// Whether something else (another copy of the app) has the library at `copy` open. SQLite lets go of the library's
/// log and shared-memory file as the last connection to it closes, having put what the log held into it: opened and
/// closed here, they're gone unless something else holds it. Windows won't move or remove a file that's open anyway;
/// elsewhere one can be, so this is how it's told there. A library so damaged SQLite can't open it at all counts as
/// held by nothing (its files are as they are).
pub(super) fn held_elsewhere(copy: &Path) -> bool {
    let Ok(conn) =
        Connection::open_with_flags(copy, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX)
    else {
        return false;
    };
    // Read, so SQLite opens it and its log: one that's never read is closed without touching them. Busy is held. An
    // empty file (nothing in it, not even its list of tables) counts as held by nothing too: SQLite keeps no log for
    // it, so a shared-memory file a reader left beside the library before stays there, held or not.
    match conn.query_row("PRAGMA schema_version", [], |r| r.get::<_, i64>(0)) {
        Err(e) if is_damage(&e) => return false,
        Err(_) => return true,
        Ok(0) => return false,
        Ok(_) => {}
    }
    if conn.close().is_err() {
        return true;
    }
    ["-wal", "-shm"].iter().any(|end| side(copy, end).exists())
}
