//! The Rust app's own MIDI index, `midi-index.db`, beside the library: the only MIDI index the app opens. The C#
//! app's `midi.db` is opened once, read-only, to take in the corrections you made there ([`bring_up_to_date`]), and
//! never again after that.
//!
//! Each fact is stored once (rule 23):
//! - `files`: which file the index read, when it was last changed, its size, and what read it, so it's only read again
//!   when one of those changes. Nothing else about a project: its name, folder, DAW and tempo are in the song library,
//!   joined by path (letter case aside), never by the library's save id, which can change.
//! - `notes`: each set of notes once, with the facts that come from the notes alone (how many, bars, lowest and
//!   highest, pitch-class shares, shape). A pattern kept through 36 saves of a song is stored once, where the C#
//!   `midi.db` stores it 36 times. Notes keep MIDI 2.0's 16-bit velocity and their attribute (rule 22), so two files
//!   with the same notes played harder are two rows (`sound`), which share the C# app's note `hash` (timing and keys
//!   only), by which the page groups copies and your corrections hold.
//! - `note_data`: the notes themselves ([`NOTE_BYTES`] each), in a table of their own, so the rows the list reads pack
//!   tightly and the list never goes past the notes: only the piano roll and a key read again open them.
//! - `parts`: where each set of notes was found and what its names say: one row per part per file.
//! - `keys`: `mo-harmony`'s key reading of each set of notes (8 bytes, [`KeyFact`]), with what read it
//!   ([`keys::VERSION`]) and how (the kind the notes were heard as, and whether they're a piece cut from a longer
//!   stretch), so it's read again from the stored notes, without opening any file, when the finder gets better or the
//!   kind changes (rule 7). It's per set of notes, velocities and all: a stray soft touch isn't heard. A table of its
//!   own, so the list reads it without going past the notes, and finding what to read again is one index lookup.
//! - `shapes`: each set of notes' shape in any key (8 bytes, [`crate::analyser::shape_of`]), so a pack's loop in
//!   every key is one line on the page. A table of its own, added after layout 2 came out: an index made before it
//!   gets the table when the indexer opens it, and its shapes from the stored notes after the next read.
//! - `your_roles`, `your_octaves`: what you said about a set of notes, by its note hash, so it holds for every copy: the
//!   ones you made in the C# app, taken in once, and the Rust app's own, which win (a row with no value says "back to
//!   the app's guess", which wins too).
//! - `about`: what the index says of itself: when the C# app's corrections were taken in.
//!
//! The layout has a number (`PRAGMA user_version`, [`LAYOUT`]). An index of an older layout is copied into a new file
//! of this one, checked row by row against it, and then given its name ([`MidiStore::open`]): never changed in place.
//!
//! Writes happen on one connection, in batches, from the indexer's one saving thread; the page reads on its own
//! connections, so neither waits (rule 12).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mo_formats::dates::RealDates;
use mo_formats::{NoteAttribute, NoteEvent, case};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use crate::analyser;
use crate::extract::Found;
use crate::index::{MidiError, MidiIndex, Source, SourceKind, StoredPhrase};
use crate::keys;
use crate::member::PhraseMember;
use crate::roles::Role;
use mo_harmony::KeyFact;

/// The index's file name, in the same folder as the library.
pub const FILE_NAME: &str = "midi-index.db";

/// Marks a file as `midi-index.db` ("MOMI"), so the C# `midi.db` or another app's database is never taken for it.
const APPLICATION_ID: i64 = 0x4D4F_4D49;

/// The tables, as this app's layout ([`LAYOUT`]) has them.
const TABLES: &str = "
    CREATE TABLE IF NOT EXISTS files (
        id INTEGER PRIMARY KEY,
        path TEXT NOT NULL UNIQUE COLLATE NOCASE,
        kind INTEGER NOT NULL,          -- 0 a project, 1 a .mid file
        mtime INTEGER NOT NULL,         -- Unix seconds; -1 when it couldn't be opened (read again next time)
        size INTEGER NOT NULL,
        version INTEGER NOT NULL,       -- what read it (extract::FILE_VERSION, extract::project_version)
        error TEXT                      -- why it couldn't be read, or NULL
    );
    CREATE TABLE IF NOT EXISTS notes (
        id INTEGER PRIMARY KEY,
        sound TEXT NOT NULL UNIQUE,     -- the hash of its note_data: these exact notes, velocities and all
        hash TEXT NOT NULL,             -- the C# app's note hash: timing and keys only
        count INTEGER NOT NULL,
        bars INTEGER NOT NULL,
        lowest INTEGER NOT NULL,
        highest INTEGER NOT NULL,
        chroma BLOB NOT NULL,           -- 12 bytes, the strongest 255
        features BLOB NOT NULL          -- 48 bytes, 0 to 1 as 0 to 255; the scale counts from the key
    );
    CREATE TABLE IF NOT EXISTS note_data (
        notes INTEGER PRIMARY KEY REFERENCES notes(id) ON DELETE CASCADE,
        data BLOB NOT NULL              -- NOTE_BYTES a note (encode_notes)
    );
    CREATE TABLE IF NOT EXISTS parts (
        id INTEGER PRIMARY KEY,
        file INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
        notes INTEGER NOT NULL REFERENCES notes(id),
        part TEXT NOT NULL,
        clip TEXT NOT NULL,
        role INTEGER NOT NULL,          -- Role::code
        why TEXT NOT NULL,
        drums INTEGER NOT NULL,
        program INTEGER,
        track_name TEXT NOT NULL,
        members TEXT                    -- PhraseMember::encode, for a phrase joined from clips
    );
    CREATE TABLE IF NOT EXISTS keys (
        notes INTEGER PRIMARY KEY REFERENCES notes(id) ON DELETE CASCADE,
        version INTEGER NOT NULL,       -- keys::VERSION that read it; 0 not read yet
        role INTEGER NOT NULL,          -- the kind the notes were heard as (Role::code)
        cut INTEGER NOT NULL,           -- 1 for a piece cut from a longer stretch, heard as a window
        fact BLOB                       -- mo-harmony's KeyFact (8 bytes); NULL for drums or no pitched notes
    );
    -- The notes' shape in any key (analyser::shape_of): a loop and the same loop moved up or down share it. Filled
    -- when notes are saved, and for notes saved before this table was there, after a read (indexer::fill_shapes).
    CREATE TABLE IF NOT EXISTS shapes (
        notes INTEGER PRIMARY KEY REFERENCES notes(id) ON DELETE CASCADE,
        shape INTEGER NOT NULL
    );
    -- NULL: back to the app's guess (which wins over the C# app's correction too).
    CREATE TABLE IF NOT EXISTS your_roles (hash TEXT PRIMARY KEY, role INTEGER) WITHOUT ROWID;
    CREATE TABLE IF NOT EXISTS your_octaves (hash TEXT PRIMARY KEY, octaves INTEGER) WITHOUT ROWID;
    CREATE TABLE IF NOT EXISTS about (name TEXT PRIMARY KEY, value) WITHOUT ROWID;
";

/// The tables' indexes: made after the rows when the index is copied into a new layout, so each is built once.
const INDEXES: &str = "
    CREATE INDEX IF NOT EXISTS ix_notes_hash ON notes(hash);
    CREATE INDEX IF NOT EXISTS ix_parts_file ON parts(file);
    CREATE INDEX IF NOT EXISTS ix_parts_notes ON parts(notes);
    CREATE INDEX IF NOT EXISTS ix_keys_version ON keys(version);
