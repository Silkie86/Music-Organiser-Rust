//! The Rust app's own sound library: `sounds-rust.db`, made once from the C# app's `sounds.db` and kept beside it.
//!
//! The C# app's file keeps each sound's analysis as text, both of its fingerprints in the same row as its name and
//! stars, and the duplicate finder's prints beside them: 4.6 GB for Solomon's 453,885 sounds, of which the Sounds
//! page had to read 3.7 GB before it could list anything. This file holds the same facts, stored compactly:
//!
//! - Each sound's row holds what the list shows, with the analysis as numbers (the chords' times too, which text
//!   doesn't always bring back to the last bit). The 12 notes' chroma is packed as thousandths, the way the C#
//!   analyser rounds them, only when each comes back as the very same number; otherwise it's kept as it is.
//! - The waveform, packed the same way, is in a table of its own (`sound_waves`), so what reads the rows without it
//!   reads less.
//! - The two fingerprints are in tables of their own, read after the list. The analyser's are kept as they are; CLAP's
//!   are kept as CLAP's index holds them, made unit length and at half precision ([`crate::index::held`]), so "sounds
//!   like this" finds the very same sounds from half the room. Each sound's row says how many numbers its CLAP has
//!   (`clap_numbers`), so how many sounds CLAP has heard is told without reading them.
//! - Tags, plays, galaxy places, the copies the duplicate finder found, hidden copies and settings are copied table
//!   for table as the C# app made them, so the same questions get the same answers from either file.
//! - Of the duplicate finder's prints only the hash of exact copies is kept: the rest is for the C# app's own finder.
//!
//! **Made once.** The C# app is retired (Solomon, 2026-11-05). The Sounds page makes this file from `sounds.db` on
//! the pass that reads `sounds.db` for the page ([`sync_reading`]), and from then on it *is* the sound library: its
//! `copy_info` notes since when ([`own_since`]), and `sounds.db` is never read into it again. `sounds.db` itself is
//! only ever read. The file is made again from `sounds.db` only when it's found damaged while `sounds.db` is still
//! there. Nothing the Rust app has written lives only in it yet, so that loses nothing; it has to change once the app
//! writes plays, stars or scans into it. A copy made while the C# app still ran that's behind `sounds.db` is made
//! afresh from it one last time ([`last_sync`]): nothing follows the C# app's saves one by one any more, so the copy
//! keeps no hash of each row.
//!
//! **Laid out anew.** [`FORMAT`] goes up whenever the layout changes. A copy of the format before ([`FORMERLY`]: the
//! waveform in each sound's row, CLAP's numbers as CLAP gave them, a hash of each row) is read as it is
//! ([`crate::SoundsDb`] reads both), and once the page has what it needs from it, copied into this format from itself,
//! checked, and only then given its name ([`lay_out`]): `sounds.db` may have gone by then. Until that can be done
//! (something else holding it, say), it's read as it is.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use chrono::NaiveDateTime;
use rusqlite::types::{ToSqlOutput, Value, ValueRef};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Row, Transaction, TransactionBehavior, params, params_from_iter,
};

use crate::db::{Reading, SOUND_COLUMNS, int_at, real_at, sound_at, sound_columns, text_at, word_at};
use crate::index::{held, held_bytes};
use crate::{ChordSpan, Everything, Peaks, Sound, SoundAnalysis, SoundsDb, SoundsError};

/// The copy's name, beside `sounds.db`.
pub const FILE_NAME: &str = "sounds-rust.db";

/// The copy's own table: what it was made from and how (`format`, `stamp`, and a `hash:` for each table copied whole),
/// and since when it's been the app's own sound library ([`OWN_SINCE`]).
pub(crate) const INFO_TABLE: &str = "copy_info";

/// The key in [`INFO_TABLE`] that says since when (Unix seconds) the copy has been the app's own sound library, no
/// longer following `sounds.db`.
const OWN_SINCE: &str = "own_since";

/// Goes up whenever the copy's layout or what goes into it changes. A copy of the format before ([`FORMERLY`]) is laid
/// out anew from itself ([`lay_out`]), and read as it is until it has been; one of an older format still is made
/// afresh from `sounds.db`; and one a newer version of the app made is left alone (an older version can't read it).
/// Before this goes up again, the format before must be one this version lays out anew: `sounds.db` may have gone.
pub(crate) const FORMAT: i64 = 2;

/// The format before [`FORMAT`]: the waveform in each sound's row, CLAP's numbers as CLAP gave them, and a hash of
/// each row, by which the copy followed the C# app's saves (`sound_hashes`).
pub const FORMERLY: i64 = 1;

/// Whether this version reads a copy laid out in `format` ([`FORMAT`] or [`FORMERLY`]).
pub(crate) fn readable(format: i64) -> bool {
    format == FORMAT || format == FORMERLY
}

/// SQLite's pages are 4 KB unless asked otherwise, and a CLAP fingerprint (512 numbers, 2 KB) can't share one with
/// another, so each took a whole page, half of it empty. Fifteen share a 32 KB page.
const PAGE_SIZE: i64 = 32_768;

/// The analysis's columns in the copy's `sounds` table, in the order [`analysis_values`] gives them and
/// [`analysis_at`] reads them, which takes the waveform straight after them. One list for the table, the writing and
/// the reading, so they can't drift apart. In the format before ([`FORMERLY`]) the waveform came after them in the
/// same row.
macro_rules! analysis_columns {
    () => {
        "seconds, sample_rate, channels, peak_db, rms_db, attack_ms, decay_ms, brightness, noisiness, width, is_loop, \
         bpm, bpm_source, pitch, pitch_conf, key_tonic, key_minor, key_conf, key_source, chord, chords, chroma, \
         character"
    };
}
pub(crate) use analysis_columns;

/// How many columns [`analysis_columns`] lists.
const ANALYSIS_COLUMNS: usize = 23;

/// A sound's row's columns, all but how many numbers its CLAP has: the same in both formats, so a copy of the format
/// before is copied column for column ([`lay_out_anew`]).
macro_rules! row_columns {
    () => {
        concat!(
            "id, path, size, mtime, error, name_type, user_type, type, type_source, type_conf, favourite, rating, \
             plays, last_played, ",
            analysis_columns!()
        )
    };
}

/// The copy's tables. The C# app's tables that are copied whole (see [`WHOLE`]) are made as the C# app made them.
///
/// The analysis's columns have no type, so SQLite keeps each number exactly as it's given: a REAL column would store
/// -0.0 as 0, and the C# analyser's rounding gives -0.0 (a peak of -0.04 dB rounds to -0.0).
///
/// `clap_numbers`: how many numbers the sound's CLAP fingerprint has; none when it has none, 0 when what `sounds.db`
/// held for it isn't one (a damaged row). The fingerprint itself, when it's whole floats, is in `sound_clap` as CLAP's
/// index holds it ([`held_bytes`] of [`held`]). `sound_features` holds the analyser's as `sounds.db` held them.
const SCHEMA: &str = concat!(
    "CREATE TABLE copy_info (key TEXT PRIMARY KEY, value);
     CREATE TABLE sounds (
         id INTEGER PRIMARY KEY,
         path TEXT NOT NULL COLLATE NOCASE,
         size INTEGER, mtime INTEGER, error TEXT,
         name_type TEXT, user_type TEXT, type TEXT, type_source TEXT, type_conf REAL,
         favourite INTEGER DEFAULT 0, rating INTEGER DEFAULT 0, plays INTEGER DEFAULT 0, last_played INTEGER,
         ",
    analysis_columns!(),
    ",
         clap_numbers INTEGER
     );
     CREATE TABLE sound_waves (sound_id INTEGER PRIMARY KEY, v BLOB NOT NULL);
     CREATE TABLE sound_features (sound_id INTEGER PRIMARY KEY, v);
     CREATE TABLE sound_clap (sound_id INTEGER PRIMARY KEY, v BLOB NOT NULL);"
);

/// Made once the rows are in, which is quicker than keeping them up to date row by row. `sounds_clap` holds only the
/// sounds CLAP has heard: counting them reads it alone (a few MB), not the rows.
const INDEXES: &str = "CREATE INDEX sounds_path ON sounds(path);
     CREATE INDEX sounds_clap ON sounds(clap_numbers) WHERE clap_numbers IS NOT NULL;";

/// Each sound in `sounds.db`, every column, in the order the rows are stored (never `ORDER BY`: see
/// [`SoundsDb::everything`]). The page's read takes the first columns, in its own order, so one pass over the file
/// can serve both ([`sync_reading`]).
const OLD_SOUNDS: &str = concat!("SELECT ", sound_columns!(), ", features, clap, size, last_played FROM sounds");

/// Where [`OLD_SOUNDS`] has the analysis, the two fingerprints and the two columns the page doesn't read.
const OLD_ANALYSIS: usize = 4;
const OLD_FEATURES: usize = SOUND_COLUMNS;
const OLD_CLAP: usize = SOUND_COLUMNS + 1;
const OLD_SIZE: usize = SOUND_COLUMNS + 2;
const OLD_LAST_PLAYED: usize = SOUND_COLUMNS + 3;

/// The columns of [`OLD_SOUNDS`] that are copied as they are, in the order [`INSERT_SOUND`] takes them after the id.
const AS_THEY_ARE: [usize; 13] = [1, OLD_SIZE, 2, 3, 5, 6, 7, 8, 9, 10, 11, 12, OLD_LAST_PLAYED];

const INSERT_SOUND: &str = concat!(
    "INSERT INTO sounds (",
    row_columns!(),
    ", clap_numbers) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19,
         ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32, ?33, ?34, ?35, ?36, ?37, ?38)"
);

/// The C# app's tables that are copied whole, rows, row numbers and indexes, as the C# app made them: the same
/// queries then read them the same way, in the same order, from either file.
const WHOLE: [&str; 6] = ["settings", "sound_tags", "sound_events", "sound_maps", "sound_matches", "sound_hidden"];

/// Of the duplicate finder's prints, the hash of each sound that has one: all that's read of them (exact copies).
/// From the index the C# app keeps of the hashes, so not one of the prints themselves is read.
const OLD_PRINTS: &str = "SELECT sound_id, exact FROM sound_prints WHERE exact IS NOT NULL";

const PRINTS: &str = "CREATE TABLE sound_prints (sound_id INTEGER PRIMARY KEY, exact BLOB);
    CREATE INDEX ix_sound_prints_exact ON sound_prints(exact);";

/// How long a save to the copy waits for another one (a second copy of the app bringing it up to date too).
const WAIT: Duration = Duration::from_secs(5);

/// A half-made copy whose file hasn't changed for this long was left by an app that stopped while making it.
const STALE: Duration = Duration::from_secs(10 * 60);

/// What's left free on the disk once a copy is made, at least: making one never fills the disk (Solomon's C: was
/// 98.8% full, 2026-11-03).
const ROOM_LEFT: u64 = 1 << 30;

/// One [`sync`] (or laying out anew) at a time in this app: two at once would make the same half-made copy.
static SYNCING: Mutex<()> = Mutex::new(());

/// Held while the copy is swapped for a new one (from when it's last seen that nothing has it open, to when the new
/// one has its name: [`lay_out_anew`], [`make`]), and taken by every [`SoundsDb`] as it opens (it waits meanwhile, a
/// moment): a reader that opened the copy then would hold it through the swap, on a Mac or Linux reading the old file
/// with SQLite's files of the new one beside it.
pub(crate) static LAYING_OUT: Mutex<()> = Mutex::new(());

/// Why making or updating the copy failed, in words, and whether SQLite found a file damaged on the way (the copy or
/// `sounds.db`: it doesn't say which).
struct Failed {
    why: String,
    damaged: bool,
}

impl Failed {
    fn new(why: impl Into<String>) -> Failed {
        Failed { why: why.into(), damaged: false }
    }
}

impl From<rusqlite::Error> for Failed {
    fn from(e: rusqlite::Error) -> Failed {
        Failed { damaged: crate::db::is_damage(&e), why: e.to_string() }
    }
}

impl From<std::io::Error> for Failed {
    fn from(e: std::io::Error) -> Failed {
        Failed::new(e.to_string())
    }
}

/// What [`sync`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Synced {
    /// The copy was made afresh: there was none, one of an older format, or one behind `sounds.db`. Otherwise it was
    /// up to date with `sounds.db` already, and nothing was made.
    pub made: bool,
    /// How many sounds it was made with (none when nothing was made).
    pub sounds: usize,
    /// This pass made the copy the app's own sound library ([`own_since`]): `sounds.db` won't be read into it again.
    pub adopted: bool,
}

