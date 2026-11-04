//! Reading `sounds.db`, the C# app's sound library, without ever changing it.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{Local, NaiveDateTime};
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row, params};

use crate::index::Gathered;
use crate::{Sound, SoundActivity, SoundAnalysis, Word, compact, event, types};

/// What went wrong reading the sound library.
#[derive(Debug)]
pub enum SoundsError {
    /// The file isn't there, can't be opened, or isn't a Music Organiser sound library.
    Open { path: PathBuf, why: String },
    /// A read failed (the file is damaged, or locked for longer than the wait).
    Read(rusqlite::Error),
    /// The Rust app's own copy of the library ([`crate::compact`]) couldn't be made or brought up to date.
    Copy { path: PathBuf, why: String },
}

impl fmt::Display for SoundsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SoundsError::Open { path, why } => write!(f, "Couldn't open the sound library {}: {why}", path.display()),
            SoundsError::Read(e) => write!(f, "Couldn't read the sound library: {e}"),
            SoundsError::Copy { path, why } => {
                write!(
                    f,
                    "Couldn't bring the Rust app's copy of the sound library {} up to date: {why}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for SoundsError {}

impl SoundsError {
    /// Whether SQLite found the file damaged (a file cut short, a page spoilt) or the disk couldn't read part of it
    /// (a bad sector): reading it again won't help.
    pub fn damaged(&self) -> bool {
        matches!(self, SoundsError::Read(e) if is_damage(e))
    }
}

/// [`SoundsError::damaged`], for an error SQLite gave.
pub(crate) fn is_damage(e: &rusqlite::Error) -> bool {
    use rusqlite::ffi;
    match e {
        rusqlite::Error::SqliteFailure(e, _) => {
            matches!(e.code, rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase)
                || matches!(e.extended_code, ffi::SQLITE_IOERR_READ | ffi::SQLITE_IOERR_SHORT_READ)
        }
        _ => false,
    }
}

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
    /// Each sound's vector from the analyser, as [`SoundsDb::vectors`] gives them for "features", gathered in one
    /// block in the order the rows are stored.
    pub features: Gathered,
    /// Each sound's vector from CLAP, as [`SoundsDb::vectors`] gives them for "clap", gathered likewise.
    pub clap: Gathered,
}

impl Everything {
    /// How many sounds have been analysed ([`count_analysed`]).
    pub fn analysed(&self) -> usize {
        count_analysed(&self.sounds)
    }
}

/// How many of `sounds` have been analysed, hidden copies too, as the C# app counts them when it decides whether deep
/// listening has heard enough (`SoundOrganiser.Reload`): what [`crate::index::SoundIndex::build`] takes.
pub fn count_analysed(sounds: &[Sound]) -> usize {
    sounds.iter().filter(|s| s.analysis.is_some()).count()
}

/// A sound's columns, in the order [`sound_at`] takes them: one list for every read of them (the copy's too, as it's
/// made), so they can't drift apart.
macro_rules! sound_columns {
    () => {
        "id, path, mtime, error, analysis, name_type, user_type, type, type_source, type_conf, favourite, rating, plays"
    };
}
pub(crate) use sound_columns;

/// How many columns [`sound_columns`] lists, counted from the list itself: the vectors come straight after them.
pub(crate) const SOUND_COLUMNS: usize = columns(sound_columns!());

/// How many columns a comma-separated list names.
const fn columns(list: &str) -> usize {
    let bytes = list.as_bytes();
    let (mut n, mut i) = (1, 0);
    while i < bytes.len() {
        if bytes[i] == b',' {
            n += 1;
        }
        i += 1;
    }
    n
}

/// Every sound, in the order the rows are stored.
const SOUND_SQL: &str = concat!("SELECT ", sound_columns!(), " FROM sounds");

/// Every sound in the Rust app's copy of the library, in the order the rows are stored: the columns [`copy_sound_at`]
/// takes, the analysis's as [`compact::analysis_at`] takes them.
const COPY_SOUND_SQL: &str = concat!(
    "SELECT id, path, mtime, error, name_type, user_type, type, type_source, type_conf, favourite, rating, plays, ",
    compact::analysis_columns!(),
    " FROM sounds"
);

/// Where [`COPY_SOUND_SQL`] has the analysis's first column.
const COPY_ANALYSIS: usize = 12;

/// The same with each one's two vectors after its columns, the analyser's and CLAP's.
const SOUND_AND_VECTORS_SQL: &str = concat!("SELECT ", sound_columns!(), ", features, clap FROM sounds");

/// The sound library, open for reading only: SQLite itself refuses any change, so the C# app's data is safe.
///
/// Either the C# app's `sounds.db` or the Rust app's compact copy of it ([`compact`]): both give the same answers.
///
/// SQLite may leave `sounds.db-wal` and `sounds.db-shm` beside the file after reading (it needs them to read while the
/// C# app writes). They hold nothing of their own.
pub struct SoundsDb {
    pub(crate) conn: Connection,
    path: PathBuf,
    /// The tables this library has: an older C# app's may lack the duplicate finder's, which then read as empty.
    tables: HashSet<String>,
    /// The Rust app's copy, which keeps the analysis as numbers and the fingerprints in tables of their own.
    copy: bool,
}

impl SoundsDb {
    /// Opens the sound library at `path` for reading. It must already exist: this never makes one.
    pub fn open_read_only(path: &Path) -> Result<SoundsDb, SoundsError> {
        let open_error = |why: String| SoundsError::Open { path: path.to_owned(), why };
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(path, flags).map_err(|e| open_error(e.to_string()))?;
        // The C# app may be saving a scan while this reads: wait a little for it rather than fail.
        conn.busy_timeout(Duration::from_secs(2)).map_err(|e| open_error(e.to_string()))?;
        conn.pragma_update(None, "query_only", true).map_err(|e| open_error(e.to_string()))?;
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
        let copy = tables.contains(compact::INFO_TABLE);
        // A copy laid out by another version of the app isn't read: its columns may mean something else.
        if copy && compact::format_of(&conn).map_err(|e| open_error(e.to_string()))? != Some(compact::FORMAT) {
            return Err(open_error("it's a copy another version of the app made, laid out differently".into()));
        }
        Ok(SoundsDb { conn, path: path.to_owned(), tables, copy })
    }

    /// What the sound library at `path` looks like now, as a line of text that changes whenever anything saves into
    /// it, without opening it (so it never waits): the file's size and time, its log's (`-wal`) when it has anything
    /// in it, and where the log has got to as SQLite notes it in its index of the log (`-shm`).
    ///
    /// The last is what catches every save the C# app makes while it runs. Once SQLite has copied the log into the
    /// file it writes the log again from the start, so a save can leave both files the size they were; and Windows
    /// may not move a file's time while a program holds it open for writing, as the C# app holds its log. The index
    /// notes the log's last frame, its checksum and the log's salt, which every save changes.
    pub fn stamp(path: &Path) -> String {
        let side = |end: &str| {
            let mut p = path.as_os_str().to_owned();
            p.push(end);
            PathBuf::from(p)
        };
        let look = |p: &Path| match std::fs::metadata(p) {
            Ok(m) if m.len() > 0 => {
                let at = m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_nanos());
                format!("{}@{at}", m.len())
            }
            // An empty log counts as none: SQLite makes one when a reader opens the file.
            _ => "-".to_owned(),
        };
        format!("{} {} {}", look(path), look(&side("-wal")), wal_index(&side("-shm")))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether this is the Rust app's compact copy of the library rather than the C# app's own file.
    pub fn is_copy(&self) -> bool {
        self.copy
    }

    pub(crate) fn has(&self, table: &str) -> bool {
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
        let sql = if self.copy {
            // From the lengths kept beside the hashes: the fingerprints themselves are a gigabyte.
            "SELECT COUNT(*) FROM sound_hashes WHERE clap_len IS NOT NULL"
        } else {
            "SELECT COUNT(*) FROM sounds WHERE clap IS NOT NULL"
        };
        Ok(self.conn.query_row(sql, [], |r| r.get(0))?)
    }

    /// Every file the library knows, as the last scan saw it.
    pub fn known_files(&self) -> Result<Vec<KnownSound>, SoundsError> {
        let mut st = self.conn.prepare(if self.copy {
            "SELECT path, COALESCE(size, 0), COALESCE(mtime, 0),
                 EXISTS (SELECT 1 FROM sound_hashes h WHERE h.sound_id = sounds.id AND h.clap_len IS NOT NULL)
             FROM sounds"
        } else {
            "SELECT path, COALESCE(size, 0), COALESCE(mtime, 0), clap IS NOT NULL FROM sounds"
        })?;
        let rows = st.query_map([], |r| {
            Ok(KnownSound { path: r.get(0)?, size: r.get(1)?, mtime: r.get(2)?, has_clap: r.get(3)? })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Every sound, ordered by path ignoring letter case (as the C# app lists them), with its tags, activity and
    /// whether you've hidden it as a duplicate. `now` sets what counts as recent activity: the year before it.
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
        // One read transaction, so every query below sees the library as it was at one moment, even while the C# app
        // saves a scan or hides a batch of copies in between.
        let snapshot = self.conn.unchecked_transaction()?;
        let mut reading = Reading::start(self, now, vectors)?;
        // Read in the order the rows are stored, then sorted here. Asking SQLite for `ORDER BY path` walks the path
        // index and fetches each row from wherever it sits, and every row carries its fingerprints (`features`,
        // `clap`), which come before the type and rating columns, so each one meant reading several kilobytes from a
        // random place in a file of gigabytes: minutes on a cold disk with a big library. In stored order the same
        // read goes straight through the file once. On one connection: several at once were no faster on a slow
        // disk, and they held up the other pages reading from it (Solomon's PC, 2026-11-03).
        if self.copy {
            // The copy keeps the fingerprints in tables of their own: the list comes first, then each of them.
            let mut st = self.conn.prepare(COPY_SOUND_SQL)?;
            let mut rows = st.query([])?;
            while let Some(r) = rows.next()? {
                reading.add(copy_sound_at(r)?);
            }
            drop(rows);
            drop(st);
            if vectors {
                // In the same order as from sounds.db: by sound, as its rows are stored.
                reading.out.features = self.copied_gathered("sound_features")?;
                reading.out.clap = self.copied_gathered("sound_clap")?;
            }
        } else {
            let mut st = self.conn.prepare(if vectors { SOUND_AND_VECTORS_SQL } else { SOUND_SQL })?;
            let mut rows = st.query([])?;
            while let Some(r) = rows.next()? {
                reading.add_row(r)?;
            }
        }
        snapshot.finish()?;
        Ok(reading.finish())
    }

    /// The fingerprints a [`crate::index::SoundIndex`] keeps of this library's sounds, gathered as
    /// [`SoundsDb::everything`] gathers them: the analyser's, and CLAP's once deep listening has heard enough of the
    /// `analysed` sounds ([`crate::learn::heard_enough`]). Until then the index drops CLAP's, so they aren't read
    /// (a gigabyte for a library like Solomon's).
    ///
    /// Quick from the Rust app's copy, which keeps each kind in a table of its own and notes how long each sound's
    /// CLAP is. From `sounds.db` it's a pass over the whole file: [`SoundsDb::everything`] takes them on the same pass
    /// as the list.
    pub fn fingerprints(&self, analysed: usize) -> Result<(Gathered, Gathered), SoundsError> {
        let snapshot = self.conn.unchecked_transaction()?;
        if !self.copy {
            // As the page's read gathers them, in the order the rows are stored.
            let (mut features, mut clap) = (Gathered::default(), Gathered::default());
            let mut st = self.conn.prepare("SELECT id, features, clap FROM sounds")?;
            let mut rows = st.query([])?;
            while let Some(r) = rows.next()? {
                let id: i64 = r.get(0)?;
                if let Ok(ValueRef::Blob(blob)) = r.get_ref(1) {
                    features.push_blob(id, blob);
                }
                if let Ok(ValueRef::Blob(blob)) = r.get_ref(2) {
                    clap.push_blob(id, blob);
                }
            }
            drop(rows);
            drop(st);
            snapshot.finish()?;
            if !crate::learn::heard_enough(clap.common_length().1, analysed) {
                clap = Gathered::default();
            }
            return Ok((features, clap));
        }
        // As the index counts them: the CLAP vectors of the most common length (a damaged one, not whole floats, has
        // none).
        let heard: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(n), 0) FROM (SELECT COUNT(*) AS n FROM sound_hashes
                 WHERE clap_len > 0 AND clap_len % 4 = 0 GROUP BY clap_len)",
            [],
            |r| r.get(0),
        )?;
        let deep = crate::learn::heard_enough(usize::try_from(heard).unwrap_or(0), analysed);
        let features = self.copied_gathered("sound_features")?;
        let clap = if deep { self.copied_gathered("sound_clap")? } else { Gathered::default() };
        snapshot.finish()?;
        Ok((features, clap))
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
        let cutoff = unix(year_before(now));
        let mut result: HashMap<i64, SoundActivity> = HashMap::new();
        if self.has("sound_events") {
            let mut st = self.conn.prepare(
                "SELECT sound_id, kind, SUM(at >= ?1), SUM(at < ?1) FROM sound_events
                 WHERE sound_id IS NOT NULL GROUP BY sound_id, kind",
            )?;
            let rows = st.query_map([cutoff], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                    r.get::<_, Option<i64>>(3)?,
                ))
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

    /// Each sound's tags, sorted as `ORDER BY tag` sorts them (byte by byte). Read in the order they're stored and
    /// sorted here, so SQLite doesn't fetch each row from wherever it sits by way of the tag index.
    fn tags(&self) -> Result<HashMap<i64, Vec<String>>, SoundsError> {
        let mut tags: HashMap<i64, Vec<String>> = HashMap::new();
        if self.has("sound_tags") {
            let mut st = self.conn.prepare("SELECT sound_id, tag FROM sound_tags")?;
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
        let sql = match (self.copy, space == "clap") {
            (true, true) => "SELECT sound_id, v FROM sound_clap".to_owned(),
            (true, false) => "SELECT sound_id, v FROM sound_features".to_owned(),
            (false, clap) => {
                let column = if clap { "clap" } else { "features" };
                format!("SELECT id, {column} FROM sounds WHERE {column} IS NOT NULL")
            }
        };
        let mut st = self.conn.prepare(&sql)?;
        let mut rows = st.query([])?;
        let mut out = HashMap::new();
        while let Some(r) = rows.next()? {
            if let Some(v) = vector_at(r, 1) {
                out.insert(r.get::<_, i64>(0)?, v);
            }
        }
        Ok(out)
    }

    /// The vectors in one of the copy's fingerprint tables, gathered by sound as [`SoundsDb::everything`] gathers them
    /// from `sounds.db`.
    fn copied_gathered(&self, table: &str) -> Result<Gathered, SoundsError> {
        let mut st = self.conn.prepare(&format!("SELECT sound_id, v FROM {table}"))?;
        let mut rows = st.query([])?;
        let mut out = Gathered::default();
        while let Some(r) = rows.next()? {
            if let Ok(ValueRef::Blob(blob)) = r.get_ref(1) {
                out.push_blob(r.get(0)?, blob);
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
        if !self.has("sound_hidden") {
            return Ok(HashSet::new());
        }
        let mut st = self.conn.prepare("SELECT sound_id FROM sound_hidden")?;
        let rows = st.query_map([], |r| r.get::<_, i64>(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
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

/// The sounds as they're read, a row at a time, made into what [`SoundsDb::everything`] gives: each with its tags,
/// plays and hidden mark, its vectors gathered (when asked for), sorted by path at the end. One way of doing it for
/// the page's read and for making the copy ([`compact`]), which reads `sounds.db` for the page on the same pass.
pub(crate) struct Reading {
    activity: HashMap<i64, SoundActivity>,
    tags: HashMap<i64, Vec<String>>,
    hidden: HashSet<i64>,
    vectors: bool,
    out: Everything,
}

impl Reading {
    /// Starts with what the other tables hold for each sound. Inside the read transaction the rows are read in, so
    /// they all come from one moment.
    pub(crate) fn start(db: &SoundsDb, now: NaiveDateTime, vectors: bool) -> Result<Reading, SoundsError> {
        Ok(Reading {
            activity: db.dated_activity(now)?,
            tags: db.tags()?,
            hidden: db.hidden()?,
            vectors,
            out: Everything::default(),
        })
    }

    /// A row of `sounds.db` whose columns start as [`SOUND_AND_VECTORS_SQL`]'s do (as [`SOUND_SQL`]'s without the
    /// vectors).
    pub(crate) fn add_row(&mut self, r: &Row<'_>) -> rusqlite::Result<()> {
        let s = sound_at(r)?;
        self.add_read(r, s);
        Ok(())
    }

    /// The same, with the row's sound already read from it ([`sound_at`]).
    pub(crate) fn add_read(&mut self, r: &Row<'_>, s: Sound) {
        if self.vectors {
            // Straight from SQLite's copy of each blob into the one block: no list of its own for each sound.
            if let Ok(ValueRef::Blob(blob)) = r.get_ref(SOUND_COLUMNS) {
                self.out.features.push_blob(s.id, blob);
            }
            if let Ok(ValueRef::Blob(blob)) = r.get_ref(SOUND_COLUMNS + 1) {
                self.out.clap.push_blob(s.id, blob);
            }
        }
        self.add(s);
    }

    fn add(&mut self, mut s: Sound) {
        s.tags = self.tags.remove(&s.id).unwrap_or_default();
        s.hidden = self.hidden.contains(&s.id);
        let mut a = self.activity.remove(&s.id).unwrap_or_default();
        add_undated_plays(&mut a, s.plays);
        s.activity = a;
        self.out.sounds.push(s);
    }

    pub(crate) fn finish(mut self) -> Everything {
        // The list lives as long as the page: what it grew into and didn't use goes back (up to half of it).
        self.out.sounds.shrink_to_fit();
        self.out.sounds.sort_unstable_by(|a, b| by_path(&a.path, &b.path));
        self.out
    }
}

/// A sound from a row of [`SOUND_SQL`], without its tags, activity or hidden mark yet.
pub(crate) fn sound_at(r: &Row<'_>) -> Result<Sound, rusqlite::Error> {
    // Every column but the id and path is read as the C# app reads it: a value of an unexpected kind in a damaged row
    // reads as none (or 0) rather than stopping the whole list.
    Ok(Sound {
        id: r.get(0)?,
        path: r.get(1)?,
        added: local(int_at(r, 2).unwrap_or(0)),
        error: text_at(r, 3),
        // An analysis that can't be read shows as none, as in the C# app.
        analysis: analysis_at(r, 4),
        name_type: word_at(r, 5),
        user_type: word_at(r, 6),
        sound_type: word_at(r, 7).unwrap_or_else(|| Word::new(types::UNKNOWN)),
        type_source: word_at(r, 8).unwrap_or_default(),
        type_confidence: real_at(r, 9).unwrap_or(0.0),
        favourite: int_at(r, 10) == Some(1),
        rating: int(int_at(r, 11)),
        plays: int(int_at(r, 12)),
        tags: Vec::new(),
        hidden: false,
        activity: SoundActivity::default(),
    })
}

/// A sound from a row of [`COPY_SOUND_SQL`], without its tags, activity or hidden mark yet: every column read as
/// [`sound_at`] reads it from `sounds.db`, so the two give the same sound.
fn copy_sound_at(r: &Row<'_>) -> Result<Sound, rusqlite::Error> {
    Ok(Sound {
        id: r.get(0)?,
        path: r.get(1)?,
        added: local(int_at(r, 2).unwrap_or(0)),
        error: text_at(r, 3),
        analysis: compact::analysis_at(r, COPY_ANALYSIS),
        name_type: word_at(r, 4),
        user_type: word_at(r, 5),
        sound_type: word_at(r, 6).unwrap_or_else(|| Word::new(types::UNKNOWN)),
        type_source: word_at(r, 7).unwrap_or_default(),
        type_confidence: real_at(r, 8).unwrap_or(0.0),
        favourite: int_at(r, 9) == Some(1),
        rating: int(int_at(r, 10)),
        plays: int(int_at(r, 11)),
        tags: Vec::new(),
        hidden: false,
        activity: SoundActivity::default(),
    })
}

/// The analysis JSON in column `i`, parsed straight from SQLite's copy of it; none when it isn't text or can't be read.
pub(crate) fn analysis_at(r: &Row<'_>, i: usize) -> Option<SoundAnalysis> {
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

/// The order SQLite's `COLLATE NOCASE` gives the `path` column, so the list comes out as the C# app's did: byte by
/// byte, with only the English letters A to Z folded to lower case.
///
/// Paths in one folder start with the same letters, written the same way: the bytes two paths share as they are are the
/// same folded too, so they're passed over 8 at a time and only the rest is folded and compared byte by byte (sorting
/// Solomon's 453,885 sounds took a second, most of it going through folders' names).
fn by_path(a: &str, b: &str) -> std::cmp::Ordering {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut same = 0;
    while let (Some(x), Some(y)) = (a.get(same..same + 8), b.get(same..same + 8))
        && x == y
    {
        same += 8;
    }
    let (a, b) = (a.get(same..).unwrap_or_default(), b.get(same..).unwrap_or_default());
    a.iter().map(u8::to_ascii_lowercase).cmp(b.iter().map(u8::to_ascii_lowercase))
}

/// A stored count as an `i32`, clamped (a damaged row can't wrap it).
fn int(n: Option<i64>) -> i32 {
    n.unwrap_or(0).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// A whole number, or `None` for NULL or a value of another kind (the C# app's `Long` reads those as 0).
pub(crate) fn int_at(r: &Row<'_>, i: usize) -> Option<i64> {
    match r.get_ref(i) {
        Ok(ValueRef::Integer(n)) => Some(n),
        _ => None,
    }
}

/// A number, or `None` for NULL or a value of another kind.
pub(crate) fn real_at(r: &Row<'_>, i: usize) -> Option<f64> {
    match r.get_ref(i) {
        Ok(ValueRef::Real(x)) => Some(x),
        Ok(ValueRef::Integer(n)) => Some(n as f64),
        _ => None,
    }
}

/// Text, or `None` for NULL or a value of another kind (the C# app's `as string`).
/// A text column as a shared [`Word`], read without a copy of its own on the way.
pub(crate) fn word_at(r: &Row<'_>, i: usize) -> Option<Word> {
    match r.get_ref(i) {
        Ok(ValueRef::Text(t)) => Some(Word::new(&String::from_utf8_lossy(t))),
        _ => None,
    }
}

pub(crate) fn text_at(r: &Row<'_>, i: usize) -> Option<String> {
    match r.get_ref(i) {
        Ok(ValueRef::Text(t)) => Some(String::from_utf8_lossy(t).into_owned()),
        _ => None,
    }
}

/// Where the log of a library in WAL mode has got to, from SQLite's index of it (`-shm`, laid out as
/// <https://sqlite.org/walformat.html> says): the last frame saved, the database's size in pages, the last frame's
/// checksum and the log's salt. "-" when there's no index or nothing in the log.
fn wal_index(shm: &Path) -> String {
    use std::io::Read;
    let Ok(mut file) = std::fs::File::open(shm) else { return "-".to_owned() };
    // SQLite writes the header twice, one copy after the other; read again until the two agree (a save caught half
    // way), a few times at most.
    let mut header = [0u8; 96];
    for _ in 0..4 {
        if file.read_exact(&mut header).is_err() {
            return "-".to_owned();
        }
        let (one, two) = header.split_at(48);
        if one == two {
            break;
        }
        let _ = std::io::Seek::rewind(&mut file);
    }
    let one = &header[..48];
    // Offset 12: whether the index is set up; 16-19: the last frame saved (0 with nothing in the log).
    if one[12] == 0 || one[16..20] == [0; 4] {
        return "-".to_owned();
    }
    one[16..40].iter().map(|b| format!("{b:02x}")).collect()
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
    fn damage_is_a_spoilt_file_or_a_sector_the_disk_cant_read() {
        use rusqlite::ffi;
        let sqlite = |code| SoundsError::Read(rusqlite::Error::SqliteFailure(ffi::Error::new(code), None));
        for code in [ffi::SQLITE_CORRUPT, ffi::SQLITE_NOTADB, ffi::SQLITE_IOERR_READ, ffi::SQLITE_IOERR_SHORT_READ] {
            assert!(sqlite(code).damaged(), "{code}");
        }
        // Reading again may well work: another program holding the file, a full disk, a lock.
        for code in [ffi::SQLITE_BUSY, ffi::SQLITE_FULL, ffi::SQLITE_IOERR_LOCK, ffi::SQLITE_CANTOPEN] {
            assert!(!sqlite(code).damaged(), "{code}");
        }
        assert!(!SoundsError::Open { path: PathBuf::new(), why: String::new() }.damaged());
    }

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

    /// The quick way of comparing paths gives what folding every letter gives, wherever two paths part and whatever
    /// their length: paths made of a few letters, so they share long beginnings and differ by case.
    #[test]
    fn paths_compare_the_same_the_quick_way() {
        let slow =
            |a: &str, b: &str| a.bytes().map(|c| c.to_ascii_lowercase()).cmp(b.bytes().map(|c| c.to_ascii_lowercase()));
        let letters = ["a", "A", "b", "\\", "_", "É", "é", "["];
        let seed = std::cell::Cell::new(0x2545_f491_4f6c_dd1du64);
        let next = || {
            let mut x = seed.get();
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            seed.set(x);
            x
        };
        let path = |n: u64| -> String { (0..n).map(|_| letters[(next() % 8) as usize]).collect() };
        for _ in 0..20_000 {
            let a = path(next() % 30);
            // Often the same as the first for a while, then apart.
            let mut b: String = a.chars().take((next() % 30) as usize).collect();
            b.push_str(&path(next() % 12));
            assert_eq!(by_path(&a, &b), slow(&a, &b), "{a:?} {b:?}");
            assert_eq!(by_path(&b, &a), slow(&b, &a), "{b:?} {a:?}");
            assert_eq!(by_path(&a, &a), std::cmp::Ordering::Equal);
        }
    }

    #[test]
    fn the_vectors_come_straight_after_the_sound_columns() {
        let conn = Connection::open_in_memory().unwrap();
        // The C# app's sounds table (`SoundLibrary.Schema`).
        conn.execute_batch(
            "CREATE TABLE sounds (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE, size INTEGER,
                 mtime INTEGER, error TEXT, analysis TEXT, features BLOB, clap BLOB, name_type TEXT, user_type TEXT,
                 type TEXT, type_source TEXT, type_conf REAL, favourite INTEGER DEFAULT 0, rating INTEGER DEFAULT 0,
                 plays INTEGER DEFAULT 0, last_played INTEGER)",
        )
        .unwrap();
        assert_eq!(SOUND_COLUMNS, 13);
        assert_eq!(conn.prepare(SOUND_SQL).unwrap().column_count(), SOUND_COLUMNS);
        let st = conn.prepare(SOUND_AND_VECTORS_SQL).unwrap();
        assert_eq!(st.column_index("features").unwrap(), SOUND_COLUMNS);
        assert_eq!(st.column_index("clap").unwrap(), SOUND_COLUMNS + 1);
        assert_eq!(st.column_count(), SOUND_COLUMNS + 2);
        assert_eq!(columns("a"), 1);
        assert_eq!(columns("a, b,c"), 3);
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
