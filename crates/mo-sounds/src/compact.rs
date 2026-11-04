//! The Rust app's own sound library: `sounds-rust.db`, a compact copy of the C# app's `sounds.db`, kept beside it.
//!
//! The C# app's file keeps each sound's analysis as text, both of its fingerprints in the same row as its name and
//! stars, and the duplicate finder's prints beside them: 4.6 GB for Solomon's 453,885 sounds, of which the Sounds
//! page had to read 3.7 GB before it could list anything. The copy holds the same facts, stored compactly:
//!
//! - Each sound's row holds what the list shows, with the analysis as numbers (the chords' times too, which text
//!   doesn't always bring back to the last bit). The waveform and the 12 notes' chroma are packed as thousandths, the
//!   way the C# analyser rounds them, and only when each comes back as the very same number; otherwise they're kept as
//!   they are.
//! - The two fingerprints, the analyser's features and CLAP's, are in tables of their own, read after the list.
//! - Tags, plays, galaxy places, the copies the duplicate finder found, hidden copies and settings are copied table
//!   for table as the C# app made them, so the same questions get the same answers from either file.
//! - Of the duplicate finder's prints only the hash of exact copies is kept: the rest is for the C# app's own finder.
//!
//! [`sync`] makes the copy and keeps it up to date: it reads `sounds.db` once and writes only what changed since it
//! last looked. `sounds.db` itself is only ever read. [`crate::SoundsDb`] reads either file.
//!
//! The C# app marks no row as changed when it saves, so finding what changed means reading all of `sounds.db` again:
//! [`in_step`] tells from the files' sizes and times alone, in a millisecond, whether there's anything to find.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use chrono::NaiveDateTime;
use rusqlite::types::{ToSqlOutput, Value, ValueRef};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Row, Transaction, TransactionBehavior, params, params_from_iter,
};

use crate::db::{Reading, SOUND_COLUMNS, int_at, real_at, sound_at, sound_columns, text_at};
use crate::{ChordSpan, Everything, Sound, SoundAnalysis, SoundsDb, SoundsError};

/// The copy's name, beside `sounds.db`.
pub const FILE_NAME: &str = "sounds-rust.db";

/// The copy's own table: what it was made from and how (`format`, `stamp`, and a `hash:` for each table copied whole).
pub(crate) const INFO_TABLE: &str = "copy_info";

/// Goes up whenever the copy's layout or what goes into it changes: a copy of an older format is made afresh, and one
/// a newer version of the app made is left alone (that version would only have to make it again).
pub(crate) const FORMAT: i64 = 1;

/// SQLite's pages are 4 KB unless asked otherwise, and a CLAP fingerprint (512 numbers, 2 KB) can't share one with
/// another, so each took a whole page, half of it empty. Fifteen share a 32 KB page.
const PAGE_SIZE: i64 = 32_768;

/// The analysis's columns in the copy's `sounds` table, in the order [`analysis_values`] gives them and
/// [`analysis_at`] reads them. One list for the table, the writing and the reading, so they can't drift apart.
macro_rules! analysis_columns {
    () => {
        "seconds, sample_rate, channels, peak_db, rms_db, attack_ms, decay_ms, brightness, noisiness, width, is_loop, \
         bpm, bpm_source, pitch, pitch_conf, key_tonic, key_minor, key_conf, key_source, chord, chords, chroma, \
         character, waveform"
    };
}
pub(crate) use analysis_columns;

/// How many columns [`analysis_columns`] lists.
const ANALYSIS_COLUMNS: usize = 24;

/// The copy's tables. The C# app's tables that are copied whole (see [`WHOLE`]) are made as the C# app made them.
///
/// The analysis's columns have no type, so SQLite keeps each number exactly as it's given: a REAL column would store
/// -0.0 as 0, and the C# analyser's rounding gives -0.0 (a peak of -0.04 dB rounds to -0.0).
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
    "
     );
     CREATE TABLE sound_features (sound_id INTEGER PRIMARY KEY, v);
     CREATE TABLE sound_clap (sound_id INTEGER PRIMARY KEY, v);
     CREATE TABLE sound_hashes (sound_id INTEGER PRIMARY KEY, row INTEGER NOT NULL, features INTEGER NOT NULL,
         clap INTEGER NOT NULL, clap_len INTEGER);"
);

