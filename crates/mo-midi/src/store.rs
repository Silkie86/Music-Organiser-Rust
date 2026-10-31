//! The Rust app's own MIDI index, `midi-index.db`, beside the library. A new file: the C# app's `midi.db` is never
//! written, only read (see [`crate::MidiIndex`]).
//!
//! Each fact is stored once (rule 23):
//! - `files`: which file the index read, when it was last changed, its size, and what read it, so it's only read again
//!   when one of those changes. Nothing else about a project: its name, folder, DAW and tempo are in `library.db`,
//!   joined by path (letter case aside), never by the library's save id, which the C# app's scans can change.
//! - `notes`: each set of notes once, by its hash, with the facts that come from the notes alone (how many, bars,
//!   lowest and highest, pitch-class shares, shape). A pattern kept through 36 saves of a song is stored once, where
//!   the C# `midi.db` stores it 36 times. Velocities keep MIDI 2.0's 16 bits (rule 22).
//! - `parts`: where each set of notes was found and what its names say: one row per part per file.
//! - `harmony`: the key and chords `mo-harmony` reads from a set of notes, with the version that read them, so they're
//!   read again when it gets better (rule 7). Empty until `mo-harmony`'s key finder lands.
//! - `your_roles`, `your_octaves`: what you said about a set of notes in the Rust app. Until the Rust MIDI page is
//!   trusted, the C# app's `midi.db` is still where you correct parts day to day, so its two tables are read live
//!   (read-only) and only corrections made in the Rust app are kept here, winning over the C# ones. They're copied over
//!   once, when the Rust app takes over.
//!
//! Writes happen on one connection, in batches, from the indexer's one saving thread; the page reads on its own
//! connections, so neither waits (rule 12).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mo_formats::{NoteAttribute, NoteEvent, case, paths};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use crate::analyser;
use crate::extract::Found;
use crate::index::{Key, MidiError, Source, SourceKind, StoredPhrase};
use crate::member::PhraseMember;
use crate::roles::Role;

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
        hash TEXT PRIMARY KEY,
        data BLOB NOT NULL,             -- 11 bytes a note: start, length (i32), key, velocity (u16), little-endian
        count INTEGER NOT NULL,
        bars INTEGER NOT NULL,
        lowest INTEGER NOT NULL,
        highest INTEGER NOT NULL,
        chroma BLOB NOT NULL,           -- 12 bytes, the strongest 255
        features BLOB NOT NULL          -- 48 bytes, 0 to 1 as 0 to 255
    ) WITHOUT ROWID;
    CREATE TABLE IF NOT EXISTS parts (
        id INTEGER PRIMARY KEY,
        file INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
        hash TEXT NOT NULL,
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
    CREATE INDEX IF NOT EXISTS ix_parts_hash ON parts(hash);
    CREATE TABLE IF NOT EXISTS harmony (
        hash TEXT PRIMARY KEY,
        version INTEGER NOT NULL,       -- mo-harmony's version that read it
        key_tonic INTEGER,              -- 0 to 11, or NULL for no clear key
        key_mode INTEGER,               -- 0 major, 1 minor, NULL unclear
        confidence REAL,
        chords BLOB
    ) WITHOUT ROWID;
    CREATE TABLE IF NOT EXISTS your_roles (hash TEXT PRIMARY KEY, role INTEGER NOT NULL) WITHOUT ROWID;
    CREATE TABLE IF NOT EXISTS your_octaves (hash TEXT PRIMARY KEY, octaves INTEGER NOT NULL) WITHOUT ROWID;
";

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
        conn.execute_batch(SCHEMA).map_err(|e| open_error(e.to_string()))?;
        Ok(MidiStore { conn, path: path.to_owned() })
    }

    /// Opens the index for reading only, for the page: it must already exist.
    pub fn open_read_only(path: &Path) -> Result<MidiStore, MidiError> {
        let open_error = |why: String| MidiError::Open { path: path.to_owned(), why };
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(path, flags).map_err(|e| open_error(e.to_string()))?;
        conn.busy_timeout(Duration::from_secs(2)).map_err(|e| open_error(e.to_string()))?;
        conn.pragma_update(None, "query_only", true).map_err(|e| open_error(e.to_string()))?;
        Ok(MidiStore { conn, path: path.to_owned() })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Every file read so far, by path (letter case aside).
    pub fn stamps(&self) -> Result<HashMap<String, Stamp>, MidiError> {
        let mut st = self.conn.prepare("SELECT path, mtime, size, version, error IS NOT NULL FROM files")?;
        let rows = st.query_map([], |r| {
            let path: String = r.get(0)?;
            Ok((
                case::key(&path),
                Stamp {
                    path,
                    mtime: r.get(1)?,
                    size: r.get(2)?,
                    version: u32::try_from(r.get::<_, i64>(3)?).unwrap_or(0),
                    failed: r.get(4)?,
                },
            ))
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
            let mut note = tx.prepare_cached(
                // A newer analysis may find more in the same notes: what it found replaces the old.
                "INSERT INTO notes (hash, data, count, bars, lowest, highest, chroma, features)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(hash) DO UPDATE SET data = ?2, count = ?3, bars = ?4, lowest = ?5, highest = ?6,
                     chroma = ?7, features = ?8",
            )?;
            let mut part = tx.prepare_cached(
                "INSERT INTO parts (file, hash, part, clip, role, why, drums, program, track_name, members)
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
                            note.execute(params![
                                a.hash,
                                encode_notes(&p.notes),
                                p.notes.len() as i64,
                                a.bars,
                                a.lowest,
                                a.highest,
                                analyser::bytes(&a.chroma, true),
                                analyser::bytes(&a.features, false),
                            ])?;
                            let members = (!p.members.is_empty()).then(|| PhraseMember::encode(&p.members));
                            part.execute(params![
                                id,
                                a.hash,
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

    /// Drops notes no file has any more, and harmony read from them: once at the end of each run, not with every batch.
    pub fn tidy(&mut self) -> Result<usize, MidiError> {
        let tx = self.conn.transaction()?;
        let n =
            tx.execute("DELETE FROM notes WHERE NOT EXISTS (SELECT 1 FROM parts p WHERE p.hash = notes.hash)", [])?;
        tx.execute("DELETE FROM harmony WHERE NOT EXISTS (SELECT 1 FROM notes n WHERE n.hash = harmony.hash)", [])?;
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
        let mut st = self.conn.prepare(
            "SELECT p.id, f.id, f.path, f.kind, f.mtime, f.size, p.part, p.clip, p.role, p.why, y.role, p.hash,
                    n.count, n.bars, n.lowest, n.highest, n.chroma, n.features, p.members, o.octaves, p.program,
                    p.drums, p.track_name, h.key_tonic, h.key_mode, h.confidence
             FROM parts p JOIN files f ON f.id = p.file JOIN notes n ON n.hash = p.hash
                  LEFT JOIN your_roles y ON y.hash = p.hash LEFT JOIN your_octaves o ON o.hash = p.hash
                  LEFT JOIN harmony h ON h.hash = p.hash
             ORDER BY f.mtime DESC, p.id",
        )?;
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
            let key = match (r.get::<_, Option<i64>>(23)?, r.get::<_, Option<i64>>(24)?) {
                (Some(tonic), Some(mode)) => Some(Key {
                    tonic: tonic.rem_euclid(12) as u8,
                    minor: mode == 1,
                    confidence: r.get::<_, Option<f64>>(25)?.unwrap_or(0.0),
                }),
                _ => None,
            };
            let your_role = r.get::<_, Option<i64>>(10)?.map(Role::from_code).or_else(|| cs_roles.get(&hash).copied());
            let your_octave = r
                .get::<_, Option<i64>>(19)?
                .map(|o| o.clamp(-10, 10) as i32)
                .or_else(|| cs_octaves.get(&hash).copied());
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
                key,
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
        match role {
            Some(r) => self.conn.execute(
                "INSERT INTO your_roles (hash, role) VALUES (?1, ?2) ON CONFLICT(hash) DO UPDATE SET role = ?2",
                params![hash, r.code()],
            )?,
            None => self.conn.execute("DELETE FROM your_roles WHERE hash = ?1", [hash])?,
        };
        Ok(())
    }

    /// Remembers the octaves you moved these notes by (every copy), or `None` to let the app pick again.
    pub fn set_your_octave(&mut self, hash: &str, octaves: Option<i32>) -> Result<(), MidiError> {
        match octaves {
            Some(o) => self.conn.execute(
                "INSERT INTO your_octaves (hash, octaves) VALUES (?1, ?2) ON CONFLICT(hash) DO UPDATE SET octaves = ?2",
                params![hash, o],
            )?,
            None => self.conn.execute("DELETE FROM your_octaves WHERE hash = ?1", [hash])?,
        };
        Ok(())
    }

    /// A set of notes, at [`crate::PPQ`], or none when no file has them any more.
    pub fn notes(&self, hash: &str) -> Result<Vec<NoteEvent>, MidiError> {
        let data: Option<Vec<u8>> =
            self.conn.query_row("SELECT data FROM notes WHERE hash = ?1", [hash], |r| r.get(0)).optional()?;
        Ok(data.map(|d| decode_notes(&d)).unwrap_or_default())
    }
}

/// What a project's DAW is, from its ending; "MIDI" for a `.mid` file.
pub fn daw_of(path: &str, kind: SourceKind) -> &'static str {
    if kind == SourceKind::File {
        return "MIDI";
    }
    match case::to_lower(paths::extension(paths::file_name(path))).as_str() {
        ".flp" => "FL Studio",
        ".bwproject" => "Bitwig",
        ".song" => "Studio One",
        _ => "",
    }
}

/// 11 bytes a note: start and length (32-bit, little-endian, held to what fits), key, and the 16-bit velocity.
pub fn encode_notes(notes: &[NoteEvent]) -> Vec<u8> {
    let mut data = Vec::with_capacity(notes.len() * 11);
    for n in notes {
        data.extend(clamp32(n.start).to_le_bytes());
        data.extend(clamp32(n.length).to_le_bytes());
        data.push(n.key);
        data.extend(n.velocity.to_le_bytes());
    }
    data
}

fn clamp32(v: i64) -> i32 {
    v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// The notes [`encode_notes`] wrote. A short piece at the end (a damaged row) is left out.
pub fn decode_notes(data: &[u8]) -> Vec<NoteEvent> {
    let (notes, _damaged) = data.as_chunks::<11>();
    notes
        .iter()
        .map(|&[s0, s1, s2, s3, l0, l1, l2, l3, key, v0, v1]| NoteEvent {
            start: i64::from(i32::from_le_bytes([s0, s1, s2, s3])),
            length: i64::from(i32::from_le_bytes([l0, l1, l2, l3])),
            key,
            velocity: u16::from_le_bytes([v0, v1]),
            muted: false,
            step: false,
            attribute: NoteAttribute::NONE,
        })
        .collect()
}
