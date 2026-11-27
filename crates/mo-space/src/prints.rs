//! Fingerprints of the Bitwig multisamples Space has read, kept between runs in `bitwig-prints.db` (the app's own, in
//! its data folder), so a look for Bitwig copies after a restart doesn't read every same-size multisample again (on
//! Solomon's PC, the first look after each start read hundreds of gigabytes and froze the window for a second). The
//! Space page's look at your songs and the backup plan keep the prints of song saves the same way, each in a store of
//! its own, to find the saves that are copies of another ([`crate::song`]).
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

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};

use crate::size::file_hash;

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

/// Each file's prints, and what happened to it since the last [`Prints::keep`] as marks beside them ([`ASKED`],
/// [`READ`], [`WHOLE_READ`]): each path is held once (a Library of 235,000 files held it four times, about 80 MB more).
#[derive(Default)]
struct State {
    known: HashMap<PathBuf, Slot>,
    /// How many are marked [`READ`] and how many [`WHOLE_READ`], so a keep with nothing to do looks at none of them.
    reads: usize,
    whole_reads: usize,
    /// Files a whole look forgot that a keep couldn't take out of the store yet: the next keep tries again.
    forget: Vec<PathBuf>,
}

#[derive(Clone, Copy)]
struct Slot {
    known: Known,
    marks: u8,
}

/// Asked about since the last [`Prints::keep`] of a whole look.
const ASKED: u8 = 1;
/// Read since the last [`Prints::keep`], to write to the store.
const READ: u8 = 2;
/// Its whole print was read since the last [`Prints::keep`] (a read of the whole file, or of a small file's ends, which
/// are the whole of it), so it's of the file as it is now.
const WHOLE_READ: u8 = 4;

impl State {
    /// Marks a slot, counting the marks new to it.
    fn mark(slot: &mut Slot, marks: u8, reads: &mut usize, whole_reads: &mut usize) {
        let new = marks & !slot.marks;
        *reads += usize::from(new & READ != 0);
        *whole_reads += usize::from(new & WHOLE_READ != 0);
        slot.marks |= marks;
    }
}

impl Prints {
    /// Opens the store at `path` (making it if it isn't there), and takes in the prints it keeps. A store that's
    /// damaged or from another version is made again, empty. An error says why there's no store: the prints are kept
    /// while the app runs only.
    pub fn remember_in(&mut self, path: &Path) -> Result<usize, String> {
        let conn = open(path)?;
        // Read straight into a table of their own (no list of them all first), taken in only once the whole store
        // has been read: one that stops part way (damaged, busy) leaves nothing of it here.
        let mut fresh: HashMap<PathBuf, Slot> = HashMap::new();
        let loaded = load(&conn, |path, known| {
            // A file kept twice (two spellings of one path) keeps the first, as it always has.
            fresh.entry(path).or_insert(Slot { known, marks: 0 });
        });
        let n = match loaded {
            Ok(n) => n,
            Err(e) if is_damage(&e) => {
                // Read as far as the header (it's ours), but not its rows: damaged. Made again, empty.
                drop(conn);
                remove(path);
                let conn = open(path).map_err(|again| format!("{e}; made again, {again}"))?;
                self.store = Some(Mutex::new(conn));
                return Ok(0);
            }
            // Busy, or a disk that couldn't be read just now: left as it is, and tried again at the next look.
            Err(e) => return Err(e.to_string()),
        };
        if let Ok(mut s) = self.state.lock() {
            if s.known.is_empty() {
                s.known = fresh;
            } else {
                // What was read while the app ran wins over what the store had.
                for (path, slot) in fresh {
                    s.known.entry(path).or_insert(slot);
                }
            }
        }
        self.store = Some(Mutex::new(conn));
        Ok(n)
    }

    /// The print of a file's first and last [`END_BYTES`] (the whole file when it's no longer than both): kept, or
    /// read now. `None` when it can't be read (Bitwig may have it open: it's tried again next time), or when stopped.
    pub fn ends(&self, path: &Path, stop: &AtomicBool) -> Option<[u8; 32]> {
        self.of(path, Kind::Ends, false, stop).map(|(print, _)| print)
    }