/// Made once the first copy's rows are in, which is quicker than keeping it up to date row by row.
const INDEXES: &str = "CREATE INDEX sounds_path ON sounds(path);";

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

/// The columns of [`OLD_SOUNDS`] a row's hash is made from: all but the id and the two fingerprints (each hashed on
/// its own).
const HASHED: [usize; 14] = [1, OLD_SIZE, 2, 3, OLD_ANALYSIS, 5, 6, 7, 8, 9, 10, 11, 12, OLD_LAST_PLAYED];

const INSERT_SOUND: &str = concat!(
    "INSERT OR REPLACE INTO sounds (id, path, size, mtime, error, name_type, user_type, type, type_source, type_conf,
         favourite, rating, plays, last_played, ",
    analysis_columns!(),
    ") VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22,
         ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32, ?33, ?34, ?35, ?36, ?37, ?38)"
);

/// The C# app's tables that are copied whole, rows, row numbers and indexes, as the C# app made them: the same
/// queries then read them the same way, in the same order, from either file.
const WHOLE: [&str; 6] = ["settings", "sound_tags", "sound_events", "sound_maps", "sound_matches", "sound_hidden"];

/// Of [`WHOLE`], the ones the page's list of sounds is read from: their tags, plays and hidden copies.
const LISTED: [&str; 3] = ["sound_tags", "sound_events", "sound_hidden"];

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

/// One [`sync`] at a time in this app: two at once would make the same half-made copy.
static SYNCING: Mutex<()> = Mutex::new(());

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
    /// The copy was made afresh: there was none, or one of an older format.
    pub made: bool,
    /// How many sounds the copy holds now.
    pub sounds: usize,
    /// Sounds new to the copy, ones that changed, and ones gone from `sounds.db` since.
    pub added: usize,
    pub changed: usize,
    pub removed: usize,
    /// How many of the tables copied whole (tags, plays, galaxy places…) had changed.
    pub tables: usize,
    /// Of those, how many the page's list of sounds is read from (tags, plays, hidden copies).
    pub listed_tables: usize,
    /// How many sounds gained, lost or changed a fingerprint (their features or CLAP's), the new and gone ones among
    /// them: while none did, fingerprints read from the copy before are still right.
    pub prints: usize,
}

impl Synced {
    /// Whether the page's list of sounds reads any differently now: not when only what the C# app keeps for itself
    /// changed (its settings, the galaxy's saved places, the duplicate finder's copies and prints).
    pub fn changed_the_list(&self) -> bool {
        self.made || self.added > 0 || self.changed > 0 || self.removed > 0 || self.listed_tables > 0
    }
}

/// Where the copy of the sound library at `sounds_db` is kept: beside it.
pub fn copy_of(sounds_db: &Path) -> PathBuf {
    sounds_db.with_file_name(FILE_NAME)
}

/// Whether `copy` is up to date with `sounds_db`: made by this version of the app from `sounds_db` as it is now. Looks
/// only at the files' sizes and times, SQLite's note of its log ([`SoundsDb::stamp`]) and the copy's own note, and
/// never waits: a copy that's busy just then counts as not up to date.
pub fn in_step(sounds_db: &Path, copy: &Path) -> bool {
    let now = SoundsDb::stamp(sounds_db);
    matches!(look_at(copy, Duration::ZERO), Ok(Found::Copy { format: FORMAT, stamp: Some(made_from) }) if made_from == now)
}

/// Brings `copy` up to date with the sound library at `sounds_db`, making it when there's none (or one of an older
/// format). Reads `sounds_db` once, all of it, at one moment, and never changes it.
///
/// A new copy is made under another name, checked, and only then given its own name, so a copy that's there is
/// always whole. An update happens in one transaction: if anything goes wrong, the copy stays as it was.
///
/// Nothing but a copy is ever removed or replaced: a database of anything else where the copy goes (or the library
/// itself) is left as it is, and this says so. Nor is one made on a disk without room for it and
/// [`ROOM_LEFT`] to spare.
pub fn sync(sounds_db: &Path, copy: &Path) -> Result<Synced, SoundsError> {
    run(sounds_db, copy, None, &mut None, false)
}

