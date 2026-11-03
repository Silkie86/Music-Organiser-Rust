//! Reading `sounds.db`, the C# app's sound library, without ever changing it.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use chrono::{Local, NaiveDateTime};
use rayon::prelude::*;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row, params};

use crate::learn::Vectors;
use crate::{Sound, SoundActivity, SoundAnalysis, event, types};

/// What went wrong reading the sound library.
#[derive(Debug)]
pub enum SoundsError {
    /// The file isn't there, can't be opened, or isn't a Music Organiser sound library.
    Open { path: PathBuf, why: String },
    /// A read failed (the file is damaged, or locked for longer than the wait).
    Read(rusqlite::Error),
}

impl fmt::Display for SoundsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SoundsError::Open { path, why } => write!(f, "Couldn't open the sound library {}: {why}", path.display()),
            SoundsError::Read(e) => write!(f, "Couldn't read the sound library: {e}"),
        }
    }
}

impl std::error::Error for SoundsError {}

impl From<rusqlite::Error> for SoundsError {
    fn from(e: rusqlite::Error) -> SoundsError {
        SoundsError::Read(e)
    }
}

/// Where a sound sits on a galaxy map.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StarPoint {
    pub x: f32,
    pub y: f32,
}

/// Two sounds the duplicate finder found to be copies (`a` is the smaller id).
#[derive(Clone, Debug, PartialEq)]
pub struct SoundMatch {
    pub a: i64,
    pub b: i64,
    /// "Exact", "Converted" or "Trimmed", as the C# app stores it.
    pub kind: String,
    pub score: f64,
    /// Where the shorter starts in the longer, in seconds.
    pub offset: f64,
    /// How far apart the two sound where they overlap, in dB: at worst, and typically (`None` until checked).
    pub apart: Option<f64>,
    pub typical: Option<f64>,
}

/// One tag, with how many sounds you gave it and how many the app suggested.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagCount {
    pub tag: String,
    pub yours: i64,
    pub learned: i64,
}

/// A file the library knows, as the last scan saw it: for the next scan to tell what's new or changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnownSound {
    pub path: String,
    pub size: i64,
    pub mtime: i64,
    pub has_clap: bool,
}

/// Everything [`SoundsDb::everything`] reads in its one pass over the library.
#[derive(Clone, Debug, Default)]
pub struct Everything {
    /// Every sound, as [`SoundsDb::all`] gives them.
    pub sounds: Vec<Sound>,
    /// Each sound's vector from the analyser, as [`SoundsDb::vectors`] gives them for "features".
    pub features: Vectors,
    /// Each sound's vector from CLAP, as [`SoundsDb::vectors`] gives them for "clap".
    pub clap: Vectors,
    /// How many connections read the sounds at once: 1 when the C# app kept saving while they started.
    pub readers: usize,
}

/// The most connections one read of the whole library uses at once: enough to keep a fast disk busy.
const MOST_READERS: usize = 16;

/// How many stretches of ids each reader gets on average: enough that they all finish at about the same time.
const STRETCHES_PER_READER: usize = 32;

/// How many times the readers are started again when the C# app saves while they start, before one reads it all.
const TRIES: usize = 3;

/// A sound's columns, as [`sound_at`] takes them.
const SOUND_SQL: &str = "SELECT id, path, mtime, error, analysis, name_type, user_type, type, type_source, type_conf,
        favourite, rating, plays
    FROM sounds WHERE id BETWEEN ?1 AND ?2";

/// The same with its two vectors after them, the analyser's and CLAP's.
const SOUND_AND_VECTORS_SQL: &str = "SELECT id, path, mtime, error, analysis, name_type, user_type, type, type_source,
        type_conf, favourite, rating, plays, features, clap
    FROM sounds WHERE id BETWEEN ?1 AND ?2";

/// The sound library, open for reading only: SQLite itself refuses any change, so the C# app's data is safe.
///
/// SQLite may leave `sounds.db-wal` and `sounds.db-shm` beside the file after reading (it needs them to read while the
/// C# app writes). They hold nothing of their own.
pub struct SoundsDb {
    conn: Connection,
    path: PathBuf,
    /// The tables this library has: an older C# app's may lack the duplicate finder's, which then read as empty.
    tables: HashSet<String>,
}