";

/// The tables' layout this app makes (`PRAGMA user_version`). 2: the notes in `note_data`, a table of their own (1 kept
/// them in the last column of `notes`, and 0 had no `keys`).
pub const LAYOUT: i64 = 2;

/// `about`'s row saying when the C# app's corrections were taken in (Unix seconds).
const TOOK_IN_OLD: &str = "took_in_old_corrections";

/// How long a write waits for another to finish.
const WAIT: Duration = Duration::from_secs(5);

/// Room always left free on the index's disk when it's copied into a new layout.
const SPARE_ROOM: u64 = 256 << 20;

/// One copy into a new layout at a time in this app (the page and Ctrl+K can each read the index at once), and no
/// reader here opens the index while one is being made: a reader opened after the check that nothing has it open, and
/// before the copy takes its name, would be left on the old file on a Mac or Linux.
static BRINGING: Mutex<()> = Mutex::new(());

/// How copying an index of an older layout into this one went ([`copy_to_layout`]).
#[derive(Debug, PartialEq, Eq)]
enum Copied {
    /// Copied, checked and given its name: how many sets of notes and parts it holds.
    Done { notes: i64, parts: i64 },
    /// It was already of this layout (another copy of the app copied it meanwhile).
    Already,
    /// Not this time, and why: the index stays as it was, and it's tried again the next time it's opened.
    NotNow(String),
}

/// Copies the index at `path`, of an older layout, into a new file of this one beside it (`midi-index.db.new-<process>`),
/// checks every row of the copy against the index, and then gives the copy the index's name. The index itself is only
/// read: if anything goes wrong on the way, the copy is thrown away and the index stays as it was. Only done while
/// nothing else has the index open (another copy of the app, or a reader in this one): what that wrote meanwhile would
/// be lost, and the log SQLite keeps beside the index would be read into the new file as if it were its own.
fn copy_to_layout(path: &Path) -> Result<Copied, String> {
    let part = with_ending(path, &format!(".new-{}", std::process::id()));
    remove_with_sides(&part)?;
    let _part = Part(part.clone());
    let Some(old) = path.to_str() else { return Err("its folder's name can't be handed to SQLite".into()) };
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    if let Ok(free) = fs4::available_space(dir) {
        // The copy is no bigger than the index and its log: the notes are the same, only packed apart.
        let takes = ["", "-wal"]
            .iter()
            .filter_map(|end| fs::metadata(with_ending(path, end)).ok())
            .map(|m| m.len())
            .sum::<u64>()
            .saturating_add(SPARE_ROOM);
        if free < takes {
            return Ok(Copied::NotNow(format!(
                "only {} MB is free on its disk, and copying it takes up to {} MB",
                free >> 20,
                takes >> 20
            )));
        }
    }
    let Some((notes, parts)) = fill_copy(&part, old)? else { return Ok(Copied::Already) };
    // The last connection to an index to close takes SQLite's files beside it away (once it has put what its log held
    // into the index), and the copy's was the last here: still there, something else has the index open.
    if ["-wal", "-shm"].iter().any(|end| with_ending(path, end).exists()) {
        return Ok(Copied::NotNow("another copy of the app has it open".into()));
    }
    // Windows doesn't let a file another program has open be replaced, so one opened since is never lost either.
    if let Err(e) = fs::rename(&part, path) {
        return Ok(Copied::NotNow(format!("the copy couldn't be given its name ({e})")));
    }
    Ok(Copied::Done { notes, parts })
}

/// The copy being made: removed, with SQLite's files beside it, on every way out (there's nothing left to remove once
/// it has the index's name).
struct Part(PathBuf);

impl Drop for Part {
    fn drop(&mut self) {
        let _ = remove_with_sides(&self.0);
    }
}

/// Fills the new file at `part` from the index at `old` in one read of it, checks it, and puts it on the disk: how many
/// sets of notes and parts it holds, or `None` when the index is of this layout already.
fn fill_copy(part: &Path, old: &str) -> Result<Option<(i64, i64)>, String> {
    let sql = |e: rusqlite::Error| e.to_string();
    let conn = Connection::open(part).map_err(sql)?;
    conn.busy_timeout(WAIT).map_err(sql)?;
    // A file being made needs no journal: if this stops half way, it's thrown away. The rows are the index's own, so
    // nothing is checked against another table as it goes in: the copy is checked whole once it's in.
    conn.pragma_update(Some("main"), "journal_mode", "OFF").map_err(sql)?;
    conn.pragma_update(Some("main"), "synchronous", "OFF").map_err(sql)?;
    conn.pragma_update(None, "foreign_keys", false).map_err(sql)?;
    conn.pragma_update(None, "application_id", APPLICATION_ID).map_err(sql)?;
    conn.execute_batch(TABLES).map_err(sql)?;
    // Opened for writing (nothing is written to it), so that when this connection closes, it's the one that tidies
    // SQLite's files beside the index away if it's the last.
    conn.execute("ATTACH DATABASE ?1 AS old", [old]).map_err(sql)?;
    // One read of the index from here to the end: what's copied and what it's checked against are the same.
    conn.execute_batch("BEGIN").map_err(sql)?;
    let id: i64 = conn.query_row("PRAGMA old.application_id", [], |r| r.get(0)).map_err(sql)?;
    let layout: i64 = conn.query_row("PRAGMA old.user_version", [], |r| r.get(0)).map_err(sql)?;
    if id != APPLICATION_ID {
        return Err("it isn't the Rust app's MIDI index".into());
    }
    if layout >= LAYOUT {
        return Ok(None);
    }
    let tables: Vec<String> = {
        let mut st = conn.prepare("SELECT name FROM old.sqlite_master WHERE type = 'table'").map_err(sql)?;
        st.query_map([], |r| r.get(0)).map_err(sql)?.collect::<rusqlite::Result<_>>().map_err(sql)?
    };
    let has = |name: &str| tables.iter().any(|t| t == name);
    if !["files", "notes", "parts"].iter().all(|t| has(t)) {
        return Err("it has no files, notes or parts table".into());
    }
    copy_rows(&conn, &has).map_err(sql)?;
    conn.execute_batch(INDEXES).map_err(sql)?;
    let counts = check_rows(&conn, &has).map_err(sql)??;
    conn.pragma_update(Some("main"), "user_version", LAYOUT).map_err(sql)?;
    conn.execute_batch("COMMIT").map_err(sql)?;
    let check: String = conn.query_row("PRAGMA main.integrity_check", [], |r| r.get(0)).map_err(sql)?;
    if check != "ok" {
        return Err(format!("the copy didn't read back whole ({check})"));
    }
    // From now on it's written while the page may be reading it: in WAL mode readers never wait for a writer.
    conn.pragma_update(Some("main"), "journal_mode", "WAL").map_err(sql)?;
    conn.close().map_err(|(_, e)| e.to_string())?;
    // On the disk before it has the index's name, so an index with that name is whole even after a power cut.
    fs::OpenOptions::new()
        .write(true)
        .open(part)
        .and_then(|f| f.sync_all())
        .map_err(|e| format!("the copy couldn't be put on the disk ({e})"))?;
    Ok(Some(counts))
}

