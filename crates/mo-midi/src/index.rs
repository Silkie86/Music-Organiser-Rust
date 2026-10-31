//! Reading `midi.db`, the C# app's index of MIDI parts, without ever changing it.
//!
//! The C# app keeps one row per part per file (`phrases`), the files it read (`sources`), and what you said about a
//! part, by the hash of its notes so it holds for every copy (`your_roles`, `your_octaves`). This reads all of it
//! read-only; SQLite itself refuses any change.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mo_formats::{NoteAttribute, NoteEvent, case, velocity};
use rusqlite::{Connection, ErrorCode, InterruptHandle, OpenFlags, OptionalExtension, Row};

use crate::chord::PhraseChord;
use crate::roles::Role;

/// What went wrong reading the index.
#[derive(Debug)]
pub enum MidiError {
    /// The file isn't there, can't be opened, or isn't a MIDI index.
    Open { path: PathBuf, why: String },
    /// A read failed (the file is damaged, or locked for longer than the wait).
    Read(rusqlite::Error),
    /// Stopped part way, because what it was for isn't wanted any more.
    Stopped,
}

impl fmt::Display for MidiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MidiError::Open { path, why } => write!(f, "Couldn't open the MIDI index {}: {why}", path.display()),
            MidiError::Read(e) => write!(f, "Couldn't read the MIDI index: {e}"),
            MidiError::Stopped => write!(f, "Stopped"),
        }
    }
}

impl std::error::Error for MidiError {}

impl From<rusqlite::Error> for MidiError {
    fn from(e: rusqlite::Error) -> MidiError {
        match e.sqlite_error_code() {
            Some(ErrorCode::OperationInterrupted) => MidiError::Stopped,
            _ => MidiError::Read(e),
        }
    }
}

/// Stops what a [`MidiIndex`] is reading, from another thread: when a newer read is wanted, the older one ends at
/// once, even while SQLite is still sorting (rule 12, newest wins). The read then ends with [`MidiError::Stopped`].
pub struct Stopper(InterruptHandle);

impl Stopper {
    pub fn stop(&self) {
        self.0.interrupt();
    }
}

/// Where a part was found: in a project, or in a `.mid` file in one of your MIDI folders.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SourceKind {
    Project,
    File,
}

impl SourceKind {
    /// As `midi.db` writes it: "project" or "file". Anything else is taken for a file.
    fn from_db(kind: &str) -> SourceKind {
        if kind == "project" { SourceKind::Project } else { SourceKind::File }
    }
}

/// A file the index has read.
#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    pub path: String,
    pub kind: SourceKind,
    /// "FL Studio", "Bitwig", "Studio One", or "MIDI" for a `.mid` file.
    pub daw: String,
    /// When the file was last written, as Unix seconds, when it was read.
    pub mtime: i64,
    pub size: i64,
    pub tempo: Option<f64>,
}

/// A file that couldn't be read, and why, in a few words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub path: String,
    pub kind: SourceKind,
    pub why: String,
}

impl Failure {
    pub fn name(&self) -> &str {
        mo_formats::paths::file_name(&self.path)
    }
}

/// A key the notes are in, with how sure the reading is (0 to 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Key {
    /// 0 = C … 11 = B.
    pub tonic: u8,
    pub minor: bool,
    pub confidence: f64,
}

impl Key {
    /// From this confidence on the page shows the key (the C# `MusicTheory.SureMidiKey`).
    pub const SURE: f64 = 0.05;

    pub fn is_sure(&self) -> bool {
        self.confidence >= Key::SURE
    }

    /// "Eb major", "C# minor": flat keys with flats, as charts and pack names write them.
    pub fn name(&self) -> String {
        format!("{} {}", self.note(), if self.minor { "minor" } else { "major" })
    }

    /// "Eb", "C#m".
    pub fn short(&self) -> String {
        format!("{}{}", self.note(), if self.minor { "m" } else { "" })
    }