    /// The print of the whole file: kept, or read now. As [`Prints::ends`].
    pub fn whole(&self, path: &Path, stop: &AtomicBool) -> Option<[u8; 32]> {
        self.of(path, Kind::Whole, false, stop).map(|(print, _)| print)
    }

    /// As [`Prints::whole`], and whether the whole file was read since the last [`Prints::keep`], so the print is of
    /// it as it is now, rather than known from before by its size and time alone. A read of its ends alone doesn't
    /// count: the whole print kept with them can be older.
    pub fn whole_and_if_read(&self, path: &Path, stop: &AtomicBool) -> Option<([u8; 32], bool)> {
        self.of(path, Kind::Whole, false, stop)
    }

    /// The print of the whole file read now, whatever is kept, and kept in place of the one before: for a print that's
    /// about to be acted on (the backup leaves a copy out on its word). One that isn't the print kept means the file
    /// was written again keeping its size and time, so the ends kept with that print go too.
    pub fn whole_afresh(&self, path: &Path, stop: &AtomicBool) -> Option<[u8; 32]> {
        self.of(path, Kind::Whole, true, stop).map(|(print, _)| print)
    }

    /// The print of `kind`, and whether the whole file was read since the last [`Prints::keep`]: kept while the file's
    /// size and time are as they were (unless `afresh`), or read now.
    fn of(&self, path: &Path, kind: Kind, afresh: bool, stop: &AtomicBool) -> Option<([u8; 32], bool)> {
        let meta = fs::metadata(path).ok()?;
        let (len, modified) = (meta.len(), meta.modified().ok());
        let (kept, whole_read) = {
            let mut s = self.state.lock().ok()?;
            // Only a file known is marked: a whole look's keep forgets the known ones it didn't ask about, and one
            // asked about but not known has nothing to forget.
            match s.known.get_mut(path) {
                Some(slot) => {
                    slot.marks |= ASKED;
                    (Some(slot.known), slot.marks & WHOLE_READ != 0)
                }
                None => (None, false),
            }
        };
        let same = kept.filter(|k| Some(k.modified) == modified && k.len == len);
        match (same, kind) {
            (Some(Known { ends: Some(p), .. }), Kind::Ends) | (Some(Known { whole: Some(p), .. }), Kind::Whole)
                if !afresh =>
            {
                return Some((p, whole_read));
            }
            _ => {}
        }
        let print = match kind {
            Kind::Ends => ends_hash(path, stop)?,
            Kind::Whole => file_hash(path, stop)?,
        };
        // A file no longer than both ends is read whole either way: its two prints are the same.
        let small = len <= 2 * END_BYTES;
        let whole_now = kind == Kind::Whole || small;
        // A file without a time can't be checked next time: its print is used now and read again then.
        let Some(modified) = modified else { return Some((print, whole_now)) };
        // Changed between the look above and the read: it's kept with the older size and time, so it's read again.
        let mut known = same.unwrap_or(Known { len, modified, ends: None, whole: None });
        let unchanged = afresh && known.whole == Some(print);
        if afresh && known.whole.is_some_and(|w| w != print) {
            known.ends = None;
        }
        if kind == Kind::Ends || small {
            known.ends = Some(print);
        }
        if whole_now {
            known.whole = Some(print);
        }
        // Read again and the same as kept: nothing new for the store.
        let marks = ASKED | if unchanged { 0 } else { READ } | if whole_now { WHOLE_READ } else { 0 };
        let mut s = self.state.lock().ok()?;
        let s = &mut *s;
        match s.known.get_mut(path) {
            Some(slot) => {
                slot.known = known;
                State::mark(slot, marks, &mut s.reads, &mut s.whole_reads);
            }
            None => {
                let mut slot = Slot { known, marks: 0 };
                State::mark(&mut slot, marks, &mut s.reads, &mut s.whole_reads);
                s.known.insert(path.to_path_buf(), slot);
            }
        }
        Some((print, whole_now || whole_read))
    }