/// The Sounds page's first read of the library: the quickest way to its sounds there is.
pub enum PageRead {
    /// From the copy, a fraction of the reading: the sounds now, their fingerprints after, from `db`
    /// ([`SoundsDb::fingerprints`]). `behind`: `sounds.db` has changed since the copy was last brought up to date,
    /// so these are as they were then; [`sync`] it, then read it again.
    Copy { db: SoundsDb, sounds: Vec<Sound>, behind: bool },
    /// From `sounds.db`, the fingerprints too, on the one pass that made the copy (`made` says how that went): there
    /// was no copy this version of the app can read.
    Library { everything: Everything, made: Result<Synced, SoundsError> },
}

/// The Sounds page's first read of the sound library at `sounds_db`: from the copy beside it whenever there's one to
/// read, even one that's behind (the page shows it at once and brings it up to date after), else from `sounds_db`
/// while the copy is made on the same pass ([`sync_reading`]). `now` is the page's "now", for what counts as recent.
pub fn read_for_page(sounds_db: &Path, now: NaiveDateTime) -> Result<PageRead, SoundsError> {
    let copy = copy_of(sounds_db);
    // Looked at before reading, so a save the C# app makes while this reads counts as one next time.
    let behind = !in_step(sounds_db, &copy);
    let mut damaged = false;
    if let Ok(db) = SoundsDb::open_read_only(&copy)
        && db.is_copy()
    {
        match db.all(now) {
            Ok(sounds) => {
                // A copy left half made by an app closed while it made one goes now, not only the next time one is
                // made: that may be never, and it's most of the copy's size (on a disk that was nearly full).
                remove_parts(&copy, false);
                return Ok(PageRead::Copy { db, sounds, behind });
            }
            Err(e) => damaged = e.damaged(),
        }
    }
    // None, or none this version can read: made (or brought up to date) on the page's own read. One damaged inside
    // is made again: brought up to date, it would stay damaged.
    let (everything, made) =
        if damaged { remake_reading(sounds_db, &copy, now)? } else { sync_reading(sounds_db, &copy, now)? };
    Ok(PageRead::Library { everything, made })
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

/// [`sync_reading`] or [`remake_reading`].
fn read_while(
    sounds_db: &Path,
    copy: &Path,
    now: NaiveDateTime,
    afresh: bool,
) -> Result<(Everything, Result<Synced, SoundsError>), SoundsError> {
    let mut read = None;
    let synced = run(sounds_db, copy, Some(now), &mut read, afresh);
    let everything = match read {
        Some(everything) => everything,
        None => SoundsDb::open_read_only(sounds_db)?.everything(now)?,
    };
    Ok((everything, synced))
}

/// What [`sync`] does, also reading the library for the page into `read` on the way when `read_for` says when "now"
/// is ([`sync_reading`]). `read` is left as `None` if the pass stopped before every sound was read. `afresh`: a copy
/// of this version is made again rather than brought up to date ([`remake_reading`]).
fn run(
    sounds_db: &Path,
    copy: &Path,
    read_for: Option<NaiveDateTime>,
    read: &mut Option<Everything>,
    afresh: bool,
) -> Result<Synced, SoundsError> {
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
    match found {
        Found::Copy { format: FORMAT, .. } if !afresh => {
            match update(&old, copy, Pass { stamp: &stamp, read_for, read: &mut *read, dir }) {
                // Damaged where only writing to it reaches (an index, say): every update would stop there, so it's
                // made again. Not when it's `sounds.db` that's damaged: the copy is all the page has then.
                Err(Failed { damaged: true, .. }) if copy_damaged(copy) => {
                    make(&old, copy, Pass { stamp: &stamp, read_for, read, dir })
                }
                done => done.map_err(|Failed { why, .. }| failed(why)),
            }
        }
        Found::Copy { format, .. } if format > FORMAT => {
            Err(failed(format!("a newer version of the app made it (format {format}), so it's left as it is")))
        }
        // Older, or (`afresh`) this version's made again.
        Found::Nothing | Found::Unreadable | Found::Copy { .. } => {
            make(&old, copy, Pass { stamp: &stamp, read_for, read, dir })
        }
        Found::Foreign => {
            Err(failed("a database that isn't a copy of the library is there, so it's left as it is".into()))
        }
    }
}

/// Whether SQLite finds the copy at `copy` damaged when it looks it over, every page (as long as reading the copy
/// takes): `false` when it can't tell (the copy can't be opened just now).
fn copy_damaged(copy: &Path) -> bool {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let Ok(conn) = Connection::open_with_flags(copy, flags) else { return false };
    match conn.query_row("PRAGMA quick_check(1)", [], |r| r.get::<_, String>(0)) {
        Ok(said) => said != "ok",
        Err(e) => crate::db::is_damage(&e),
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

/// What one pass over `sounds.db` is for: the copy brought up to `stamp`, and (when `read_for` says when "now" is) the
/// page's read into `read`. `dir`: the folder the copy is written in, its disk watched for room meanwhile ([`Room`]).
struct Pass<'a> {
    stamp: &'a str,
    read_for: Option<NaiveDateTime>,
    read: &'a mut Option<Everything>,
    dir: &'a Path,
}

/// A new copy, made under a name of its own beside the copy's and then renamed.
fn make(old: &SoundsDb, copy: &Path, pass: Pass<'_>) -> Result<Synced, SoundsError> {
    let failed = |why: String| SoundsError::Copy { path: copy.to_owned(), why };
    if let (Ok(free), Ok(library)) = (fs4::available_space(pass.dir), fs::metadata(old.path())) {
        // What's there now and goes first counts as free.
        let going: u64 =
            ["", "-wal", "-shm"].iter().filter_map(|end| fs::metadata(side(copy, end)).ok()).map(|m| m.len()).sum();
        room_for(free.saturating_add(going), library.len() / 2, "making it").map_err(failed)?;
    }
    // A copy of an older format (or a file that can't be read) goes first, with SQLite's files beside it: a log left
    // beside the new one would be read into it as if it were its own.
    remove_database(copy).map_err(|e| failed(format!("the old copy couldn't be removed ({e})")))?;
    let part = part_of(copy);
    remove_database(&part).map_err(|e| failed(format!("a half-made copy couldn't be removed ({e})")))?;
    let made = (|| -> Result<Synced, Failed> {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = open_for_writing(&part, flags)?;
        // A file being made needs no journal: if this stops half way, the file is thrown away.
        conn.pragma_update(None, "page_size", PAGE_SIZE)?;
        conn.pragma_update(None, "journal_mode", "OFF")?;
        conn.pragma_update(None, "synchronous", "OFF")?;
        conn.execute_batch(SCHEMA)?;
        let synced = fill(old, &conn, true, pass)?;
        // From now on it's updated while the page may be reading it: in WAL mode readers never wait for a writer.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.close().map_err(|(_, e)| e)?;
        // On the disk before it has its name, so a copy with its name is whole even after a power cut.
        fs::OpenOptions::new().write(true).open(&part)?.sync_all()?;
        Ok(synced)
    })();
    let synced = match made {
        Ok(synced) => synced,
        Err(Failed { why, .. }) => {
            let _ = remove_database(&part);
            return Err(failed(why));
        }
    };
    if let Err(e) = fs::rename(&part, copy) {
        let _ = remove_database(&part);
        return Err(failed(format!("the new copy couldn't be given its name ({e})")));
    }
    Ok(synced)
}

/// Whether a disk with `free` bytes free has room for work on the copy that `takes` up to so many bytes more, with
/// [`ROOM_LEFT`] to spare. Making a copy takes under half the library's size (0.29 of it with a library like
/// Solomon's), known before it starts; what bringing one up to date takes is known only as it's written ([`Room`]).
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

/// How many sounds are written between two looks at the room left on the disk ([`Room`]): a few MB of the copy, or
/// some tens of MB of an update's log when each one changes a page of its own.
const ROOM_EVERY: usize = 256;

/// Watches the disk the copy is written on, so the work stops (and what it wrote is undone) before less than
/// [`ROOM_LEFT`] is free there: an update's log grows by every page it changes, as far as the copy's own size again
/// when every sound changed, and other programs fill the disk meanwhile too.
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

/// The copy brought up to date in place, in one transaction.
fn update(old: &SoundsDb, copy: &Path, pass: Pass<'_>) -> Result<Synced, Failed> {
    // What it takes is known only as it's written: started with room to spare, then watched.
    Room::new(pass.dir).look()?;
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = open_for_writing(copy, flags)?;
    conn.busy_timeout(WAIT)?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    let synced = fill(old, &conn, false, pass)?;
    // What this update wrote goes into the file itself, and the log beside it back to nothing: the disk the
    // library is on may be nearly full. Not done if the page is reading just then (without waiting for it): SQLite
    // does it with a later save.
    if conn.busy_timeout(Duration::ZERO).is_ok() {
        let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
    }
    Ok(synced)
}

/// The copy, opened to be written to. The tables copied whole keep the C# app's `REFERENCES sounds(id) ON DELETE
/// CASCADE`, and the SQLite built into the app checks those unless told not to: replacing a sound's row would then
/// delete its tags, plays and galaxy places from the copy, and a row `sounds.db` holds for a sound it no longer has
/// couldn't be copied. The copy holds what `sounds.db` holds, so it checks nothing of its own.
fn open_for_writing(path: &Path, flags: OpenFlags) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(path, flags)?;
    conn.pragma_update(None, "foreign_keys", false)?;
    Ok(conn)
}

/// What each sound in the copy was copied from: the hashes of its row in `sounds.db`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Hashes {
    /// Everything but the two fingerprints.
    row: i64,
    features: i64,
    clap: i64,
}