impl Synced {
    /// Whether the page's list of sounds reads any differently now: only when the copy was made afresh.
    pub fn changed_the_list(&self) -> bool {
        self.made
    }
}

/// Where the copy of the sound library at `sounds_db` is kept: beside it.
pub fn copy_of(sounds_db: &Path) -> PathBuf {
    sounds_db.with_file_name(FILE_NAME)
}

/// Whether `copy` is up to date with `sounds_db`: made by this version of the app from `sounds_db` as it is now, or
/// the app's own sound library already ([`own_since`]), which has nothing more to take from `sounds_db` (nor has any
/// copy once `sounds_db` has gone), and laid out in this version's format. Looks only at the files' sizes and times,
/// SQLite's note of its log ([`SoundsDb::stamp`]) and the copy's own note, and never waits: a copy that's busy just
/// then counts as not up to date.
pub fn in_step(sounds_db: &Path, copy: &Path) -> bool {
    match look_at(copy, Duration::ZERO) {
        Ok(Found::Copy { format: FORMAT, own: Some(_), .. }) => true,
        Ok(Found::Copy { format: FORMAT, stamp: made_from, .. }) => !has_more_for(sounds_db, made_from.as_deref()),
        _ => false,
    }
}

/// Since when (Unix seconds) the copy at `copy` has been the app's own sound library, no longer following
/// `sounds.db`; `None` while it still follows it, or when there's no copy this version can read.
pub fn own_since(copy: &Path) -> Option<i64> {
    match look_at(copy, WAIT) {
        Ok(Found::Copy { format, own, .. }) if readable(format) => own,
        _ => None,
    }
}

/// Brings `copy` up to date with the sound library at `sounds_db`: makes it afresh when there's none, or one that's
/// behind `sounds_db` or of another format than this version's; nothing to do for one that's up to date already.
/// Reads `sounds_db` once, all of it, at one moment, and never changes it.
///
/// A new copy is made under another name, checked, and only then given its own name, so a copy that's there is
/// always whole.
///
/// Nothing but a copy is ever removed or replaced: a database of anything else where the copy goes (or the library
/// itself) is left as it is, and this says so. Nor is one made on a disk without room for it and
/// [`ROOM_LEFT`] to spare. Nor is the app's own sound library ([`own_since`]) made again: that's refused.
pub fn sync(sounds_db: &Path, copy: &Path) -> Result<Synced, SoundsError> {
    run(sounds_db, copy, Job { read_for: None, adopt: false, afresh: false }, &mut None)
}

/// [`sync`] one last time, and the copy becomes the app's own sound library ([`own_since`]): for a copy the page
/// read while it was behind `sounds.db`. Nothing to do for one that's the app's own already.
pub fn last_sync(sounds_db: &Path, copy: &Path) -> Result<Synced, SoundsError> {
    if own_since(copy).is_some() {
        return Ok(Synced::default());
    }
    run(sounds_db, copy, Job { read_for: None, adopt: true, afresh: false }, &mut None)
}

/// The Sounds page's first read of the library: the quickest way to its sounds there is.
pub enum PageRead {
    /// From the copy, a fraction of the reading: the sounds now, their fingerprints after, from `db`
    /// ([`SoundsDb::fingerprints`]). `behind`: `sounds.db` has changed since the copy was last brought up to date,
    /// and the copy isn't the app's own library yet, so these are as they were then; [`last_sync`] it, then read it
    /// again. `adopted`: it was up to date, and became the app's own library on this read. One of the format before
    /// ([`SoundsDb::is_formerly`]) is read as it is: [`lay_out`] it once `db` has gone.
    Copy { db: SoundsDb, sounds: Vec<Sound>, behind: bool, adopted: bool },
    /// From `sounds.db`, the fingerprints too, on the one pass that made the copy (`made` says how that went, and
    /// whether it's the app's own library now): there was no copy this version of the app can read.
    Library { everything: Everything, made: Result<Synced, SoundsError> },
}

/// How laying a copy of the format before ([`FORMERLY`]) out anew went ([`lay_out`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaidOut {
    /// Done: how big the copy was before and is now, in bytes (with SQLite's log beside it).
    Done { before: u64, after: u64 },
    /// Not done this time, and why (something else holding it, the disk short of room…): it's left as it was, read as
    /// it is, and laid out anew another time.
    NotNow(String),
    /// SQLite found the copy damaged on the way, and where: it's left as it was. While `sounds.db` is there, the copy
    /// is made again from it ([`remake_reading`]); without, it's read for whatever of it can be.
    Damaged(String),
}

/// The Sounds page's first read of the sound library at `sounds_db` (or, once the copy beside it is the app's own
/// library, of that alone: `sounds_db` may have gone): from the copy whenever there's one to read, even one that's
/// behind (the page shows it at once and makes it afresh after, one last time), else from `sounds_db` while the
/// copy is made on the same pass ([`sync_reading`]). A copy that's up to date becomes the app's own library here.
/// `now` is the page's "now", for what counts as recent.
pub fn read_for_page(sounds_db: &Path, now: NaiveDateTime) -> Result<PageRead, SoundsError> {
    let copy = copy_of(sounds_db);
    // One look at the copy's own note, waiting its turn: a copy busy just then isn't taken for one that's behind.
    // Looked at before reading, so a save the C# app makes while this reads counts as one next time.
    let (own, behind) = match look_at(&copy, WAIT) {
        Ok(Found::Copy { format, own: Some(_), .. }) if readable(format) => (true, false),
        Ok(Found::Copy { format, stamp: made_from, .. }) if readable(format) => {
            (false, has_more_for(sounds_db, made_from.as_deref()))
        }
        _ => (false, true),
    };
    // The app's own library is never left for `sounds.db` when it can't be read just now: only when it's damaged
    // (made again from `sounds.db`, while that's still there).
    let mut damaged = false;
    match SoundsDb::open_read_only(&copy) {
        Ok(db) if db.is_copy() => match db.all(now) {
            Ok(sounds) => {
                // A copy left half made by an app closed while it made one goes now, not only the next time one is
                // made: that may be never, and it's most of the copy's size (on a disk that was nearly full).
                remove_parts(&copy, false);
                // Up to date: it's the app's own library from now on. If that can't be noted just now (another
                // program holding the copy), the next start tries again; meanwhile nothing's different.
                let adopted = !own && !behind && adopt(sounds_db, &copy).unwrap_or(false);
                return Ok(PageRead::Copy { db, sounds, behind, adopted });
            }
            Err(e) if e.damaged() => damaged = true,
            Err(e) if own => return Err(e),
            Err(_) => {}
        },
        Err(e) if own => return Err(e),
        _ => {}
    }
    // None, or none this version can read: made (or made afresh) on the page's own read. One damaged inside is made
    // again even when it's the app's own.
    let (everything, made) =
        if damaged { remake_reading(sounds_db, &copy, now)? } else { sync_reading(sounds_db, &copy, now)? };
    Ok(PageRead::Library { everything, made })
}

/// Whether `sounds_db` has anything for a copy made from it as it looked at `made_from` (its [`SoundsDb::stamp`]
/// then): not when it looks the same now, nor when it has gone (archived, or left in the old data folder when it
/// moved: the copy is all there is).
fn has_more_for(sounds_db: &Path, made_from: Option<&str>) -> bool {
    !library_gone(sounds_db) && made_from != Some(SoundsDb::stamp(sounds_db).as_str())
}

/// Lays the copy beside `sounds_db` out anew in this version's format, from itself, when it's of the format before
/// ([`FORMERLY`]) and has nothing more to take from `sounds_db` (one that's behind is made afresh by [`last_sync`]);
/// `None` when there's nothing to do (and `starting` isn't called). For the Sounds page, once it's done with the copy
/// it read ([`read_for_page`]): one something has open is left for another time. `starting` is called as the work
/// starts, once it's sure it can: it takes a while for a big library (it reads the copy a few times, and writes most
/// of its size), and never waits for anything else.
pub fn lay_out(sounds_db: &Path, starting: impl FnOnce()) -> Option<LaidOut> {
    let copy = copy_of(sounds_db);
    match look_at(&copy, WAIT) {
        Ok(Found::Copy { format: FORMERLY, own, stamp })
            if own.is_some() || !has_more_for(sounds_db, stamp.as_deref()) => {}
        Ok(_) => return None,
        Err(why) => return Some(LaidOut::NotNow(format!("it couldn't be read just now ({why})"))),
    }
    match lay_out_anew(&copy, starting) {
        Relaid::Done(done) => Some(done),
        Relaid::Already => None,
        Relaid::NotNow(why) => Some(LaidOut::NotNow(why)),
        Relaid::Damaged(why) => Some(LaidOut::Damaged(why)),
    }
}

/// Notes that the copy at `copy`, up to date with `sounds_db` (or the only library left, with `sounds_db` gone), is
/// the app's own sound library from now on. Whether this made it so: not when it was already, or when it isn't a
/// whole copy this version reads, up to date with `sounds_db` as it is now (it's looked at again here, under the same
/// lock as every update).
fn adopt(sounds_db: &Path, copy: &Path) -> Result<bool, String> {
    let _one_at_a_time = SYNCING.lock().unwrap_or_else(PoisonError::into_inner);
    // `None`: `sounds.db` has gone, so the copy is taken as it is.
    let wanted = (!library_gone(sounds_db)).then(|| SoundsDb::stamp(sounds_db));
    match look_at(copy, WAIT)? {
        Found::Copy { format, own: None, stamp } if readable(format) && (wanted.is_none() || stamp == wanted) => {}
        _ => return Ok(false),
    }
    note_own(copy, wanted.as_deref(), unix_now()).map_err(|e| e.to_string())
}