    /// Writes the prints read since the last time to the store. After a whole look (`whole_look`), the files it didn't
    /// ask about are forgotten, in the store and here, so neither grows past one look's files. An error says why the
    /// store couldn't be written (the prints are still kept while the app runs).
    pub fn keep(&self, whole_look: bool) -> Result<(), String> {
        let (read, gone) = {
            let Ok(mut s) = self.state.lock() else { return Ok(()) };
            let s = &mut *s;
            // Only a whole look uses (and starts again) the files asked about: one kept part way through, or stopped,
            // leaves them for the whole look's end, so what it asked about before then isn't forgotten. The paths
            // forgotten are moved out of the table, not copied.
            let mut gone = std::mem::take(&mut s.forget);
            if whole_look {
                let before = s.known.len();
                for (path, slot) in s.known.extract_if(|_, slot| slot.marks & ASKED == 0) {
                    s.reads -= usize::from(slot.marks & READ != 0);
                    s.whole_reads -= usize::from(slot.marks & WHOLE_READ != 0);
                    gone.push(path);
                }
                // Only when much went: the table is often let go just after ([`Prints::let_go`]).
                if s.known.len() < before / 2 {
                    s.known.shrink_to_fit();
                }
            }
            // (Rows only the store has, such as another copy of the app's, are taken in by the next
            // [`Prints::remember_in`], at every look once the prints are let go, and forgotten at the whole look after.)
            // A part-way keep with something read walks the table once (about a millisecond for 235,000 files).
            // The prints read, to write: their marks stay until the store has them, so none is lost to a store
            // that's busy, or not there yet.
            let mut read: Vec<(PathBuf, Known)> = Vec::new();
            let write_them = self.store.is_some() && s.reads > 0;
            if whole_look || s.whole_reads > 0 || write_them {
                for (path, slot) in &mut s.known {
                    if write_them && slot.marks & READ != 0 {
                        read.push((path.clone(), slot.known));
                    }
                    slot.marks &= if whole_look { READ } else { READ | ASKED };
                }
                s.whole_reads = 0;
            }
            (read, gone)
        };
        // Nothing new and nothing forgotten: the store is as it should be. With no store, nothing to forget in it.
        let Some(store) = &self.store else { return Ok(()) };
        if read.is_empty() && gone.is_empty() {
            return Ok(());
        }
        let written = match store.lock() {
            Ok(mut conn) => write(&mut conn, &read, &gone).map_err(|e| e.to_string()),
            Err(_) => Err("the prints store couldn't be used".to_owned()),
        };
        let Ok(mut s) = self.state.lock() else { return written };
        let s = &mut *s;
        match &written {
            Ok(()) => {
                // Written as they were then: one read again since (other prints) stays marked for the next keep.
                for (path, known) in &read {
                    if let Some(slot) = s.known.get_mut(path)
                        && slot.marks & READ != 0
                        && slot.known == *known
                    {
                        slot.marks &= !READ;
                        s.reads -= 1;
                    }
                }
            }
            Err(_) => {
                // Tried again at the next keep, each path once however many keeps fail.
                s.forget.extend(gone);
                s.forget.sort_unstable();
                s.forget.dedup();
            }
        }
        written
    }

    /// Lets go of the prints held here, when the store has every one of them (a store, and nothing read or forgotten
    /// that it doesn't have yet): they're read back from it by the next [`Prints::remember_in`], a quick read of one
    /// table, so they needn't be held between looks (a Library of 235,000 files: about 170 MB). Whether it did; if
    /// not, nothing changes.
    pub fn let_go(&mut self) -> bool {
        let all_written = self.store.is_some() && self.state.lock().is_ok_and(|s| s.reads == 0 && s.forget.is_empty());
        if all_written {
            *self = Prints::default();
        }
        all_written
    }

