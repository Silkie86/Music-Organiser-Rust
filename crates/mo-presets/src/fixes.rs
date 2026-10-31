//! Your fixes to the part guess, kept in the app's own `presets.db` (beside `library.db` in the app's data folder).
//!
//! One table, one fact per row: `your_preset_roles (hash, role)`. A fix is kept by a fingerprint of the sound playing
//! its part ([`fix_key`]: the plugin, its settings and the notes it plays), as the C# app's `midi.db` keeps MIDI part
//! fixes by a hash of the notes. So it holds for every copy of that sound in that part, in every save and copied
//! folder, and survives a rescan; a clone of the channel playing other notes, or a plugin left at its defaults on
//! another track, keeps its own. Removing the row brings the guess back.
//!
//! The C# app has no presets database, so this one is the Rust app's alone; like every database here its tables are
//! only ever added to. A new database is made beside the real name, checked, then renamed into place, so a half-made
//! file is never taken for the real one.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

use mo_formats::NoteEvent;

use crate::role::Role;

/// The file's name in the app's data folder.
pub const FILE_NAME: &str = "presets.db";

/// The tables, as made in a new database. Only ever added to (`IF NOT EXISTS`), never changed. (`NOT NULL` on the
/// key too: SQLite would otherwise let a row with no fingerprint in.)
const SCHEMA: &str =
    "CREATE TABLE IF NOT EXISTS your_preset_roles (hash TEXT PRIMARY KEY NOT NULL, role TEXT NOT NULL);";

/// A sound's fingerprint: the first 16 bytes of the SHA-256 of its settings, as 32 lowercase hex digits. The same
/// settings always give the same fingerprint, on every computer and in every version of the app.
pub fn sound_hash(data: &[u8]) -> String {
    hex16(&Sha256::digest(data))
}

/// The fingerprint a fix is kept by: the SHA-256 of the plugin's name (letter case aside), its settings as the reader
/// gives them, and the notes it plays (for an effect, the notes of everything feeding it), each part's notes counted
/// from its first so moving the whole part along doesn't matter; muted notes left out. The first 16 bytes, as 32
/// lowercase hex digits. The same sound in the same part gives the same fingerprint on every computer.
///
/// A change to how a reader wraps a plugin's settings changes the fingerprint, and the fixes on those sounds stop
/// applying; such a change bumps the reader's version (rule 7), so it can be seen coming.
pub fn fix_key(plugin: &str, settings: &[u8], parts: &[&[NoteEvent]]) -> String {
    let mut h = Sha256::new();
    h.update(b"mo-presets fix 1\0");
    h.update(plugin.to_lowercase().as_bytes());
    h.update([0]);
    h.update((settings.len() as u64).to_le_bytes());
    h.update(settings);
    for part in parts {
        let mut notes: Vec<&NoteEvent> = part.iter().filter(|n| !n.muted).collect();
        notes.sort_by_key(|n| (n.start, n.key, n.length));
        let first = notes.first().map_or(0, |n| n.start);
        h.update(b"part");
        h.update((notes.len() as u64).to_le_bytes());
        for n in notes {
            h.update((n.start - first).to_le_bytes());
            h.update(n.length.to_le_bytes());
            h.update([n.key]);
        }
    }
    hex16(&h.finalize())
}

fn hex16(digest: &[u8]) -> String {
    let mut out = String::with_capacity(32);
    for b in digest.iter().take(16) {
        // Writing to a String can't fail.
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Whether a file is at `path`. A file that can't be looked at is an error, never "not there": that would make a new,
/// empty database and put it over yours.
fn is_there(path: &Path) -> Result<bool, FixesError> {
    path.try_exists().map_err(|e| FixesError(format!("couldn't look for it: {e}")))
}

/// What went wrong with `presets.db`.
#[derive(Debug)]
pub struct FixesError(pub String);

impl std::fmt::Display for FixesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "presets.db: {}", self.0)
    }
}

impl std::error::Error for FixesError {}

impl From<rusqlite::Error> for FixesError {
    fn from(e: rusqlite::Error) -> FixesError {
        FixesError(e.to_string())
    }
}

/// Your part fixes, open on `presets.db`. Asks the disk, so never on the window's thread.
pub struct RoleFixes {
    conn: Connection,
}