    fn note(&self) -> &'static str {
        const MAJOR: [&str; 12] = ["C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B"];
        const MINOR: [&str; 12] = ["C", "C#", "D", "Eb", "E", "F", "F#", "G", "G#", "A", "Bb", "B"];
        let names = if self.minor { &MINOR } else { &MAJOR };
        names.get(self.tonic as usize % 12).copied().unwrap_or("C")
    }
}

/// One part as the index keeps it, without its notes (they're read when the part is shown, with
/// [`MidiIndex::notes`]).
#[derive(Clone, Debug)]
pub struct StoredPhrase {
    /// The row's id. A file read again gets new ids, so the page holds on to a part by [`StoredPhrase::hash`].
    pub id: i64,
    /// Shared by every part from the same file.
    pub source: Arc<Source>,
    /// The track, channel or `.mid` track name.
    pub part: String,
    /// The pattern or clip name, or the `.mid` file's name; with " · bars 9–16" for a piece of a longer part.
    pub clip: String,
    /// What the C# app guessed it is.
    pub role: Role,
    /// Why, for the tooltip.
    pub role_why: String,
    /// What you said it is, which wins over the guess.
    pub your_role: Option<Role>,
    pub bars: i32,
    pub notes: i32,
    pub lowest: u8,
    pub highest: u8,
    pub key: Option<Key>,
    /// How long each pitch class sounds, the strongest as 255.
    pub chroma: [u8; 12],
    /// The chords in order, a repeated chord once, with no timing.
    pub chords: Vec<PhraseChord>,
    /// The part's shape for "more like this", each number 0 to 1 as 0 to 255.
    pub features: Vec<u8>,
    /// The same notes give the same hash, so a pattern kept through 20 saves is one entry.
    pub hash: String,
    /// For a phrase joined from clips, its clips ([`crate::PhraseMember::encode`]); else "".
    pub members: String,
    /// The General MIDI program a `.mid` file sets for the part's channel.
    pub program: Option<u8>,
    /// On MIDI channel 10 (a `.mid` file) or played by an unpitched drum plugin (a project).
    pub drums: bool,
    /// A `.mid` track's own name where `part` leaves it out, such as "Keyscape", else "".
    pub track_name: String,
    /// The octaves you moved these notes by, or none.
    pub your_octave: Option<i32>,
}

impl StoredPhrase {
    /// What it is: what you said, or else the guess.
    pub fn role(&self) -> Role {
        self.your_role.unwrap_or(self.role)
    }
}

/// The C# app's `midi.db`, open for reading only.
///
/// One `MidiIndex` is one connection: the page's list and the notes for rows coming into view each open their own, so
/// neither waits behind the other (rule 12). The C# app's pool of connections and locks isn't needed for that.
pub struct MidiIndex {
    conn: Connection,
    path: PathBuf,
    /// Columns and tables added to `midi.db` over time: an older one may lack them, which then read as empty.
    has_members: bool,
    has_program: bool,
    has_track_name: bool,
    has_octaves: bool,
    has_roles: bool,
}

impl MidiIndex {
    /// `midi.db` in the same folder as the library.
    pub fn path_beside(library: &Path) -> PathBuf {
        library.with_file_name("midi.db")
    }

