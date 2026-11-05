//! Fingerprints of the Bitwig multisamples Space has read, kept between runs in `bitwig-prints.db` (the app's own, in
//! its data folder), so a look for Bitwig copies after a restart doesn't read every same-size multisample again (on
//! Solomon's PC, the first look after each start read hundreds of gigabytes and froze the window for a second).
//!
//! Each file is kept with its size and last-written time (to the nanosecond), and used again only while both are as
//! they were. Two prints are kept: one of the file's first and last 64 KB (its "ends"), read first, and one of the
//! whole file, read only when the ends of a copy and a Library file are the same. Most files of the same size differ
//! in their ends, so most are never read whole.
//!
//! It's only what was read before: a store that's damaged, or kept by another version, is made again empty. A file
//! with another app's id in it is never thrown away: the prints are then kept only while the app runs. After a whole
//! look, files that look didn't ask about are forgotten, so the store never holds more than one look's files (rule
//! 28).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};

use crate::size::{file_hash, open_to_read_once, read_once_done};

/// The store's name, in the app's data folder.
pub const FILE_NAME: &str = "bitwig-prints.db";

/// What the store keeps (`PRAGMA user_version`). Bump it when a print is worked out differently, so every file is read
/// again (rule 7).
const VERSION: i64 = 1;

/// Marks the file as this app's prints store (`PRAGMA application_id`): "MOBP".
const APPLICATION_ID: i64 = 0x4D4F_4250;

/// How much of each end of a file its ends print reads.
pub const END_BYTES: u64 = 64 * 1024;

const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS prints (path TEXT PRIMARY KEY NOT NULL, size INTEGER NOT NULL,
    modified INTEGER NOT NULL, ends BLOB, whole BLOB) WITHOUT ROWID;";

/// A file's prints, with the size and time it had when they were read: used again only while they still match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Known {
    len: u64,
    modified: SystemTime,
    ends: Option<[u8; 32]>,
    whole: Option<[u8; 32]>,
}

/// Which print to read.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Ends,
    Whole,
}

/// Fingerprints of files once read, used again only while each file's size and time are as they were; kept in a store
/// between runs when one is given ([`Prints::remember_in`]).
#[derive(Default)]
pub struct Prints {
    state: Mutex<State>,
    store: Option<Mutex<Connection>>,
}

#[derive(Default)]
struct State {
    known: HashMap<PathBuf, Known>,
    /// The files asked about since the last [`Prints::keep`].
    asked: HashSet<PathBuf>,
    /// The files read since the last [`Prints::keep`], to write to the store.
    read: HashSet<PathBuf>,
}

impl Prints {
    /// Opens the store at `path` (making it if it isn't there), and takes in the prints it keeps. A store that's
    /// damaged or from another version is made again, empty. An error says why there's no store: the prints are kept
    /// while the app runs only.
    pub fn remember_in(&mut self, path: &Path) -> Result<usize, String> {
        let conn = open(path)?;
        let rows = match load(&conn) {
            Ok(rows) => rows,
            Err(e) => {
                // Read as far as the header (it's ours), but not its rows: damaged. Made again, empty.
                drop(conn);
                remove(path);
                let conn = open(path).map_err(|again| format!("{e}; made again, {again}"))?;
                self.store = Some(Mutex::new(conn));
                return Ok(0);
            }
        };
        let n = rows.len();
        if let Ok(mut s) = self.state.lock() {
            for (path, k) in rows {
                s.known.entry(path).or_insert(k);
            }
        }
        self.store = Some(Mutex::new(conn));
        Ok(n)
    }

    /// The print of a file's first and last [`END_BYTES`] (the whole file when it's no longer than both): kept, or
    /// read now. `None` when it can't be read (Bitwig may have it open: it's tried again next time), or when stopped.
    pub fn ends(&self, path: &Path, stop: &AtomicBool) -> Option<[u8; 32]> {
        self.of(path, Kind::Ends, stop)
    }

    /// The print of the whole file: kept, or read now. As [`Prints::ends`].
    pub fn whole(&self, path: &Path, stop: &AtomicBool) -> Option<[u8; 32]> {
        self.of(path, Kind::Whole, stop)
    }

