//! The Rust app's own MIDI index, `midi-index.db`, beside the library. A new file: the C# app's `midi.db` is never
//! written, only read (see [`crate::MidiIndex`]).
//!
//! Each fact is stored once (rule 23):
//! - `files`: which file the index read, when it was last changed, its size, and what read it, so it's only read again
//!   when one of those changes. Nothing else about a project: its name, folder, DAW and tempo are in `library.db`,
//!   joined by path (letter case aside), never by the library's save id, which the C# app's scans can change.
//! - `notes`: each set of notes once, with the facts that come from the notes alone (how many, bars, lowest and
//!   highest, pitch-class shares, shape). A pattern kept through 36 saves of a song is stored once, where the C#
//!   `midi.db` stores it 36 times. Notes keep MIDI 2.0's 16-bit velocity and their attribute (rule 22), so two files
//!   with the same notes played harder are two rows (`sound`), which share the C# app's note `hash` (timing and keys
//!   only), by which the page groups copies and your corrections hold.
//! - `parts`: where each set of notes was found and what its names say: one row per part per file.
//! - `keys`: `mo-harmony`'s key reading of each set of notes (8 bytes, [`KeyFact`]), with what read it
//!   ([`keys::VERSION`]) and how (the kind the notes were heard as, and whether they're a piece cut from a longer
//!   stretch), so it's read again from the stored notes, without opening any file, when the finder gets better or the
//!   kind changes (rule 7). It's per set of notes, velocities and all: a stray soft touch isn't heard. A table of its
//!   own, so the list reads it without going past the notes, and finding what to read again is one index lookup.
//! - `your_roles`, `your_octaves`: what you said about a set of notes in the Rust app. Until the Rust MIDI page is
//!   trusted, the C# app's `midi.db` is still where you correct parts day to day, so its two tables are read live
//!   (read-only) and only corrections made in the Rust app are kept here, winning over the C# ones (a row with no value
//!   says "back to the app's guess", which wins too). They're copied over once, when the Rust app takes over.
//!
//! Writes happen on one connection, in batches, from the indexer's one saving thread; the page reads on its own
//! connections, so neither waits (rule 12).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mo_formats::{NoteAttribute, NoteEvent, case};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use crate::analyser;
use crate::extract::Found;
use crate::index::{MidiError, Source, SourceKind, StoredPhrase};
use crate::keys;
use crate::member::PhraseMember;
use crate::roles::Role;
use mo_harmony::KeyFact;

/// Marks a file as `midi-index.db` ("MOMI"), so the C# `midi.db` or another app's database is never taken for it.
const APPLICATION_ID: i64 = 0x4D4F_4D49;

const SCHEMA: &str = "
    PRAGMA journal_mode = WAL;
    PRAGMA foreign_keys = ON;
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
        sound TEXT NOT NULL UNIQUE,     -- the hash of `data`: these exact notes, velocities and all
        hash TEXT NOT NULL,             -- the C# app's note hash: timing and keys only
        count INTEGER NOT NULL,
        bars INTEGER NOT NULL,
        lowest INTEGER NOT NULL,
        highest INTEGER NOT NULL,
        chroma BLOB NOT NULL,           -- 12 bytes, the strongest 255
        features BLOB NOT NULL,         -- 48 bytes, 0 to 1 as 0 to 255; the scale counts from the key
        data BLOB NOT NULL              -- last, so the list reads the small facts without the notes (NOTE_BYTES each)
    );
    CREATE INDEX IF NOT EXISTS ix_notes_hash ON notes(hash);
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
    CREATE INDEX IF NOT EXISTS ix_parts_file ON parts(file);
    CREATE INDEX IF NOT EXISTS ix_parts_notes ON parts(notes);
    CREATE TABLE IF NOT EXISTS keys (
        notes INTEGER PRIMARY KEY REFERENCES notes(id) ON DELETE CASCADE,
        version INTEGER NOT NULL,       -- keys::VERSION that read it; 0 not read yet
        role INTEGER NOT NULL,          -- the kind the notes were heard as (Role::code)
        cut INTEGER NOT NULL,           -- 1 for a piece cut from a longer stretch, heard as a window
        fact BLOB                       -- mo-harmony's KeyFact (8 bytes); NULL for drums or no pitched notes
    );
    CREATE INDEX IF NOT EXISTS ix_keys_version ON keys(version);
    -- NULL: back to the app's guess (which wins over the C# app's correction too).
    CREATE TABLE IF NOT EXISTS your_roles (hash TEXT PRIMARY KEY, role INTEGER) WITHOUT ROWID;
    CREATE TABLE IF NOT EXISTS your_octaves (hash TEXT PRIMARY KEY, octaves INTEGER) WITHOUT ROWID;
";

/// The tables' layout this app makes (`PRAGMA user_version`).
const LAYOUT: i64 = 1;

/// Brings an index made by an older Rust app up to this one's tables, only adding: sets of notes saved before keys were
/// read get a `keys` row saying "not read yet", so the next run reads them. One transaction, so a crash leaves it as it
/// was, and two apps opening it at once do it once.
fn upgrade(conn: &Connection) -> rusqlite::Result<()> {
    let layout: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if layout >= LAYOUT {
        return Ok(());
    }
    conn.execute_batch(&format!(
        "BEGIN IMMEDIATE;
         INSERT OR IGNORE INTO keys (notes, version, role, cut, fact) SELECT id, 0, {other}, 0, NULL FROM notes;
         PRAGMA user_version = {LAYOUT};
         COMMIT;",
        other = Role::Other.code()
    ))
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
    /// Another program had it open: what was read from it before is kept, and it's read again next time.
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
}

impl MidiStore {
    /// `midi-index.db` in the same folder as the library.
    pub fn path_beside(library: &Path) -> PathBuf {
        library.with_file_name("midi-index.db")
    }