impl SoundsDb {
    /// Opens the sound library at `path` for reading. It must already exist: this never makes one.
    pub fn open_read_only(path: &Path) -> Result<SoundsDb, SoundsError> {
        let open_error = |why: String| SoundsError::Open { path: path.to_owned(), why };
        let conn = connect(path).map_err(|e| open_error(e.to_string()))?;
        let tables: HashSet<String> = {
            let mut st = conn
                .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
                .map_err(|e| open_error(format!("it isn't a Music Organiser sound library ({e})")))?;
            let names = st.query_map([], |r| r.get::<_, String>(0)).and_then(|rows| rows.collect());
            names.map_err(|e: rusqlite::Error| open_error(e.to_string()))?
        };
        if !tables.contains("sounds") {
            return Err(open_error("it isn't a Music Organiser sound library (it has no sounds table)".into()));
        }
        Ok(SoundsDb { conn, path: path.to_owned(), tables })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn has(&self, table: &str) -> bool {
        self.tables.contains(table)
    }

    /// A setting the C# app keeps in `sounds.db` (the classifier's weights, the galaxy's basis…), if set.
    pub fn setting(&self, key: &str) -> Result<Option<String>, SoundsError> {
        if !self.has("settings") {
            return Ok(None);
        }
        let v = self
            .conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get::<_, Option<String>>(0))
            .optional()?;
        Ok(v.flatten())
    }

    pub fn count(&self) -> Result<i64, SoundsError> {
        Ok(self.conn.query_row("SELECT COUNT(*) FROM sounds", [], |r| r.get(0))?)
    }

    /// How many sounds the CLAP model has listened to.
    pub fn count_with_clap(&self) -> Result<i64, SoundsError> {
        Ok(self.conn.query_row("SELECT COUNT(*) FROM sounds WHERE clap IS NOT NULL", [], |r| r.get(0))?)
    }

