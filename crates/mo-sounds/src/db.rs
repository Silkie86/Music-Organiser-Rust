//! Reading `sounds.db`, the C# app's sound library, without ever changing it.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{Local, NaiveDateTime};
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row, params};

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
    pub fn all(&self, now: NaiveDateTime) -> Result<Vec<Sound>, SoundsError> {
        // One read transaction, so every query below sees the library as it was at one moment, even while the C# app
        // saves a scan or hides a batch of copies in between.
        let snapshot = self.conn.unchecked_transaction()?;
        let mut activity = self.dated_activity(now)?;
        let mut tags: HashMap<i64, Vec<String>> = HashMap::new();
        if self.has("sound_tags") {
            let mut st = self.conn.prepare("SELECT sound_id, tag FROM sound_tags ORDER BY tag")?;
            let rows = st.query_map([], |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, String>(1)?)))?;
            for row in rows {
                if let (Some(id), tag) = row? {
                    tags.entry(id).or_default().push(tag);
                }
            }
        }
        let hidden = self.hidden()?;
        let mut st = self.conn.prepare(
            "SELECT id, path, mtime, error, analysis, name_type, user_type, type, type_source, type_conf,
                    favourite, rating, plays
             FROM sounds ORDER BY path",
        )?;
        // Every column but the id and path is read as the C# app reads it: a value of an unexpected kind in a damaged
        // row reads as none (or 0) rather than stopping the whole list.
        let rows = st.query_map([], |r| {
            Ok(Sound {
                id: r.get(0)?,
                path: r.get(1)?,
                added: local(int_at(r, 2).unwrap_or(0)),
                error: text_at(r, 3),
                // An analysis that can't be read shows as none, as in the C# app.
                analysis: text_at(r, 4).as_deref().and_then(SoundAnalysis::from_json),
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
        })?;
        let mut sounds = Vec::new();
        for row in rows {
            let mut s = row?;
            s.tags = tags.remove(&s.id).unwrap_or_default();
            s.hidden = hidden.contains(&s.id);
            let mut a = activity.remove(&s.id).unwrap_or_default();
            add_undated_plays(&mut a, s.plays);
            s.activity = a;
            sounds.push(s);
        }
        drop(st);
        snapshot.finish()?;
        Ok(sounds)
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

    /// Each sound's file size in bytes (0 when not known).
    pub fn sizes(&self) -> Result<HashMap<i64, i64>, SoundsError> {
        let mut st = self.conn.prepare("SELECT id, size FROM sounds")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, int_at(r, 1).unwrap_or(0))))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
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
    // As .NET's Enum.TryParse: the name, or the number it stands for.
    match kind {
        Some("Exact" | "0") => "Exact",
        Some("Trimmed" | "2") => "Trimmed",
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
    fn a_year_before() {
        let leap = chrono::NaiveDate::from_ymd_opt(2024, 2, 29).unwrap().and_hms_opt(12, 0, 0).unwrap();
        assert_eq!(year_before(leap).to_string(), "2023-02-28 12:00:00");
    }
}