    /// Opens the index for writing, making it (and its tables) when it isn't there yet.
    pub fn open(path: &Path) -> Result<MidiStore, MidiError> {
        let open_error = |why: String| MidiError::Open { path: path.to_owned(), why };
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(|e| open_error(e.to_string()))?;
        }
        let conn = Connection::open(path).map_err(|e| open_error(e.to_string()))?;
        conn.busy_timeout(Duration::from_secs(5)).map_err(|e| open_error(e.to_string()))?;
        // Never another database: the C# app's midi.db (phrases, sources) or anything not made here.
        let id: i64 =
            conn.pragma_query_value(None, "application_id", |r| r.get(0)).map_err(|e| open_error(e.to_string()))?;
        let tables: i64 = conn
            .query_row("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'", [], |r| r.get(0))
            .map_err(|e| open_error(e.to_string()))?;
        if id != APPLICATION_ID && (id != 0 || tables > 0) {
            return Err(open_error("it isn't the Rust app's MIDI index, so it's left as it is".into()));
        }
        conn.pragma_update(None, "application_id", APPLICATION_ID).map_err(|e| open_error(e.to_string()))?;
        conn.execute_batch(SCHEMA).map_err(|e| open_error(e.to_string()))?;
        upgrade(&conn).map_err(|e| open_error(e.to_string()))?;
        Ok(MidiStore { conn, path: path.to_owned(), has_keys: true })
    }

    /// Opens the index for reading only, for the page: it must already exist.
    pub fn open_read_only(path: &Path) -> Result<MidiStore, MidiError> {
        let open_error = |why: String| MidiError::Open { path: path.to_owned(), why };
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(path, flags).map_err(|e| open_error(e.to_string()))?;
        conn.busy_timeout(Duration::from_secs(2)).map_err(|e| open_error(e.to_string()))?;
        conn.pragma_update(None, "query_only", true).map_err(|e| open_error(e.to_string()))?;
        let id: i64 =
            conn.pragma_query_value(None, "application_id", |r| r.get(0)).map_err(|e| open_error(e.to_string()))?;
        if id != APPLICATION_ID {
            return Err(open_error("it isn't the Rust app's MIDI index".into()));
        }
        // An index made before keys were read (until the indexer next opens it): listed without keys.
        let has_keys = conn
            .query_row("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'keys'", [], |r| r.get(0))
            .map_err(|e| open_error(e.to_string()))?;
        Ok(MidiStore { conn, path: path.to_owned(), has_keys })
    }

    pub fn path(&self) -> &Path {
        &self.path
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
                "INSERT INTO notes (sound, hash, count, bars, lowest, highest, chroma, features, data)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(sound) DO UPDATE SET hash = ?2, count = ?3, bars = ?4, lowest = ?5, highest = ?6,
                     chroma = ?7, features = ?8
                 RETURNING id",
            )?;
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
                            let data = encode_notes(&p.notes);
                            let notes: i64 = note.query_row(
                                params![
                                    sound_of(&data),
                                    a.hash,
                                    p.notes.len() as i64,
                                    a.bars,
                                    a.lowest,
                                    a.highest,
                                    analyser::bytes(&a.chroma, true),
                                    analyser::bytes(&a.features, false),
                                    data,
                                ],
                                |r| r.get(0),
                            )?;
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
            "SELECT k.notes, k.cut, n.data,
                    (SELECT p.role FROM parts p WHERE p.notes = k.notes ORDER BY p.id DESC LIMIT 1)
             FROM keys k JOIN notes n ON n.id = k.notes WHERE k.version < ?1 LIMIT ?2",
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

    /// Every part, newest file first, without its notes, as the page lists them. `cs_roles` and `cs_octaves` are the
    /// C# app's corrections (read live from its `midi.db`), which the Rust app's own win over.
    pub fn all(
        &self,
        cs_roles: &HashMap<String, Role>,
        cs_octaves: &HashMap<String, i32>,
    ) -> Result<Vec<StoredPhrase>, MidiError> {
        let (key_fact, keys_join) =
            if self.has_keys { ("k.fact", "LEFT JOIN keys k ON k.notes = n.id") } else { ("NULL", "") };
        let mut st = self.conn.prepare(&format!(
            "SELECT p.id, f.id, f.path, f.kind, f.mtime, f.size, p.part, p.clip, p.role, p.why, y.role, n.hash,
                    n.count, n.bars, n.lowest, n.highest, n.chroma, n.features, p.members, o.octaves, p.program,
                    p.drums, p.track_name, {key_fact}, y.hash IS NOT NULL, o.hash IS NOT NULL
             FROM parts p JOIN files f ON f.id = p.file JOIN notes n ON n.id = p.notes
                  LEFT JOIN your_roles y ON y.hash = n.hash LEFT JOIN your_octaves o ON o.hash = n.hash
                  {keys_join}
             ORDER BY f.mtime DESC, p.id"
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
            });
        }
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

    /// A part's notes, at [`crate::PPQ`]: the part's own (`id`, while its `hash` still matches, as ids of a file
    /// read again go to new parts), else any copy with the same `hash`; none when no file has them any more.
    pub fn notes(&self, id: i64, hash: &str) -> Result<Vec<NoteEvent>, MidiError> {
        let own: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT n.data FROM parts p JOIN notes n ON n.id = p.notes WHERE p.id = ?1 AND n.hash = ?2",
                params![id, hash],
                |r| r.get(0),
            )
            .optional()?;
        let data = match own {
            Some(d) => Some(d),
            None => self
                .conn
                .query_row("SELECT data FROM notes WHERE hash = ?1 LIMIT 1", [hash], |r| r.get(0))
                .optional()?,
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