fn known(conn: &Connection) -> rusqlite::Result<HashMap<i64, Hashes>> {
    let mut st = conn.prepare("SELECT sound_id, row, features, clap FROM sound_hashes")?;
    let rows =
        st.query_map([], |r| Ok((r.get(0)?, Hashes { row: r.get(1)?, features: r.get(2)?, clap: r.get(3)? })))?;
    rows.collect()
}

/// Writes into the copy at `conn` what's changed in `old` since the copy was last brought up to date, all in one
/// transaction, reading `old` at one moment. `made`: `conn` is a new, empty copy. The page's read, when the pass is
/// for it too, is kept from the moment the last sound is read, whatever happens to the copy after: the page has it
/// once the pass ends (a new copy's last few seconds, its indexes and its trip to the disk, come first).
fn fill(old: &SoundsDb, conn: &Connection, made: bool, pass: Pass<'_>) -> Result<Synced, Failed> {
    let mut synced = Synced { made, ..Synced::default() };
    let snapshot = old.conn.unchecked_transaction()?;
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    // Read once the copy is this update's alone, so another copy of the app updating it too can't be missed.
    let mut known = if made { HashMap::new() } else { known(&tx)? };
    // From the same moment as the rows: the page gets what the copy gets.
    let mut reading =
        pass.read_for.map(|now| Reading::start(old, now, true)).transpose().map_err(|e| Failed::new(e.to_string()))?;
    // A fingerprint's hash when there's none.
    let none = hash_of(ValueRef::Null);
    let mut room = Room::new(pass.dir);
    let mut st = old.conn.prepare(OLD_SOUNDS)?;
    let mut rows = st.query([])?;
    while let Some(r) = rows.next()? {
        synced.sounds += 1;
        let id: i64 = r.get(0)?;
        // The page's sound, when the pass reads for it too: its analysis is read once, for both.
        let sound = if reading.is_some() { Some(sound_at(r)?) } else { None };
        let now = Hashes {
            row: hash_row(r)?,
            features: hash_of(r.get_ref(OLD_FEATURES)?),
            clap: hash_of(r.get_ref(OLD_CLAP)?),
        };
        let was = known.remove(&id);
        if was.map_or((none, none), |w| (w.features, w.clap)) != (now.features, now.clap) {
            synced.prints += 1;
        }
        if was != Some(now) {
            if was.is_none_or(|w| w.row != now.row) {
                let read_here;
                let analysis = match &sound {
                    Some(s) => s.analysis.as_ref(),
                    None => {
                        read_here = crate::db::analysis_at(r, OLD_ANALYSIS);
                        read_here.as_ref()
                    }
                };
                write_sound(&tx, id, r, analysis)?;
            }
            if was.is_none_or(|w| w.features != now.features) {
                write_vector(&tx, "sound_features", id, r.get_ref(OLD_FEATURES)?)?;
            }
            if was.is_none_or(|w| w.clap != now.clap) {
                write_vector(&tx, "sound_clap", id, r.get_ref(OLD_CLAP)?)?;
            }
            tx.prepare_cached("INSERT OR REPLACE INTO sound_hashes VALUES (?1, ?2, ?3, ?4, ?5)")?.execute(params![
                id,
                now.row,
                now.features,
                now.clap,
                clap_len(r.get_ref(OLD_CLAP)?)
            ])?;
            if was.is_some() {
                synced.changed += 1;
            } else {
                synced.added += 1;
            }
            room.wrote()?;
        }
        if let (Some(reading), Some(sound)) = (reading.as_mut(), sound) {
            reading.add_read(r, sound);
        }
    }
    drop(rows);
    drop(st);
    *pass.read = reading.map(Reading::finish);
    // What's left wasn't in `sounds.db` any more.
    for (id, was) in known {
        for table in ["sounds", "sound_features", "sound_clap", "sound_hashes"] {
            let key = if table == "sounds" { "id" } else { "sound_id" };
            tx.prepare_cached(&format!("DELETE FROM {table} WHERE {key} = ?1"))?.execute([id])?;
        }
        synced.removed += 1;
        if (was.features, was.clap) != (none, none) {
            synced.prints += 1;
        }
        room.wrote()?;
    }
    for table in WHOLE {
        room.look()?;
        if copy_whole(&old.conn, &tx, table)? {
            synced.tables += 1;
            if LISTED.contains(&table) {
                synced.listed_tables += 1;
            }
        }
    }
    room.look()?;
    if copy_prints(old, &tx)? {
        synced.tables += 1;
    }
    room.look()?;
    if made {
        tx.execute_batch(INDEXES)?;
    }
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
    tx.commit()?;
    snapshot.finish()?;
    Ok(synced)
}