    fn of(&self, path: &Path, kind: Kind, stop: &AtomicBool) -> Option<[u8; 32]> {
        let meta = fs::metadata(path).ok()?;
        let (len, modified) = (meta.len(), meta.modified().ok());
        let kept = {
            let mut s = self.state.lock().ok()?;
            s.asked.insert(path.to_path_buf());
            s.known.get(path).copied()
        };
        let same = kept.filter(|k| Some(k.modified) == modified && k.len == len);
        let print = match (same, kind) {
            (Some(Known { ends: Some(p), .. }), Kind::Ends) | (Some(Known { whole: Some(p), .. }), Kind::Whole) => {
                return Some(p);
            }
            _ => match kind {
                Kind::Ends => ends_hash(path, stop)?,
                Kind::Whole => file_hash(path, stop)?,
            },
        };
        // A file without a time can't be checked next time, so it's read again then.
        let modified = modified?;
        // Changed between the look above and the read: it's kept with the older size and time, so it's read again.
        let mut known = same.unwrap_or(Known { len, modified, ends: None, whole: None });
        // A file no longer than both ends is read whole either way: its two prints are the same.
        let small = len <= 2 * END_BYTES;
        if kind == Kind::Ends || small {
            known.ends = Some(print);
        }
        if kind == Kind::Whole || small {
            known.whole = Some(print);
        }
        let mut s = self.state.lock().ok()?;
        s.known.insert(path.to_path_buf(), known);
        s.read.insert(path.to_path_buf());
        Some(print)
    }

    /// Writes the prints read since the last time to the store. After a whole look (`whole_look`), the files it didn't
    /// ask about are forgotten, in the store and here, so neither grows past one look's files. An error says why the
    /// store couldn't be written (the prints are still kept while the app runs).
    pub fn keep(&self, whole_look: bool) -> Result<(), String> {
        let Ok(mut s) = self.state.lock() else { return Ok(()) };
        let s = &mut *s;
        let read: Vec<(PathBuf, Known)> =
            s.read.drain().filter_map(|p| s.known.get(&p).copied().map(|k| (p, k))).collect();
        let asked = std::mem::take(&mut s.asked);
        if whole_look {
            s.known.retain(|p, _| asked.contains(p));
        }
        let Some(store) = &self.store else { return Ok(()) };
        let Ok(mut conn) = store.lock() else { return Ok(()) };
        write(&mut conn, &read, whole_look.then_some(&asked)).map_err(|e| e.to_string())
    }

    #[cfg(test)]
    fn known(&self) -> usize {
        self.state.lock().map(|s| s.known.len()).unwrap_or(0)
    }
}

/// The print of a file's first and last [`END_BYTES`], or of the whole file when it's no longer than both; `None` when
/// it can't be read, changed while it was read, or when stopped.
pub fn ends_hash(path: &Path, stop: &AtomicBool) -> Option<[u8; 32]> {
    if stop.load(Ordering::Relaxed) {
        return None;
    }
    let mut file = open_to_read_once(path).ok()?;
    let before = file.metadata().ok()?;
    let len = before.len();
    if len <= 2 * END_BYTES {
        drop(file);
        return file_hash(path, stop);
    }
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; END_BYTES as usize];
    file.read_exact(&mut buf).ok()?;
    hash.update(&buf);
    file.seek(SeekFrom::Start(len - END_BYTES)).ok()?;
    file.read_exact(&mut buf).ok()?;
    hash.update(&buf);
    read_once_done(&file);
    let after = fs::metadata(path).ok()?;
    let same = after.len() == len && after.modified().ok() == before.modified().ok();
    same.then(|| hash.finalize().into())
}

fn nanos(t: SystemTime) -> i64 {
    let at = t.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO);
    i64::try_from(at.as_nanos()).unwrap_or(i64::MAX)
}

fn time_of(nanos: i64) -> SystemTime {
    UNIX_EPOCH + Duration::from_nanos(u64::try_from(nanos).unwrap_or(0))
}