/// Copies every row of the index (`old`) into the copy's tables, each keeping its id, in the order of its ids.
fn copy_rows(conn: &Connection, has: &dyn Fn(&str) -> bool) -> rusqlite::Result<()> {
    conn.execute_batch(
        "INSERT INTO main.files (id, path, kind, mtime, size, version, error)
             SELECT id, path, kind, mtime, size, version, error FROM old.files ORDER BY id;
         INSERT INTO main.notes (id, sound, hash, count, bars, lowest, highest, chroma, features)
             SELECT id, sound, hash, count, bars, lowest, highest, chroma, features FROM old.notes ORDER BY id;
         INSERT INTO main.note_data (notes, data) SELECT id, data FROM old.notes ORDER BY id;
         INSERT INTO main.parts (id, file, notes, part, clip, role, why, drums, program, track_name, members)
             SELECT id, file, notes, part, clip, role, why, drums, program, track_name, members
             FROM old.parts ORDER BY id;",
    )?;
    if has("keys") {
        conn.execute_batch(
            "INSERT INTO main.keys (notes, version, role, cut, fact)
                 SELECT notes, version, role, cut, fact FROM old.keys ORDER BY notes;",
        )?;
    }
    // Sets of notes saved before keys were read (layout 0): not read yet, so the next run reads them.
    conn.execute(
        "INSERT OR IGNORE INTO main.keys (notes, version, role, cut, fact) SELECT id, 0, ?1, 0, NULL FROM main.notes",
        [Role::Other.code()],
    )?;
    for (table, column) in [("your_roles", "role"), ("your_octaves", "octaves")] {
        if has(table) {
            conn.execute_batch(&format!(
                "INSERT INTO main.{table} (hash, {column}) SELECT hash, {column} FROM old.{table};"
            ))?;
        }
    }
    Ok(())
}

/// Whether the copy holds every row of the index (`old`) as it was, and nothing else: how many sets of notes and parts
/// it holds when it does, else what differs.
fn check_rows(conn: &Connection, has: &dyn Fn(&str) -> bool) -> rusqlite::Result<Result<(i64, i64), String>> {
    let count = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0));
    // Each table: the index's rows, the copy's, and how many of the index's are in the copy as they were. Paths are
    // compared letter for letter, not as the column's letter case rule would.
    let mut tables = vec![
        (
            "files",
            "SELECT COUNT(*) FROM old.files",
            "SELECT COUNT(*) FROM main.files",
            "SELECT COUNT(*) FROM old.files o JOIN main.files m ON m.id = o.id
             WHERE m.path = o.path COLLATE BINARY AND m.kind IS o.kind AND m.mtime IS o.mtime AND m.size IS o.size
                 AND m.version IS o.version AND m.error IS o.error",
        ),
        (
            "notes",
            "SELECT COUNT(*) FROM old.notes",
            "SELECT COUNT(*) FROM main.notes",
            "SELECT COUNT(*) FROM old.notes o JOIN main.notes m ON m.id = o.id JOIN main.note_data d ON d.notes = o.id
             WHERE m.sound IS o.sound AND m.hash IS o.hash AND m.count IS o.count AND m.bars IS o.bars
                 AND m.lowest IS o.lowest AND m.highest IS o.highest AND m.chroma IS o.chroma
                 AND m.features IS o.features AND d.data IS o.data",
        ),
        (
            "note_data",
            "SELECT COUNT(*) FROM old.notes",
            "SELECT COUNT(*) FROM main.note_data",
            "SELECT COUNT(*) FROM main.note_data d JOIN main.notes n ON n.id = d.notes",
        ),
        (
            "parts",
            "SELECT COUNT(*) FROM old.parts",
            "SELECT COUNT(*) FROM main.parts",
            "SELECT COUNT(*) FROM old.parts o JOIN main.parts m ON m.id = o.id
             WHERE m.file IS o.file AND m.notes IS o.notes AND m.part IS o.part AND m.clip IS o.clip
                 AND m.role IS o.role AND m.why IS o.why AND m.drums IS o.drums AND m.program IS o.program
                 AND m.track_name IS o.track_name AND m.members IS o.members",
        ),
        // Every set of notes has its key row: the index's own, or "not read yet".
        (
            "keys",
            "SELECT COUNT(*) FROM main.notes",
            "SELECT COUNT(*) FROM main.keys",
            "SELECT COUNT(*) FROM main.keys k JOIN main.notes n ON n.id = k.notes",
        ),
    ];
    if has("keys") {
        tables.push((
            "keys read",
            "SELECT COUNT(*) FROM old.keys",
            "SELECT COUNT(*) FROM old.keys",
            "SELECT COUNT(*) FROM old.keys o JOIN main.keys m ON m.notes = o.notes
             WHERE m.version IS o.version AND m.role IS o.role AND m.cut IS o.cut AND m.fact IS o.fact",
        ));
    }
    if has("your_roles") {
        tables.push((
            "your_roles",
            "SELECT COUNT(*) FROM old.your_roles",
            "SELECT COUNT(*) FROM main.your_roles",
            "SELECT COUNT(*) FROM old.your_roles o JOIN main.your_roles m ON m.hash = o.hash WHERE m.role IS o.role",
        ));
    }
    if has("your_octaves") {
        tables.push((
            "your_octaves",
            "SELECT COUNT(*) FROM old.your_octaves",
            "SELECT COUNT(*) FROM main.your_octaves",
            "SELECT COUNT(*) FROM old.your_octaves o JOIN main.your_octaves m ON m.hash = o.hash
             WHERE m.octaves IS o.octaves",
        ));
    }
    for (what, old, new, same) in tables {
        let (old, new, same) = (count(old)?, count(new)?, count(same)?);
        if old != new || same != old {
            return Ok(Err(format!(
                "the copy's {what} don't match ({old} in the index, {new} copied, {same} the same)"
            )));
        }
    }
    // Nothing points at a row that isn't there.
    let loose = {
        let mut st = conn.prepare("PRAGMA main.foreign_key_check")?;
        let mut rows = st.query([])?;
        let mut n = 0;
        while rows.next()?.is_some() {
            n += 1;
        }
        n
    };
    if loose > 0 {
        return Ok(Err(format!("{loose} rows of the copy point at rows that aren't there")));
    }
    Ok(Ok((count("SELECT COUNT(*) FROM main.notes")?, count("SELECT COUNT(*) FROM main.parts")?)))
}

/// `path` with `end` added to its name: `midi-index.db` and "-wal" give `midi-index.db-wal`.
fn with_ending(path: &Path, end: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(end);
    PathBuf::from(name)
}

/// Removes a database and the files SQLite keeps beside it. Fine when there are none.
fn remove_with_sides(path: &Path) -> Result<(), String> {
    for end in ["", "-wal", "-shm", "-journal"] {
        match fs::remove_file(with_ending(path, end)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{} couldn't be removed ({e})", with_ending(path, end).display())),
        }
    }
    Ok(())
}

/// What [`bring_up_to_date`] did, for the log.
#[derive(Debug, Default)]
pub struct BroughtUp {
    /// What was done just now, a line each (none when nothing was needed).
    pub done: Vec<String>,
    /// What couldn't be done, and why, a line each: it's tried again the next time.
    pub later: Vec<String>,
    /// The C# app's corrections, when they're not yet kept in the index (it couldn't be written to): the page counts
    /// them this time, as it read them.
    pub old_roles: HashMap<String, Role>,
    pub old_octaves: HashMap<String, i32>,
}