/// One sound's row in the copy, from its row in `sounds.db` and the `analysis` read from it: the analysis as numbers,
/// everything else as it is.
fn write_sound(tx: &Transaction<'_>, id: i64, r: &Row<'_>, analysis: Option<&SoundAnalysis>) -> rusqlite::Result<()> {
    // Each value passed on as SQLite gave it, even text that isn't readable as text (a damaged row).
    let mut values: Vec<ToSqlOutput<'_>> = Vec::with_capacity(1 + AS_THEY_ARE.len() + ANALYSIS_COLUMNS);
    values.push(ToSqlOutput::Owned(Value::Integer(id)));
    for i in AS_THEY_ARE {
        values.push(ToSqlOutput::Borrowed(r.get_ref(i)?));
    }
    values.extend(analysis_values(analysis).into_iter().map(ToSqlOutput::Owned));
    tx.prepare_cached(INSERT_SOUND)?.execute(params_from_iter(values))?;
    Ok(())
}

/// One sound's fingerprint in `table`, as it is (none when it has none).
fn write_vector(tx: &Transaction<'_>, table: &str, id: i64, v: ValueRef<'_>) -> rusqlite::Result<()> {
    if v == ValueRef::Null {
        tx.prepare_cached(&format!("DELETE FROM {table} WHERE sound_id = ?1"))?.execute([id])?;
    } else {
        tx.prepare_cached(&format!("INSERT OR REPLACE INTO {table} (sound_id, v) VALUES (?1, ?2)"))?
            .execute(params![id, ToSqlOutput::Borrowed(v)])?;
    }
    Ok(())
}