fn print_of(blob: Option<Vec<u8>>) -> Option<[u8; 32]> {
    blob.and_then(|b| <[u8; 32]>::try_from(b.as_slice()).ok())
}

/// Opens the store, making it (and its folder) when it isn't there. One kept by another version is emptied; one with
/// another app's id is left as it is (an error).
fn open(path: &Path) -> Result<Connection, String> {
    let err = |e: rusqlite::Error| e.to_string();
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let conn = match Connection::open(path).and_then(|c| check_id(&c).map(|ours| (c, ours))) {
        Ok((conn, true)) => conn,
        Ok((_, false)) => {
            return Err(format!("{} isn't the app's Bitwig prints store, so it's left as it is", path.display()));
        }
        // Not a database at all (damaged where SQLite keeps its header): only what was read before, made again.
        Err(_) => {
            remove(path);
            let conn = Connection::open(path).map_err(err)?;
            if !check_id(&conn).map_err(err)? {
                return Err("it couldn't be made again".into());
            }
            conn
        }
    };
    conn.busy_timeout(Duration::from_secs(2)).map_err(err)?;
    conn.pragma_update(None, "journal_mode", "WAL").map_err(err)?;
    // Only what was read before: a write lost to a power cut is read again, so no wait for the disk at each one.
    conn.pragma_update(None, "synchronous", "NORMAL").map_err(err)?;
    conn.pragma_update(None, "temp_store", "MEMORY").map_err(err)?;
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0)).map_err(err)?;
    if version != VERSION {
        conn.execute_batch(&format!(
            "BEGIN IMMEDIATE; DROP TABLE IF EXISTS prints; {SCHEMA} PRAGMA user_version = {VERSION}; COMMIT;"
        ))
        .map_err(err)?;
    }
    conn.pragma_update(None, "application_id", APPLICATION_ID).map_err(err)?;
    Ok(conn)
}

/// Whether the file is this store or a new, empty file (never another database).
fn check_id(conn: &Connection) -> rusqlite::Result<bool> {
    let id: i64 = conn.pragma_query_value(None, "application_id", |r| r.get(0))?;
    let tables: i64 = conn.query_row("SELECT COUNT(*) FROM sqlite_master", [], |r| r.get(0))?;
    Ok(id == APPLICATION_ID || (id == 0 && tables == 0))
}

/// Removes the store and the files SQLite keeps beside it.
fn remove(path: &Path) {
    for end in ["", "-wal", "-shm", "-journal"] {
        let mut p = path.as_os_str().to_owned();
        p.push(end);
        // Not there is fine; one Windows holds open stays, and opening it says so.
        let _ = fs::remove_file(PathBuf::from(p));
    }
}

fn load(conn: &Connection) -> rusqlite::Result<Vec<(PathBuf, Known)>> {
    let mut st = conn.prepare("SELECT path, size, modified, ends, whole FROM prints")?;
    let rows = st.query_map([], |r| {
        let path: String = r.get(0)?;
        let known = Known {
            len: u64::try_from(r.get::<_, i64>(1)?).unwrap_or(0),
            modified: time_of(r.get(2)?),
            ends: print_of(r.get(3)?),
            whole: print_of(r.get(4)?),
        };
        Ok((PathBuf::from(path), known))
    })?;
    rows.collect()
}

