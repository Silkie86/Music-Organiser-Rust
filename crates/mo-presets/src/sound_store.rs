//! What the Presets page found in each save, kept between runs in `preset-sounds.db` (the app's own, in its data
//! folder), so a start reads only the saves that are new or changed since: before, every song's newest save (about
//! 3,000 project files on Solomon's PC) was opened and read again each time the page first showed.
//!
//! Each save is kept with its file's size and last-written time (to the nanosecond) and what read it ([`VERSION`]
//! here, the FL and Bitwig readers' versions, and the guessing limits), and is read again when any of them differs.
//! Only what the list shows is kept (never a plugin's settings: exporting reads the save again). A save that couldn't
//! be read isn't kept, so it's tried again next time. Saves no longer asked for are forgotten, so the file never holds
//! more than the page lists (rule 28).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use mo_formats::PluginKind;
use mo_formats::presets::PresetBlob;
use rayon::prelude::*;
use rusqlite::{Connection, OptionalExtension, params};

use crate::fixes::{FixesError, is_there, make_new};
use crate::library::SaveSounds;
use crate::list::SongPreset;
use crate::role::{Basis, Guess, Role, Thresholds};

/// The file's name, in the app's data folder.
pub const FILE_NAME: &str = "preset-sounds.db";

/// What the page's own guessing (`list`, `role`, `octave`) finds. Bump it whenever a change there would list a save's
/// sounds differently (rule 7), so every save is read again.
pub const VERSION: i64 = 1;

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS sound_meta (key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS sound_reads (id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL, modified INTEGER NOT NULL,
        size INTEGER NOT NULL, warning TEXT);
    CREATE TABLE IF NOT EXISTS sounds (save INTEGER NOT NULL, at INTEGER NOT NULL, name TEXT NOT NULL,
        plugin TEXT NOT NULL, extension TEXT NOT NULL, size INTEGER NOT NULL, kind INTEGER NOT NULL,
        native INTEGER NOT NULL, place TEXT NOT NULL, channel INTEGER, program INTEGER NOT NULL, track TEXT NOT NULL,
        plays_nothing INTEGER NOT NULL, role TEXT, basis INTEGER NOT NULL, reason TEXT NOT NULL,
        octave_checked INTEGER NOT NULL, weak INTEGER NOT NULL, hash TEXT NOT NULL, PRIMARY KEY (save, at))
        WITHOUT ROWID;";

/// One batch of saves from [`SoundStore::saves`].
pub struct Batch {
    /// The saves, in the order asked for.
    pub saves: Vec<SaveSounds>,
    /// How many were read from their files.
    pub read: usize,
    /// Why what was read couldn't be kept for next time, when it couldn't (the saves are still all here).
    pub not_kept: Option<String>,
}

/// The saves' sounds kept between runs.
pub struct SoundStore {
    conn: Connection,
}

/// A save's file as it is now: its last-written time (nanoseconds since 1970) and size; None when it can't be looked
/// at (gone, or a drive unplugged).
fn stamp(path: &str) -> Option<(i64, i64)> {
    let m = std::fs::metadata(path).ok()?;
    let at = m.modified().ok()?.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO);
    Some((i64::try_from(at.as_nanos()).unwrap_or(i64::MAX), i64::try_from(m.len()).unwrap_or(i64::MAX)))
}

fn kind_number(k: PluginKind) -> i64 {
    match k {
        PluginKind::Unknown => 0,
        PluginKind::Instrument => 1,
        PluginKind::Effect => 2,
        PluginKind::NoteEffect => 3,
    }
}

fn kind_of(n: i64) -> PluginKind {
    match n {
        1 => PluginKind::Instrument,
        2 => PluginKind::Effect,
        3 => PluginKind::NoteEffect,
        _ => PluginKind::Unknown,
    }
}

fn basis_number(b: Basis) -> i64 {
    match b {
        Basis::Notes => 0,
        Basis::Name => 1,
        Basis::Track => 2,
    }
}

fn basis_of(n: i64) -> Basis {
    match n {
        1 => Basis::Name,
        2 => Basis::Track,
        _ => Basis::Notes,
    }
}

