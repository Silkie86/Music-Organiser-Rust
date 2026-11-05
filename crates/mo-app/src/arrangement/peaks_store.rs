//! Arrangement waveforms kept between picks and runs in `arrangement-peaks.db` (the app's own, in its data folder), so
//! picking a song again doesn't decode its audio files again: before, every pick decoded up to a minute of each clip's
//! file, 16 to 32 at once.
//!
//! Each file's [`Peaks`] are kept with its size and last-written time (to the nanosecond), and used again only while
//! both are as they were. A value is kept as one byte (0 to 255 for 0 to 1): finer than a pixel of the tallest lane,
//! and a minute of sound takes 12 KB. The store is held under [`LIMIT`] bytes of peaks: the files picked longest ago
//! go first (rule 28).
//!
//! It's only what was read before: a store that SQLite says is damaged, or from another version, is made again empty.
//! A file with another app's id in it is never thrown away: the waveforms are then read each time, as before.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};

use super::Peaks;

/// The store's name, in the app's data folder.
pub const FILE_NAME: &str = "arrangement-peaks.db";

/// What the store keeps (`PRAGMA user_version`). Bump it when [`Peaks::of`] boils a file down differently (rule 7).
/// What reads the audio is the caller's to say ([`PeakStore::open`]'s `reader`).
const VERSION: i64 = 1;

/// Marks the file as this app's waveform store (`PRAGMA application_id`): "MOAP".
const APPLICATION_ID: i64 = 0x4D4F_4150;

/// The most bytes of peaks kept: about 5,000 one-minute files.
pub const LIMIT: i64 = 64 * 1024 * 1024;

const SCHEMA: &str =
    "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL) WITHOUT ROWID;
    CREATE TABLE IF NOT EXISTS peaks (path TEXT PRIMARY KEY NOT NULL, size INTEGER NOT NULL,
    modified INTEGER NOT NULL, used INTEGER NOT NULL, file_seconds REAL NOT NULL, step REAL NOT NULL,
    peaks BLOB NOT NULL) WITHOUT ROWID;";

/// A file as it is now: its last-written time (nanoseconds since 1970) and size; `None` when it can't be looked at.
pub fn stamp(path: &Path) -> Option<(i64, i64)> {
    let m = std::fs::metadata(path).ok()?;
    let at = m.modified().ok()?.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO);
    Some((i64::try_from(at.as_nanos()).unwrap_or(i64::MAX), i64::try_from(m.len()).unwrap_or(i64::MAX)))
}

/// Now, in milliseconds since 1970: when a waveform was last used.
fn now() -> i64 {
    let at = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO);
    i64::try_from(at.as_millis()).unwrap_or(i64::MAX)
}

fn to_bytes(values: &[f32]) -> Vec<u8> {
    values.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8).collect()
}

fn from_bytes(bytes: &[u8]) -> Vec<f32> {
    bytes.iter().map(|&b| f32::from(b) / 255.0).collect()
}

fn is_damage(e: &rusqlite::Error) -> bool {
    matches!(e.sqlite_error_code(), Some(rusqlite::ErrorCode::NotADatabase | rusqlite::ErrorCode::DatabaseCorrupt))
}

/// The waveforms kept between runs.
pub struct PeakStore {
    conn: Connection,
}

impl PeakStore {
    /// Opens the store at `path`, making it (and its folder) if it isn't there. One SQLite says is damaged, or from
    /// another version, is made again empty; one with another app's id is left as it is. `reader` names what reads
    /// the audio (its version, how much of a file it reads): kept waveforms read another way are forgotten. An error
    /// says why there's no store.
    pub fn open(path: &Path, reader: &str) -> Result<PeakStore, String> {
        match Self::open_as_is(path, reader) {
            Err(e) if is_damage(&e) => {
                for end in ["", "-wal", "-shm", "-journal"] {
                    let mut p = path.as_os_str().to_owned();
                    p.push(end);
                    let _ = std::fs::remove_file(PathBuf::from(p));
                }
                match Self::open_as_is(path, reader) {
                    Ok(Some(store)) => Ok(store),
                    Ok(None) => Err(format!("{e}; it couldn't be made again")),
                    Err(again) => Err(format!("{e}; made again, {again}")),
                }
            }
            Err(e) => Err(e.to_string()),
            Ok(None) => Err(format!("{} isn't the app's waveform store, so it's left as it is", path.display())),
            Ok(Some(store)) => Ok(store),
        }
    }