/// Writes the prints read, in one go; with `only`, rows for files not in it go too.
fn write(conn: &mut Connection, read: &[(PathBuf, Known)], only: Option<&HashSet<PathBuf>>) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    if let Some(only) = only {
        let kept: Vec<String> = {
            let mut st = tx.prepare("SELECT path FROM prints")?;
            st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
        };
        let mut gone = tx.prepare("DELETE FROM prints WHERE path = ?1")?;
        for path in kept.iter().filter(|p| !only.contains(Path::new(p))) {
            gone.execute([path])?;
        }
    }
    {
        let mut put = tx.prepare(
            "INSERT INTO prints (path, size, modified, ends, whole) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(path) DO UPDATE SET size = ?2, modified = ?3, ends = ?4, whole = ?5",
        )?;
        for (path, k) in read {
            // A path that isn't Unicode is kept while the app runs only (the store keeps text).
            let Some(text) = path.to_str() else { continue };
            put.execute(params![
                text,
                i64::try_from(k.len).unwrap_or(i64::MAX),
                nanos(k.modified),
                k.ends.as_ref().map(<[u8; 32]>::as_slice),
                k.whole.as_ref().map(<[u8; 32]>::as_slice),
            ])?;
        }
    }
    tx.commit()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mo-space-prints-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("test folder");
        dir
    }

    #[test]
    fn ends_read_only_the_ends_and_small_files_whole() {
        let dir = folder("ends");
        let stop = AtomicBool::new(false);
        let big = |middle: u8| {
            let mut b = vec![1u8; 3 * END_BYTES as usize];
            b[END_BYTES as usize + 10] = middle;
            b
        };
        let (a, b) = (dir.join("a.multisample"), dir.join("b.multisample"));
        fs::write(&a, big(5)).expect("write");
        fs::write(&b, big(6)).expect("write");
        assert_eq!(ends_hash(&a, &stop), ends_hash(&b, &stop), "only the middle differs");
        assert_ne!(file_hash(&a, &stop), file_hash(&b, &stop));
        let small = dir.join("small.multisample");
        fs::write(&small, b"tiny").expect("write");
        assert_eq!(ends_hash(&small, &stop), file_hash(&small, &stop));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn prints_are_kept_between_runs_and_forgotten_after_a_whole_look() {
        let dir = folder("kept");
        let store = dir.join(FILE_NAME);
        let stop = AtomicBool::new(false);
        let (a, b) = (dir.join("a.multisample"), dir.join("b.multisample"));
        fs::write(&a, vec![2u8; 300 * 1024]).expect("write");
        fs::write(&b, b"bee").expect("write");
        let mut first = Prints::default();
        assert_eq!(first.remember_in(&store), Ok(0));
        let (ends, whole) = (first.ends(&a, &stop), first.whole(&a, &stop));
        assert!(ends.is_some() && whole.is_some() && ends != whole);
        assert!(first.whole(&b, &stop).is_some());
        first.keep(true).expect("kept");
        drop(first);

        // The next run has them without reading: a file that changed is read again.
        let mut next = Prints::default();
        assert_eq!(next.remember_in(&store), Ok(2));
        assert_eq!(next.whole(&a, &stop), whole);
        assert_eq!(next.ends(&a, &stop), ends);
        // Only `a` was asked about in this whole look: `b` is forgotten.
        next.keep(true).expect("kept");
        assert_eq!(next.known(), 1);
        drop(next);
        let mut third = Prints::default();
        assert_eq!(third.remember_in(&store), Ok(1));
        // A look that stopped forgets nothing.
        third.keep(false).expect("kept");
        assert_eq!(third.known(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_changed_file_is_read_again() {
        let dir = folder("changed");
        let stop = AtomicBool::new(false);
        let a = dir.join("a.multisample");
        fs::write(&a, b"one").expect("write");
        let prints = Prints::default();
        let first = prints.whole(&a, &stop);
        fs::write(&a, b"two!").expect("write");
        assert_ne!(prints.whole(&a, &stop), first);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_damaged_store_is_made_again_and_another_apps_file_is_left() {
        let dir = folder("damaged");
        let store = dir.join(FILE_NAME);
        fs::write(&store, vec![7u8; 4096]).expect("write");
        let mut prints = Prints::default();
        assert_eq!(prints.remember_in(&store), Ok(0));
        drop(prints);

        let other = dir.join("other.db");
        let conn = Connection::open(&other).expect("open");
        conn.execute_batch("CREATE TABLE mine (x); INSERT INTO mine VALUES (1);").expect("make");
        drop(conn);
        let mut prints = Prints::default();
        assert!(prints.remember_in(&other).is_err());
        let conn = Connection::open(&other).expect("open");
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM mine", [], |r| r.get(0)).expect("still there");
        assert_eq!(n, 1);
        let _ = fs::remove_dir_all(&dir);
    }
}