impl SoundStore {
    /// Opens the store at `path`, making it first if it isn't there. `limits` are the guessing limits the page uses.
    /// A store kept by another reader (or other limits) is emptied, so every save is read again. A store that's
    /// damaged is only what was found before: it's made again, empty.
    pub fn open(path: &Path, limits: &Thresholds) -> Result<SoundStore, FixesError> {
        match Self::open_as_is(path, limits) {
            Ok(store) => Ok(store),
            Err(_) if is_there(path)? => {
                for end in ["", "-wal", "-shm", "-journal"] {
                    let mut p = path.as_os_str().to_owned();
                    p.push(end);
                    let _ = std::fs::remove_file(std::path::PathBuf::from(p));
                }
                Self::open_as_is(path, limits)
            }
            Err(e) => Err(e),
        }
    }

    fn open_as_is(path: &Path, limits: &Thresholds) -> Result<SoundStore, FixesError> {
        let err = |e: rusqlite::Error| FixesError(e.to_string());
        if !is_there(path)? {
            make_new(path, SCHEMA, "sounds")?;
        }
        let conn = Connection::open(path).map_err(err)?;
        conn.busy_timeout(Duration::from_secs(2)).map_err(err)?;
        conn.pragma_update(None, "journal_mode", "WAL").map_err(err)?;
        // Only what was found before: a write lost to a power cut is read again, so no wait for the disk at each one.
        conn.pragma_update(None, "synchronous", "NORMAL").map_err(err)?;
        conn.execute_batch(SCHEMA).map_err(err)?;
        let check: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0)).map_err(err)?;
        if check != "ok" {
            return Err(FixesError(format!("it's damaged ({check})")));
        }
        let version =
            format!("{VERSION}/fl {}/bitwig {}/{limits:?}", mo_formats::fl::VERSION, mo_formats::bitwig::VERSION);
        let kept: Option<String> = conn
            .query_row("SELECT value FROM sound_meta WHERE key = 'version'", [], |r| r.get(0))
            .optional()
            .map_err(err)?;
        if kept.as_deref() != Some(version.as_str()) {
            conn.execute_batch("BEGIN; DELETE FROM sounds; DELETE FROM sound_reads;").map_err(err)?;
            conn.execute("INSERT OR REPLACE INTO sound_meta VALUES ('version', ?1)", [&version]).map_err(err)?;
            conn.execute_batch("COMMIT;").map_err(err)?;
        }
        Ok(SoundStore { conn })
    }

    /// The saves at `paths`, in that order: those kept and unchanged from the store, the others read by `read` (over
    /// every core, as [`crate::library::read_saves`] does) and kept for next time. Like `read`, a save of a DAW whose
    /// projects keep no settings is left out. Call it on a pool of low-priority threads: the files are looked at over
    /// every core of it.
    pub fn saves(
        &mut self,
        paths: &[String],
        read: impl FnOnce(&[String]) -> Vec<SaveSounds>,
    ) -> Result<Batch, FixesError> {
        let err = |e: rusqlite::Error| FixesError(e.to_string());
        let mut seen = HashSet::new();
        let paths: Vec<&String> = paths.iter().filter(|p| seen.insert(p.as_str())).collect();
        let looked: Vec<Option<(i64, i64)>> = paths.par_iter().map(|p| stamp(p)).collect();
        let mut kept: HashMap<String, SaveSounds> = HashMap::new();
        let mut to_read = Vec::new();
        let mut stamps = HashMap::new();
        {
            let mut known = self
                .conn
                .prepare_cached("SELECT id, modified, size, warning FROM sound_reads WHERE path = ?1")
                .map_err(err)?;
            for (p, now) in paths.iter().zip(looked) {
                let found = known
                    .query_row([p], |r| {
                        Ok((
                            r.get::<_, i64>(0)?,
                            (r.get::<_, i64>(1)?, r.get::<_, i64>(2)?),
                            r.get::<_, Option<String>>(3)?,
                        ))
                    })
                    .optional()
                    .map_err(err)?;
                if let (Some((id, was, warning)), Some(now)) = (&found, now)
                    && *was == now
                    // A row that doesn't read back (written by something else) only means this save is read again.
                    && let Ok(sounds) = self.sounds_of(*id)
                {
                    let warning = warning.clone();
                    kept.insert((*p).clone(), SaveSounds { path: (*p).clone(), sounds, error: None, warning });
                    continue;
                }
                if let Some(now) = now {
                    stamps.insert((*p).clone(), now);
                }
                to_read.push((*p).clone());
            }
        }
        let fresh = if to_read.is_empty() { Vec::new() } else { read(&to_read) };
        let count = fresh.len();
        let not_kept = self.keep(&fresh, &stamps).err().map(|e| e.to_string());
        for s in fresh {
            kept.insert(s.path.clone(), s);
        }
        let saves = paths.iter().filter_map(|p| kept.remove(p.as_str())).collect();
        Ok(Batch { saves, read: count, not_kept })
    }

    /// A kept save's sounds, in the order they were found.
    fn sounds_of(&self, save: i64) -> rusqlite::Result<Vec<SongPreset>> {
        let mut st = self.conn.prepare_cached(
            "SELECT name, plugin, extension, size, kind, native, place, channel, program, track, plays_nothing, role, \
             basis, reason, octave_checked, weak, hash FROM sounds WHERE save = ?1 ORDER BY at",
        )?;
        let rows = st.query_map([save], |r| {
            let mut blob =
                PresetBlob::new(&r.get::<_, String>(0)?, &r.get::<_, String>(1)?, r.get::<_, String>(2)?, Vec::new());
            blob.kind = kind_of(r.get(4)?);
            blob.native = r.get::<_, i64>(5)? != 0;
            blob.place = r.get(6)?;
            blob.channel = r.get::<_, Option<i64>>(7)?.and_then(|c| u32::try_from(c).ok());
            blob.program = u32::try_from(r.get::<_, i64>(8)?).unwrap_or(0);
            Ok(SongPreset {
                blob,
                size: usize::try_from(r.get::<_, i64>(3)?).unwrap_or(0),
                track: r.get(9)?,
                plays_nothing: r.get::<_, i64>(10)? != 0,
                guess: Guess {
                    role: r.get::<_, Option<String>>(11)?.as_deref().and_then(Role::from_key),
                    basis: basis_of(r.get(12)?),
                    reason: r.get(13)?,
                    octave_checked: r.get::<_, i64>(14)? != 0,
                    weak: r.get::<_, i64>(15)? != 0,
                },
                hash: r.get(16)?,
                fixed: None,
            })
        })?;
        rows.collect()
    }

    /// Keeps the saves just read, each with its file's stamp from before it was read (a write since makes it differ,
    /// so it's read again next time). A save that couldn't be read, or couldn't be looked at (a drive unplugged),
    /// keeps what was found in it before, as `curves.db` does, and is tried again next time.
    fn keep(&mut self, read: &[SaveSounds], stamps: &HashMap<String, (i64, i64)>) -> rusqlite::Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut forget =
                tx.prepare_cached("DELETE FROM sounds WHERE save = (SELECT id FROM sound_reads WHERE path = ?1)")?;
            let mut save = tx.prepare_cached(
                "INSERT INTO sound_reads (path, modified, size, warning) VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT(path) DO UPDATE SET modified = excluded.modified, size = excluded.size, \
                 warning = excluded.warning RETURNING id",
            )?;
            let mut sound = tx.prepare_cached(
                "INSERT INTO sounds (save, at, name, plugin, extension, size, kind, native, place, channel, program, \
                 track, plays_nothing, role, basis, reason, octave_checked, weak, hash) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
            )?;
            for s in read {
                let Some(&(modified, size)) = stamps.get(&s.path).filter(|_| s.error.is_none()) else { continue };
                forget.execute([&s.path])?;
                let id: i64 = save.query_row(params![s.path, modified, size, s.warning], |r| r.get(0))?;
                for (at, p) in s.sounds.iter().enumerate() {
                    let b = &p.blob;
                    sound.execute(params![
                        id,
                        at as i64,
                        b.name,
                        b.plugin,
                        b.extension.as_ref(),
                        p.size as i64,
                        kind_number(b.kind),
                        b.native,
                        b.place,
                        b.channel.map(i64::from),
                        i64::from(b.program),
                        p.track,
                        p.plays_nothing,
                        p.guess.role.map(Role::key),
                        basis_number(p.guess.basis),
                        p.guess.reason,
                        p.guess.octave_checked,
                        p.guess.weak,
                        p.hash,
                    ])?;
                }
            }
        }
        tx.commit()
    }

    /// Forgets every save not among `paths` (no longer a song's newest), so the store holds only what the page lists.
    pub fn keep_only(&mut self, paths: &[String]) -> Result<(), FixesError> {
        let err = |e: rusqlite::Error| FixesError(e.to_string());
        let wanted: HashSet<&str> = paths.iter().map(String::as_str).collect();
        let known: Vec<(i64, String)> = {
            let mut st = self.conn.prepare("SELECT id, path FROM sound_reads").map_err(err)?;
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(err)?.collect::<Result<_, _>>().map_err(err)?
        };
        let tx = self.conn.transaction().map_err(err)?;
        for (id, path) in known.iter().filter(|(_, p)| !wanted.contains(p.as_str())) {
            tx.execute("DELETE FROM sounds WHERE save = ?1", [id]).map_err(err)?;
            tx.execute("DELETE FROM sound_reads WHERE path = ?1", [path]).map_err(err)?;
        }
        tx.commit().map_err(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sound(name: &str, role: Option<Role>) -> SongPreset {
        let mut blob = PresetBlob::new(name, "Serum", ".fxp", Vec::new());
        blob.kind = PluginKind::Instrument;
        blob.place = "Insert 3 · Bass".to_owned();
        blob.channel = Some(4);
        blob.program = 2;
        SongPreset {
            blob,
            size: 1234,
            track: "Bass".to_owned(),
            plays_nothing: false,
            guess: Guess {
                role,
                basis: Basis::Name,
                reason: "the name says bass".to_owned(),
                octave_checked: true,
                weak: true,
            },
            hash: "abc".to_owned(),
            fixed: None,
        }
    }

    #[test]
    fn a_save_is_read_once_until_it_changes() {
        let dir = std::env::temp_dir().join(format!("mo-presets-sound-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a.flp"), dir.join("b.flp"));
        std::fs::write(&a, b"one").unwrap();
        std::fs::write(&b, b"two").unwrap();
        let paths: Vec<String> = [&a, &b].iter().map(|p| p.to_string_lossy().into_owned()).collect();
        let limits = Thresholds::default();
        let mut store = SoundStore::open(&dir.join(FILE_NAME), &limits).unwrap();
        let reader = |asked: &[String]| -> Vec<SaveSounds> {
            asked
                .iter()
                .map(|p| SaveSounds {
                    path: p.clone(),
                    sounds: vec![sound("Wobble", Some(Role::Bass)), sound("Hit", None)],
                    error: None,
                    warning: p.ends_with("b.flp").then(|| "cut short".to_owned()),
                })
                .collect()
        };
        let first = store.saves(&paths, reader).unwrap();
        let read = first.read;
        let first = first.saves;
        assert_eq!(read, 2);
        let second = store.saves(&paths, |_| panic!("nothing should be read")).unwrap();
        let read = second.read;
        let second = second.saves;
        assert_eq!(read, 0);
        assert_eq!(second, first);

        // A changed save is read again; the other comes from the store.
        std::fs::write(&a, b"one, longer").unwrap();
        let third = store.saves(&paths, reader).unwrap();
        let read = third.read;
        let third = third.saves;
        assert_eq!(read, 1);
        assert_eq!(third, first);

        // A save that couldn't be read is tried again next time.
        std::fs::write(&b, b"two, longer").unwrap();
        let broken = |asked: &[String]| -> Vec<SaveSounds> {
            asked
                .iter()
                .map(|p| SaveSounds {
                    path: p.clone(),
                    sounds: Vec::new(),
                    error: Some("locked".into()),
                    warning: None,
                })
                .collect()
        };
        assert_eq!(store.saves(&paths, broken).unwrap().read, 1);
        assert_eq!(store.saves(&paths, reader).unwrap().read, 1);

        // A save on a drive that's away keeps what was found in it.
        let away = dir.join("away.flp").to_string_lossy().into_owned();
        let mut stamps = HashMap::new();
        stamps.insert(away.clone(), (1, 1));
        store.keep(&reader(std::slice::from_ref(&away)), &stamps).unwrap();
        let gone = store.saves(std::slice::from_ref(&away), broken).unwrap();
        assert_eq!(gone.read, 1);
        let rows: i64 = store.conn.query_row("SELECT count(*) FROM sounds", [], |r| r.get(0)).unwrap();
        assert_eq!(rows, 6);

        // A save no longer asked for is forgotten.
        store.keep_only(&paths[..1]).unwrap();
        let left: i64 = store.conn.query_row("SELECT count(*) FROM sound_reads", [], |r| r.get(0)).unwrap();
        let sounds: i64 = store.conn.query_row("SELECT count(*) FROM sounds", [], |r| r.get(0)).unwrap();
        assert_eq!((left, sounds), (1, 2));
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