/// Copies `table` whole from `old` into the copy when it's changed since the last time (or isn't there yet): made
/// as the C# app made it, with the same row numbers and the same indexes. Gone from the copy when it's gone from
/// `old`. Whether the copy changed.
fn copy_whole(old: &Connection, tx: &Transaction<'_>, table: &str) -> rusqlite::Result<bool> {
    let made = table_sql(old, table)?;
    let had = table_sql(tx, table)?.is_some();
    let Some(made) = made else {
        if had {
            tx.execute_batch(&format!("DROP TABLE {}", quoted(table)))?;
        }
        return Ok(had);
    };
    let indexes = index_sqls(old, table)?;
    let columns = Columns::of(old, table, &made)?;
    let select = format!("SELECT {} FROM {}", columns.list(), quoted(table));
    let mut hash = Hash::new();
    hash.bytes(made.as_bytes());
    for sql in &indexes {
        hash.bytes(sql.as_bytes());
    }
    hash_rows(old, &select, &mut hash)?;
    let key = format!("hash:{table}");
    if had && info_value(tx, &key)? == Some(hash.get()) {
        return Ok(false);
    }
    tx.execute_batch(&format!("DROP TABLE IF EXISTS {}", quoted(table)))?;
    tx.execute_batch(&made)?;
    let placeholders: Vec<String> = (1..=columns.names.len()).map(|i| format!("?{i}")).collect();
    let insert = format!("INSERT INTO {} ({}) VALUES ({})", quoted(table), columns.list(), placeholders.join(", "));
    copy_rows(old, &select, tx, &insert)?;
    for sql in &indexes {
        tx.execute_batch(sql)?;
    }
    set_info(tx, &key, hash.get())?;
    Ok(true)
}