    /// Whether the prints are kept in a store (an earlier [`Prints::remember_in`] worked).
    pub fn remembers(&self) -> bool {
        self.store.is_some()
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
    // Opened plainly, not as a read straight through: the system would read on well past each end.
    let mut file = fs::File::open(path).ok()?;
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
            return Err(format!("{} isn't one of the app's prints stores, so it's left as it is", path.display()));
        }
        // Not a database at all (damaged where SQLite keeps its header): only what was read before, made again.
        Err(e) if is_damage(&e) => {
            remove(path);
            let conn = Connection::open(path).map_err(err)?;
            if !check_id(&conn).map_err(err)? {
                return Err("it couldn't be made again".into());
            }
            conn
        }
        // Busy (another copy of the app writing), or a disk that couldn't be read just now: left as it is.
        Err(e) => return Err(e.to_string()),
    };
    conn.busy_timeout(Duration::from_secs(2)).map_err(err)?;
    conn.pragma_update(None, "journal_mode", "WAL").map_err(err)?;
    // Only what was read before: a write lost to a power cut is read again, so no wait for the disk at each one.
    conn.pragma_update(None, "synchronous", "NORMAL").map_err(err)?;
    conn.pragma_update(None, "temp_store", "MEMORY").map_err(err)?;
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0)).map_err(err)?;
    if version != VERSION {
        // The id goes in with the table, in one go: a store cut short between the two would look like another app's.
        conn.execute_batch(&format!(
            "BEGIN IMMEDIATE; PRAGMA application_id = {APPLICATION_ID}; DROP TABLE IF EXISTS prints; {SCHEMA}
             PRAGMA user_version = {VERSION}; COMMIT;"
        ))
        .map_err(err)?;
    }
    Ok(conn)
}

/// Whether the file is this store or a new, empty file (never another database).
fn check_id(conn: &Connection) -> rusqlite::Result<bool> {
    let id: i64 = conn.pragma_query_value(None, "application_id", |r| r.get(0))?;
    let tables: i64 = conn.query_row("SELECT COUNT(*) FROM sqlite_master", [], |r| r.get(0))?;
    Ok(id == APPLICATION_ID || (id == 0 && tables == 0))
}

/// Whether an error says the file is damaged (or isn't a database at all), rather than busy or unreadable just now.
fn is_damage(e: &rusqlite::Error) -> bool {
    // A row that doesn't read back as written (no SQLite code) is damage too.
    matches!(
        e.sqlite_error_code(),
        None | Some(rusqlite::ErrorCode::NotADatabase | rusqlite::ErrorCode::DatabaseCorrupt)
    )
}