/// Notes in the copy at `copy` that it's been the app's own sound library since `since` (Unix seconds), when it was
/// made from `sounds.db` as it looked at `wanted` (or whatever it was made from, with `None`). Whether it noted it: not
/// when it was the app's own already, nor when it was made from another `sounds.db` (looked at again inside the
/// write: another copy of the app may have made it again meanwhile). Only under [`SYNCING`].
fn note_own(copy: &Path, wanted: Option<&str>, since: i64) -> rusqlite::Result<bool> {
    let conn = open_for_writing(copy, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    conn.busy_timeout(WAIT)?;
    let tx = Transaction::new_unchecked(&conn, TransactionBehavior::Immediate)?;
    let made_from: Option<String> =
        tx.query_row("SELECT value FROM copy_info WHERE key = 'stamp'", [], |r| r.get(0)).optional()?.flatten();
    if wanted.is_some_and(|wanted| made_from.as_deref() != Some(wanted)) {
        return Ok(false);
    }
    let added =
        tx.execute("INSERT OR IGNORE INTO copy_info (key, value) VALUES (?1, ?2)", params![OWN_SINCE, since])?;
    tx.commit()?;
    Ok(added == 1)
}

/// How [`lay_out_anew`] went.
enum Relaid {
    Done(LaidOut),
    /// It's of this format already: another read laid it out anew meanwhile.
    Already,
    /// Not done this time, and why: it's left as it was.
    NotNow(String),
    /// SQLite found the copy damaged on the way, and where: it's left as it was.
    Damaged(String),
}

/// The tables a copy of the format before ([`FORMERLY`]) may hold, besides those copied whole from `sounds.db`
/// ([`WHOLE`]): one that holds any other is left as it is, so nothing in it is lost. (SQLite's own numbers for the
/// query planner, `sqlite_stat…`, are made again whenever they're wanted, so they're left behind.)
const FORMERLY_HELD: [&str; 7] =
    ["copy_info", "sounds", "sound_features", "sound_clap", "sound_hashes", "sound_prints", "sqlite_sequence"];

/// Lays the copy at `copy`, of the format before ([`FORMERLY`]), out anew in this version's format, from itself:
/// copied into a new file beside it in one read of it, every row of every table checked against it, the new file
/// checked whole and put on the disk, and only then given the copy's name, and only when nothing else has the copy
/// open. Every fact is kept as it was (what it was made from, and since when it's the app's own library), CLAP's
/// fingerprints as CLAP's index holds them ([`held`]), and the row hashes by which it followed `sounds.db` are left
/// behind. A fingerprint of a sound the copy doesn't have (it never kept one) goes too. `starting` is called once
/// it's sure it can start.
fn lay_out_anew(copy: &Path, starting: impl FnOnce()) -> Relaid {
    let _one_at_a_time = SYNCING.lock().unwrap_or_else(PoisonError::into_inner);
    // Looked at again under the lock: another read may have laid it out anew meanwhile.
    match look_at(copy, WAIT) {
        Ok(Found::Copy { format: FORMERLY, .. }) => {}
        Ok(Found::Copy { format: FORMAT, .. }) => return Relaid::Already,
        Ok(_) => return Relaid::NotNow("it isn't a copy of the format before any more".into()),
        Err(why) => return Relaid::NotNow(format!("it couldn't be read just now ({why})")),
    }
    // Something holding it now would still hold it at the end: not worth the work meanwhile.
    if let Err(why) = let_go_of(copy) {
        return Relaid::NotNow(why);
    }
    // How it looks now, with nothing holding it: still so at the end, nothing wrote to it meanwhile.
    let stamp = SoundsDb::stamp(copy);
    remove_parts(copy, true);
    let size = |path: &Path| -> u64 {
        ["", "-wal"].iter().filter_map(|end| fs::metadata(side(path, end)).ok()).map(|m| m.len()).sum()
    };
    let before = size(copy);
    // The new file is smaller than the copy (its fingerprints take half the room): the copy's size is room enough.
    if let Ok(free) = fs4::available_space(dir_of(copy))
        && let Err(why) = room_for(free, before, "laying it out anew")
    {
        return Relaid::NotNow(why);
    }
    let part = part_of(copy);
    if let Err(e) = remove_database(&part) {
        return Relaid::NotNow(format!("a half-made copy couldn't be removed ({e})"));
    }
    let _part = Part(part.clone());
    starting();
    match fill_anew(&part, copy) {
        Ok(()) => {}
        Err(Failed { why, damaged: true }) => return Relaid::Damaged(why),
        Err(Failed { why, .. }) => return Relaid::NotNow(why),
    }
    // No reader in this app opens the copy from here until the new one has its name: one that did would hold the old
    // one through the swap.
    let _readers_wait = LAYING_OUT.lock().unwrap_or_else(PoisonError::into_inner);
    // Something that has it open now (a page reading it, or another copy of the app) would go on reading the old file,
    // with SQLite's files for the new one beside it.
    if let Err(why) = let_go_of(copy) {
        return Relaid::NotNow(why);
    }
    // Another copy of the app may have written to it meanwhile (this one writes only under the same lock): the new
    // file wouldn't have that.
    if SoundsDb::stamp(copy) != stamp {
        return Relaid::NotNow("something wrote to it meanwhile (another copy of the app)".into());
    }
    // Windows doesn't let a file another program has open be replaced, so one opened since isn't either.
    if let Err(e) = fs::rename(&part, copy) {
        return Relaid::NotNow(format!("the new copy couldn't be given its name ({e})"));
    }
    Relaid::Done(LaidOut::Done { before, after: size(copy) })
}

/// A file being made beside the copy: removed, with SQLite's files beside it, on every way out (there's nothing left
/// to remove once it has the copy's name).
struct Part(PathBuf);

impl Drop for Part {
    fn drop(&mut self) {
        let _ = remove_database(&self.0);
    }
}

/// Fills the new file at `part` from the copy at `copy`, of the format before, in one read of it; checks it against
/// the copy, row by row, then whole; and puts it on the disk.
fn fill_anew(part: &Path, copy: &Path) -> Result<(), Failed> {
    let Some(old) = copy.to_str() else { return Err(Failed::new("its folder's name can't be handed to SQLite")) };
    // Named so it's opened only if it's there: gone meanwhile, an empty file would be made in its place.
    let old = existing_only(copy).unwrap_or_else(|| old.to_owned());
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_CREATE
        | OpenFlags::SQLITE_OPEN_URI
        | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = open_for_writing(part, flags)?;
    conn.busy_timeout(WAIT)?;
    // A file being made needs no journal: if this stops half way, the file is thrown away.
    conn.pragma_update(Some("main"), "page_size", PAGE_SIZE)?;
    conn.pragma_update(Some("main"), "journal_mode", "OFF")?;
    conn.pragma_update(Some("main"), "synchronous", "OFF")?;
    conn.execute_batch(SCHEMA)?;
    // Opened for writing (nothing is written to it), so that when this connection lets go of it, it's the one that
    // tidies SQLite's files beside it away if it's the last.
    conn.execute("ATTACH DATABASE ?1 AS old", [&old])?;
    // One read of the copy from here to the end: what's copied and what it's checked against are the same.
    conn.execute_batch("BEGIN")?;
    let format: Option<i64> = conn
        .query_row("SELECT value FROM old.copy_info WHERE key = 'format'", [], |r| {
            Ok(match r.get_ref(0)? {
                ValueRef::Integer(n) => Some(n),
                _ => None,
            })
        })
        .optional()?
        .flatten();
    if format != Some(FORMERLY) {
        return Err(Failed::new("it isn't a copy of the format before"));
    }
    let tables: Vec<String> = {
        let mut st = conn.prepare("SELECT name FROM old.sqlite_master WHERE type = 'table'")?;
        st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
    };
    let has = |name: &str| tables.iter().any(|t| t == name);
    if let Some(unknown) = tables
        .iter()
        .find(|t| !FORMERLY_HELD.contains(&t.as_str()) && !WHOLE.contains(&t.as_str()) && !t.starts_with("sqlite_stat"))
    {
        return Err(Failed::new(format!(
            "it holds a table this version doesn't know ({unknown}), so it's left as it is"
        )));
    }
    if let Some(missing) = ["copy_info", "sounds", "sound_features", "sound_clap"].into_iter().find(|t| !has(t)) {
        return Err(Failed::new(format!("it has no {missing} table")));
    }
    let whole: Vec<&str> = WHOLE.into_iter().chain(["sound_prints"]).filter(|t| has(t)).collect();
    // The disk the new file is written on, watched meanwhile: other programs fill it too.
    let room = Room::new(dir_of(part));
    let claps = copy_anew(&conn, &whole, has("sqlite_sequence"), &room)?;
    room.look()?;
    conn.execute_batch(INDEXES)?;
    check_anew(&conn, &whole, &claps)?;
    conn.execute("INSERT INTO main.copy_info (key, value) VALUES ('format', ?1)", [FORMAT])?;
    conn.execute_batch("COMMIT")?;
    let check: String = conn.query_row("PRAGMA main.integrity_check(1)", [], |r| r.get(0))?;
    if check != "ok" {
        return Err(Failed::new(format!("the new copy didn't read back whole ({check})")));
    }
    // From now on it's written to while the page may be reading it: in WAL mode readers never wait for a writer.
    conn.pragma_update(Some("main"), "journal_mode", "WAL")?;
    conn.execute_batch("DETACH DATABASE old")?;
    conn.close().map_err(|(_, e)| e)?;
    // On the disk before it has its name, so a copy with its name is whole even after a power cut.
    fs::OpenOptions::new().write(true).open(part)?.sync_all()?;
    Ok(())
}

/// What laying a copy out anew made of one sound's CLAP fingerprint, kept to check the new file against: how many
/// numbers it has, and the hash of the numbers as CLAP's index holds them, when they're whole floats.
struct ClapMade {
    id: i64,
    numbers: i64,
    held: Option<i64>,
}

/// How many rows are copied at a time when a copy is laid out anew, between two looks at the room left on the disk
/// ([`Room`]): tens of MB.
const ROWS_AT_A_TIME: i64 = 16_384;

/// Copies every row of the copy attached as `old` into the new file's tables (`main`): each sound's row column for
/// column, with how many numbers its CLAP fingerprint has ([`clap_numbers`]); its waveform into a row of its own; its
/// fingerprints, CLAP's as CLAP's index holds them; the `whole` tables as they are, row numbers and indexes too (and,
/// with `sequence`, SQLite's count for a table whose rows count up); and the copy's own notes but for its format and
/// the row hashes. Watches the disk's room on the way. What it made of each CLAP fingerprint.
fn copy_anew(conn: &Connection, whole: &[&str], sequence: bool, room: &Room<'_>) -> Result<Vec<ClapMade>, Failed> {
    // How many numbers each CLAP fingerprint has, as `clap_numbers` counts them (checked against it after).
    let sounds = conn.prepare(concat!(
        "INSERT INTO main.sounds (",
        row_columns!(),
        ", clap_numbers) SELECT ",
        row_columns!(),
        ", CASE WHEN c.v IS NULL THEN NULL
                WHEN typeof(c.v) = 'blob' AND length(c.v) > 0 AND length(c.v) % 4 = 0 THEN length(c.v) / 4
                ELSE 0 END
         FROM old.sounds LEFT JOIN old.sound_clap c ON c.sound_id = old.sounds.id
         WHERE old.sounds.id >= ?1 ORDER BY old.sounds.id LIMIT ?2"
    ))?;
    let mut waves = conn.prepare(
        "INSERT INTO main.sound_waves (sound_id, v)
             SELECT id, waveform FROM old.sounds WHERE id BETWEEN ?1 AND ?2 AND waveform IS NOT NULL ORDER BY id",
    )?;
    let last_sound = "SELECT MAX(id) FROM main.sounds";
    in_turn(conn, sounds, last_sound, room, |from, to| Ok(waves.execute(params![from, to]).map(|_| ())?))?;
    let features = conn.prepare(
        "INSERT INTO main.sound_features (sound_id, v)
             SELECT sound_id, v FROM old.sound_features WHERE sound_id >= ?1 ORDER BY sound_id LIMIT ?2",
    )?;
    in_turn(conn, features, "SELECT MAX(sound_id) FROM main.sound_features", room, |_, _| Ok(()))?;
    conn.execute_batch(
        "INSERT INTO main.copy_info (key, value)
             SELECT key, value FROM old.copy_info WHERE key <> 'format' AND substr(key, 1, 5) <> 'hash:';",
    )?;
    // A fingerprint of a sound the copy doesn't have goes.
    let mut made = Vec::new();
    let mut st = conn.prepare(
        "SELECT c.sound_id, c.v FROM old.sound_clap c
         WHERE c.v IS NOT NULL AND EXISTS (SELECT 1 FROM old.sounds s WHERE s.id = c.sound_id) ORDER BY c.sound_id",
    )?;
    let mut insert = conn.prepare("INSERT INTO main.sound_clap (sound_id, v) VALUES (?1, ?2)")?;
    let mut rows = st.query([])?;
    let mut counted = Room::new(room.dir);
    while let Some(r) = rows.next()? {
        let id: i64 = r.get(0)?;
        let v = r.get_ref(1)?;
        let Some(numbers) = clap_numbers(v) else { continue };
        let held = whole_floats(v).map(|floats| held_bytes(&held(&floats)));
        if let Some(bytes) = &held {
            insert.execute(params![id, bytes])?;
            counted.wrote()?;
        }
        made.push(ClapMade { id, numbers, held: held.map(|bytes| hash_of(ValueRef::Blob(&bytes))) });
    }
    for &table in whole {
        room.look()?;
        let Some(sql) = table_sql(conn, "old", table)? else { continue };
        conn.execute_batch(&sql)?;
        let columns = Columns::of(conn, "old", table, &sql)?.list();
        let name = quoted(table);
        conn.execute_batch(&format!("INSERT INTO main.{name} ({columns}) SELECT {columns} FROM old.{name}"))?;
        for sql in index_sqls(conn, "old", table)? {
            conn.execute_batch(&sql)?;
        }
    }
    // SQLite's own count for a table whose rows count up (AUTOINCREMENT), made with that table and moved on by the
    // rows copied into it: as it was, which may be past the last row there.
    if sequence && table_sql(conn, "main", "sqlite_sequence")?.is_some() {
        conn.execute_batch(
            "DELETE FROM main.sqlite_sequence;
             INSERT INTO main.sqlite_sequence (name, seq) SELECT name, seq FROM old.sqlite_sequence;",
        )?;
    }
    Ok(made)
}

/// Runs `insert`, which copies up to `?2` rows from the key `?1` on, in turn until it copies none, [`ROWS_AT_A_TIME`]
/// at a time, looking at the room left on the disk between each: `last` gives the last key copied so far, and `then`
/// is told the keys each turn copied, from and to.
fn in_turn(
    conn: &Connection,
    mut insert: rusqlite::Statement<'_>,
    last: &str,
    room: &Room<'_>,
    mut then: impl FnMut(i64, i64) -> Result<(), Failed>,
) -> Result<(), Failed> {
    let mut from = Some(i64::MIN);
    while let Some(start) = from {
        if insert.execute(params![start, ROWS_AT_A_TIME])? == 0 {
            break;
        }
        let to: i64 = conn.query_row(last, [], |r| r.get(0))?;
        then(start, to)?;
        room.look()?;
        from = to.checked_add(1);
    }
    Ok(())
}

/// Checks the new file (`main`) against the copy attached as `old`, every row of every table, to the last bit: what
/// [`copy_anew`] copied, and CLAP's fingerprints as it made them (`claps`).
fn check_anew(conn: &Connection, whole: &[&str], claps: &[ClapMade]) -> Result<(), Failed> {
    let compare = |what: &str, ours: &str, theirs: &str| -> Result<(), Failed> {
        same_rows(conn, ours, theirs)?.map_err(|at| Failed::new(format!("{what} didn't come out the same ({at})")))
    };
    compare(
        "the sounds",
        concat!("SELECT ", row_columns!(), " FROM main.sounds ORDER BY id"),
        concat!("SELECT ", row_columns!(), " FROM old.sounds ORDER BY id"),
    )?;
    compare(
        "the waveforms",
        "SELECT sound_id, v FROM main.sound_waves ORDER BY sound_id",
        "SELECT id, waveform FROM old.sounds WHERE waveform IS NOT NULL ORDER BY id",
    )?;
    compare(
        "the features",
        "SELECT sound_id, v FROM main.sound_features ORDER BY sound_id",
        "SELECT sound_id, v FROM old.sound_features ORDER BY sound_id",
    )?;
    compare(
        "the copy's notes",
        "SELECT key, value FROM main.copy_info ORDER BY key",
        "SELECT key, value FROM old.copy_info WHERE key <> 'format' AND substr(key, 1, 5) <> 'hash:' ORDER BY key",
    )?;
    for &table in whole {
        let Some(sql) = table_sql(conn, "old", table)? else { continue };
        let columns = Columns::of(conn, "old", table, &sql)?.list();
        let name = quoted(table);
        compare(table, &format!("SELECT {columns} FROM main.{name}"), &format!("SELECT {columns} FROM old.{name}"))?;
    }
    if table_sql(conn, "old", "sqlite_sequence")?.is_some() {
        let ours = if table_sql(conn, "main", "sqlite_sequence")?.is_some() {
            "SELECT name, seq FROM main.sqlite_sequence ORDER BY name"
        } else {
            "SELECT NULL, NULL WHERE 0"
        };
        compare("SQLite's counts", ours, "SELECT name, seq FROM old.sqlite_sequence ORDER BY name")?;
    }
    // CLAP's: how many numbers each has, in each sound's row, and the numbers as CLAP's index holds them.
    let clap_failed = |at: String| Failed::new(format!("CLAP's fingerprints didn't come out the same ({at})"));
    let mut st = conn.prepare("SELECT id, clap_numbers FROM main.sounds WHERE clap_numbers IS NOT NULL ORDER BY id")?;
    let numbers: Vec<(i64, i64)> =
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
    if !numbers.iter().copied().eq(claps.iter().map(|c| (c.id, c.numbers))) {
        return Err(clap_failed("how many numbers each has".into()));
    }
    let mut st = conn.prepare("SELECT sound_id, v FROM main.sound_clap ORDER BY sound_id")?;
    let mut rows = st.query([])?;
    let mut wanted = claps.iter().filter_map(|c| c.held.map(|hash| (c.id, hash)));
    while let Some(r) = rows.next()? {
        let id: i64 = r.get(0)?;
        if wanted.next() != Some((id, hash_of(r.get_ref(1)?))) {
            return Err(clap_failed(format!("sound {id}")));
        }
    }
    if wanted.next().is_some() {
        return Err(clap_failed("some are missing".into()));
    }
    Ok(())
}

/// Whether `ours` and `theirs` give the same rows, in the same order, each value the same to the last bit (a -0.0
/// isn't a 0.0, and text isn't a blob of the same bytes); else where they first differ.
fn same_rows(conn: &Connection, ours: &str, theirs: &str) -> rusqlite::Result<Result<(), String>> {
    let mut a = conn.prepare(ours)?;
    let mut b = conn.prepare(theirs)?;
    let names: Vec<String> = a.column_names().map(str::to_owned).collect();
    if b.column_count() != names.len() {
        return Ok(Err("not the same columns".into()));
    }
    let (mut ra, mut rb) = (a.query([])?, b.query([])?);
    let mut row = 0u64;
    loop {
        match (ra.next()?, rb.next()?) {
            (None, None) => return Ok(Ok(())),
            (Some(x), Some(y)) => {
                row += 1;
                for (i, name) in names.iter().enumerate() {
                    if !same_value(x.get_ref(i)?, y.get_ref(i)?) {
                        return Ok(Err(format!("row {row}, {name}")));
                    }
                }
            }
            _ => return Ok(Err(format!("one has more rows than the other, after {row}"))),
        }
    }
}

/// Two values the same to the last bit: a number's every bit, and text and blobs byte for byte.
fn same_value(a: ValueRef<'_>, b: ValueRef<'_>) -> bool {
    match (a, b) {
        (ValueRef::Real(x), ValueRef::Real(y)) => x.to_bits() == y.to_bits(),
        (a, b) => a == b,
    }
}

/// Whether there's no `sounds.db` at `sounds_db` at all (not only one that can't be looked at just now).
fn library_gone(sounds_db: &Path) -> bool {
    matches!(fs::metadata(sounds_db), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
}

/// The time now, in Unix seconds.
fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

/// [`sync`], reading the library for the Sounds page on the same pass over `sounds_db`: what
/// [`SoundsDb::everything`] gives for it, as it was at the moment the copy is brought up to (`now` is the page's
/// "now", for what counts as recent). For when the page has no copy to read yet: making one reads all of
/// `sounds_db` anyway, so it isn't read twice.
///
/// The sounds come back however the copy fared (the second part says): only a library that can't be read at all is
/// an error. When the copy couldn't be made or brought up to date before every sound was read, they're read for the
/// page alone.
pub fn sync_reading(
    sounds_db: &Path,
    copy: &Path,
    now: NaiveDateTime,
) -> Result<(Everything, Result<Synced, SoundsError>), SoundsError> {
    read_while(sounds_db, copy, now, false)
}

/// [`sync_reading`], making the copy afresh even when there's one this version can read: for a copy SQLite found
/// damaged inside ([`SoundsError::damaged`]), which bringing up to date would leave damaged. The copy must not be
/// open meanwhile (Windows won't remove an open file).
pub fn remake_reading(
    sounds_db: &Path,
    copy: &Path,
    now: NaiveDateTime,
) -> Result<(Everything, Result<Synced, SoundsError>), SoundsError> {
    read_while(sounds_db, copy, now, true)
}

/// [`sync_reading`] or [`remake_reading`]: the page's own passes, so the copy becomes the app's own library on them
/// (or stays it, from the same moment, when it's made again).
fn read_while(
    sounds_db: &Path,
    copy: &Path,
    now: NaiveDateTime,
    afresh: bool,
) -> Result<(Everything, Result<Synced, SoundsError>), SoundsError> {
    let mut read = None;
    let synced = run(sounds_db, copy, Job { read_for: Some(now), adopt: true, afresh }, &mut read);
    let everything = match read {
        Some(everything) => everything,
        None => SoundsDb::open_read_only(sounds_db)?.everything(now)?,
    };
    Ok((everything, synced))
}

/// What one pass over `sounds.db` is asked to do.
#[derive(Clone, Copy)]
struct Job {
    /// When "now" is for the page's read, taken on the same pass ([`sync_reading`]); `None` when the page isn't
    /// reading.
    read_for: Option<NaiveDateTime>,
    /// The copy becomes the app's own sound library in the same transaction ([`own_since`]).
    adopt: bool,
    /// A copy of this version is made again rather than brought up to date ([`remake_reading`]).
    afresh: bool,
}

/// What [`sync`] does, also reading the library for the page into `read` on the way when the job says when "now" is
/// ([`sync_reading`]). `read` is left as `None` if the pass stopped before every sound was read, or there was nothing
/// to make.
fn run(sounds_db: &Path, copy: &Path, job: Job, read: &mut Option<Everything>) -> Result<Synced, SoundsError> {
    let Job { read_for, afresh, .. } = job;
    let _one_at_a_time = SYNCING.lock().unwrap_or_else(PoisonError::into_inner);
    let failed = |why: String| SoundsError::Copy { path: copy.to_owned(), why };
    if same_file(sounds_db, copy) {
        return Err(failed("that's the library itself".into()));
    }
    // Looked at before reading, so a save the C# app makes while this reads counts as a change next time.
    let stamp = SoundsDb::stamp(sounds_db);
    let old = SoundsDb::open_read_only(sounds_db)?;
    if old.is_copy() {
        return Err(SoundsError::Open {
            path: sounds_db.to_owned(),
            why: "it's the Rust app's copy, not the C# app's library".into(),
        });
    }
    // A copy that can't be read just now (another program holding it) is left as it is, for the next time.
    let found = look_at(copy, WAIT).map_err(|why| failed(format!("it couldn't be read just now ({why})")))?;
    // Half-made copies left by an app that stopped while making one, whatever this pass does.
    remove_parts(copy, true);
    let dir = dir_of(copy);
    // Made again, the app's own library keeps the time it became so.
    let own = match &found {
        Found::Copy { format, own, .. } if readable(*format) => *own,
        _ => None,
    };
    let adopt = job.adopt.then(|| own.unwrap_or_else(unix_now));
    let was_own = own.is_some();
    match found {
        // The app's own sound library takes nothing more from `sounds.db`: only made again, when it's damaged.
        Found::Copy { format, own: Some(_), .. } if readable(format) && !afresh => {
            Err(failed("it's the app's own sound library now, so sounds.db isn't read into it any more".into()))
        }
        // Up to date already: nothing to make (the page reads `sounds.db` for itself). Asked to, it becomes the
        // app's own library.
        Found::Copy { format: FORMAT, stamp: Some(made_from), .. } if !afresh && made_from == stamp => {
            let adopted = match adopt {
                Some(since) => note_own(copy, Some(&stamp), since).map_err(|e| failed(e.to_string()))?,
                None => false,
            };
            Ok(Synced { made: false, sounds: 0, adopted })
        }
        Found::Copy { format, .. } if format > FORMAT => {
            Err(failed(format!("a newer version of the app made it (format {format}), so it's left as it is")))
        }
        // Behind `sounds.db`, older, or (`afresh`) made again: nothing follows each save any more, so it's made anew.
        // One this version reads is kept until the new one is whole: made from a `sounds.db` found damaged on the way,
        // it's what the page still has.
        Found::Nothing | Found::Unreadable | Found::Copy { .. } => {
            let keep = matches!(found, Found::Copy { format, .. } if readable(format)) && !afresh;
            make(&old, copy, keep, Pass { stamp: &stamp, read_for, read, dir, adopt, was_own })
        }
        Found::Foreign => {
            Err(failed("a database that isn't a copy of the library is there, so it's left as it is".into()))
        }
    }
}

/// The folder `copy` is in (and its half-made copies).
fn dir_of(copy: &Path) -> &Path {
    copy.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."))
}

/// Whether `a` and `b` are one file (by two names, or two spellings of one).
fn same_file(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// What one pass over `sounds.db` is for: the copy made from it as it looked at `stamp`, and (when `read_for` says
/// when "now" is) the page's read into `read`. `dir`: the folder the copy is written in, its disk watched for room
/// meanwhile ([`Room`]).
struct Pass<'a> {
    stamp: &'a str,
    read_for: Option<NaiveDateTime>,
    read: &'a mut Option<Everything>,
    dir: &'a Path,
    /// The copy is the app's own sound library once this pass is kept, since this time (Unix seconds).
    adopt: Option<i64>,
    /// It was already, before this pass.
    was_own: bool,
}

/// A new copy, made under a name of its own beside the copy's and then renamed. `keep`: the copy there now stays until
/// the new one is whole (else it goes first, and its room counts as free).
fn make(old: &SoundsDb, copy: &Path, keep: bool, pass: Pass<'_>) -> Result<Synced, SoundsError> {
    let failed = |why: String| SoundsError::Copy { path: copy.to_owned(), why };
    if let (Ok(free), Ok(library)) = (fs4::available_space(pass.dir), fs::metadata(old.path())) {
        // What's there now and goes first counts as free.
        let going: u64 = if keep {
            0
        } else {
            ["", "-wal", "-shm"].iter().filter_map(|end| fs::metadata(side(copy, end)).ok()).map(|m| m.len()).sum()
        };
        room_for(free.saturating_add(going), library.len() / 2, "making it").map_err(failed)?;
    }
    // One that's kept till the new one is whole is replaced only when nothing else has it open: something holding it
    // now would still hold it then, so it isn't worth the work meanwhile.
    if keep {
        let_go_of(copy).map_err(|why| failed(format!("the old copy couldn't be replaced ({why})")))?;
    }
    // How the one that's kept looks now, with nothing holding it: still so when it's replaced, nothing wrote to it
    // meanwhile.
    let kept = keep.then(|| SoundsDb::stamp(copy));
    // A copy of an older format (or a file that can't be read) goes first, with SQLite's files beside it: a log left
    // beside the new one would be read into it as if it were its own.
    if !keep {
        remove_database(copy).map_err(|e| failed(format!("the old copy couldn't be removed ({e})")))?;
    }
    let part = part_of(copy);
    remove_database(&part).map_err(|e| failed(format!("a half-made copy couldn't be removed ({e})")))?;
    let _part = Part(part.clone());
    let made = (|| -> Result<Synced, Failed> {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = open_for_writing(&part, flags)?;
        // A file being made needs no journal: if this stops half way, the file is thrown away.
        conn.pragma_update(None, "page_size", PAGE_SIZE)?;
        conn.pragma_update(None, "journal_mode", "OFF")?;
        conn.pragma_update(None, "synchronous", "OFF")?;
        conn.execute_batch(SCHEMA)?;
        let synced = fill(old, &conn, pass)?;
        // From now on it's written to while the page may be reading it: in WAL mode readers never wait for a writer.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.close().map_err(|(_, e)| e)?;
        // On the disk before it has its name, so a copy with its name is whole even after a power cut.
        fs::OpenOptions::new().write(true).open(&part)?.sync_all()?;
        Ok(synced)
    })();
    let synced = made.map_err(|Failed { why, .. }| failed(why))?;
    // No reader in this app opens the copy while it's swapped: one that did would hold the old file through it.
    let _readers_wait = LAYING_OUT.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(kept) = kept {
        let_go_of(copy).map_err(|why| failed(format!("the old copy couldn't be replaced ({why})")))?;
        // Another copy of the app may have written to it meanwhile (this one writes only under the same lock).
        if SoundsDb::stamp(copy) != kept {
            return Err(failed("the old copy couldn't be replaced (something wrote to it meanwhile)".into()));
        }
    }
    if let Err(e) = fs::rename(&part, copy) {
        return Err(failed(format!("the new copy couldn't be given its name ({e})")));
    }
    Ok(synced)
}

/// Readies the copy at `copy` to be replaced: SQLite's files beside it go, as this connection, the last to it, closes
/// (having put whatever its log held into it). Why not, when they're still there: something else has it open (a page
/// reading it, or another copy of the app), which isn't waited for. Left beside a new copy, its log would be read into
/// that one as if it were its own; and a reader holding the copy would go on reading the old file (on Windows, it
/// couldn't be replaced at all).
fn let_go_of(copy: &Path) -> Result<(), String> {
    let conn = Connection::open_with_flags(copy, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|e| e.to_string())?;
    conn.busy_timeout(WAIT).map_err(|e| e.to_string())?;
    // Read, so SQLite opens it and its log: one that's never read is closed without touching them.
    conn.query_row("SELECT COUNT(*) FROM sqlite_master", [], |_| Ok(())).map_err(|e| e.to_string())?;
    conn.close().map_err(|(_, e)| e.to_string())?;
    if ["-wal", "-shm", "-journal"].iter().any(|end| side(copy, end).exists()) {
        return Err("something else has it open (a page reading it, or another copy of the app)".into());
    }
    Ok(())
}

/// Whether a disk with `free` bytes free has room for work on the copy that `takes` up to so many bytes more, with
/// [`ROOM_LEFT`] to spare. Making a copy takes under half the library's size (0.29 of it with a library like
/// Solomon's, less since CLAP's fingerprints are kept at half precision), known before it starts, and laying one out
/// anew no more than its own size; other programs fill the disk meanwhile too ([`Room`]).
fn room_for(free: u64, takes: u64, doing: &str) -> Result<(), String> {
    if free >= takes.saturating_add(ROOM_LEFT) {
        return Ok(());
    }
    Err(format!(
        "only {} is free on its disk: {doing} takes up to {}, and {} is always left free",
        gb(free),
        gb(takes),
        gb(ROOM_LEFT)
    ))
}

/// Whether a disk with `free` bytes free still has [`ROOM_LEFT`] free.
fn room_left(free: u64) -> Result<(), String> {
    if free >= ROOM_LEFT {
        return Ok(());
    }
    Err(format!("only {} is free on its disk, and {} is always left free", gb(free), gb(ROOM_LEFT)))
}

/// `bytes` in GB, for people.
fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

/// How many sounds are written between two looks at the room left on the disk ([`Room`]): a few MB of the copy.
const ROOM_EVERY: usize = 256;

/// Watches the disk the copy is written on, so the work stops (and what it wrote is thrown away) before less than
/// [`ROOM_LEFT`] is free there: other programs fill the disk meanwhile too.
struct Room<'a> {
    dir: &'a Path,
    written: usize,
}

impl Room<'_> {
    fn new(dir: &Path) -> Room<'_> {
        Room { dir, written: 0 }
    }

    /// One more sound written: every [`ROOM_EVERY`], the disk is looked at.
    fn wrote(&mut self) -> Result<(), Failed> {
        self.written += 1;
        if self.written.is_multiple_of(ROOM_EVERY) { self.look() } else { Ok(()) }
    }

    /// Whether [`ROOM_LEFT`] is still free on the disk (taken as so when the disk can't say).
    fn look(&self) -> Result<(), Failed> {
        match fs4::available_space(self.dir) {
            Ok(free) => room_left(free).map_err(Failed::new),
            Err(_) => Ok(()),
        }
    }
}