    /// Every file the library knows, as the last scan saw it.
    pub fn known_files(&self) -> Result<Vec<KnownSound>, SoundsError> {
        let mut st =
            self.conn.prepare("SELECT path, COALESCE(size, 0), COALESCE(mtime, 0), clap IS NOT NULL FROM sounds")?;
        let rows = st.query_map([], |r| {
            Ok(KnownSound { path: r.get(0)?, size: r.get(1)?, mtime: r.get(2)?, has_clap: r.get(3)? })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Every sound, ordered by path ignoring letter case (as the C# app lists them), with its tags, activity and
    /// whether you've hidden it as a duplicate. `now` sets what counts as recent activity: the year before it.
    ///
    /// Read on several connections at once (one for each thread of the rayon pool it's called on, up to 16), all
    /// seeing the library at the same moment, so call it on a low-priority pool (rule 21).
    pub fn all(&self, now: NaiveDateTime) -> Result<Vec<Sound>, SoundsError> {
        Ok(self.read_sounds(now, false)?.sounds)
    }

    /// Every sound as [`SoundsDb::all`] gives them, with each one's vectors as [`SoundsDb::vectors`] gives them, from
    /// one pass over the library. Each row keeps its vectors before its type and stars, so reading the list already
    /// goes through them: taking them on the way saves reading the whole file twice more.
    pub fn everything(&self, now: NaiveDateTime) -> Result<Everything, SoundsError> {
        self.read_sounds(now, true)
    }

    fn read_sounds(&self, now: NaiveDateTime, vectors: bool) -> Result<Everything, SoundsError> {
        // Each connection reads one page at a time and waits for the disk in between; several at once keep the disk
        // busy (a cold read of 453,885 sounds took minutes on one) and parse the analyses on every core.
        let mut readers = self.readers(rayon::current_num_threads().clamp(1, MOST_READERS))?.into_iter();
        let count = readers.len();
        let Some(first) = readers.next() else { return Ok(Everything::default()) };
        // Two lookups, each going straight to an end of the table. Asked together in one query, SQLite reads every
        // id to find them both.
        let (low, high): (Option<i64>, Option<i64>) =
            first.query_row("SELECT (SELECT MIN(id) FROM sounds), (SELECT MAX(id) FROM sounds)", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?;
        let stretches = match (low, high) {
            (Some(low), Some(high)) => stretches(low, high, count * STRETCHES_PER_READER),
            _ => Vec::new(),
        };
        let next = AtomicUsize::new(0);
        let failed = AtomicBool::new(false);
        let read: Mutex<Vec<StretchRead>> = Mutex::new(Vec::new());
        // Each reader takes the next stretch until there are none left (or one went wrong).
        let work = |conn: &Connection| {
            while !failed.load(Ordering::Relaxed) {
                let k = next.fetch_add(1, Ordering::Relaxed);
                let Some(&(first, last)) = stretches.get(k) else { break };
                let got = read_stretch(conn, first, last, vectors);
                if got.is_err() {
                    failed.store(true, Ordering::Relaxed);
                }
                read.lock().unwrap_or_else(PoisonError::into_inner).push((k, got));
            }
        };
        // Only what can go to other threads: not `self`, whose connection stays on this one.
        let (work, failed, tables) = (&work, &failed, &self.tables);
        // The small tables on the first reader, while the others start on the sounds; then it joins them.
        let mine = move || {
            let side = Side::read(&first, tables, now);
            if side.is_err() {
                failed.store(true, Ordering::Relaxed);
            }
            work(&first);
            side
        };
        let side = rayon::scope(move |scope| {
            for conn in readers {
                scope.spawn(move |_| work(&conn));
            }
            mine()
        });
        let Side { mut activity, mut tags, hidden } = side?;
        let mut read = read.into_inner().unwrap_or_else(PoisonError::into_inner);
        // In id order, so the first error is always the same one.
        read.sort_unstable_by_key(|(k, _)| *k);
        let mut out = Everything { readers: count, ..Everything::default() };
        for (_, rows) in read {
            for found in rows? {
                let mut s = found.sound;
                s.tags = tags.remove(&s.id).unwrap_or_default();
                s.hidden = hidden.contains(&s.id);
                let mut a = activity.remove(&s.id).unwrap_or_default();
                add_undated_plays(&mut a, s.plays);
                s.activity = a;
                if let Some(v) = found.features {
                    out.features.insert(s.id, v);
                }
                if let Some(v) = found.clap {
                    out.clap.insert(s.id, v);
                }
                out.sounds.push(s);
            }
        }
        // Read in the order the rows are stored, then sorted here. Asking SQLite for `ORDER BY path` walks the path
        // index and fetches each row from wherever it sits, and every row carries its fingerprints (`features`,
        // `clap`), which come before the type and rating columns, so each one meant reading several kilobytes from a
        // random place in a file of gigabytes: minutes on a cold disk with a big library. In stored order the same
        // read goes straight through the file once. Paths that tie come in id order, as SQLite gives them, so the
        // order never depends on the readers' timing (and sorting in place needs no second copy of the list).
        out.sounds.par_sort_unstable_by(|a, b| by_path(&a.path, &b.path).then(a.id.cmp(&b.id)));
        Ok(out)
    }

    /// Up to `n` connections to the library, each in a read transaction of its own, all seeing the library as it
    /// was at one moment. When the C# app saves while they start, they start again; if it keeps saving, one
    /// connection does it all.
    fn readers(&self, n: usize) -> Result<Vec<Connection>, SoundsError> {
        if n > 1 {
            for _ in 0..TRIES {
                let before = data_version(&self.conn)?;
                let readers = (0..n).map(|_| self.reader()).collect::<Result<Vec<_>, _>>()?;
                // A save that landed while they started would show here as a new data version; without one, every
                // reader took its picture of the library as it was when `before` was read.
                if data_version(&self.conn)? == before {
                    return Ok(readers);
                }
            }
        }
        Ok(vec![self.reader()?])
    }

    /// Another read-only connection to the library, already looking at it: it sees the library as it is now until
    /// it's dropped, whatever the C# app saves meanwhile.
    fn reader(&self) -> Result<Connection, SoundsError> {
        let conn =
            connect(&self.path).map_err(|e| SoundsError::Open { path: self.path.clone(), why: e.to_string() })?;
        conn.execute_batch("BEGIN")?;
        // A read transaction takes its picture of the library at its first read.
        conn.query_row("SELECT COUNT(*) FROM sqlite_schema", [], |r| r.get::<_, i64>(0))?;
        Ok(conn)
    }

    /// What's been done with each sound in the year before `now` and before that: deliberate plays, drags out of the
    /// app and sends into a DAW. Plays counted before the app kept dates count as recent, as in the C# app.
    pub fn activity(&self, now: NaiveDateTime) -> Result<HashMap<i64, SoundActivity>, SoundsError> {
        let mut result = self.dated_activity(now)?;
        // Plays counted on the sound before each one was dated: those beyond the dated ones count as recent.
        let mut st = self.conn.prepare("SELECT id, plays FROM sounds WHERE plays > 0")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, int(int_at(r, 1)))))?;
        for row in rows {
            let (id, plays) = row?;
            add_undated_plays(result.entry(id).or_default(), plays);
        }
        Ok(result)
    }

    /// The dated part of [`SoundsDb::activity`]: what `sound_events` holds, without the plays counted before then.
    fn dated_activity(&self, now: NaiveDateTime) -> Result<HashMap<i64, SoundActivity>, SoundsError> {
        dated_activity(&self.conn, &self.tables, now)
    }

    /// The sounds you've played on purpose, most recent first, each once with when you last played it (local time).
    pub fn history(&self, limit: usize) -> Result<Vec<(i64, NaiveDateTime)>, SoundsError> {
        if !self.has("sound_events") {
            return Ok(Vec::new());
        }
        let mut st = self.conn.prepare(
            "SELECT sound_id, MAX(at) AS last FROM sound_events WHERE kind = 'play' AND sound_id IS NOT NULL
             GROUP BY sound_id ORDER BY last DESC LIMIT ?1",
        )?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = st.query_map([limit], |r| Ok((r.get::<_, i64>(0)?, local(r.get(1)?))))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Where each sound sits on the galaxy map placed by `space` ("features" or "clap").
    pub fn map(&self, space: &str) -> Result<HashMap<i64, StarPoint>, SoundsError> {
        if !self.has("sound_maps") {
            return Ok(HashMap::new());
        }
        let mut st = self.conn.prepare("SELECT sound_id, x, y FROM sound_maps WHERE space = ?1")?;
        let rows = st.query_map([space], |r| {
            Ok((r.get::<_, i64>(0)?, StarPoint { x: r.get::<_, f64>(1)? as f32, y: r.get::<_, f64>(2)? as f32 }))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Each sound's vector in a space: "clap" (once the model has listened) or, for anything else, "features" (the
    /// analyser's own). Sounds without one, or with a damaged one, are left out.
    pub fn vectors(&self, space: &str) -> Result<HashMap<i64, Vec<f32>>, SoundsError> {
        let column = if space == "clap" { "clap" } else { "features" };
        let mut st = self.conn.prepare(&format!("SELECT id, {column} FROM sounds WHERE {column} IS NOT NULL"))?;
        let mut rows = st.query([])?;
        let mut out = HashMap::new();
        while let Some(r) = rows.next()? {
            // Anything but a blob of whole floats is left out, as the C# app leaves it out.
            if let ValueRef::Blob(blob) = r.get_ref(1)?
                && let Some(v) = floats(blob)
            {
                out.insert(r.get::<_, i64>(0)?, v);
            }
        }
        Ok(out)
    }

    /// Every tag, with how many sounds you gave it and how many the app suggested; most of yours first.
    pub fn tag_counts(&self) -> Result<Vec<TagCount>, SoundsError> {
        if !self.has("sound_tags") {
            return Ok(Vec::new());
        }
        let mut st = self.conn.prepare(
            "SELECT tag, SUM(source = 'you') AS yours, SUM(source = 'learned') AS learned
             FROM sound_tags GROUP BY tag ORDER BY yours DESC, tag",
        )?;
        let rows = st.query_map([], |r| {
            Ok(TagCount {
                tag: r.get(0)?,
                yours: r.get::<_, Option<i64>>(1)?.unwrap_or(0),
                learned: r.get::<_, Option<i64>>(2)?.unwrap_or(0),
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The sounds tagged `tag`, by `source` ("you" for your own tags, "learned" for the app's suggestions), in the
    /// order SQLite gives them (as the C# app reads them, so learning from them goes the same way).
    pub fn tagged(&self, tag: &str, source: &str) -> Result<Vec<i64>, SoundsError> {
        if !self.has("sound_tags") {
            return Ok(Vec::new());
        }
        let mut st = self.conn.prepare("SELECT sound_id FROM sound_tags WHERE tag = ?1 AND source = ?2")?;
        let rows = st.query_map([tag, source], |r| r.get::<_, Option<i64>>(0))?;
        Ok(rows.filter_map(|r| r.transpose()).collect::<Result<_, _>>()?)
    }

    /// The sounds you've hidden as duplicates.
    pub fn hidden(&self) -> Result<HashSet<i64>, SoundsError> {
        hidden(&self.conn, &self.tables)
    }

    /// Every pair of copies the duplicate finder found.
    pub fn matches(&self) -> Result<Vec<SoundMatch>, SoundsError> {
        if !self.has("sound_matches") {
            return Ok(Vec::new());
        }
        // `apart` and `typical` came after the first test builds: an older table lacks them.
        let has_apart: bool = self.conn.query_row(
            "SELECT COUNT(*) = 2 FROM pragma_table_info('sound_matches') WHERE name IN ('apart', 'typical')",
            [],
            |r| r.get(0),
        )?;
        let extra = if has_apart { "apart, typical" } else { "NULL, NULL" };
        let mut st = self.conn.prepare(&format!("SELECT a, b, kind, score, offset, {extra} FROM sound_matches"))?;
        let rows = st.query_map(params![], |r| {
            Ok(SoundMatch {
                a: r.get(0)?,
                b: r.get(1)?,
                kind: copy_kind(text_at(r, 2).as_deref()).to_owned(),
                score: r.get::<_, Option<f64>>(3)?.unwrap_or(0.0),
                offset: r.get::<_, Option<f64>>(4)?.unwrap_or(0.0),
                apart: r.get(5)?,
                typical: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

impl SoundsDb {
    /// Sounds whose audio is exactly the same as another's, by the hash the duplicate finder kept (in capitals, as
    /// SQLite's `hex` gives it). Only hashes that more than one sound has.
    pub fn exact_copies(&self) -> Result<Vec<(i64, String)>, SoundsError> {
        if !self.has("sound_prints") {
            return Ok(Vec::new());
        }
        let mut st = self.conn.prepare(
            "SELECT sound_id, hex(exact) FROM sound_prints
             WHERE exact IN (SELECT exact FROM sound_prints WHERE exact IS NOT NULL GROUP BY exact HAVING COUNT(*) > 1)",
        )?;
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// What the duplicate finder found: its matches, the exact copies and every file's size, read at one moment (the
    /// C# app may be saving more while the page reads).
    #[allow(clippy::type_complexity)]
    pub fn duplicate_finds(&self) -> Result<(Vec<SoundMatch>, Vec<(i64, String)>, HashMap<i64, i64>), SoundsError> {
        let snapshot = self.conn.unchecked_transaction()?;
        let found = (self.matches()?, self.exact_copies()?, self.sizes()?);
        snapshot.finish()?;
        Ok(found)
    }

    /// Each sound's file size in bytes (0 when not known).
    pub fn sizes(&self) -> Result<HashMap<i64, i64>, SoundsError> {
        let mut st = self.conn.prepare("SELECT id, size FROM sounds")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, int_at(r, 1).unwrap_or(0))))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

/// The dated part of [`SoundsDb::activity`], read on `conn`: what `sound_events` holds, without the plays counted before
/// then.
fn dated_activity(
    conn: &Connection,
    tables: &HashSet<String>,
    now: NaiveDateTime,
) -> Result<HashMap<i64, SoundActivity>, SoundsError> {
    let cutoff = unix(year_before(now));
    let mut result: HashMap<i64, SoundActivity> = HashMap::new();
    if tables.contains("sound_events") {
        let mut st = conn.prepare(
            "SELECT sound_id, kind, SUM(at >= ?1), SUM(at < ?1) FROM sound_events
             WHERE sound_id IS NOT NULL GROUP BY sound_id, kind",
        )?;
        let rows = st.query_map([cutoff], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<i64>>(2)?, r.get::<_, Option<i64>>(3)?))
        })?;
        for row in rows {
            let (id, kind, recent, older) = row?;
            let (recent, older) = (int(recent), int(older));
            let a = result.entry(id).or_default();
            match kind.as_str() {
                event::PLAY => (a.plays, a.older_plays) = (recent, older),
                event::DRAG => (a.drags, a.older_drags) = (recent, older),
                event::SEND => (a.sends, a.older_sends) = (recent, older),
                _ => {}
            }
        }
    }
    Ok(result)
}

/// Opens the library at `path` for reading only.
fn connect(path: &Path) -> Result<Connection, rusqlite::Error> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(path, flags)?;
    // The C# app may be saving a scan while this reads: wait a little for it rather than fail.
    conn.busy_timeout(Duration::from_secs(2))?;
    conn.pragma_update(None, "query_only", true)?;
    Ok(conn)
}

/// A number SQLite changes whenever another connection has saved since `conn` last looked.
fn data_version(conn: &Connection) -> Result<i64, rusqlite::Error> {
    conn.query_row("PRAGMA data_version", [], |r| r.get(0))
}

/// `[low, high]` cut into `n` stretches of ids (fewer when there aren't that many ids), in order, with no gaps.
fn stretches(low: i64, high: i64, n: usize) -> Vec<(i64, i64)> {
    if high < low {
        return Vec::new();
    }
    let span = i128::from(high) - i128::from(low) + 1;
    let n = i128::try_from(n).unwrap_or(i128::MAX).clamp(1, span);
    let at = |k: i128| i128::from(low) + span * k / n;
    // Every bound lies between `low` and `high + 1`, so each fits back into an id.
    (0..n).filter_map(|k| Some((i64::try_from(at(k)).ok()?, i64::try_from(at(k + 1) - 1).ok()?))).collect()
}

/// One sound as a stretch reads it, with its vectors when they were asked for.
struct Found {
    sound: Sound,
    features: Option<Vec<f32>>,
    clap: Option<Vec<f32>>,
}

/// One stretch's sounds (or what went wrong reading them), with the stretch's place in line.
type StretchRead = (usize, Result<Vec<Found>, SoundsError>);

/// The sounds with ids from `first` to `last`, in id order.
fn read_stretch(conn: &Connection, first: i64, last: i64, vectors: bool) -> Result<Vec<Found>, SoundsError> {
    let mut st = conn.prepare_cached(if vectors { SOUND_AND_VECTORS_SQL } else { SOUND_SQL })?;
    let mut rows = st.query([first, last])?;
    let mut found = Vec::new();
    while let Some(r) = rows.next()? {
        let (features, clap) = if vectors { (vector_at(r, 13), vector_at(r, 14)) } else { (None, None) };
        found.push(Found { sound: sound_at(r)?, features, clap });
    }
    Ok(found)
}

/// A sound from a row of [`SOUND_SQL`], without its tags, activity or hidden mark yet.
fn sound_at(r: &Row<'_>) -> Result<Sound, rusqlite::Error> {
    // Every column but the id and path is read as the C# app reads it: a value of an unexpected kind in a damaged row
    // reads as none (or 0) rather than stopping the whole list.
    Ok(Sound {
        id: r.get(0)?,
        path: r.get(1)?,
        added: local(int_at(r, 2).unwrap_or(0)),
        error: text_at(r, 3),
        // An analysis that can't be read shows as none, as in the C# app.
        analysis: analysis_at(r, 4),
        name_type: text_at(r, 5),
        user_type: text_at(r, 6),
        sound_type: text_at(r, 7).unwrap_or_else(|| types::UNKNOWN.to_owned()),
        type_source: text_at(r, 8).unwrap_or_default(),
        type_confidence: real_at(r, 9).unwrap_or(0.0),
        favourite: int_at(r, 10) == Some(1),
        rating: int(int_at(r, 11)),
        plays: int(int_at(r, 12)),
        tags: Vec::new(),
        hidden: false,
        activity: SoundActivity::default(),
    })
}

/// The analysis JSON in column `i`, parsed straight from SQLite's copy of it; none when it isn't text or can't be read.
fn analysis_at(r: &Row<'_>, i: usize) -> Option<SoundAnalysis> {
    match r.get_ref(i) {
        Ok(ValueRef::Text(t)) => match std::str::from_utf8(t) {
            Ok(json) => SoundAnalysis::from_json(json),
            Err(_) => SoundAnalysis::from_json(&String::from_utf8_lossy(t)),
        },
        _ => None,
    }
}

/// The vector in column `i`, when it's a blob of whole floats (anything else is left out, as the C# app leaves it out).
fn vector_at(r: &Row<'_>, i: usize) -> Option<Vec<f32>> {
    match r.get_ref(i) {
        Ok(ValueRef::Blob(blob)) => floats(blob),
        _ => None,
    }
}

/// What the list needs from the library's smaller tables.
struct Side {
    activity: HashMap<i64, SoundActivity>,
    tags: HashMap<i64, Vec<String>>,
    hidden: HashSet<i64>,
}

impl Side {
    fn read(conn: &Connection, tables: &HashSet<String>, now: NaiveDateTime) -> Result<Side, SoundsError> {
        Ok(Side {
            activity: dated_activity(conn, tables, now)?,
            tags: tags(conn, tables)?,
            hidden: hidden(conn, tables)?,
        })
    }
}

/// Each sound's tags, sorted as `ORDER BY tag` sorts them (byte by byte). Read in the order they're stored and sorted
/// here, so SQLite doesn't fetch each row from wherever it sits by way of the tag index.
fn tags(conn: &Connection, tables: &HashSet<String>) -> Result<HashMap<i64, Vec<String>>, SoundsError> {
    let mut tags: HashMap<i64, Vec<String>> = HashMap::new();
    if tables.contains("sound_tags") {
        let mut st = conn.prepare("SELECT sound_id, tag FROM sound_tags")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            if let (Some(id), tag) = row? {
                tags.entry(id).or_default().push(tag);
            }
        }
    }
    for list in tags.values_mut() {
        list.sort_unstable();
    }
    Ok(tags)
}

/// The sounds you've hidden as duplicates, read on `conn`.
fn hidden(conn: &Connection, tables: &HashSet<String>) -> Result<HashSet<i64>, SoundsError> {
    if !tables.contains("sound_hidden") {
        return Ok(HashSet::new());
    }
    let mut st = conn.prepare("SELECT sound_id FROM sound_hidden")?;
    let rows = st.query_map([], |r| r.get::<_, i64>(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The order SQLite's `COLLATE NOCASE` gives the `path` column, so the list comes out as the C# app's did: byte by
/// byte, with only the English letters A to Z folded to lower case.
fn by_path(a: &str, b: &str) -> std::cmp::Ordering {
    a.bytes().map(|c| c.to_ascii_lowercase()).cmp(b.bytes().map(|c| c.to_ascii_lowercase()))
}

/// A stored count as an `i32`, clamped (a damaged row can't wrap it).
fn int(n: Option<i64>) -> i32 {
    n.unwrap_or(0).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// A whole number, or `None` for NULL or a value of another kind (the C# app's `Long` reads those as 0).
fn int_at(r: &Row<'_>, i: usize) -> Option<i64> {
    match r.get_ref(i) {
        Ok(ValueRef::Integer(n)) => Some(n),
        _ => None,
    }
}

/// A number, or `None` for NULL or a value of another kind.
fn real_at(r: &Row<'_>, i: usize) -> Option<f64> {
    match r.get_ref(i) {
        Ok(ValueRef::Real(x)) => Some(x),
        Ok(ValueRef::Integer(n)) => Some(n as f64),
        _ => None,
    }
}

/// Text, or `None` for NULL or a value of another kind (the C# app's `as string`).
fn text_at(r: &Row<'_>, i: usize) -> Option<String> {
    match r.get_ref(i) {
        Ok(ValueRef::Text(t)) => Some(String::from_utf8_lossy(t).into_owned()),
        _ => None,
    }
}

/// Adds the plays counted on a sound before each one was dated: those beyond the dated ones count as recent.
fn add_undated_plays(a: &mut SoundActivity, plays: i32) {
    let dated = a.plays.saturating_add(a.older_plays);
    a.plays = a.plays.saturating_add(plays.saturating_sub(dated).max(0));
}

/// A copy's kind as the C# app reads it: one it doesn't know reads as "Converted".
fn copy_kind(kind: Option<&str>) -> &'static str {
    // As .NET's Enum.TryParse: names or numbers (spaces around them allowed), several joined by commas adding up;
    // anything else, or a number with no name, reads as Converted.
    let mut value = 0i64;
    for part in kind.unwrap_or("").split(',') {
        value |= match part.trim() {
            "Exact" => 0,
            "Converted" => 1,
            "Trimmed" => 2,
            n => match n.parse::<i64>() {
                Ok(n) => n,
                Err(_) => return "Converted",
            },
        };
    }
    match value {
        0 => "Exact",
        2 => "Trimmed",
        _ => "Converted",
    }
}

/// Floats stored as the C# app stores them: little-endian 4-byte floats, one after another. `None` when the length
/// isn't a whole number of floats.
fn floats(blob: &[u8]) -> Option<Vec<f32>> {
    if blob.is_empty() || !blob.len().is_multiple_of(4) {
        return None;
    }
    Some(blob.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect())
}

/// Unix seconds as local time; a time out of range reads as the start of 1970.
fn local(seconds: i64) -> NaiveDateTime {
    chrono::DateTime::from_timestamp(seconds, 0).unwrap_or_default().with_timezone(&Local).naive_local()
}

/// Local time as Unix seconds. A time the clocks skipped reads as the same time in UTC.
fn unix(t: NaiveDateTime) -> i64 {
    t.and_local_timezone(Local).earliest().map_or_else(|| t.and_utc().timestamp(), |t| t.timestamp())
}

/// The same moment a year earlier; 29 February goes to 28 February, as .NET's `AddYears(-1)` does.
fn year_before(t: NaiveDateTime) -> NaiveDateTime {
    t.checked_sub_months(chrono::Months::new(12)).unwrap_or(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_blobs() {
        let blob: Vec<u8> = [1.5f32, -2.0].iter().flat_map(|f| f.to_le_bytes()).collect();
        assert_eq!(floats(&blob), Some(vec![1.5, -2.0]));
        assert_eq!(floats(&blob[..5]), None);
        assert_eq!(floats(&[]), None);
    }

    #[test]
    fn paths_sort_as_sqlite_nocase_does() {
        let paths = [
            r"C:\Kicks\b.wav",
            r"c:\kicks\A.wav",
            r"C:\Kicks_\x.wav",
            r"C:\Kicks\Ébc.wav",
            r"C:\Kicks\ébd.wav",
            r"C:\KICKS\z.wav",
            r"C:\Kicks\[1].wav",
            r"C:\Kicks\Z1.wav",
            r"C:\Kicks\a.wav ",
            "",
        ];
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE t (p TEXT COLLATE NOCASE)", []).unwrap();
        for p in paths {
            conn.execute("INSERT INTO t VALUES (?1)", [p]).unwrap();
        }
        let mut st = conn.prepare("SELECT p FROM t ORDER BY p").unwrap();
        let sqlite: Vec<String> = st.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
        let mut ours: Vec<&str> = paths.to_vec();
        ours.sort_unstable_by(|a, b| by_path(a, b));
        assert_eq!(ours, sqlite);
    }

    #[test]
    fn stretches_cover_every_id_once() {
        assert_eq!(stretches(1, 10, 3), [(1, 3), (4, 6), (7, 10)]);
        assert_eq!(stretches(5, 7, 100), [(5, 5), (6, 6), (7, 7)], "never more stretches than ids");
        assert_eq!(stretches(-3, 2, 2), [(-3, -1), (0, 2)]);
        assert_eq!(stretches(4, 4, 0), [(4, 4)]);
        assert!(stretches(5, 4, 3).is_empty());
        let whole = stretches(i64::MIN, i64::MAX, 7);
        assert_eq!((whole[0].0, whole[6].1), (i64::MIN, i64::MAX));
        assert!(whole.windows(2).all(|w| w[0].1.checked_add(1) == Some(w[1].0)));
    }

    #[test]
    fn a_year_before() {
        let leap = chrono::NaiveDate::from_ymd_opt(2024, 2, 29).unwrap().and_hms_opt(12, 0, 0).unwrap();
        assert_eq!(year_before(leap).to_string(), "2023-02-28 12:00:00");
    }
}

#[cfg(test)]
mod copy_kind_tests {
    use super::copy_kind;

    #[test]
    fn kinds_read_as_dotnet_reads_them() {
        assert_eq!(copy_kind(Some("Exact")), "Exact");
        assert_eq!(copy_kind(Some(" Trimmed ")), "Trimmed");
        assert_eq!(copy_kind(Some("0")), "Exact");
        assert_eq!(copy_kind(Some("02")), "Trimmed");
        assert_eq!(copy_kind(Some("Exact, Trimmed")), "Trimmed");
        assert_eq!(copy_kind(Some("exact")), "Converted");
        assert_eq!(copy_kind(Some("Mystery")), "Converted");
        assert_eq!(copy_kind(None), "Converted");
    }
}