    fn open_as_is(path: &Path, reader: &str) -> rusqlite::Result<Option<PeakStore>> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            let _ = std::fs::create_dir_all(dir);
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(2))?;
        let id: i64 = conn.pragma_query_value(None, "application_id", |r| r.get(0))?;
        let tables: i64 = conn.query_row("SELECT COUNT(*) FROM sqlite_master", [], |r| r.get(0))?;
        if id != APPLICATION_ID && (id != 0 || tables > 0) {
            return Ok(None);
        }
        conn.pragma_update(None, "journal_mode", "WAL")?;
        // Only what was read before: a write lost to a power cut is read again, so no wait for the disk at each one.
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "temp_store", "MEMORY")?;
        // The WAL file doesn't stay bigger than this after a big write (rule 28).
        conn.pragma_update(None, "journal_size_limit", 4 * 1024 * 1024)?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version != VERSION {
            // The id goes in with the table, in one go: a store cut short between the two would look like another
            // app's.
            // Pages freed when old waveforms go are given back to the disk ([`PeakStore::put`]); set before any
            // table is made in a new file.
            conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
            conn.execute_batch(&format!(
                "BEGIN IMMEDIATE; PRAGMA application_id = {APPLICATION_ID}; DROP TABLE IF EXISTS peaks;
                 DROP TABLE IF EXISTS meta; {SCHEMA} PRAGMA user_version = {VERSION}; COMMIT;"
            ))?;
        }
        let kept: Option<String> =
            conn.query_row("SELECT value FROM meta WHERE key = 'reader'", [], |r| r.get(0)).optional()?;
        if kept.as_deref() != Some(reader) {
            conn.execute_batch("BEGIN IMMEDIATE; DELETE FROM peaks;")?;
            conn.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('reader', ?1)", [reader])?;
            conn.execute_batch("COMMIT;")?;
        }
        // A quick look at one row, so a damaged table shows now (and is made again) rather than at each pick.
        conn.query_row("SELECT COUNT(*) FROM peaks", [], |r| r.get::<_, i64>(0))?;
        Ok(Some(PeakStore { conn }))
    }

    /// A file's waveform, if it's kept for the file as it is now (`stamp`). Mark the ones used with [`PeakStore::put`].
    pub fn get(&self, path: &Path, stamp: (i64, i64)) -> Option<Peaks> {
        let text = path.to_str()?;
        let row: Option<(f64, f64, Vec<u8>)> = self
            .conn
            .query_row(
                "SELECT file_seconds, step, peaks FROM peaks WHERE path = ?1 AND modified = ?2 AND size = ?3",
                params![text, stamp.0, stamp.1],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .ok()
            .flatten();
        let (file_seconds, step, bytes) = row?;
        Some(Peaks { file_seconds, step, values: from_bytes(&bytes) })
    }

    /// Keeps the waveforms read now, each for its file as it was (`stamp`), and marks the kept ones `used` now, in one
    /// go; then the files picked longest ago go until the store is under [`LIMIT`].
    pub fn put(&mut self, read: &[(PathBuf, (i64, i64), &Peaks)], used: &[&Path]) -> rusqlite::Result<()> {
        if read.is_empty() && used.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        let mut freed = false;
        {
            let mut touch = tx.prepare_cached("UPDATE peaks SET used = ?2 WHERE path = ?1")?;
            for path in used {
                if let Some(text) = path.to_str() {
                    touch.execute(params![text, now()])?;
                }
            }
            let mut put = tx.prepare_cached(
                "INSERT OR REPLACE INTO peaks (path, size, modified, used, file_seconds, step, peaks)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            let used = now();
            for (path, (at, size), peaks) in read {
                // A path that isn't Unicode is read each time.
                let Some(text) = path.to_str() else { continue };
                put.execute(params![text, size, at, used, peaks.file_seconds, peaks.step, to_bytes(&peaks.values)])?;
            }
            let total: i64 = tx.query_row("SELECT COALESCE(SUM(LENGTH(peaks)), 0) FROM peaks", [], |r| r.get(0))?;
            if total > LIMIT {
                let mut over = total - LIMIT;
                let oldest: Vec<(String, i64)> = {
                    let mut st = tx.prepare("SELECT path, LENGTH(peaks) FROM peaks ORDER BY used, path")?;
                    st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?
                };
                let mut gone = tx.prepare_cached("DELETE FROM peaks WHERE path = ?1")?;
                for (path, bytes) in oldest {
                    if over <= 0 {
                        break;
                    }
                    gone.execute([&path])?;
                    over -= bytes;
                    freed = true;
                }
            }
        }
        tx.commit()?;
        if freed {
            // Gives the freed pages back to the disk, so the file stays near the limit too.
            self.conn.execute_batch("PRAGMA incremental_vacuum;")?;
        }
        Ok(())
    }

    #[cfg(test)]
    fn count(&self) -> i64 {
        self.conn.query_row("SELECT COUNT(*) FROM peaks", [], |r| r.get(0)).unwrap_or(-1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mo-app-peaks-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("test folder");
        dir
    }

    #[test]
    fn a_waveform_is_kept_for_its_file_as_it_was() {
        let dir = folder("kept");
        let store_path = dir.join("data").join(FILE_NAME);
        let audio = dir.join("kick.wav");
        std::fs::write(&audio, b"pretend audio").expect("write");
        let stamp = stamp(&audio).expect("stamp");
        let peaks = Peaks { file_seconds: 2.0, step: 0.005, values: vec![0.0, 0.5, 1.0, 0.25] };
        let mut store = PeakStore::open(&store_path, "test 60 s").expect("open");
        assert_eq!(store.get(&audio, stamp), None);
        store.put(&[(audio.clone(), stamp, &peaks)], &[]).expect("put");
        drop(store);

        let store = PeakStore::open(&store_path, "test 60 s").expect("open again");
        let kept = store.get(&audio, stamp).expect("kept");
        assert_eq!((kept.file_seconds, kept.step, kept.values.len()), (2.0, 0.005, 4));
        for (a, b) in kept.values.iter().zip(&peaks.values) {
            assert!((a - b).abs() <= 0.51 / 255.0, "{a} against {b}");
        }
        // The file changed: not used.
        assert_eq!(store.get(&audio, (stamp.0 + 1, stamp.1)), None);
        drop(store);
        // Read another way: forgotten.
        let store = PeakStore::open(&store_path, "test 120 s").expect("open with another reader");
        assert_eq!(store.get(&audio, stamp), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_store_stays_under_its_limit_by_dropping_the_oldest() {
        let dir = folder("limit");
        let mut store = PeakStore::open(&dir.join(FILE_NAME), "test").expect("open");
        // Each a quarter of the limit: the fifth pushes out the first.
        let big = Peaks { file_seconds: 1.0, step: 0.005, values: vec![0.5; (LIMIT / 4) as usize] };
        for n in 0..5 {
            let path = dir.join(format!("{n}.wav"));
            store.conn.execute("UPDATE peaks SET used = used - 10", []).expect("age the ones before");
            store.put(&[(path, (n, 1), &big)], &[]).expect("put");
        }
        assert_eq!(store.count(), 4);
        assert_eq!(store.get(&dir.join("0.wav"), (0, 1)), None, "the oldest went");
        assert!(store.get(&dir.join("4.wav"), (4, 1)).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_damaged_store_is_made_again_and_another_apps_file_is_left() {
        let dir = folder("damaged");
        let path = dir.join(FILE_NAME);
        std::fs::write(&path, vec![3u8; 4096]).expect("write");
        assert!(PeakStore::open(&path, "test").is_ok());
        let other = dir.join("other.db");
        let conn = Connection::open(&other).expect("open");
        conn.execute_batch("CREATE TABLE mine (x); INSERT INTO mine VALUES (1);").expect("make");
        drop(conn);
        assert!(PeakStore::open(&other, "test").is_err());
        let n: i64 = Connection::open(&other)
            .and_then(|c| c.query_row("SELECT COUNT(*) FROM mine", [], |r| r.get(0)))
            .expect("still there");
        assert_eq!(n, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