/// The copy, opened to be written to. The tables copied whole keep the C# app's `REFERENCES sounds(id) ON DELETE
/// CASCADE`, and the SQLite built into the app checks those unless told not to: a row `sounds.db` holds for a sound
/// it no longer has couldn't be copied. The copy holds what `sounds.db` holds, so it checks nothing of its own.
fn open_for_writing(path: &Path, flags: OpenFlags) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(path, flags)?;
    conn.pragma_update(None, "foreign_keys", false)?;
    Ok(conn)
}

/// Writes into the new, empty copy at `conn` all of `old`, in one transaction, reading `old` at one moment. The page's
/// read, when the pass is for it too, is kept from the moment the last sound is read, whatever happens to the copy
/// after: the page has it once the pass ends (a new copy's last few seconds, its indexes and its trip to the disk,
/// come first).
fn fill(old: &SoundsDb, conn: &Connection, pass: Pass<'_>) -> Result<Synced, Failed> {
    let mut synced = Synced { made: true, ..Synced::default() };
    let snapshot = old.conn.unchecked_transaction()?;
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    // From the same moment as the rows: the page gets what the copy gets.
    let mut reading =
        pass.read_for.map(|now| Reading::start(old, now, true)).transpose().map_err(|e| Failed::new(e.to_string()))?;
    let mut room = Room::new(pass.dir);
    let mut st = old.conn.prepare(OLD_SOUNDS)?;
    let mut rows = st.query([])?;
    while let Some(r) = rows.next()? {
        synced.sounds += 1;
        let id: i64 = r.get(0)?;
        // The page's sound, when the pass reads for it too: its analysis is read once, for both.
        let sound = if reading.is_some() { Some(sound_at(r)?) } else { None };
        let read_here;
        let analysis = match &sound {
            Some(s) => s.analysis.as_ref(),
            None => {
                read_here = crate::db::analysis_at(r, OLD_ANALYSIS);
                read_here.as_ref()
            }
        };
        let clap = r.get_ref(OLD_CLAP)?;
        write_sound(&tx, id, r, analysis, clap_numbers(clap))?;
        write_wave(&tx, id, analysis)?;
        write_features(&tx, id, r.get_ref(OLD_FEATURES)?)?;
        write_clap(&tx, id, clap)?;
        room.wrote()?;
        if let (Some(reading), Some(sound)) = (reading.as_mut(), sound) {
            reading.add_read(r, sound);
        }
    }
    drop(rows);
    drop(st);
    *pass.read = reading.map(Reading::finish);
    for table in WHOLE {
        room.look()?;
        copy_whole(&old.conn, &tx, table)?;
    }
    room.look()?;
    copy_prints(old, &tx)?;
    room.look()?;
    tx.execute_batch(INDEXES)?;
    // Checked before it's kept: as many sounds in the copy as were read.
    let copied: i64 = tx.query_row("SELECT COUNT(*) FROM sounds", [], |r| r.get(0))?;
    if usize::try_from(copied).ok() != Some(synced.sounds) {
        return Err(Failed::new(format!(
            "the copy came out with {copied} sounds where sounds.db has {}",
            synced.sounds
        )));
    }
    set_info(&tx, "format", FORMAT)?;
    set_info(&tx, "stamp", pass.stamp)?;
    if let Some(since) = pass.adopt {
        set_info(&tx, OWN_SINCE, since)?;
        synced.adopted = !pass.was_own;
    }
    tx.commit()?;
    snapshot.finish()?;
    Ok(synced)
}