/// Before the page reads the index at `path`: an index of an older layout is copied into this one's, and the
/// corrections you made in the C# app are taken in from its `midi.db` beside it, once, so `midi.db` isn't opened again.
/// One quick look, read-only, once both are done (or there's no `midi.db`). Nothing to do while there's no index yet:
/// the first reading of your MIDI makes it, and the next look takes the corrections in. Reads and writes files, so
/// never on the window's thread.
pub fn bring_up_to_date(path: &Path) -> BroughtUp {
    let mut up = BroughtUp::default();
    if !path.exists() {
        return up;
    }
    let _one = BRINGING.lock().unwrap_or_else(PoisonError::into_inner);
    let midi_db = MidiIndex::path_beside(path);
    // The old app's corrections are taken in from its midi.db, while there's one beside the index (it may come back,
    // with the data folder put back from a backup): none there is nothing to do yet, not a "none to take in".
    let take_in = match look(path) {
        Ok((layout, took_in)) => {
            let take_in = !took_in && midi_db.exists();
            if layout == LAYOUT && !take_in {
                return up;
            }
            take_in
        }
        Err(e) => {
            up.later.push(format!("The MIDI index couldn't be read: {e}"));
            return up;
        }
    };
    let mut store = match open_for_writing(path) {
        Ok((store, copied)) => {
            if let Some((notes, parts)) = copied {
                up.done.push(format!(
                    "Copied the MIDI index into its new layout, the notes in a table of their own ({notes} sets of \
                     notes, {parts} parts, every row checked)"
                ));
            }
            store
        }
        Err(e) => {
            up.later.push(e.to_string());
            if take_in {
                match old_corrections(&midi_db) {
                    Ok((roles, octaves)) => (up.old_roles, up.old_octaves) = (roles, octaves),
                    Err(e) => up.later.push(format!(
                        "The old app's corrections couldn't be read from midi.db ({e}); they're tried again the next time"
                    )),
                }
            }
            return up;
        }
    };
    if take_in {
        match old_corrections(&midi_db) {
            Ok((roles, octaves)) => match store.take_in_old(&roles, &octaves) {
                Ok((r, o)) => up.done.push(took_in_text(r, o, roles.len() + octaves.len() - r - o)),
                Err(e) => {
                    up.later.push(format!(
                        "The old app's corrections couldn't be kept in the MIDI index ({e}); they count as read this \
                         time, and they're tried again the next time"
                    ));
                    (up.old_roles, up.old_octaves) = (roles, octaves);
                }
            },
            Err(e) => up.later.push(format!(
                "The old app's corrections couldn't be read from midi.db ({e}); they're tried again the next time"
            )),
        }
    }
    up
}

/// The log's line for the C# app's corrections taken in: `roles` kinds and `octaves` octave moves, `kept` left out
/// as this app had its own for those notes.
fn took_in_text(roles: usize, octaves: usize, kept: usize) -> String {
    if roles + octaves + kept == 0 {
        return "The old app's midi.db has no corrections to take in; it isn't opened again".to_owned();
    }
    let n = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let mut line = format!(
        "Took in {} and {} you gave parts in the old app, from midi.db, which isn't opened again",
        n(roles, "kind", "kinds"),
        n(octaves, "octave move", "octave moves")
    );
    if kept > 0 {
        line.push_str(&format!(
            " ({} given in this app since, which {})",
            n(kept, "was", "were"),
            if kept == 1 { "stays" } else { "stay" }
        ));
    }
    line
}

/// When (Unix seconds) the index at `path` took in the corrections you made in the C# app ([`bring_up_to_date`]):
/// from then on its `midi.db` isn't opened again, so it may be archived (`mo_app::old_databases`). `None` while it
/// hasn't, while the index is of an older layout (it's copied into this one first), or when that can't be told (no
/// index there, one that can't be read just now).
pub fn took_in_old_at(path: &Path) -> Option<i64> {
    if !path.is_file() {
        return None;
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(path, flags).ok()?;
    conn.busy_timeout(WAIT).ok()?;
    let layout: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0)).ok()?;
    if layout != LAYOUT {
        return None;
    }
    conn.query_row("SELECT value FROM about WHERE name = ?1", [TOOK_IN_OLD], |r| r.get(0)).ok()
}

/// The index's layout, and whether the C# app's corrections have been taken in: one look, read-only.
fn look(path: &Path) -> Result<(i64, bool), MidiError> {
    let open_error = |why: String| MidiError::Open { path: path.to_owned(), why };
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(path, flags).map_err(|e| open_error(e.to_string()))?;
    conn.busy_timeout(WAIT).map_err(|e| open_error(e.to_string()))?;
    let layout: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let has_about: bool =
        conn.query_row("SELECT COUNT(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = 'about'", [], |r| {
            r.get(0)
        })?;
    let took_in = has_about
        && conn.query_row("SELECT COUNT(*) > 0 FROM about WHERE name = ?1", [TOOK_IN_OLD], |r| r.get::<_, bool>(0))?;
    Ok((layout, took_in))
}

/// Kinds and octave moves you gave parts, by note hash.
type Corrections = (HashMap<String, Role>, HashMap<String, i32>);

/// What you said about parts in the C# app, from its `midi.db` (read-only).
fn old_corrections(midi_db: &Path) -> Result<Corrections, MidiError> {
    let old = MidiIndex::open_read_only(midi_db)?;
    Ok((old.your_roles()?, old.your_octaves()?))
}

/// A set of notes whose key is to be read again ([`MidiStore::stale_keys`]).
#[derive(Clone, Debug)]
pub struct StaleKey {
    pub notes_id: i64,
    /// The kind the newest part holding them gives them.
    pub role: Role,
    pub cut: bool,
    pub notes: Vec<NoteEvent>,
}

/// A key read again, to keep ([`MidiStore::save_keys`]).
#[derive(Clone, Debug)]
pub struct ReadKey {
    pub notes_id: i64,
    pub role: Role,
    pub fact: Option<KeyFact>,
    pub features: [f32; analyser::FEATURE_COUNT],
}

/// Bytes a note takes in `notes.data`: start and length (i32), key, velocity (u16), attribute kind and data (u16),
/// little-endian.
pub const NOTE_BYTES: usize = 14;

impl Role {
    /// The number `midi-index.db` keeps a role as.
    pub fn code(self) -> i64 {
        match self {
            Role::Chords => 0,
            Role::Melody => 1,
            Role::Bass => 2,
            Role::Arp => 3,
            Role::Drums => 4,
            Role::Other => 5,
        }
    }

    pub fn from_code(code: i64) -> Role {
        Role::ALL.into_iter().find(|r| r.code() == code).unwrap_or(Role::Other)
    }
}

/// What the indexer read from one file, ready to save.
#[derive(Clone, Debug)]
pub struct FileRead {
    pub path: String,
    pub kind: SourceKind,
    pub mtime: i64,
    pub size: i64,
    pub version: u32,
    pub outcome: Outcome,
}

/// How reading a file went.
#[derive(Clone, Debug)]
pub enum Outcome {
    /// Its parts (none for a file without notes).
    Parts(Vec<Found>),
    /// It couldn't be read; noted once and only tried again when asked.
    Failed(String),
    /// Another program had it open, or it couldn't be reached just now: what was read from it before is kept, and
    /// it's read again next time.
    InUse(String),
}