/// Copies the hashes of exact copies from the duplicate finder's prints, when they've changed. Whether they had.
fn copy_prints(old: &SoundsDb, tx: &Transaction<'_>) -> rusqlite::Result<bool> {
    let had = table_sql(tx, "sound_prints")?.is_some();
    if !old.has("sound_prints") {
        if had {
            tx.execute_batch("DROP TABLE sound_prints")?;
        }
        return Ok(had);
    }
    let mut hash = Hash::new();
    hash_rows(&old.conn, OLD_PRINTS, &mut hash)?;
    if had && info_value(tx, "hash:sound_prints")? == Some(hash.get()) {
        return Ok(false);
    }
    tx.execute_batch("DROP TABLE IF EXISTS sound_prints")?;
    tx.execute_batch(PRINTS)?;
    copy_rows(&old.conn, OLD_PRINTS, tx, "INSERT INTO sound_prints (sound_id, exact) VALUES (?1, ?2)")?;
    set_info(tx, "hash:sound_prints", hash.get())?;
    Ok(true)
}

/// A table's columns as a copy of it reads and writes them: its row number first when it has one of its own (one
/// that isn't also a column), so the copy's rows keep their numbers and come out in the same order.
struct Columns {
    names: Vec<String>,
}

impl Columns {
    fn of(conn: &Connection, table: &str, sql: &str) -> rusqlite::Result<Columns> {
        let mut st = conn.prepare("SELECT name, type, pk FROM pragma_table_info(?1)")?;
        let info: Vec<(String, String, i64)> =
            st.query_map([table], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
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

/// Adds every value of every row `select` gives to `hash`, in the order SQLite gives them.
fn hash_rows(conn: &Connection, select: &str, hash: &mut Hash) -> rusqlite::Result<()> {
    let mut st = conn.prepare(select)?;
    let n = st.column_count();
    let mut rows = st.query([])?;
    while let Some(r) = rows.next()? {
        for i in 0..n {
            hash.value(r.get_ref(i)?);
        }
    }
    Ok(())
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

fn table_sql(conn: &Connection, table: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1", [table], |r| r.get(0)).optional()
}

/// The indexes made on `table` by name (those its own keys make, which come with the table, have none), by name.
fn index_sqls(conn: &Connection, table: &str) -> rusqlite::Result<Vec<String>> {
    let mut st = conn.prepare(
        "SELECT sql FROM sqlite_master WHERE type = 'index' AND tbl_name = ?1 AND sql IS NOT NULL ORDER BY name",
    )?;
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
    /// A copy, laid out in `format`, and the stamp of the `sounds.db` it was last brought up to date with.
    Copy {
        format: i64,
        stamp: Option<String>,
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
        Ok(Found::Copy { format, stamp })
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
        packed(&a.waveform),
    ]
}

/// The analysis from a row of the copy whose analysis columns start at `at`; `None` when the sound has none.
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
        bpm_source: text(12).unwrap_or_default(),
        pitch: real(13),
        pitch_confidence: real(14).unwrap_or(0.0),
        key_tonic: int(15),
        key_minor: int(16) == Some(1),
        key_confidence: real(17).unwrap_or(0.0),
        key_source: text(18).unwrap_or_default(),
        chord: text(19),
        chords: match get(20) {
            Some(ValueRef::Blob(b)) => unpacked_chords(b),
            _ => from_json_list(text(20).as_deref()),
        },
        chroma: unpacked(blob(21)),
        character: from_json_list(text(22).as_deref()),
        waveform: unpacked(blob(23)),
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
trait Packable: Copy + 'static {
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

const F32_THOUSANDTHS: [f32; 1024] = {
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
fn thousandths<T: Packable>(x: T) -> Option<u16> {
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
        Some(ks) => {
            let mut out = Vec::with_capacity(1 + (ks.len() * 10).div_ceil(8));
            out.push(THOUSANDTHS);
            let (mut bits, mut held) = (0u32, 0u32);
            for k in ks {
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

/// A list [`packed`] packed. Empty when there's none (or it isn't one).
fn unpacked<T: Packable>(bytes: &[u8]) -> Vec<T> {
    match bytes.split_first() {
        Some((&THOUSANDTHS, rest)) => {
            // Each byte holds 8 bits and each number 10, and the last byte's spare bits are fewer than 10.
            let n = rest.len() * 8 / 10;
            // Four numbers to every 5 bytes, the first in the lowest bits; then the last few, in fewer bytes.
            let number = |word: u64, j: u32| T::THOUSANDTHS[((word >> (10 * j)) & 0x3ff) as usize];
            let mut out = Vec::with_capacity(n);
            let (fives, last) = rest.as_chunks::<5>();
            for b in fives {
                let word = u64::from(b[0])
                    | (u64::from(b[1]) << 8)
                    | (u64::from(b[2]) << 16)
                    | (u64::from(b[3]) << 24)
                    | (u64::from(b[4]) << 32);
                out.extend_from_slice(&[number(word, 0), number(word, 1), number(word, 2), number(word, 3)]);
            }
            let word = last.iter().rev().fold(0u64, |word, &b| (word << 8) | u64::from(b));
            for j in 0..(n - out.len()) as u32 {
                out.push(number(word, j));
            }
            out
        }
        Some((&AS_IS, rest)) => rest.chunks_exact(T::SIZE).map(T::take).collect(),
        _ => Vec::new(),
    }
}

/// A 64-bit FNV-1a hash: simple, and the same in every version of the app (Rust's own may change between releases),
/// so a copy made by one version can be brought up to date by the next.
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

/// How long a sound's CLAP fingerprint is, in bytes, kept beside its hashes: so how many sounds CLAP has listened to,
/// and with how many numbers, can be told without reading the fingerprints (a gigabyte of them for half a million
/// sounds). None when it has none, 0 when what's there isn't a fingerprint (a damaged row).
fn clap_len(v: ValueRef<'_>) -> Option<i64> {
    match v {
        ValueRef::Null => None,
        ValueRef::Blob(b) => Some(i64::try_from(b.len()).unwrap_or(i64::MAX)),
        _ => Some(0),
    }
}

/// The hash of a row of [`OLD_SOUNDS`] without its fingerprints.
fn hash_row(r: &Row<'_>) -> rusqlite::Result<i64> {
    let mut hash = Hash::new();
    for i in HASHED {
        hash.value(r.get_ref(i)?);
    }
    Ok(hash.get())
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
    fn the_hash_stays_the_same_from_version_to_version() {
        // A copy made by one version is brought up to date by the next: the hash of a row mustn't change, or every
        // sound would be copied again.
        let mut hash = Hash::new();
        for v in [
            ValueRef::Null,
            ValueRef::Integer(-7),
            ValueRef::Real(-0.0),
            ValueRef::Text(b"Kick"),
            ValueRef::Blob(&[1, 2]),
        ] {
            hash.value(v);
        }
        // Worked out apart from this code too (FNV-1a over the same bytes, in Python).
        assert_eq!(hash.get(), 0xd32e_0959_9259_9eb6_u64 as i64);
        // Text and a blob of the same bytes, or two values run together, hash differently.
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
        assert_eq!(INSERT_SOUND.matches('?').count(), 1 + AS_THEY_ARE.len() + ANALYSIS_COLUMNS);
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
        // Hashed: every column but the id and the fingerprints, each once.
        let mut hashed: Vec<String> = HASHED.iter().map(|&i| name(i)).collect();
        let mut rest: Vec<String> =
            (0..st.column_count()).map(name).filter(|n| !["id", "features", "clap"].contains(&n.as_str())).collect();
        hashed.sort();
        rest.sort();
        assert_eq!(hashed, rest);
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
