//! A small copy of the saves list, kept beside the library in a file of the app's own, so the list doesn't have to be
//! read out of the library on every start.
//!
//! Why: the C# app keeps each save's track details (its `info` column, tens of KB of JSON) in the middle of the
//! `projects` row, and SQLite can only reach the columns after it (status, rating, key, notes...) by reading through
//! it. So listing the saves read nearly the whole library (about 640 MB of a 720 MB test copy) though the list never
//! shows those details: 47 s on Solomon's nearly full C: drive (his log, 2026-11-04 04:29). The copy holds only what
//! the list shows, as the library gave it (about a hundredth of the size), and is read instead while the library is as
//! it was when the copy was made: same size and last-written time for `library.db` and for SQLite's `-wal` beside it.
//! Any write to the library (the C# app's scan, an edit) changes one of them, and the next read makes a new copy.
//!
//! The copy is made only for the library [`keep`] names (the app's own), never for a test's or a copy's. Making it
//! never changes the library: it's read as before, with the same query, and its rows go into the new copy as it's
//! read. A copy that can't be made or read is no worse than none: the list is read from the library.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Instant, UNIX_EPOCH};

use rusqlite::types::ToSqlOutput;
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row, Statement};

use crate::{LibraryError, Save};

/// The copy's file name, in the app's data folder beside `library.db`.
pub const FILE_NAME: &str = "library-list-rust.db";

/// What the copy's layout is. A copy made by another layout is made again.
const FORMAT: &str = "1";

/// The library the copy is kept for and where the copy goes, once the app has said ([`keep`]).
static KEPT: OnceLock<(PathBuf, PathBuf)> = OnceLock::new();

/// One list read or copy made at a time: pages that ask for the list together at start wait for the first one, then
/// read its copy, rather than each reading the whole library at once (which only slows them all on one disk). It
/// holds when the last copy made here began to be read, and the library's stamp then.
static ONE_AT_A_TIME: Mutex<Option<(Instant, String)>> = Mutex::new(None);

/// Keeps a copy of `library`'s saves list at `copy`, from now on. Only the first call counts.
pub fn keep(library: &Path, copy: &Path) {
    let _ = KEPT.set((library.to_owned(), copy.to_owned()));
}

/// Where the copy of `library`'s list goes, when [`keep`] named it.
fn copy_for(library: &Path) -> Option<&'static Path> {
    KEPT.get().filter(|(lib, _)| lib == library).map(|(_, copy)| copy.as_path())
}