impl RoleFixes {
    /// Opens the database at `path`, making it first when it isn't there: made under a name beside it, checked, then
    /// renamed into place.
    pub fn open(path: &Path) -> Result<RoleFixes, FixesError> {
        if !is_there(path)? {
            make_new(path)?;
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(2))?;
        // As every database the app shares (spec section 6): a reader never waits on a write, nor a write on a reader.
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |r| r.get::<_, String>(0))?;
        // A database made by an older app gets the tables it lacks; nothing already there changes.
        conn.execute_batch(SCHEMA)?;
        Ok(RoleFixes { conn })
    }

    /// Every fix, by fingerprint. A row whose part this app doesn't know (written by a newer one) is left out, and left
    /// alone.
    pub fn all(&self) -> Result<HashMap<String, Role>, FixesError> {
        let mut st = self.conn.prepare("SELECT hash, role FROM your_preset_roles")?;
        let rows = st.query_map([], |r| Ok((text_at(r, 0)?, text_at(r, 1)?)))?;
        let mut out = HashMap::new();
        for row in rows {
            // An empty fingerprint is never one of ours (`set` refuses it): kept, it would match every preset file's
            // row, whose fingerprint is empty.
            if let (Some(hash), Some(role)) = row?
                && !hash.is_empty()
                && let Some(role) = Role::from_key(&role)
            {
                out.insert(hash, role);
            }
        }
        Ok(out)
    }

    /// The fix for one sound, if you made one.
    pub fn get(&self, hash: &str) -> Result<Option<Role>, FixesError> {
        if hash.is_empty() {
            return Ok(None);
        }
        let role = self
            .conn
            .query_row("SELECT role FROM your_preset_roles WHERE hash = ?1", [hash], |r| text_at(r, 0))
            .optional()?
            .flatten();
        Ok(role.as_deref().and_then(Role::from_key))
    }

    /// Sets a sound's part; it wins over the guess from now on.
    pub fn set(&self, hash: &str, role: Role) -> Result<(), FixesError> {
        if hash.is_empty() {
            return Err(FixesError("a fix needs the sound's fingerprint".into()));
        }
        self.conn.execute(
            "INSERT INTO your_preset_roles (hash, role) VALUES (?1, ?2) \
             ON CONFLICT(hash) DO UPDATE SET role = excluded.role",
            params![hash, role.key()],
        )?;
        Ok(())
    }

    /// Takes a fix away, so the guess shows again.
    pub fn clear(&self, hash: &str) -> Result<(), FixesError> {
        self.conn.execute("DELETE FROM your_preset_roles WHERE hash = ?1", [hash])?;
        Ok(())
    }
}

/// A column's text, or `None` when it holds something else (a newer app's row, or a hand edit): such a row is passed
/// over like an unknown part, rather than hiding every other fix.
fn text_at(r: &rusqlite::Row<'_>, i: usize) -> rusqlite::Result<Option<String>> {
    r.get_ref(i).map(|v| v.as_str().ok().map(str::to_owned))
}

/// Makes a new database at `path`: under a name beside it first, checked, then renamed into place.
fn make_new(path: &Path) -> Result<(), FixesError> {
    // Its own name, so two copies of the app starting at once never touch each other's half-made file.
    let mut temp = path.as_os_str().to_owned();
    temp.push(format!(".new-{}", std::process::id()));
    let temp = PathBuf::from(temp);
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| FixesError(format!("couldn't make its folder: {e}")))?;
    }
    remove_old_half_made(path);
    if let Err(e) = make_checked(&temp) {
        // Whatever stopped it, the half-made file doesn't stay.
        let _ = std::fs::remove_file(&temp);
        return Err(e);
    }
    // SQLite would replay an old journal left beside a database that's gone into the new one, bringing back what was
    // in the old file (or calling it damaged). Only when there's no database are they leftovers.
    if !is_there(path)? {
        for end in ["-wal", "-journal", "-shm"] {
            let mut old = path.as_os_str().to_owned();
            old.push(end);
            let _ = std::fs::remove_file(PathBuf::from(old));
        }
    }
    // Another copy of the app may have made it meanwhile (and kept fixes in it): keep theirs. A link never replaces a
    // file that's there, where a rename would; a disk that can't link gets the rename, only when nothing is there.
    let placed = match std::fs::hard_link(&temp, path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(_) => match is_there(path) {
            Ok(false) => std::fs::rename(&temp, path)
                .map_err(|e| FixesError(format!("couldn't put the new database in place: {e}"))),
            other => other.map(|_| ()),
        },
    };
    let _ = std::fs::remove_file(&temp);
    placed
}

/// Removes half-made databases that a copy of the app left when it stopped part way (`presets.db.new-<number>`), once
/// they're a minute old, so one being made right now by another copy is left alone.
fn remove_old_half_made(path: &Path) {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else { return };
    let dir = if dir.as_os_str().is_empty() { Path::new(".") } else { dir };
    let mut prefix = name.to_os_string();
    prefix.push(".new-");
    let prefix = prefix.to_string_lossy().into_owned();
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let old = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok());
        if e.file_name().to_string_lossy().starts_with(&prefix) && old.is_some_and(|o| o.as_secs() >= 60) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Makes the tables in a new file at `temp` and checks them. The file is closed again when this returns, so it can be
/// removed or put in place.
fn make_checked(temp: &Path) -> Result<(), FixesError> {
    let conn = Connection::open(temp)?;
    conn.execute_batch(SCHEMA)?;
    let check: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    let tables: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'your_preset_roles'",
        [],
        |r| r.get(0),
    )?;
    if check != "ok" || tables != 1 {
        return Err(FixesError(format!("the new database didn't check out ({check})")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fingerprint_is_the_same_every_time() {
        assert_eq!(sound_hash(b"abc"), "ba7816bf8f01cfea414140de5dae2223");
        assert_ne!(sound_hash(b"abc"), sound_hash(b"abd"));
        assert_eq!(sound_hash(&[]).len(), 32);
    }
}