    /// Opens the index at `path` for reading. It must already exist: this never makes one.
    pub fn open_read_only(path: &Path) -> Result<MidiIndex, MidiError> {
        let open_error = |why: String| MidiError::Open { path: path.to_owned(), why };
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(path, flags).map_err(|e| open_error(e.to_string()))?;
        // The C# app may be saving what it read while this reads: wait a little for it rather than fail.
        conn.busy_timeout(Duration::from_secs(2)).map_err(|e| open_error(e.to_string()))?;
        conn.pragma_update(None, "query_only", true).map_err(|e| open_error(e.to_string()))?;
        let names = |sql: &str| -> rusqlite::Result<HashSet<String>> {
            let mut st = conn.prepare(sql)?;
            st.query_map([], |r| r.get::<_, String>(0))?.collect()
        };
        let tables = names("SELECT name FROM sqlite_master WHERE type = 'table'")
            .map_err(|e| open_error(format!("it isn't a MIDI index ({e})")))?;
        if !tables.contains("phrases") || !tables.contains("sources") {
            return Err(open_error("it isn't a MIDI index (it has no phrases or sources table)".into()));
        }
        let columns = names("SELECT name FROM pragma_table_info('phrases')").map_err(|e| open_error(e.to_string()))?;
        Ok(MidiIndex {
            has_members: columns.contains("members"),
            has_program: columns.contains("program"),
            has_track_name: columns.contains("track_name"),
            has_octaves: tables.contains("your_octaves"),
            has_roles: tables.contains("your_roles"),
            conn,
            path: path.to_owned(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A way to stop this index's current read from another thread.
    pub fn stopper(&self) -> Stopper {
        Stopper(self.conn.get_interrupt_handle())
    }

    /// Every file read so far, by path (letter case aside, as `midi.db` compares them): when it was last changed, its
    /// size and which version of the reading read it.
    pub fn sources(&self) -> Result<HashMap<String, (i64, i64, i64)>, MidiError> {
        let mut st = self.conn.prepare("SELECT path, mtime, size, version FROM sources")?;
        let rows = st.query_map([], |r| {
            Ok((
                case::key(&r.get::<_, String>(0)?),
                (
                    r.get::<_, Option<i64>>(1)?.unwrap_or(0),
                    r.get::<_, Option<i64>>(2)?.unwrap_or(0),
                    r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                ),
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The files that couldn't be read last time, and why, by name.
    pub fn failures(&self) -> Result<Vec<Failure>, MidiError> {
        let mut st = self.conn.prepare("SELECT path, kind, error FROM sources WHERE error IS NOT NULL")?;
        let mut list = st
            .query_map([], |r| {
                Ok(Failure {
                    path: r.get(0)?,
                    kind: SourceKind::from_db(&r.get::<_, Option<String>>(1)?.unwrap_or_default()),
                    why: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        list.sort_by_cached_key(|f| case::key(f.name()));
        Ok(list)
    }

    pub fn phrase_count(&self) -> Result<usize, MidiError> {
        let n: i64 = self.conn.query_row("SELECT COUNT(*) FROM phrases", [], |r| r.get(0))?;
        Ok(usize::try_from(n).unwrap_or(0))
    }

    /// Every part, newest file first, without its notes. A [`Stopper`] can end it part way.
    pub fn all(&self) -> Result<Vec<StoredPhrase>, MidiError> {
        let opt = |has: bool, column: &str| if has { column.to_owned() } else { "NULL".to_owned() };
        let sql = format!(
            "SELECT ph.id, ph.source, ph.part, ph.clip, ph.role, ph.role_why, {your_role}, ph.bars, ph.notes, ph.lowest,
                    ph.highest, ph.key_tonic, ph.key_minor, ph.key_conf, ph.chroma, ph.chords, ph.features, ph.hash,
                    s.kind, s.daw, s.mtime, s.size, s.tempo, {members}, {octaves}, {program}, ph.drums, {track_name}
             FROM phrases ph JOIN sources s ON s.path = ph.source {roles_join} {octaves_join}
             ORDER BY s.mtime DESC, ph.id",
            your_role = opt(self.has_roles, "y.role"),
            members = opt(self.has_members, "ph.members"),
            octaves = opt(self.has_octaves, "o.octaves"),
            program = opt(self.has_program, "ph.program"),
            track_name = opt(self.has_track_name, "ph.track_name"),
            roles_join = if self.has_roles { "LEFT JOIN your_roles y ON y.hash = ph.hash" } else { "" },
            octaves_join = if self.has_octaves { "LEFT JOIN your_octaves o ON o.hash = ph.hash" } else { "" },
        );
        let mut st = self.conn.prepare(&sql)?;
        let mut rows = st.query([])?;
        let mut list = Vec::new();
        // One shared copy of each file's facts, by its path as stored (rows of one file are joined on that).
        let mut sources: HashMap<String, Arc<Source>> = HashMap::new();
        while let Some(r) = rows.next()? {
            let path: String = r.get(1)?;
            let source = match sources.get(&path) {
                Some(s) => Arc::clone(s),
                None => {
                    let s = Arc::new(source_of(r, path.clone())?);
                    sources.insert(path, Arc::clone(&s));
                    s
                }
            };
            list.push(phrase_of(r, source)?);
        }
        Ok(list)
    }

    /// What you said each set of notes is, in the C# app, by note hash.
    pub fn your_roles(&self) -> Result<HashMap<String, Role>, MidiError> {
        if !self.has_roles {
            return Ok(HashMap::new());
        }
        let mut st = self.conn.prepare("SELECT hash, role FROM your_roles")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, Role::from_name(&r.get::<_, String>(1)?))))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The octaves you moved each set of notes by, in the C# app, by note hash.
    pub fn your_octaves(&self) -> Result<HashMap<String, i32>, MidiError> {
        if !self.has_octaves {
            return Ok(HashMap::new());
        }
        let mut st = self.conn.prepare("SELECT hash, octaves FROM your_octaves")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?.clamp(-10, 10) as i32)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// A part's notes, at [`crate::PPQ`]. A file read again gets new ids, so while the list shown still has the old
    /// ones, the same notes are found by their `hash` instead. Better than the C# app: a row id is only taken when its
    /// hash still matches, as SQLite gives the ids of the rows a new reading replaced to the new rows, so the old id
    /// can now be another part (the C# app showed that part's notes). Empty when the notes are gone.
    pub fn notes(&self, id: i64, hash: Option<&str>) -> Result<Vec<NoteEvent>, MidiError> {
        let by_id: Option<Option<Vec<u8>>> = match hash {
            Some(hash) => self
                .conn
                .query_row("SELECT data FROM phrases WHERE id = ?1 AND hash = ?2", (id, hash), |r| r.get(0))
                .optional()?,
            None => self.conn.query_row("SELECT data FROM phrases WHERE id = ?1", [id], |r| r.get(0)).optional()?,
        };
        if let Some(Some(data)) = by_id {
            return Ok(decode_notes(&data));
        }
        let Some(hash) = hash else { return Ok(Vec::new()) };
        let by_hash: Option<Option<Vec<u8>>> =
            self.conn.query_row("SELECT data FROM phrases WHERE hash = ?1 LIMIT 1", [hash], |r| r.get(0)).optional()?;
        Ok(by_hash.flatten().map(|d| decode_notes(&d)).unwrap_or_default())
    }
}

fn source_of(r: &Row<'_>, path: String) -> rusqlite::Result<Source> {
    Ok(Source {
        path,
        kind: SourceKind::from_db(&r.get::<_, Option<String>>(18)?.unwrap_or_default()),
        daw: r.get::<_, Option<String>>(19)?.unwrap_or_default(),
        mtime: r.get::<_, Option<i64>>(20)?.unwrap_or(0),
        size: r.get::<_, Option<i64>>(21)?.unwrap_or(0),
        tempo: r.get(22)?,
    })
}

fn phrase_of(r: &Row<'_>, source: Arc<Source>) -> rusqlite::Result<StoredPhrase> {
    let text = |i: usize| -> rusqlite::Result<String> { Ok(r.get::<_, Option<String>>(i)?.unwrap_or_default()) };
    let int = |i: usize| -> rusqlite::Result<i64> { Ok(r.get::<_, Option<i64>>(i)?.unwrap_or(0)) };
    let key = match (r.get::<_, Option<i64>>(11)?, r.get::<_, Option<i64>>(12)?) {
        (Some(tonic), minor) => Some(Key {
            tonic: tonic.rem_euclid(12) as u8,
            minor: minor == Some(1),
            confidence: r.get::<_, Option<f64>>(13)?.unwrap_or(0.0),
        }),
        (None, _) => None,
    };
    let mut chroma = [0u8; 12];
    let stored: Vec<u8> = r.get::<_, Option<Vec<u8>>>(14)?.unwrap_or_default();
    for (to, from) in chroma.iter_mut().zip(stored) {
        *to = from;
    }
    Ok(StoredPhrase {
        id: r.get(0)?,
        source,
        part: text(2)?,
        clip: text(3)?,
        role: Role::from_name(&text(4)?),
        role_why: text(5)?,
        your_role: r.get::<_, Option<String>>(6)?.map(|s| Role::from_name(&s)),
        bars: int(7)?.clamp(0, i32::MAX as i64) as i32,
        notes: int(8)?.clamp(0, i32::MAX as i64) as i32,
        lowest: int(9)?.clamp(0, 127) as u8,
        highest: int(10)?.clamp(0, 127) as u8,
        key,
        chroma,
        chords: PhraseChord::decode(&text(15)?),
        features: r.get::<_, Option<Vec<u8>>>(16)?.unwrap_or_default(),
        hash: text(17)?,
        members: text(23)?,
        your_octave: r.get::<_, Option<i64>>(24)?.map(|o| o.clamp(-10, 10) as i32),
        program: r.get::<_, Option<i64>>(25)?.filter(|p| (0..128).contains(p)).map(|p| p as u8),
        drums: int(26)? == 1,
        track_name: text(27)?,
    })
}

/// The C# app's notes: 10 bytes each, start and length in ticks (32-bit, little-endian), key and MIDI 1.0
/// velocity, which comes back as MIDI 2.0's 16 bits (rule 22). A short piece at the end (a damaged row) is left out.
pub fn decode_notes(data: &[u8]) -> Vec<NoteEvent> {
    let (notes, _damaged) = data.as_chunks::<10>();
    notes
        .iter()
        .map(|&[s0, s1, s2, s3, l0, l1, l2, l3, key, vel]| NoteEvent {
            start: i32::from_le_bytes([s0, s1, s2, s3]) as i64,
            length: i32::from_le_bytes([l0, l1, l2, l3]) as i64,
            key: key.min(127),
            velocity: velocity::from_7(vel),
            muted: false,
            step: false,
            attribute: NoteAttribute::NONE,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_come_back_at_midi_2_resolution() {
        let mut data = Vec::new();
        for (start, length, key, vel) in [(0i32, 96i32, 60u8, 127u8), (-48, 384, 36, 64)] {
            data.extend(start.to_le_bytes());
            data.extend(length.to_le_bytes());
            data.extend([key, vel]);
        }
        data.push(1); // a damaged last row
        let notes = decode_notes(&data);
        assert_eq!(notes.len(), 2);
        assert_eq!((notes[0].start, notes[0].length, notes[0].key), (0, 96, 60));
        assert_eq!(notes[0].velocity, u16::MAX);
        assert_eq!(notes[0].velocity7(), 127);
        assert_eq!(notes[1].start, -48);
        assert_eq!(notes[1].velocity7(), 64);
    }

    #[test]
    fn a_newer_read_stops_an_older_one() {
        let index = MidiIndex {
            conn: Connection::open_in_memory().unwrap(),
            path: PathBuf::new(),
            has_members: true,
            has_program: true,
            has_track_name: true,
            has_octaves: true,
            has_roles: true,
        };
        let stopper = index.stopper();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            stopper.stop();
        });
        // A query that would take minutes, stopped while SQLite is busy with it.
        let slow: Result<i64, MidiError> = index
            .conn
            .query_row(
                "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n) SELECT COUNT(*) FROM n",
                [],
                |r| r.get(0),
            )
            .map_err(MidiError::from);
        t.join().unwrap();
        assert!(matches!(slow, Err(MidiError::Stopped)), "{slow:?}");
    }

    #[test]
    fn key_names() {
        let eb = Key { tonic: 3, minor: false, confidence: 0.2 };
        assert_eq!((eb.name(), eb.short()), ("Eb major".to_owned(), "Eb".to_owned()));
        let csm = Key { tonic: 1, minor: true, confidence: 0.01 };
        assert_eq!((csm.name(), csm.short()), ("C# minor".to_owned(), "C#m".to_owned()));
        assert!(eb.is_sure() && !csm.is_sure());
    }
}