/// What the index knows of a file, to tell whether to read it again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub path: String,
    pub mtime: i64,
    pub size: i64,
    pub version: u32,
    pub failed: bool,
}

/// `midi-index.db`, open for writing (the indexer) or reading (the page).
pub struct MidiStore {
    conn: Connection,
    path: PathBuf,
    /// Whether the index has its `keys` table (always, once the indexer has opened it).
    has_keys: bool,
    /// Whether it has its `shapes` table (the same).
    has_shapes: bool,
    /// Its layout ([`LAYOUT`] once the indexer has opened it; the page reads an older one as it is).
    layout: i64,
}

/// Opens the index at `path` for writing, making it (and its tables) when it isn't there yet, and copying it into this
/// app's layout first when it's of an older one ([`copy_to_layout`]): the store, and how many sets of notes and parts
/// the copy holds when it was made just now. The caller holds [`BRINGING`].
fn open_for_writing(path: &Path) -> Result<(MidiStore, Option<(i64, i64)>), MidiError> {
    let open_error = |why: String| MidiError::Open { path: path.to_owned(), why };
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir).map_err(|e| open_error(e.to_string()))?;
    }
    let (mut copied, mut tried) = (None, false);
    loop {
        let conn = Connection::open(path).map_err(|e| open_error(e.to_string()))?;
        conn.busy_timeout(WAIT).map_err(|e| open_error(e.to_string()))?;
        // Never another database: the C# app's midi.db (phrases, sources) or anything not made here.
        let id: i64 =
            conn.pragma_query_value(None, "application_id", |r| r.get(0)).map_err(|e| open_error(e.to_string()))?;
        let tables: i64 = conn
            .query_row("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'", [], |r| r.get(0))
            .map_err(|e| open_error(e.to_string()))?;
        if id != APPLICATION_ID && (id != 0 || tables > 0) {
            return Err(open_error("it isn't the Rust app's MIDI index, so it's left as it is".into()));
        }
        let layout: i64 =
            conn.pragma_query_value(None, "user_version", |r| r.get(0)).map_err(|e| open_error(e.to_string()))?;
        if layout > LAYOUT {
            return Err(open_error("a newer version of the app made it, so this one leaves it as it is".into()));
        }
        if tables > 0 && layout < LAYOUT {
            // The copy reads it on a connection of its own, and needs this one closed.
            drop(conn);
            if std::mem::replace(&mut tried, true) {
                return Err(open_error("it's still of an older layout after being copied into this one".into()));
            }
            match copy_to_layout(path) {
                Ok(Copied::Done { notes, parts }) => copied = Some((notes, parts)),
                Ok(Copied::Already) => {}
                Ok(Copied::NotNow(why)) => {
                    return Err(open_error(format!(
                        "it's to be copied into this app's layout first, which can't be done just now ({why}); \
                         it's tried again the next time"
                    )));
                }
                Err(why) => {
                    return Err(open_error(format!(
                        "it couldn't be copied into this app's layout ({why}); it's tried again the next time"
                    )));
                }
            }
            continue;
        }
        conn.pragma_update(None, "application_id", APPLICATION_ID).map_err(|e| open_error(e.to_string()))?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")
            .and_then(|()| conn.execute_batch(TABLES))
            .and_then(|()| conn.execute_batch(INDEXES))
            // Files an older reading marked gone mid-reading (a drive away) are read again once: their stamp goes.
            .and_then(|()| {
                conn.execute("UPDATE files SET mtime = -1 WHERE error = ?1", [crate::extract::GONE_BEFORE]).map(|_| ())
            })
            // A new index: of this layout from the start.
            .and_then(|()| if layout == LAYOUT { Ok(()) } else { conn.pragma_update(None, "user_version", LAYOUT) })
            .map_err(|e| open_error(e.to_string()))?;
        let store = MidiStore { conn, path: path.to_owned(), has_keys: true, has_shapes: true, layout: LAYOUT };
        return Ok((store, copied));
    }
}

impl MidiStore {
    /// `midi-index.db` in the same folder as the library.
    pub fn path_beside(library: &Path) -> PathBuf {
        library.with_file_name(FILE_NAME)
    }

    /// Opens the index for writing, making it (and its tables) when it isn't there yet. One of an older layout is
    /// copied into this app's first; if that can't be done just now (another copy of the app has it open), this fails
    /// and the index stays as it was, for the page to read.
    pub fn open(path: &Path) -> Result<MidiStore, MidiError> {
        let _one = BRINGING.lock().unwrap_or_else(PoisonError::into_inner);
        open_for_writing(path).map(|(store, _)| store)
    }