/// One sound's row in the copy, from its row in `sounds.db` and the `analysis` read from it: the analysis as numbers,
/// everything else as it is, and how many numbers its CLAP has ([`clap_numbers`]).
fn write_sound(
    tx: &Transaction<'_>,
    id: i64,
    r: &Row<'_>,
    analysis: Option<&SoundAnalysis>,
    clap: Option<i64>,
) -> rusqlite::Result<()> {
    // Each value passed on as SQLite gave it, even text that isn't readable as text (a damaged row).
    let mut values: Vec<ToSqlOutput<'_>> = Vec::with_capacity(2 + AS_THEY_ARE.len() + ANALYSIS_COLUMNS);
    values.push(ToSqlOutput::Owned(Value::Integer(id)));
    for i in AS_THEY_ARE {
        values.push(ToSqlOutput::Borrowed(r.get_ref(i)?));
    }
    values.extend(analysis_values(analysis).into_iter().map(ToSqlOutput::Owned));
    values.push(ToSqlOutput::Owned(clap.map_or(Value::Null, Value::Integer)));
    tx.prepare_cached(INSERT_SOUND)?.execute(params_from_iter(values))?;
    Ok(())
}

/// One sound's waveform, packed ([`packed_peaks`]), in a row of its own: none when it has none (or no analysis).
fn write_wave(tx: &Transaction<'_>, id: i64, analysis: Option<&SoundAnalysis>) -> rusqlite::Result<()> {
    let Some(Value::Blob(wave)) = analysis.map(|a| packed_peaks(&a.waveform)) else { return Ok(()) };
    tx.prepare_cached("INSERT INTO sound_waves (sound_id, v) VALUES (?1, ?2)")?.execute(params![id, wave])?;
    Ok(())
}

/// One sound's features, as they are (none when it has none).
fn write_features(tx: &Transaction<'_>, id: i64, v: ValueRef<'_>) -> rusqlite::Result<()> {
    if v != ValueRef::Null {
        tx.prepare_cached("INSERT INTO sound_features (sound_id, v) VALUES (?1, ?2)")?
            .execute(params![id, ToSqlOutput::Borrowed(v)])?;
    }
    Ok(())
}

/// One sound's CLAP fingerprint as CLAP's index holds it ([`held`]), when what `sounds.db` holds is one (whole
/// floats): anything else is left out, as the index leaves it out.
fn write_clap(tx: &Transaction<'_>, id: i64, v: ValueRef<'_>) -> rusqlite::Result<()> {
    let Some(floats) = whole_floats(v) else { return Ok(()) };
    tx.prepare_cached("INSERT INTO sound_clap (sound_id, v) VALUES (?1, ?2)")?
        .execute(params![id, held_bytes(&held(&floats))])?;
    Ok(())
}

/// A fingerprint as `sounds.db` stores it, when it is one: little-endian 4-byte floats, at least one (as
/// [`crate::index::Gathered::push_blob`] takes them).
fn whole_floats(v: ValueRef<'_>) -> Option<Vec<f32>> {
    match v {
        ValueRef::Blob(b) => crate::db::floats(b),
        _ => None,
    }
}

/// How many numbers a CLAP fingerprint as `sounds.db` stores it has: none when there's none, 0 when what's there
/// isn't one (a damaged row). What tells how many sounds CLAP has heard, without reading the fingerprints.
fn clap_numbers(v: ValueRef<'_>) -> Option<i64> {
    match v {
        ValueRef::Null => None,
        ValueRef::Blob(b) if !b.is_empty() && b.len().is_multiple_of(4) => {
            Some(i64::try_from(b.len() / 4).unwrap_or(0))
        }
        _ => Some(0),
    }
}