/// The saves list: from the copy when the library hasn't changed since it was made, otherwise read by `read` (the
/// library's own list query, which hands each row it reads to the function it's given) while a new copy is made of
/// those rows. `query` is that query, so a copy made by another query (a library with other columns) isn't used, and
/// `from_copy` reads the saves out of the copy's rows as `read` does out of the library's.
pub(crate) fn saves(
    library: &Path,
    query: &str,
    read: impl FnOnce(&mut dyn FnMut(&Row<'_>)) -> Result<Vec<Save>, LibraryError>,
    from_copy: impl FnOnce(&mut Statement<'_>) -> rusqlite::Result<Vec<Save>>,
) -> Result<Vec<Save>, LibraryError> {
    let Some(copy) = copy_for(library) else { return read(&mut |_| {}) };
    let asked = Instant::now();
    let mut last = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    // A copy made by a read that began after this was asked for is as new as a read now would be, even though the
    // library has changed again since (the C# app scanning): pages asking together share one read.
    let fresh = last.as_ref().filter(|(began, _)| *began >= asked).map(|(_, stamp)| stamp.clone());
    // Before the read: a write during it makes the files differ from this, so the copy made is read again next time.
    let Some(stamp) = fresh.or_else(|| stamp_of(library)) else { return read(&mut |_| {}) };
    if let Ok(Some(saves)) = read_copy(copy, &stamp, query, from_copy) {
        return Ok(saves);
    }
    let began = Instant::now();
    let Some(stamp) = stamp_of(library) else { return read(&mut |_| {}) };
    let mut making = NewCopy::start(copy).ok();
    let saves = read(&mut |row| {
        // A row that can't be added (the disk is full) ends the copy, not the read.
        if making.as_mut().is_some_and(|m| m.add(row).is_err()) {
            making = None;
        }
    })?;
    if let Some(m) = making
        && m.finish(copy, &stamp, query).is_ok()
    {
        *last = Some((began, stamp));
    }
    Ok(saves)
}

/// The library's files as they are now: `library.db`'s and its `-wal`'s size and last-written time (to the
/// nanosecond, as the disk keeps it), and the `-wal`'s header, whose numbers SQLite changes each time it starts the
/// file again from the top (where a write would leave its size as it was). None when the library itself can't be
/// looked at.
fn stamp_of(library: &Path) -> Option<String> {
    let of = |p: &Path| -> Option<String> {
        let m = std::fs::metadata(p).ok()?;
        let at = m.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
        Some(format!("{}:{}", m.len(), at.as_nanos()))
    };
    let main = of(library)?;
    let wal_path = with_ending(library, "-wal");
    let wal = of(&wal_path).unwrap_or_else(|| "none".to_owned());
    let mut header = [0u8; 32];
    let read = std::fs::File::open(&wal_path).and_then(|mut f| std::io::Read::read(&mut f, &mut header)).unwrap_or(0);
    let header: String = header[..read].iter().map(|b| format!("{b:02x}")).collect();
    Some(format!("{main};{wal};{header}"))
}

/// `path` with `ending` added to its whole name.
fn with_ending(path: &Path, ending: &str) -> PathBuf {
    let mut p = path.as_os_str().to_owned();
    p.push(ending);
    PathBuf::from(p)
}

/// The copy's saves, when there's a copy made with this layout and `query` while the library was as `stamp` says.
fn read_copy(
    copy: &Path,
    stamp: &str,
    query: &str,
    from_copy: impl FnOnce(&mut Statement<'_>) -> rusqlite::Result<Vec<Save>>,
) -> rusqlite::Result<Option<Vec<Save>>> {
    if !copy.is_file() {
        return Ok(None);
    }
    let conn = Connection::open_with_flags(copy, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    let meta = |key: &str| -> rusqlite::Result<Option<String>> {
        conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0)).optional()
    };
    if meta("format")?.as_deref() != Some(FORMAT)
        || meta("stamp")?.as_deref() != Some(stamp)
        || meta("query")?.as_deref() != Some(query)
    {
        return Ok(None);
    }
    let mut st = conn.prepare("SELECT * FROM list ORDER BY rowid")?;
    from_copy(&mut st).map(Some)
}

/// A copy being made, in a file of its own beside where it goes (`<copy>.new-<process>`), put in place only once it's
/// whole. One that's dropped before then is removed.
struct NewCopy {
    conn: Option<Connection>,
    temp: PathBuf,
    /// How many columns the list has and the statement that adds a row, once the first row has made the table.
    columns: Option<(usize, String)>,
}

impl NewCopy {
    fn start(copy: &Path) -> rusqlite::Result<NewCopy> {
        let temp = with_ending(copy, &format!(".new-{}", std::process::id()));
        remove_with_journal(&temp);
        let conn = Connection::open(&temp)?;
        let made = NewCopy { conn: Some(conn), temp, columns: None };
        // Only a whole copy is ever put in place, so it needs no journal of its own.
        made.conn()?.execute_batch(
            "PRAGMA journal_mode = OFF; PRAGMA synchronous = OFF; BEGIN; \
             CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);",
        )?;
        Ok(made)
    }

    fn conn(&self) -> rusqlite::Result<&Connection> {
        self.conn.as_ref().ok_or(rusqlite::Error::InvalidQuery)
    }

    /// Adds one row of the library's list, as the library gave it. The first row makes the table, with its columns.
    fn add(&mut self, row: &Row<'_>) -> rusqlite::Result<()> {
        if self.columns.is_none() {
            let quoted: Vec<String> =
                row.as_ref().column_names().map(|n| format!("\"{}\"", n.replace('"', "\"\""))).collect();
            self.conn()?.execute_batch(&format!("CREATE TABLE list ({});", quoted.join(", ")))?;
            let insert = format!("INSERT INTO list VALUES ({})", vec!["?"; quoted.len()].join(", "));
            self.columns = Some((quoted.len(), insert));
        }
        let (Some((count, insert)), Some(conn)) = (&self.columns, &self.conn) else {
            return Err(rusqlite::Error::InvalidQuery);
        };
        let count = *count;
        let mut st = conn.prepare_cached(insert)?;
        let values: Vec<ToSqlOutput<'_>> =
            (0..count).map(|i| row.get_ref(i).map(ToSqlOutput::Borrowed)).collect::<Result<_, _>>()?;
        st.execute(rusqlite::params_from_iter(values))?;
        Ok(())
    }

    /// Writes down what the copy is of and puts it in place of the old one. An empty list makes no copy: there's
    /// nothing to save reading.
    fn finish(mut self, copy: &Path, stamp: &str, query: &str) -> Result<(), String> {
        if self.columns.is_none() {
            return Err("an empty list".to_owned());
        }
        let conn = self.conn.take().ok_or("no copy")?;
        let written = (|| {
            let mut st = conn.prepare("INSERT INTO meta VALUES (?1, ?2)")?;
            for (key, value) in [("format", FORMAT), ("stamp", stamp), ("query", query)] {
                st.execute([key, value])?;
            }
            drop(st);
            conn.execute_batch("COMMIT;")
        })();
        // Closed before it's moved: Windows can't move a file that's open.
        conn.close().map_err(|(_, e)| e.to_string())?;
        written.map_err(|e| e.to_string())?;
        std::fs::rename(&self.temp, copy).map_err(|e| e.to_string())
    }
}

impl Drop for NewCopy {
    fn drop(&mut self) {
        // Closed first: Windows can't remove a file that's open.
        drop(self.conn.take());
        remove_with_journal(&self.temp);
    }
}

/// Removes a half-made copy, and a journal SQLite may have left beside it.
fn remove_with_journal(temp: &Path) {
    let _ = std::fs::remove_file(temp);
    let _ = std::fs::remove_file(with_ending(temp, "-journal"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Library;

    /// A library with the C# app's own layout (the track details in the middle of the row), in a folder of its own.
    fn make(dir: &Path) -> PathBuf {
        let _ = std::fs::remove_dir_all(dir);
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join("library.db");
        let made = Connection::open(&path).unwrap();
        made.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL COLLATE NOCASE,
                 name TEXT NOT NULL, daw TEXT NOT NULL, folder TEXT, size INTEGER, mtime INTEGER, tempo REAL,
                 info TEXT, error TEXT, file_missing INTEGER DEFAULT 0, hardware TEXT DEFAULT '',
                 status TEXT DEFAULT 'Idea', rating INTEGER DEFAULT 0, notes TEXT DEFAULT '');
             CREATE TABLE plugins (project_id INTEGER, name TEXT, name_key TEXT, vendor TEXT, kind INTEGER,
                 format TEXT, path TEXT, installed INTEGER);
             CREATE TABLE samples (project_id INTEGER, path TEXT, found INTEGER);
             CREATE TABLE tags (project_id INTEGER, tag TEXT, UNIQUE(project_id, tag));
             INSERT INTO projects (path, name, daw, folder, size, mtime, tempo, info, hardware, status, rating, notes)
                 VALUES ('K:/a.flp', 'a', 'FL Studio', 'K:/', 10, 100, 120.5, '{}', 'Digitakt, TR-8', 'Mixing', 4,
                         'more bass'),
                        ('K:/b.flp', 'b', 'FL Studio', 'K:/', 20, 200, NULL, '{}', '', '', 0, '');
             INSERT INTO plugins VALUES (1, 'Serum', 'serum', 'Xfer', 0, 'VST3', '', 1),
                                        (1, 'Pro-Q 3', 'proq3', 'FabFilter', 1, 'VST3', '', 0);
             INSERT INTO samples VALUES (2, 'D:/kick.wav', 0);
             INSERT INTO tags VALUES (1, 'dark'), (1, 'acid');",
        )
        .unwrap();
        path
    }

    fn listed(saves: &[Save]) -> Vec<String> {
        saves.iter().map(|s| format!("{s:?}")).collect()
    }

    #[test]
    fn the_copy_lists_what_the_library_does_until_the_library_changes() {
        let dir = std::env::temp_dir().join(format!("mo-library-list-copy-{}", std::process::id()));
        let path = make(&dir);
        let copy = dir.join(FILE_NAME);
        keep(&path, &copy);

        // The first read is the library's own, and makes the copy.
        let lib = Library::open_read_only(&path).unwrap();
        let from_library = lib.summaries("", []).unwrap();
        assert_eq!(from_library.len(), 2);
        assert_eq!(listed(&lib.saves().unwrap()), listed(&from_library));
        assert!(copy.is_file());
        // Nothing half-made is left beside it.
        let left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".new-"))
            .collect();
        assert!(left.is_empty(), "{left:?}");

        // The next read is the copy's, and gives the very same saves.
        assert_eq!(listed(&lib.saves().unwrap()), listed(&from_library));
        let marked = Connection::open(&copy).unwrap();
        marked.execute("UPDATE list SET name = 'from the copy' WHERE id = 1", []).unwrap();
        drop(marked);
        assert!(lib.saves().unwrap().iter().any(|s| s.name == "from the copy"));

        // A change to the library (here a rating, after the track details) is read from the library again.
        let writer = Connection::open(&path).unwrap();
        writer.execute("UPDATE projects SET rating = 2 WHERE id = 1", []).unwrap();
        let after = lib.saves().unwrap();
        assert!(after.iter().all(|s| s.name != "from the copy"));
        assert_eq!(after.iter().find(|s| s.id == 1).map(|s| s.rating), Some(2));
        // And once more as the library has it, now from the new copy.
        assert_eq!(listed(&lib.saves().unwrap()), listed(&after));
        drop(writer);

        // Another library (a test's, a copy's) never gets one.
        let other_dir = dir.join("other");
        let other = make(&other_dir);
        Library::open_read_only(&other).unwrap().saves().unwrap();
        assert!(!other_dir.join(FILE_NAME).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