/// Forgets every print kept at `path` (after a look crashed, in case what was kept is why): the store and the files
/// SQLite keeps beside it are removed; the next look makes it again.
pub fn forget(path: &Path) {
    remove(path);
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

/// Hands each kept file's prints to `take` as it's read, and says how many there were. A damaged row stops it with an
/// error.
fn load(conn: &Connection, mut take: impl FnMut(PathBuf, Known)) -> rusqlite::Result<usize> {
    let mut st = conn.prepare("SELECT path, size, modified, ends, whole FROM prints")?;
    let mut rows = st.query([])?;
    let mut n = 0;
    while let Some(r) = rows.next()? {
        let path: String = r.get(0)?;
        let known = Known {
            len: u64::try_from(r.get::<_, i64>(1)?).unwrap_or(0),
            modified: time_of(r.get(2)?),
            ends: print_of(r.get(3)?),
            whole: print_of(r.get(4)?),
        };
        take(PathBuf::from(path), known);
        n += 1;
    }
    Ok(n)
}

/// Writes the prints read, and forgets the files `gone` (a whole look didn't ask about them), in one go.
fn write(conn: &mut Connection, read: &[(PathBuf, Known)], gone: &[PathBuf]) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    {
        let mut forget = tx.prepare("DELETE FROM prints WHERE path = ?1")?;
        for text in gone.iter().filter_map(|p| p.to_str()) {
            forget.execute([text])?;
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
    fn prints_are_let_go_only_when_the_store_has_them_all() {
        let dir = folder("let-go");
        let store = dir.join(FILE_NAME);
        let stop = AtomicBool::new(false);
        let a = dir.join("a.multisample");
        fs::write(&a, b"ay").expect("write");
        // No store: they're all there is, so they stay.
        let mut alone = Prints::default();
        let print = alone.whole(&a, &stop);
        assert!(print.is_some());
        assert!(!alone.let_go());
        assert_eq!(alone.known(), 1);
        // Read since the last keep: not in the store yet, so they stay.
        let mut look = Prints::default();
        assert_eq!(look.remember_in(&store), Ok(0));
        assert_eq!(look.whole(&a, &stop), print);
        assert!(!look.let_go());
        assert!(look.remembers() && look.known() == 1);
        // Kept: let go, and the next look reads them back without reading the file.
        look.keep(true).expect("kept");
        assert!(look.let_go());
        assert!(!look.remembers() && look.known() == 0);
        assert_eq!(look.remember_in(&store), Ok(1));
        assert_eq!(look.whole_and_if_read(&a, &stop), print.map(|p| (p, false)), "from the store, not read");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn prints_read_before_the_store_opens_are_written_to_it_later() {
        let dir = folder("before-store");
        let store = dir.join(FILE_NAME);
        let stop = AtomicBool::new(false);
        let (a, b) = (dir.join("a.multisample"), dir.join("b.multisample"));
        fs::write(&a, b"ay").expect("write");
        fs::write(&b, b"bee").expect("write");
        // A store from before has `b`'s print; a look with no store (busy at its start) reads `a`.
        let mut before = Prints::default();
        assert_eq!(before.remember_in(&store), Ok(0));
        let b_print = before.whole(&b, &stop);
        before.keep(true).expect("kept");
        drop(before);
        let mut look = Prints::default();
        let a_print = look.whole(&a, &stop);
        look.keep(true).expect("no store: nothing written, nothing lost");
        assert!(!look.let_go(), "no store yet");
        // The store opens at the next look: what was read here stays, and is written at the next keep.
        assert_eq!(look.remember_in(&store), Ok(1));
        assert_eq!(look.known(), 2);
        assert_eq!(look.whole_and_if_read(&a, &stop), a_print.map(|p| (p, false)));
        assert_eq!(look.whole(&b, &stop), b_print);
        assert!(!look.let_go(), "`a` isn't in the store yet");
        look.keep(true).expect("kept");
        assert!(look.let_go());
        assert_eq!(look.remember_in(&store), Ok(2));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn files_a_whole_look_forgot_go_from_the_store_once_it_can_be_written() {
        let dir = folder("forget-later");
        let store = dir.join(FILE_NAME);
        let stop = AtomicBool::new(false);
        let (a, b) = (dir.join("a.multisample"), dir.join("b.multisample"));
        fs::write(&a, b"ay").expect("write");
        fs::write(&b, b"bee").expect("write");
        let mut first = Prints::default();
        assert_eq!(first.remember_in(&store), Ok(0));
        assert!(first.whole(&a, &stop).is_some() && first.whole(&b, &stop).is_some());
        first.keep(true).expect("kept");
        // The next whole look asks only about `a`, while another copy of the app holds the store.
        assert!(first.whole(&a, &stop).is_some());
        let other = Connection::open(&store).expect("open");
        other.execute_batch("BEGIN EXCLUSIVE").expect("lock");
        assert!(first.keep(true).is_err(), "busy");
        assert!(!first.let_go(), "the store still has `b`");
        other.execute_batch("COMMIT").expect("unlock");
        drop(other);
        // The next keep takes `b` out, though it has nothing new.
        first.keep(false).expect("kept");
        let rows: i64 = Connection::open(&store)
            .and_then(|c| c.query_row("SELECT COUNT(*) FROM prints", [], |r| r.get(0)))
            .expect("count");
        assert_eq!(rows, 1);
        assert!(first.let_go());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn prints_kept_part_way_through_a_look_are_kept_and_not_forgotten_at_its_end() {
        let dir = folder("part-way");
        let store = dir.join(FILE_NAME);
        let stop = AtomicBool::new(false);
        let (a, b) = (dir.join("a.multisample"), dir.join("b.multisample"));
        fs::write(&a, b"ay").expect("write");
        fs::write(&b, b"bee").expect("write");
        let mut look = Prints::default();
        assert_eq!(look.remember_in(&store), Ok(0));
        assert!(look.whole(&a, &stop).is_some());
        // Kept part way through: already in the store, so an app closed now doesn't read `a` again.
        look.keep(false).expect("kept");
        let mut meanwhile = Prints::default();
        assert_eq!(meanwhile.remember_in(&store), Ok(1));
        drop(meanwhile);
        assert!(look.whole(&b, &stop).is_some());
        // The whole look's end still counts `a` as asked about.
        look.keep(true).expect("kept");
        assert_eq!(look.known(), 2);
        drop(look);
        let mut next = Prints::default();
        assert_eq!(next.remember_in(&store), Ok(2));
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
    fn a_print_says_whether_the_whole_file_was_read_since_the_last_keep() {
        let dir = folder("read-now");
        let stop = AtomicBool::new(false);
        let a = dir.join("a.flp");
        fs::write(&a, b"one").expect("write");
        let prints = Prints::default();
        let (print, read) = prints.whole_and_if_read(&a, &stop).expect("read");
        assert!(read, "read now");
        assert_eq!(prints.whole_and_if_read(&a, &stop), Some((print, true)), "still the read of this look");
        prints.keep(false).expect("kept");
        assert_eq!(prints.whole_and_if_read(&a, &stop), Some((print, false)), "known from before by its size and time");
        // A big file whose ends are read now, with a whole print kept from before: that print is still the old one.
        let big = dir.join("big.flp");
        fs::write(&big, vec![7u8; 3 * END_BYTES as usize]).expect("write");
        let whole = prints.whole(&big, &stop).expect("read");
        prints.keep(false).expect("kept");
        assert!(prints.ends(&big, &stop).is_some());
        assert_eq!(prints.whole_and_if_read(&big, &stop), Some((whole, false)), "only its ends were read");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_whole_print_read_afresh_takes_the_kept_ones_place() {
        let dir = folder("afresh");
        let stop = AtomicBool::new(false);
        let a = dir.join("a.flp");
        // In its first 64 KB and in its middle, so both prints change.
        let body = |fill: u8| {
            let mut b = vec![1u8; 3 * END_BYTES as usize];
            b[0] = fill;
            b[END_BYTES as usize + 10] = fill;
            b
        };
        fs::write(&a, body(5)).expect("write");
        let prints = Prints::default();
        let first = prints.whole(&a, &stop).expect("read");
        let ends = prints.ends(&a, &stop).expect("read");
        prints.keep(false).expect("kept");
        // Written again keeping its size and time: the kept print stands until it's read afresh.
        let when = fs::metadata(&a).and_then(|m| m.modified()).expect("time");
        fs::write(&a, body(6)).expect("write");
        fs::File::options().write(true).open(&a).and_then(|f| f.set_modified(when)).expect("time");
        assert_eq!(prints.whole(&a, &stop), Some(first));
        let again = prints.whole_afresh(&a, &stop).expect("read");
        assert_ne!(again, first);
        assert_eq!(prints.whole_and_if_read(&a, &stop), Some((again, true)), "kept in its place, read now");
        // The ends kept with the old print went with it: they're read again, not the ones from before.
        assert_ne!(prints.ends(&a, &stop), Some(ends));
        assert_eq!(prints.ends(&a, &stop), ends_hash(&a, &stop));
        let read = prints.state.lock().expect("state").known.get(&a).is_some_and(|slot| slot.marks & READ != 0);
        assert!(read, "the new print goes to the store");
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