/// Copies `table` whole from `old` into the new copy, when `old` has it: made as the C# app made it, with the same row
/// numbers and the same indexes.
fn copy_whole(old: &Connection, tx: &Transaction<'_>, table: &str) -> rusqlite::Result<()> {
    let Some(made) = table_sql(old, "main", table)? else { return Ok(()) };
    let columns = Columns::of(old, "main", table, &made)?;
    let select = format!("SELECT {} FROM {}", columns.list(), quoted(table));
    tx.execute_batch(&made)?;
    let placeholders: Vec<String> = (1..=columns.names.len()).map(|i| format!("?{i}")).collect();
    let insert = format!("INSERT INTO {} ({}) VALUES ({})", quoted(table), columns.list(), placeholders.join(", "));
    copy_rows(old, &select, tx, &insert)?;
    for sql in index_sqls(old, "main", table)? {
        tx.execute_batch(&sql)?;
    }
    Ok(())
}

/// Copies the hashes of exact copies from the duplicate finder's prints, when `old` has them.
fn copy_prints(old: &SoundsDb, tx: &Transaction<'_>) -> rusqlite::Result<()> {
    if !old.has("sound_prints") {
        return Ok(());
    }
    tx.execute_batch(PRINTS)?;
    copy_rows(&old.conn, OLD_PRINTS, tx, "INSERT INTO sound_prints (sound_id, exact) VALUES (?1, ?2)")
}

/// A table's columns as a copy of it reads and writes them: its row number first when it has one of its own (one
/// that isn't also a column), so the copy's rows keep their numbers and come out in the same order.
struct Columns {
    names: Vec<String>,
}

impl Columns {
    /// Of `table` in `schema` ("main", or an attached copy's name), made by `sql`.
    fn of(conn: &Connection, schema: &str, table: &str, sql: &str) -> rusqlite::Result<Columns> {
        let mut st = conn.prepare("SELECT name, type, pk FROM pragma_table_info(?1, ?2)")?;
        let info: Vec<(String, String, i64)> =
            st.query_map([table, schema], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
        // A column that is the row number: the only key column, declared INTEGER (SQLite's rule).
        let keys: Vec<&(String, String, i64)> = info.iter().filter(|c| c.2 > 0).collect();
        let row_number_column = keys.len() == 1 && keys[0].1.eq_ignore_ascii_case("INTEGER");
        // "WITHOUT ROWID" comes after the columns' closing bracket.
        let without_rowid = sql.rfind(')').is_some_and(|end| {
            sql[end..].to_ascii_uppercase().split_whitespace().collect::<String>().contains("WITHOUTROWID")
        });
        let mut names = Vec::with_capacity(info.len() + 1);
        if !row_number_column && !without_rowid {
            names.push("rowid".to_owned());
        }
        names.extend(info.into_iter().map(|c| quoted(&c.0)));
        Ok(Columns { names })
    }

    fn list(&self) -> String {
        self.names.join(", ")
    }
}

/// Copies every row `select` gives from `from` into `to` with `insert`, one at a time.
fn copy_rows(from: &Connection, select: &str, to: &Transaction<'_>, insert: &str) -> rusqlite::Result<()> {
    let mut st = from.prepare(select)?;
    let n = st.column_count();
    let mut ins = to.prepare(insert)?;
    let mut rows = st.query([])?;
    while let Some(r) = rows.next()? {
        let values = (0..n).map(|i| r.get_ref(i).map(ToSqlOutput::Borrowed)).collect::<Result<Vec<_>, _>>()?;
        ins.execute(params_from_iter(values))?;
    }
    Ok(())
}

/// What made `table` in `schema` ("main", or an attached copy's name), when it's there.
fn table_sql(conn: &Connection, schema: &str, table: &str) -> rusqlite::Result<Option<String>> {
    let sql = format!("SELECT sql FROM {}.sqlite_master WHERE type = 'table' AND name = ?1", quoted(schema));
    conn.query_row(&sql, [table], |r| r.get(0)).optional()
}

/// The indexes made on `table` in `schema` by name (those its own keys make, which come with the table, have none),
/// by name.
fn index_sqls(conn: &Connection, schema: &str, table: &str) -> rusqlite::Result<Vec<String>> {
    let mut st = conn.prepare(&format!(
        "SELECT sql FROM {}.sqlite_master WHERE type = 'index' AND tbl_name = ?1 AND sql IS NOT NULL ORDER BY name",
        quoted(schema)
    ))?;
    st.query_map([table], |r| r.get(0))?.collect()
}

fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn set_info(tx: &Transaction<'_>, key: &str, value: impl rusqlite::ToSql) -> rusqlite::Result<()> {
    tx.prepare_cached("INSERT OR REPLACE INTO copy_info (key, value) VALUES (?1, ?2)")?.execute(params![key, value])?;
    Ok(())
}

fn info_value(conn: &Connection, key: &str) -> rusqlite::Result<Option<i64>> {
    Ok(conn
        .query_row("SELECT value FROM copy_info WHERE key = ?1", [key], |r| r.get::<_, Option<i64>>(0))
        .optional()?
        .flatten())
}

/// What's where the copy goes.
#[derive(Debug, PartialEq, Eq)]
enum Found {
    Nothing,
    /// A copy, laid out in `format`, the stamp of the `sounds.db` it was last brought up to date with, and since when
    /// it's been the app's own sound library, if it has ([`own_since`]).
    Copy {
        format: i64,
        stamp: Option<String>,
        own: Option<i64>,
    },
    /// Something nothing can be read from: a file that isn't a database, a damaged one, or an empty one. Made again.
    Unreadable,
    /// A database of something else, which is never thrown away.
    Foreign,
}

/// Looks at what's where the copy goes, waiting up to `wait` for another program that holds it. An error when it
/// couldn't be looked at just then (busy beyond waiting for, or the disk failing): that's no reason to throw it away.
fn look_at(copy: &Path, wait: Duration) -> Result<Found, String> {
    match fs::metadata(copy) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Found::Nothing),
        Err(e) => return Err(e.to_string()),
        Ok(_) => {}
    }
    let found = (|| -> rusqlite::Result<Found> {
        let conn =
            Connection::open_with_flags(copy, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
        conn.busy_timeout(wait)?;
        let (tables, is_copy): (i64, bool) = conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(name = ?1), 0) > 0 FROM sqlite_master WHERE type = 'table'",
            [INFO_TABLE],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if tables == 0 {
            return Ok(Found::Unreadable);
        }
        if !is_copy {
            return Ok(Found::Foreign);
        }
        let Some(format) = format_of(&conn)? else { return Ok(Found::Unreadable) };
        let stamp = conn
            .query_row("SELECT value FROM copy_info WHERE key = 'stamp'", [], |r| {
                Ok(match r.get_ref(0)? {
                    ValueRef::Text(t) => std::str::from_utf8(t).ok().map(str::to_owned),
                    _ => None,
                })
            })
            .optional()?
            .flatten();
        let own = info_value(&conn, OWN_SINCE)?;
        Ok(Found::Copy { format, stamp, own })
    })();
    match found {
        Err(rusqlite::Error::SqliteFailure(e, _))
            if matches!(e.code, rusqlite::ErrorCode::NotADatabase | rusqlite::ErrorCode::DatabaseCorrupt) =>
        {
            Ok(Found::Unreadable)
        }
        found => found.map_err(|e| e.to_string()),
    }
}

/// The format a copy notes it's laid out in; `None` when it notes none.
pub(crate) fn format_of(conn: &Connection) -> rusqlite::Result<Option<i64>> {
    Ok(conn
        .query_row("SELECT value FROM copy_info WHERE key = 'format'", [], |r| {
            Ok(match r.get_ref(0)? {
                ValueRef::Integer(n) => Some(n),
                _ => None,
            })
        })
        .optional()?
        .flatten())
}

/// Where this app makes a new copy before giving it its name: `sounds-rust.db.part-` and the app's process number,
/// so two copies of the app running at once can't write into one file.
fn part_of(copy: &Path) -> PathBuf {
    side(copy, &format!(".part-{}", std::process::id()))
}

/// Removes the copies left half made by an app that stopped while making one, and SQLite's files beside them: others'
/// once they haven't changed for [`STALE`], and with `ours_too` this app's own (only while [`SYNCING`] is held: this
/// app makes none meanwhile). Not one another copy of the app is still making (and Windows won't remove a file that's
/// open anyway).
fn remove_parts(copy: &Path, ours_too: bool) {
    let Some(name) = copy.file_name() else { return };
    let dir = dir_of(copy);
    let start = format!("{}.part", name.to_string_lossy());
    let own = part_of(copy);
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if !entry.file_name().to_string_lossy().starts_with(&start) {
            continue;
        }
        let ours = ours_too && ["", "-wal", "-shm", "-journal"].iter().any(|end| path == side(&own, end));
        let stale = || {
            entry
                .metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|t| SystemTime::now().duration_since(t).is_ok_and(|age| age > STALE))
        };
        if ours || stale() {
            let _ = fs::remove_file(path);
        }
    }
}

/// `path` with `end` added to its name: `sounds-rust.db` and "-wal" give `sounds-rust.db-wal`.
fn side(path: &Path, end: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(end);
    PathBuf::from(name)
}

/// `path` as a name for SQLite that opens the database there only if it's there (a URI, `mode=rw`): never makes an
/// empty one in its place. `None` for a path a URI can't name (a network share's, or one that isn't text).
fn existing_only(path: &Path) -> Option<String> {
    let slashed = path.to_str()?.replace('\\', "/");
    if slashed.starts_with("//") {
        return None;
    }
    let mut uri = String::from("file:");
    // A Windows path from its drive's letter: "file:///C:/…".
    if slashed.as_bytes().get(1) == Some(&b':') {
        uri.push_str("///");
    }
    for b in slashed.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                uri.push(char::from(b))
            }
            _ => uri.push_str(&format!("%{b:02X}")),
        }
    }
    uri.push_str("?mode=rw");
    Some(uri)
}