    /// Opens the index for reading only, for the page: it must already exist. One of an older layout is read as it is.
    pub fn open_read_only(path: &Path) -> Result<MidiStore, MidiError> {
        let open_error = |why: String| MidiError::Open { path: path.to_owned(), why };
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        // Not while it's being copied into a new layout here: opened once the copy has its name (or was let be).
        let one = BRINGING.lock().unwrap_or_else(PoisonError::into_inner);
        let conn = Connection::open_with_flags(path, flags).map_err(|e| open_error(e.to_string()))?;
        drop(one);
        conn.busy_timeout(Duration::from_secs(2)).map_err(|e| open_error(e.to_string()))?;
        conn.pragma_update(None, "query_only", true).map_err(|e| open_error(e.to_string()))?;
        // Any temporary work in memory, not in a file on the system's disk, with room to read the list in (32 MB; no
        // memory-mapping, so a bad sector is a "damaged" message, not a crash). The list is sorted in Rust (all()).
        conn.pragma_update(None, "temp_store", "MEMORY").map_err(|e| open_error(e.to_string()))?;
        conn.pragma_update(None, "cache_size", -32_000).map_err(|e| open_error(e.to_string()))?;
        let id: i64 =
            conn.pragma_query_value(None, "application_id", |r| r.get(0)).map_err(|e| open_error(e.to_string()))?;
        if id != APPLICATION_ID {
            return Err(open_error("it isn't the Rust app's MIDI index".into()));
        }
        let layout: i64 =
            conn.pragma_query_value(None, "user_version", |r| r.get(0)).map_err(|e| open_error(e.to_string()))?;
        // An index made before keys were read (until the indexer next opens it): listed without keys.
        let has_table = |name: &str| {
            conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1", [name], |r| {
                r.get(0)
            })
            .map_err(|e| open_error(e.to_string()))
        };
        let has_keys = has_table("keys")?;
        // One made before shapes were kept (until the indexer next opens it): listed with no loop in other keys.
        let has_shapes = has_table("shapes")?;
        Ok(MidiStore { conn, path: path.to_owned(), has_keys, has_shapes, layout })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether it's of this app's layout. One of an older one is best not kept open between reads: it can't be
    /// copied into this one while it's open.
    pub fn is_current(&self) -> bool {
        self.layout == LAYOUT
    }

    /// A way to stop this index's current read from another thread (a newer read is wanted, rule 12).
    pub fn stopper(&self) -> crate::index::Stopper {
        crate::index::Stopper(self.conn.get_interrupt_handle())
    }

    /// Every file read so far.
    pub fn stamps(&self) -> Result<Vec<Stamp>, MidiError> {
        let mut st = self.conn.prepare("SELECT path, mtime, size, version, error IS NOT NULL FROM files")?;
        let rows = st.query_map([], |r| {
            Ok(Stamp {
                path: r.get(0)?,
                mtime: r.get(1)?,
                size: r.get(2)?,
                version: u32::try_from(r.get::<_, i64>(3)?).unwrap_or(0),
                failed: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Saves what was read from some files, each replacing what was there before, in one go.
    pub fn save(&mut self, batch: &[FileRead]) -> Result<(), MidiError> {
        if batch.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        {
            let mut upsert = tx.prepare_cached(
                "INSERT INTO files (path, kind, mtime, size, version, error) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(path) DO UPDATE SET path = ?1, kind = ?2, mtime = ?3, size = ?4, version = ?5, error = ?6
                 RETURNING id",
            )?;
            let mut note_error = tx.prepare_cached(
                "INSERT INTO files (path, kind, mtime, size, version, error) VALUES (?1, ?2, -1, -1, ?3, ?4)
                 ON CONFLICT(path) DO UPDATE SET error = ?4",
            )?;
            let mut clear = tx.prepare_cached("DELETE FROM parts WHERE file = ?1")?;
            // The same notes found again (another file, or read again): the row there is kept; a newer analysis's
            // facts replace the old.
            let mut note = tx.prepare_cached(
                "INSERT INTO notes (sound, hash, count, bars, lowest, highest, chroma, features)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(sound) DO UPDATE SET hash = ?2, count = ?3, bars = ?4, lowest = ?5, highest = ?6,
                     chroma = ?7, features = ?8
                 RETURNING id",
            )?;
            // The notes themselves once: `sound` is their hash, so the same notes found again are already there.
            let mut data =
                tx.prepare_cached("INSERT INTO note_data (notes, data) VALUES (?1, ?2) ON CONFLICT(notes) DO NOTHING")?;
            let mut shape =
                tx.prepare_cached("INSERT INTO shapes (notes, shape) VALUES (?1, ?2) ON CONFLICT(notes) DO NOTHING")?;
            let mut key = tx.prepare_cached(
                "INSERT INTO keys (notes, version, role, cut, fact) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(notes) DO UPDATE SET version = ?2, role = ?3, cut = ?4, fact = ?5",
            )?;
            let mut part = tx.prepare_cached(
                "INSERT INTO parts (file, notes, part, clip, role, why, drums, program, track_name, members)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )?;
            for f in batch {
                let kind = i64::from(f.kind == SourceKind::File);
                match &f.outcome {
                    // What was read before stays; its stamp stays too (none for a new file), so it's read again.
                    Outcome::InUse(why) => {
                        note_error.execute(params![f.path, kind, f.version, why])?;
                    }
                    Outcome::Failed(why) => {
                        let id: i64 =
                            upsert.query_row(params![f.path, kind, f.mtime, f.size, f.version, why], |r| r.get(0))?;
                        clear.execute([id])?;
                    }
                    Outcome::Parts(found) => {
                        let id: i64 = upsert
                            .query_row(params![f.path, kind, f.mtime, f.size, f.version, None::<String>], |r| {
                                r.get(0)
                            })?;
                        clear.execute([id])?;
                        for p in found {
                            let a = &p.analysis;
                            let encoded = encode_notes(&p.notes);
                            let notes: i64 = note.query_row(
                                params![
                                    sound_of(&encoded),
                                    a.hash,
                                    p.notes.len() as i64,
                                    a.bars,
                                    a.lowest,
                                    a.highest,
                                    analyser::bytes(&a.chroma, true),
                                    analyser::bytes(&a.features, false),
                                ],
                                |r| r.get(0),
                            )?;
                            data.execute(params![notes, encoded])?;
                            if let Some(s) = analyser::shape_of(&p.notes) {
                                shape.execute(params![notes, s])?;
                            }
                            key.execute(params![
                                notes,
                                keys::VERSION,
                                a.role.code(),
                                p.cut,
                                a.key_fact.map(|f| f.0.to_vec())
                            ])?;
                            let members = (!p.members.is_empty()).then(|| PhraseMember::encode(&p.members));
                            part.execute(params![
                                id,
                                notes,
                                p.part,
                                p.clip,
                                a.role.code(),
                                a.role_why,
                                p.drums,
                                p.program,
                                p.track_name,
                                members
                            ])?;
                        }
                    }
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Forgets files that are gone, or no longer in a MIDI folder or the library. Their parts go with them; their
    /// notes go at the next [`MidiStore::tidy`].
    pub fn forget<'a>(&mut self, paths: impl IntoIterator<Item = &'a str>) -> Result<usize, MidiError> {
        let tx = self.conn.transaction()?;
        let mut n = 0;
        {
            let mut st = tx.prepare_cached("DELETE FROM files WHERE path = ?1")?;
            for p in paths {
                n += st.execute([p])?;
            }
        }
        tx.commit()?;
        Ok(n)
    }

    /// How many sets of notes have a key to read again.
    pub fn stale_key_count(&self) -> Result<usize, MidiError> {
        let n: i64 =
            self.conn.query_row("SELECT COUNT(*) FROM keys WHERE version < ?1", [keys::VERSION], |r| r.get(0))?;
        Ok(n.max(0) as usize)
    }

    /// Sets of notes whose key is to be read again (an older key finder read it, or the kind it was heard as has
    /// changed), up to `limit`: each with the kind the newest part holding it gives it now, whether it's a cut piece,
    /// and its notes.
    pub fn stale_keys(&self, limit: usize) -> Result<Vec<StaleKey>, MidiError> {
        let mut st = self.conn.prepare_cached(
            "SELECT k.notes, k.cut, d.data,
                    (SELECT p.role FROM parts p WHERE p.notes = k.notes ORDER BY p.id DESC LIMIT 1)
             FROM keys k JOIN note_data d ON d.notes = k.notes WHERE k.version < ?1 LIMIT ?2",
        )?;
        let rows = st.query_map(params![keys::VERSION, limit as i64], |r| {
            Ok(StaleKey {
                notes_id: r.get(0)?,
                cut: r.get(1)?,
                notes: decode_notes(&r.get::<_, Vec<u8>>(2)?),
                role: r.get::<_, Option<i64>>(3)?.map_or(Role::Other, Role::from_code),
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Keeps keys read again ([`crate::keys::reread`]) with the "more like this" numbers that count from them.
    pub fn save_keys(&mut self, read: &[ReadKey]) -> Result<(), MidiError> {
        let tx = self.conn.transaction()?;
        {
            let mut key = tx.prepare_cached("UPDATE keys SET version = ?2, role = ?3, fact = ?4 WHERE notes = ?1")?;
            let mut features = tx.prepare_cached("UPDATE notes SET features = ?2 WHERE id = ?1")?;
            for k in read {
                key.execute(params![k.notes_id, keys::VERSION, k.role.code(), k.fact.map(|f| f.0.to_vec())])?;
                features.execute(params![k.notes_id, analyser::bytes(&k.features, false)])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// How many sets of notes have no shape yet (saved before the `shapes` table was there): two counts, each read
    /// from a small index, so it's quick when there are none.
    pub fn unshaped_count(&self) -> Result<usize, MidiError> {
        let n: i64 =
            self.conn
                .query_row("SELECT (SELECT COUNT(*) FROM notes) - (SELECT COUNT(*) FROM shapes)", [], |r| r.get(0))?;
        Ok(n.max(0) as usize)
    }

    /// Notes with no shape yet after the set of notes `after` (an id; 0 for the first), in id order, up to `limit`:
    /// each with its notes. The caller goes on from the last one given, so each row is looked at once.
    pub fn unshaped(&self, after: i64, limit: usize) -> Result<Vec<(i64, Vec<NoteEvent>)>, MidiError> {
        let mut st = self.conn.prepare_cached(
            "SELECT d.notes, d.data FROM note_data d
             WHERE d.notes > ?1 AND NOT EXISTS (SELECT 1 FROM shapes s WHERE s.notes = d.notes)
             ORDER BY d.notes LIMIT ?2",
        )?;
        let rows =
            st.query_map(params![after, limit as i64], |r| Ok((r.get(0)?, decode_notes(&r.get::<_, Vec<u8>>(1)?))))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Keeps the shapes worked out for [`MidiStore::unshaped`] notes: `None` (no notes) is kept as 0, so the same
    /// notes aren't asked for again.
    pub fn save_shapes(&mut self, shapes: &[(i64, Option<i64>)]) -> Result<(), MidiError> {
        let tx = self.conn.transaction()?;
        {
            let mut st = tx.prepare_cached(
                "INSERT INTO shapes (notes, shape) VALUES (?1, ?2) ON CONFLICT(notes) DO UPDATE SET shape = ?2",
            )?;
            for (notes, shape) in shapes {
                st.execute(params![notes, shape.unwrap_or(0)])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Drops notes no file has any more: once at the end of each run, not with every batch.
    pub fn tidy(&mut self) -> Result<usize, MidiError> {
        let tx = self.conn.transaction()?;
        let n =
            tx.execute("DELETE FROM notes WHERE NOT EXISTS (SELECT 1 FROM parts p WHERE p.notes = notes.id)", [])?;
        // A key heard as a kind no part gives those notes any more (the file that did was forgotten, or read again
        // as another kind) is read again.
        tx.execute(
            "UPDATE keys SET version = 0 WHERE version > 0
                 AND NOT EXISTS (SELECT 1 FROM parts p WHERE p.notes = keys.notes AND p.role = keys.role)",
            [],
        )?;
        tx.commit()?;
        Ok(n)
    }

    /// The files that couldn't be read, and why, by name.
    pub fn failures(&self) -> Result<Vec<crate::Failure>, MidiError> {
        let mut st = self.conn.prepare("SELECT path, kind, error FROM files WHERE error IS NOT NULL")?;
        let mut list = st
            .query_map([], |r| {
                Ok(crate::Failure {
                    path: r.get(0)?,
                    kind: if r.get::<_, i64>(1)? == 1 { SourceKind::File } else { SourceKind::Project },
                    why: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        list.sort_by_cached_key(|f| case::key(f.name()));
        Ok(list)
    }

    /// Every part, newest file first (a date that isn't real last), without its notes, as the page lists them.
    /// `cs_roles` and `cs_octaves` are the C# app's corrections when they're not yet taken in
    /// ([`BroughtUp::old_roles`]), which the index's own win over; empty once they are.
    pub fn all(
        &self,
        cs_roles: &HashMap<String, Role>,
        cs_octaves: &HashMap<String, i32>,
    ) -> Result<Vec<StoredPhrase>, MidiError> {
        // A key not read yet (version 0) has no word on whether it's cut.
        let (key_fact, key_cut, keys_join) = if self.has_keys {
            ("k.fact", "CASE WHEN k.version > 0 THEN k.cut END", "LEFT JOIN keys k ON k.notes = n.id")
        } else {
            ("NULL", "NULL", "")
        };
        let (shape, shapes_join) =
            if self.has_shapes { ("s.shape", "LEFT JOIN shapes s ON s.notes = n.id") } else { ("NULL", "") };
        let mut st = self.conn.prepare(&format!(
            "SELECT p.id, f.id, f.path, f.kind, f.mtime, f.size, p.part, p.clip, p.role, p.why, y.role, n.hash,
                    n.count, n.bars, n.lowest, n.highest, n.chroma, n.features, p.members, o.octaves, p.program,
                    p.drums, p.track_name, {key_fact}, y.hash IS NOT NULL, o.hash IS NOT NULL, {shape}, {key_cut}
             FROM parts p JOIN files f ON f.id = p.file JOIN notes n ON n.id = p.notes
                  LEFT JOIN your_roles y ON y.hash = n.hash LEFT JOIN your_octaves o ON o.hash = n.hash
                  {keys_join} {shapes_join}"
        ))?;
        let mut rows = st.query([])?;
        // One shared copy of each file's facts.
        let mut sources: HashMap<i64, Arc<Source>> = HashMap::new();
        let mut list = Vec::new();
        while let Some(r) = rows.next()? {
            let file: i64 = r.get(1)?;
            let source = match sources.get(&file) {
                Some(s) => Arc::clone(s),
                None => {
                    let path: String = r.get(2)?;
                    let kind = if r.get::<_, i64>(3)? == 1 { SourceKind::File } else { SourceKind::Project };
                    let s = Arc::new(Source {
                        daw: daw_of(&path, kind).to_owned(),
                        path,
                        kind,
                        mtime: r.get(4)?,
                        size: r.get(5)?,
                        // A song's tempo is in library.db.
                        tempo: None,
                    });
                    sources.insert(file, Arc::clone(&s));
                    s
                }
            };
            let hash: String = r.get(11)?;
            let mut chroma = [0u8; 12];
            for (to, from) in chroma.iter_mut().zip(r.get::<_, Vec<u8>>(16)?) {
                *to = from;
            }
            let key_fact = r.get::<_, Option<Vec<u8>>>(23)?.and_then(|b| <[u8; 8]>::try_from(b).ok()).map(KeyFact);
            // A Rust row (even one saying "back to the guess") wins; with none, the C# app's correction counts.
            let your_role = if r.get::<_, bool>(24)? {
                r.get::<_, Option<i64>>(10)?.map(Role::from_code)
            } else {
                cs_roles.get(&hash).copied()
            };
            let your_octave = if r.get::<_, bool>(25)? {
                r.get::<_, Option<i64>>(19)?.map(|o| o.clamp(-10, 10) as i32)
            } else {
                cs_octaves.get(&hash).copied()
            };
            list.push(StoredPhrase {
                id: r.get(0)?,
                source,
                part: r.get(6)?,
                clip: r.get(7)?,
                role: Role::from_code(r.get(8)?),
                role_why: r.get(9)?,
                your_role,
                bars: r.get::<_, i64>(13)?.clamp(0, i64::from(i32::MAX)) as i32,
                notes: r.get::<_, i64>(12)?.clamp(0, i64::from(i32::MAX)) as i32,
                lowest: r.get::<_, i64>(14)?.clamp(0, 255) as u8,
                highest: r.get::<_, i64>(15)?.clamp(0, 255) as u8,
                key: key_fact.as_ref().and_then(keys::sure_key),
                key_fact,
                chroma,
                // The chords come with mo-harmony's chord reader.
                chords: Vec::new(),
                features: r.get(17)?,
                members: r.get::<_, Option<String>>(18)?.unwrap_or_default(),
                program: r.get::<_, Option<i64>>(20)?.filter(|p| (0..128).contains(p)).map(|p| p as u8),
                drums: r.get(21)?,
                track_name: r.get(22)?,
                hash,
                your_octave,
                // 0: kept for notes with no shape.
                shape: r.get::<_, Option<i64>>(26)?.filter(|s| *s != 0),
                cut: r.get(27)?,
            });
        }
        // Newest file first, sorted here rather than in SQLite: no index has that order, so SQLite would hold a whole
        // sorted copy of every row in memory beside this list. A date that isn't real (a pack whose files all say
        // 31 Dec 2027) is no clue to when the file was saved, so it comes after every real one: it doesn't top the
        // list, and its file's copy of some notes doesn't stand for the copies with a real date.
        let real = RealDates::now();
        list.sort_unstable_by_key(|p| (!real.seconds(p.source.mtime), std::cmp::Reverse(p.source.mtime), p.id));
        Ok(list)
    }

    /// Says what kind of part this is (every copy with the same notes), or `None` to go back to the app's guess.
    pub fn set_your_role(&mut self, hash: &str, role: Option<Role>) -> Result<(), MidiError> {
        // None is kept as a row too, so it wins over a correction made in the C# app.
        self.conn.execute(
            "INSERT INTO your_roles (hash, role) VALUES (?1, ?2) ON CONFLICT(hash) DO UPDATE SET role = ?2",
            params![hash, role.map(Role::code)],
        )?;
        Ok(())
    }

    /// Remembers the octaves you moved these notes by (every copy), or `None` to let the app pick again.
    pub fn set_your_octave(&mut self, hash: &str, octaves: Option<i32>) -> Result<(), MidiError> {
        self.conn.execute(
            "INSERT INTO your_octaves (hash, octaves) VALUES (?1, ?2) ON CONFLICT(hash) DO UPDATE SET octaves = ?2",
            params![hash, octaves],
        )?;
        Ok(())
    }

    /// Takes in the corrections you made in the C# app (`roles`, `octaves`, by note hash), once: one already kept for
    /// the same notes wins (it was made in this app since), and the index notes that they're in, so the C# app's
    /// `midi.db` isn't opened again. One transaction, so they're all in, noted, or none: how many kinds and octave
    /// moves were taken in.
    pub fn take_in_old(
        &mut self,
        roles: &HashMap<String, Role>,
        octaves: &HashMap<String, i32>,
    ) -> Result<(usize, usize), MidiError> {
        let tx = self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let (mut took_roles, mut took_octaves) = (0, 0);
        {
            let mut role =
                tx.prepare("INSERT INTO your_roles (hash, role) VALUES (?1, ?2) ON CONFLICT(hash) DO NOTHING")?;
            let mut octave =
                tx.prepare("INSERT INTO your_octaves (hash, octaves) VALUES (?1, ?2) ON CONFLICT(hash) DO NOTHING")?;
            for (hash, r) in roles {
                took_roles += role.execute(params![hash, r.code()])?;
            }
            for (hash, o) in octaves {
                took_octaves += octave.execute(params![hash, o])?;
            }
            let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
            tx.execute(
                "INSERT INTO about (name, value) VALUES (?1, ?2) ON CONFLICT(name) DO UPDATE SET value = ?2",
                params![TOOK_IN_OLD, now],
            )?;
        }
        tx.commit()?;
        Ok((took_roles, took_octaves))
    }

    /// A part's notes, at [`crate::PPQ`]: the part's own (`id`, while its `hash` still matches, as ids of a file
    /// read again go to new parts), else any copy with the same `hash`; none when no file has them any more.
    pub fn notes(&self, id: i64, hash: &str) -> Result<Vec<NoteEvent>, MidiError> {
        // An index of the older layout the page reads as it is, until it's copied into this one: the notes in `notes`.
        let (own, any) = if self.layout >= 2 {
            (
                "SELECT d.data FROM parts p JOIN notes n ON n.id = p.notes JOIN note_data d ON d.notes = n.id
                 WHERE p.id = ?1 AND n.hash = ?2",
                "SELECT d.data FROM notes n JOIN note_data d ON d.notes = n.id WHERE n.hash = ?1 LIMIT 1",
            )
        } else {
            (
                "SELECT n.data FROM parts p JOIN notes n ON n.id = p.notes WHERE p.id = ?1 AND n.hash = ?2",
                "SELECT data FROM notes WHERE hash = ?1 LIMIT 1",
            )
        };
        let data = match self.conn.query_row(own, params![id, hash], |r| r.get::<_, Vec<u8>>(0)).optional()? {
            Some(d) => Some(d),
            None => self.conn.query_row(any, [hash], |r| r.get(0)).optional()?,
        };
        Ok(data.map(|d| decode_notes(&d)).unwrap_or_default())
    }
}

/// What a file's DAW is, as the library names it ("Bitwig Studio"); "MIDI" for a `.mid` file.
pub fn daw_of(path: &str, kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::File => "MIDI",
        SourceKind::Project => mo_library::scan::daw_of(path).unwrap_or(""),
    }
}

/// The hash of a set of notes exactly as stored, velocities and attributes too: 16 capital hex letters.
fn sound_of(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(data).iter().take(8).map(|b| format!("{b:02X}")).collect()
}

/// [`NOTE_BYTES`] a note: start and length (32-bit, held to what fits), key, the 16-bit velocity and the MIDI 2.0
/// attribute, little-endian.
pub fn encode_notes(notes: &[NoteEvent]) -> Vec<u8> {
    let mut data = Vec::with_capacity(notes.len() * NOTE_BYTES);
    for n in notes {
        data.extend(clamp32(n.start).to_le_bytes());
        data.extend(clamp32(n.length).to_le_bytes());
        data.push(n.key);
        data.extend(n.velocity.to_le_bytes());
        data.push(n.attribute.kind);
        data.extend(n.attribute.data.to_le_bytes());
    }
    data
}

fn clamp32(v: i64) -> i32 {
    v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// The notes [`encode_notes`] wrote. A short piece at the end (a damaged row) is left out.
pub fn decode_notes(data: &[u8]) -> Vec<NoteEvent> {
    let (notes, _damaged) = data.as_chunks::<NOTE_BYTES>();
    notes
        .iter()
        .map(|&[s0, s1, s2, s3, l0, l1, l2, l3, key, v0, v1, kind, d0, d1]| NoteEvent {
            start: i64::from(i32::from_le_bytes([s0, s1, s2, s3])),
            length: i64::from(i32::from_le_bytes([l0, l1, l2, l3])),
            key,
            velocity: u16::from_le_bytes([v0, v1]),
            muted: false,
            step: false,
            attribute: NoteAttribute { kind, data: u16::from_le_bytes([d0, d1]) },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_keep_midi_2_resolution() {
        let n = NoteEvent {
            start: -48,
            length: i64::MAX,
            key: 131,
            velocity: 40_000,
            muted: false,
            step: false,
            attribute: NoteAttribute { kind: 3, data: 0x3C80 },
        };
        let data = encode_notes(&[n]);
        assert_eq!(data.len(), NOTE_BYTES);
        let back = decode_notes(&data);
        assert_eq!(back, [NoteEvent { length: i64::from(i32::MAX), ..n }]);
        // The same notes played softer are another sound.
        let soft = encode_notes(&[NoteEvent { velocity: 1000, ..n }]);
        assert_ne!(sound_of(&data), sound_of(&soft));
    }
}