/// Removes a database and the files SQLite keeps beside it. Fine when there are none.
fn remove_database(path: &Path) -> std::io::Result<()> {
    for end in ["", "-wal", "-shm", "-journal"] {
        match fs::remove_file(side(path, end)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    Ok(())
}

/// The analysis as the copy's columns hold it ([`analysis_columns`]); every column empty when there's none.
fn analysis_values(a: Option<&SoundAnalysis>) -> Vec<Value> {
    let Some(a) = a else { return vec![Value::Null; ANALYSIS_COLUMNS] };
    let text = |s: &str| Value::Text(s.to_owned());
    let real = |x: Option<f64>| x.map_or(Value::Null, Value::Real);
    vec![
        Value::Real(a.seconds),
        Value::Integer(a.sample_rate.into()),
        Value::Integer(a.channel_count.into()),
        Value::Real(a.peak_db),
        Value::Real(a.rms_db),
        Value::Real(a.attack_ms),
        Value::Real(a.decay_ms),
        Value::Real(a.brightness),
        Value::Real(a.noisiness),
        Value::Real(a.width),
        Value::Integer(a.is_loop.into()),
        real(a.bpm),
        text(&a.bpm_source),
        real(a.pitch),
        Value::Real(a.pitch_confidence),
        a.key_tonic.map_or(Value::Null, |k| Value::Integer(k.into())),
        Value::Integer(a.key_minor.into()),
        Value::Real(a.key_confidence),
        text(&a.key_source),
        a.chord.as_deref().map_or(Value::Null, text),
        packed_chords(&a.chords),
        packed(&a.chroma),
        json_list(&a.character),
    ]
}

/// The analysis from a row of the copy whose analysis columns start at `at`, with its packed waveform straight after
/// them (from its own table, or in the format before its own column); `None` when the sound has none.
pub(crate) fn analysis_at(r: &Row<'_>, at: usize) -> Option<SoundAnalysis> {
    // Each value read as the rest of the library is read (crate::db), so the copy can't read one differently.
    let get = |i: usize| r.get_ref(at + i).ok();
    let real = |i: usize| real_at(r, at + i);
    let int = |i: usize| int_at(r, at + i).and_then(|n| i32::try_from(n).ok());
    let text = |i: usize| text_at(r, at + i);
    let blob = |i: usize| match get(i) {
        Some(ValueRef::Blob(b)) => b,
        _ => &[],
    };
    Some(SoundAnalysis {
        seconds: real(0)?,
        sample_rate: int(1).unwrap_or(0),
        channel_count: int(2).unwrap_or(0),
        peak_db: real(3).unwrap_or(0.0),
        rms_db: real(4).unwrap_or(0.0),
        attack_ms: real(5).unwrap_or(0.0),
        decay_ms: real(6).unwrap_or(0.0),
        brightness: real(7).unwrap_or(0.0),
        noisiness: real(8).unwrap_or(0.0),
        width: real(9).unwrap_or(0.0),
        is_loop: int(10) == Some(1),
        bpm: real(11),
        bpm_source: word_at(r, at + 12).unwrap_or_default(),
        pitch: real(13),
        pitch_confidence: real(14).unwrap_or(0.0),
        key_tonic: int(15),
        key_minor: int(16) == Some(1),
        key_confidence: real(17).unwrap_or(0.0),
        key_source: word_at(r, at + 18).unwrap_or_default(),
        chord: text(19),
        chords: match get(20) {
            Some(ValueRef::Blob(b)) => unpacked_chords(b),
            _ => from_json_list(text(20).as_deref()),
        },
        chroma: unpacked(blob(21)),
        character: from_json_list(text(22).as_deref()),
        waveform: unpacked_peaks(blob(23)),
    })
}

/// A list as JSON text, or nothing when it's empty (most are).
fn json_list<T: serde::Serialize>(list: &[T]) -> Value {
    if list.is_empty() {
        return Value::Null;
    }
    serde_json::to_string(list).map_or(Value::Null, Value::Text)
}

fn from_json_list<T: serde::de::DeserializeOwned>(json: Option<&str>) -> Vec<T> {
    let mut list: Vec<T> = json.and_then(|j| serde_json::from_str(j).ok()).unwrap_or_default();
    list.shrink_to_fit();
    list
}

/// How a list of numbers is packed (its first byte): as thousandths, 10 bits each, or as the numbers themselves.
const THOUSANDTHS: u8 = 1;
const AS_IS: u8 = 2;
/// The first byte of the chords as [`packed_chords`] packs them.
const CHORDS: u8 = 3;

/// The chords a sound plays, packed: for each, when it starts and ends (the numbers themselves, so they come back to
/// the last bit, which numbers written out as text don't always do) and its name. Nothing when there are none (most
/// sounds have none).
fn packed_chords(chords: &[ChordSpan]) -> Value {
    if chords.is_empty() {
        return Value::Null;
    }
    let mut out = Vec::with_capacity(1 + chords.iter().map(|c| 20 + c.name.len()).sum::<usize>());
    out.push(CHORDS);
    for c in chords {
        // A name too long to say how long in 4 bytes (4 GB) can't be a chord's: kept as text, as the C# app does.
        let Ok(len) = u32::try_from(c.name.len()) else { return json_list(chords) };
        out.extend_from_slice(&c.start.to_le_bytes());
        out.extend_from_slice(&c.end.to_le_bytes());
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(c.name.as_bytes());
    }
    Value::Blob(out)
}

/// The chords [`packed_chords`] packed. None when they aren't whole.
fn unpacked_chords(bytes: &[u8]) -> Vec<ChordSpan> {
    fn take<'a>(rest: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
        let (head, tail) = rest.split_at_checked(n)?;
        *rest = tail;
        Some(head)
    }
    fn number(rest: &mut &[u8]) -> Option<f64> {
        Some(f64::from_le_bytes(take(rest, 8)?.try_into().ok()?))
    }
    let Some((&CHORDS, mut rest)) = bytes.split_first() else { return Vec::new() };
    let mut out = Vec::new();
    while !rest.is_empty() {
        let span = (|| {
            let start = number(&mut rest)?;
            let end = number(&mut rest)?;
            let len = u32::from_le_bytes(take(&mut rest, 4)?.try_into().ok()?);
            let name = String::from_utf8(take(&mut rest, usize::try_from(len).ok()?)?.to_vec()).ok()?;
            Some(ChordSpan { start, end, name })
        })();
        let Some(span) = span else { return Vec::new() };
        out.push(span);
    }
    out.shrink_to_fit();
    out
}

/// A number type the analysis keeps lists of: the waveform's points (`f32`) and the chroma (`f64`).
pub(crate) trait Packable: Copy + 'static {
    const SIZE: usize;
    /// Each whole number of thousandths from 0 to 1023 as this kind of number, as the C# analyser makes it
    /// (`Math.Round(x, 3)`): see [`F64_THOUSANDTHS`].
    const THOUSANDTHS: &'static [Self; 1024];
    fn as_f64(self) -> f64;
    fn bits(self) -> u64;
    fn put(self, out: &mut Vec<u8>);
    fn take(bytes: &[u8]) -> Self;
}

/// Each whole number of thousandths a list can be packed with, as the number it stands for: looked up when a list is
/// unpacked rather than worked out again (a division for each of the waveform's 160 points, in every sound).
const F64_THOUSANDTHS: [f64; 1024] = {
    let mut table = [0.0; 1024];
    let mut k = 0;
    while k < 1024 {
        table[k] = k as f64 / 1000.0;
        k += 1;
    }
    table
};

pub(crate) const F32_THOUSANDTHS: [f32; 1024] = {
    let mut table = [0.0; 1024];
    let mut k = 0;
    while k < 1024 {
        table[k] = F64_THOUSANDTHS[k] as f32;
        k += 1;
    }
    table
};

impl Packable for f32 {
    const SIZE: usize = 4;
    const THOUSANDTHS: &'static [f32; 1024] = &F32_THOUSANDTHS;
    fn as_f64(self) -> f64 {
        f64::from(self)
    }
    fn bits(self) -> u64 {
        u64::from(self.to_bits())
    }
    fn put(self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.to_le_bytes());
    }
    fn take(bytes: &[u8]) -> f32 {
        let mut b = [0; 4];
        b.copy_from_slice(bytes);
        f32::from_le_bytes(b)
    }
}

impl Packable for f64 {
    const SIZE: usize = 8;
    const THOUSANDTHS: &'static [f64; 1024] = &F64_THOUSANDTHS;
    fn as_f64(self) -> f64 {
        self
    }
    fn bits(self) -> u64 {
        self.to_bits()
    }
    fn put(self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.to_le_bytes());
    }
    fn take(bytes: &[u8]) -> f64 {
        let mut b = [0; 8];
        b.copy_from_slice(bytes);
        f64::from_le_bytes(b)
    }
}

/// `x` as a whole number of thousandths (0 to 1023), when that gives back `x` itself, to the last bit.
pub(crate) fn thousandths<T: Packable>(x: T) -> Option<u16> {
    let k = (x.as_f64() * 1000.0).round();
    if !(0.0..=1023.0).contains(&k) {
        return None;
    }
    let k = k as u16;
    (T::THOUSANDTHS[usize::from(k)].bits() == x.bits()).then_some(k)
}

/// A list of numbers packed for the copy: 10 bits for each when every one is a whole number of thousandths (160
/// waveform points take 201 bytes instead of 640), else the numbers as they are. Nothing for an empty list.
fn packed<T: Packable>(list: &[T]) -> Value {
    if list.is_empty() {
        return Value::Null;
    }
    let bytes = match list.iter().map(|&x| thousandths(x)).collect::<Option<Vec<u16>>>() {
        Some(ks) => packed_thousandths(&ks),
        None => {
            let mut out = Vec::with_capacity(1 + list.len() * T::SIZE);
            out.push(AS_IS);
            for &x in list {
                x.put(&mut out);
            }
            out
        }
    };
    Value::Blob(bytes)
}

/// Whole thousandths packed 10 bits each, after the [`THOUSANDTHS`] mark.
fn packed_thousandths(ks: &[u16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + (ks.len() * 10).div_ceil(8));
    out.push(THOUSANDTHS);
    let (mut bits, mut held) = (0u32, 0u32);
    for &k in ks {
        bits |= u32::from(k) << held;
        held += 10;
        while held >= 8 {
            out.push(bits as u8);
            bits >>= 8;
            held -= 8;
        }
    }
    if held > 0 {
        out.push(bits as u8);
    }
    out
}

/// A list [`packed`] packed. Empty when there's none (or it isn't one).
fn unpacked<T: Packable>(bytes: &[u8]) -> Vec<T> {
    match bytes.split_first() {
        Some((&THOUSANDTHS, rest)) => unpacked_thousandths(rest, |k| T::THOUSANDTHS[usize::from(k)]),
        Some((&AS_IS, rest)) => rest.chunks_exact(T::SIZE).map(T::take).collect(),
        _ => Vec::new(),
    }
}

/// The 10-bit whole thousandths after a [`THOUSANDTHS`] mark, each made into what `number` gives for it.
fn unpacked_thousandths<T>(rest: &[u8], number: impl Fn(u16) -> T) -> Vec<T> {
    // Each byte holds 8 bits and each number 10, and the last byte's spare bits are fewer than 10.
    let n = rest.len() * 8 / 10;
    // Four numbers to every 5 bytes, the first in the lowest bits; then the last few, in fewer bytes.
    let at = |word: u64, j: u32| number(((word >> (10 * j)) & 0x3ff) as u16);
    let mut out = Vec::with_capacity(n);
    let (fives, last) = rest.as_chunks::<5>();
    for b in fives {
        let word = u64::from(b[0])
            | (u64::from(b[1]) << 8)
            | (u64::from(b[2]) << 16)
            | (u64::from(b[3]) << 24)
            | (u64::from(b[4]) << 32);
        out.extend([at(word, 0), at(word, 1), at(word, 2), at(word, 3)]);
    }
    let word = last.iter().rev().fold(0u64, |word, &b| (word << 8) | u64::from(b));
    for j in 0..(n - out.len()) as u32 {
        out.push(at(word, j));
    }
    out
}

/// A waveform packed for the copy, as [`packed`] packs it, straight from the thousandths it's held as.
fn packed_peaks(peaks: &Peaks) -> Value {
    match peaks.as_thousandths() {
        Some([]) => Value::Null,
        Some(ks) => Value::Blob(packed_thousandths(ks)),
        None => packed(&peaks.to_vec()),
    }
}

/// A waveform [`packed_peaks`] packed, kept as the thousandths it was packed as when it was.
fn unpacked_peaks(bytes: &[u8]) -> Peaks {
    match bytes.split_first() {
        Some((&THOUSANDTHS, rest)) => Peaks::from_thousandths(unpacked_thousandths(rest, |k| k).into()),
        _ => Peaks::new(&unpacked::<f32>(bytes)),
    }
}

/// A 64-bit FNV-1a hash: simple, and the same every time (Rust's own may change between releases). What laying a copy
/// out anew checks each fingerprint it wrote against, without holding them all.
#[derive(Clone, Copy)]
struct Hash(u64);

impl Hash {
    fn new() -> Hash {
        Hash(0xcbf2_9ce4_8422_2325)
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    /// A value with its kind and, for text and blobs, its length first, so no two different rows hash alike by
    /// running into each other.
    fn value(&mut self, v: ValueRef<'_>) {
        match v {
            ValueRef::Null => self.bytes(&[0]),
            ValueRef::Integer(n) => {
                self.bytes(&[1]);
                self.bytes(&n.to_le_bytes());
            }
            ValueRef::Real(x) => {
                self.bytes(&[2]);
                self.bytes(&x.to_bits().to_le_bytes());
            }
            ValueRef::Text(t) => {
                self.bytes(&[3]);
                self.bytes(&(t.len() as u64).to_le_bytes());
                self.bytes(t);
            }
            ValueRef::Blob(b) => {
                self.bytes(&[4]);
                self.bytes(&(b.len() as u64).to_le_bytes());
                self.bytes(b);
            }
        }
    }

    fn get(self) -> i64 {
        self.0 as i64
    }
}

fn hash_of(v: ValueRef<'_>) -> i64 {
    let mut hash = Hash::new();
    hash.value(v);
    hash.get()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousandths_pack_into_ten_bits_each_and_come_back_the_same() {
        for n in [1usize, 2, 3, 4, 5, 7, 8, 12, 159, 160, 161] {
            let list: Vec<f32> = (0..n).map(|i| F32_THOUSANDTHS[(i * 389) % 1001]).collect();
            let Value::Blob(bytes) = packed(&list) else { panic!("no blob for {n}") };
            assert_eq!(bytes[0], THOUSANDTHS);
            assert_eq!(bytes.len(), 1 + (n * 10).div_ceil(8), "{n} numbers");
            let back: Vec<f32> = unpacked(&bytes);
            assert_eq!(
                back.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
                list.iter().map(|x| x.to_bits()).collect::<Vec<_>>()
            );
        }
        // 160 waveform points: 201 bytes, where the numbers as they are take 641.
        let Value::Blob(bytes) = packed(&[0.5f32; 160]) else { panic!() };
        assert_eq!(bytes.len(), 201);
        let chroma = [0.0, 0.123, 1.0, 0.5, 0.001, 0.999, 0.25, 0.75, 0.333, 0.667, 0.1, 0.9];
        let Value::Blob(bytes) = packed(&chroma) else { panic!() };
        assert_eq!(bytes.len(), 16);
        assert_eq!(unpacked::<f64>(&bytes), chroma);
    }

    #[test]
    fn numbers_that_arent_thousandths_are_kept_as_they_are() {
        // More decimals, below 0, -0.0 (which 0 thousandths would give back as 0.0), over 1.023, nearly 0.
        for odd in [0.1234f64, -0.5, -0.0, 1.5, 1e-9] {
            let list = [0.5, odd];
            let Value::Blob(bytes) = packed(&list) else { panic!() };
            assert_eq!(bytes[0], AS_IS, "{odd}");
            let back: Vec<f64> = unpacked(&bytes);
            assert_eq!(back.iter().map(|x| x.to_bits()).collect::<Vec<_>>(), list.map(f64::to_bits), "{odd}");
        }
        // A waveform point the C# app rounded, read from its JSON as an f32: packed, and the very same number back.
        let point: f32 = serde_json::from_str("0.123").unwrap();
        let Value::Blob(bytes) = packed(&[point]) else { panic!() };
        assert_eq!(bytes[0], THOUSANDTHS);
        assert_eq!(unpacked::<f32>(&bytes)[0].to_bits(), point.to_bits());
        assert_eq!(packed::<f32>(&[]), Value::Null);
        assert!(unpacked::<f32>(&[]).is_empty());
        assert!(unpacked::<f32>(&[9, 1, 2]).is_empty());
    }

    #[test]
    fn a_waveform_packs_straight_from_its_thousandths_and_back() {
        let points: Vec<f32> = (0..160).map(|i| F32_THOUSANDTHS[(i * 389) % 1024]).collect();
        for list in [points.clone(), vec![0.5, 0.1234], vec![0.25; 3], Vec::new()] {
            let peaks = Peaks::new(&list);
            let packed_here = packed_peaks(&peaks);
            // The same bytes as the floats would pack to, and the same `Peaks` back (thousandths stay thousandths).
            assert_eq!(packed_here, packed(&list));
            let back = match &packed_here {
                Value::Blob(bytes) => unpacked_peaks(bytes),
                _ => unpacked_peaks(&[]),
            };
            assert_eq!(back, peaks);
            assert_eq!(back.as_thousandths().is_some(), peaks.as_thousandths().is_some());
        }
    }

    #[test]
    fn text_and_a_blob_of_the_same_bytes_hash_differently() {
        assert_ne!(hash_of(ValueRef::Text(b"ab")), hash_of(ValueRef::Blob(b"ab")));
        let (mut one, mut two) = (Hash::new(), Hash::new());
        one.value(ValueRef::Text(b"ab"));
        one.value(ValueRef::Text(b"c"));
        two.value(ValueRef::Text(b"a"));
        two.value(ValueRef::Text(b"bc"));
        assert_ne!(one.get(), two.get());
    }

    #[test]
    fn the_analysis_columns_are_counted_right() {
        assert_eq!(analysis_columns!().split(',').count(), ANALYSIS_COLUMNS);
        assert_eq!(analysis_values(Some(&SoundAnalysis::default())).len(), ANALYSIS_COLUMNS);
        assert_eq!(analysis_values(None).len(), ANALYSIS_COLUMNS);
        // The id, the columns copied as they are, the analysis, and how many numbers its CLAP has.
        assert_eq!(INSERT_SOUND.matches('?').count(), 1 + AS_THEY_ARE.len() + ANALYSIS_COLUMNS + 1);
        assert_eq!(row_columns!().split(',').count(), 1 + AS_THEY_ARE.len() + ANALYSIS_COLUMNS);
    }

    #[test]
    fn the_copy_sits_beside_the_library() {
        let db = Path::new("C:/Users/x/AppData/Local/MusicOrganiser/sounds.db");
        assert_eq!(copy_of(db), Path::new("C:/Users/x/AppData/Local/MusicOrganiser/sounds-rust.db"));
        assert_eq!(side(&copy_of(db), "-wal"), Path::new("C:/Users/x/AppData/Local/MusicOrganiser/sounds-rust.db-wal"));
    }

    #[test]
    fn the_old_rows_columns_are_where_they_are_taken_from() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE sounds (id INTEGER PRIMARY KEY, path, size, mtime, error, analysis, features, clap, name_type,
                 user_type, type, type_source, type_conf, favourite, rating, plays, last_played)",
        )
        .unwrap();
        let st = conn.prepare(OLD_SOUNDS).unwrap();
        let name = |i: usize| st.column_name(i).unwrap().to_owned();
        let at = [OLD_ANALYSIS, OLD_FEATURES, OLD_CLAP, OLD_SIZE, OLD_LAST_PLAYED].map(name);
        assert_eq!(at, ["analysis", "features", "clap", "size", "last_played"]);
        // Copied as they are into the columns INSERT_SOUND names after the id.
        let start = INSERT_SOUND.find('(').unwrap() + 1;
        let named: Vec<&str> = INSERT_SOUND[start..].split(',').map(str::trim).collect();
        for (k, &i) in AS_THEY_ARE.iter().enumerate() {
            assert_eq!(name(i), named[k + 1], "column {k} after the id");
        }
        // Every column but the id, the analysis and the fingerprints is copied as it is, each once.
        let mut copied: Vec<String> = AS_THEY_ARE.iter().map(|&i| name(i)).collect();
        let mut rest: Vec<String> = (0..st.column_count())
            .map(name)
            .filter(|n| !["id", "analysis", "features", "clap"].contains(&n.as_str()))
            .collect();
        copied.sort();
        rest.sort();
        assert_eq!(copied, rest);
    }

    /// How SQLite reads the copy: how many sounds CLAP has heard from the index of those it has (a few MB) alone, not
    /// from the rows.
    #[test]
    fn the_copy_counts_what_clap_heard_from_its_index_alone() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn.execute_batch(INDEXES).unwrap();
        let plan = |sql: &str| -> String {
            let mut st = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
            let steps: Vec<String> = st.query_map([], |r| r.get(3)).unwrap().map(Result::unwrap).collect();
            steps.join("; ")
        };
        for sql in [
            "SELECT COUNT(*) FROM sounds WHERE clap_numbers IS NOT NULL",
            "SELECT COALESCE(MAX(n), 0) FROM (SELECT COUNT(*) AS n FROM sounds WHERE clap_numbers > 0 GROUP BY clap_numbers)",
        ] {
            assert!(plan(sql).contains("USING COVERING INDEX sounds_clap"), "{sql}: {}", plan(sql));
        }
    }

    #[test]
    fn a_fingerprint_is_whole_floats_or_counts_as_none() {
        assert_eq!(clap_numbers(ValueRef::Null), None);
        assert_eq!(clap_numbers(ValueRef::Blob(&[0; 2048])), Some(512));
        for odd in [ValueRef::Blob(&[]), ValueRef::Blob(&[1, 2, 3]), ValueRef::Text(b"abcd"), ValueRef::Integer(4)] {
            assert_eq!(clap_numbers(odd), Some(0), "{odd:?}");
            assert_eq!(whole_floats(odd), None, "{odd:?}");
        }
        assert_eq!(whole_floats(ValueRef::Blob(&[0, 0, 128, 63])), Some(vec![1.0]));
        assert!(same_value(ValueRef::Real(0.5), ValueRef::Real(0.5)));
        assert!(!same_value(ValueRef::Real(-0.0), ValueRef::Real(0.0)));
        assert!(!same_value(ValueRef::Text(b"a"), ValueRef::Blob(b"a")));
        assert!(!same_value(ValueRef::Integer(1), ValueRef::Real(1.0)));
    }

    #[test]
    fn a_copy_is_made_only_with_room_to_spare() {
        const GB: u64 = 1_000_000_000;
        // Solomon's C:, 2026-11-03: 12 GB free, a 4.6 GB library.
        assert!(room_for(12 * GB, 4_560_000_000 / 2, "making it").is_ok());
        assert!(room_for(2 * GB + ROOM_LEFT, 2 * GB, "making it").is_ok());
        let why = room_for(2 * GB + ROOM_LEFT - 1, 2 * GB, "making it").unwrap_err();
        assert!(why.starts_with("only 3.1 GB is free on its disk: making it takes up to 2.0 GB"), "{why}");
        // While it's written (an update's log, say), what's always left free must stay free.
        assert!(room_left(ROOM_LEFT).is_ok());
        assert_eq!(room_left(GB / 2).unwrap_err(), "only 0.5 GB is free on its disk, and 1.1 GB is always left free");
        assert!(Room::new(Path::new("no such folder")).look().is_ok(), "a disk that can't say is taken to have room");
    }

    #[test]
    fn the_tables_hold_what_dividing_gives() {
        for k in 0..1024u16 {
            let i = usize::from(k);
            assert_eq!(F64_THOUSANDTHS[i].to_bits(), (f64::from(k) / 1000.0).to_bits(), "{k}");
            assert_eq!(F32_THOUSANDTHS[i].to_bits(), ((f64::from(k) / 1000.0) as f32).to_bits(), "{k}");
        }
    }

    #[test]
    fn chords_come_back_to_the_last_bit() {
        let chords = vec![
            ChordSpan { start: 0.0, end: 0.1 + 0.2, name: "Cm".into() },
            ChordSpan { start: -0.0, end: f64::MIN_POSITIVE / 3.0, name: String::new() },
            ChordSpan { start: 1.234_567_890_123_456_7, end: 1e300, name: "Ab\"maj7\" é".into() },
        ];
        let Value::Blob(bytes) = packed_chords(&chords) else { panic!("no blob") };
        assert_eq!(bytes[0], CHORDS);
        let back = unpacked_chords(&bytes);
        assert_eq!(format!("{back:?}"), format!("{chords:?}"));
        assert_eq!(
            back.iter().map(|c| (c.start.to_bits(), c.end.to_bits())).collect::<Vec<_>>(),
            chords.iter().map(|c| (c.start.to_bits(), c.end.to_bits())).collect::<Vec<_>>()
        );
        assert_eq!(packed_chords(&[]), Value::Null);
        // Cut short, or with a name that isn't text: none rather than some.
        assert!(unpacked_chords(&bytes[..bytes.len() - 1]).is_empty());
        let mut bad = packed_chords(&chords[..1]);
        if let Value::Blob(b) = &mut bad {
            *b.last_mut().unwrap() = 0xff;
            assert!(unpacked_chords(b).is_empty());
        }
        assert!(unpacked_chords(&[AS_IS, 1, 2]).is_empty());
    }

    /// How SQLite reads `sounds.db` for the copy: every sound in the order the rows are stored, and the prints' hashes
    /// from the C# app's index of them alone (a print is about 3 KB; its hash is 16 bytes).
    #[test]
    fn sounds_db_is_read_the_quick_way() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE sounds (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE, size INTEGER,
                 mtime INTEGER, error TEXT, analysis TEXT, features BLOB, clap BLOB, name_type TEXT, user_type TEXT,
                 type TEXT, type_source TEXT, type_conf REAL, favourite INTEGER DEFAULT 0, rating INTEGER DEFAULT 0,
                 plays INTEGER DEFAULT 0, last_played INTEGER);
             CREATE TABLE sound_prints (sound_id INTEGER PRIMARY KEY REFERENCES sounds(id) ON DELETE CASCADE,
                 mtime INTEGER, size INTEGER, version INTEGER, exact BLOB, frames INTEGER, bands BLOB, keys BLOB,
                 sketch BLOB, seconds REAL, sfp BLOB, compared INTEGER NOT NULL DEFAULT 0);
             CREATE INDEX ix_sound_prints_exact ON sound_prints(exact);",
        )
        .unwrap();
        let plan =
            |sql: &str| -> String { conn.query_row(&format!("EXPLAIN QUERY PLAN {sql}"), [], |r| r.get(3)).unwrap() };
        assert_eq!(plan(OLD_SOUNDS), "SCAN sounds");
        assert!(plan(OLD_PRINTS).contains("USING COVERING INDEX ix_sound_prints_exact"), "{}", plan(OLD_PRINTS));
    }
}
