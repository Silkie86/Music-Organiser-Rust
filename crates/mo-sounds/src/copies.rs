//! Sounds that are real copies of each other, for the Sounds page's "Hide duplicates": only sounds whose audio is
//! exactly the same, never ones that only sound alike (Solomon, 2026-11-05: "make it really strict so real duplicates
//! get hidden, not similar sounding").
//!
//! Two sounds are the same when their files decode to the very same audio: the same sample rate, the same number of
//! channels, the same number of frames and every sample the same, bit for bit. A file copied to another folder is
//! one, and so is the same audio saved again with other notes in the file (a renamed pack, a WAV with its tags
//! changed) or in another lossless format (WAV and FLAC). The same sound at another sample rate or bit depth, an MP3
//! of it, or a trimmed one, isn't: those may differ on purpose.
//!
//! Nothing here moves, deletes or hides a file (rule 2): the page only leaves the copies out of its list while the
//! switch is on.
//!
//! **How it stays quick.** Only sounds that could be copies are read at all: those whose analysis gives the same length
//! (to the millisecond), sample rate, channels and peak level as another's ([`candidates`]); identical audio always
//! does. Each one read is kept in `sound-copies.db` beside the library ([`FILE_NAME`]) with its file's size and when it
//! was last changed, so it's read again only when the file changes; a file no longer a candidate is forgotten, so the
//! file never holds more than the library's candidates (rule 28).

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Instant, UNIX_EPOCH};

use mo_audio::{AudioData, AudioError};
use rayon::prelude::*;
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};

use crate::{Sound, SoundsError};

/// The file the prints are kept in, beside the sound library.
pub const FILE_NAME: &str = "sound-copies.db";

/// Marks a file as `sound-copies.db` ("MOSC"), so another app's database is never taken for it.
const APP_ID: i32 = 0x4D4F_5343;

/// The file's layout, and how a print is made: a change to either starts the file afresh (its prints are only a
/// cache: they're made again as needed).
const LAYOUT: i32 = 1;

/// How many seconds of a WAV or AIFF are decoded at a time: a long file is gone through a part at a time, so it never
/// has to be held whole (about 11 MB a part for 48 kHz stereo).
const PART_SECONDS: f64 = 30.0;

/// The longest MP3, FLAC, OGG or similar decoded to compare its audio (they're decoded whole): a longer one is the
/// same as another only when the files are the same byte for byte. About 46 MB at 48 kHz stereo.
const MOST_DECODED_SECONDS: f64 = 120.0;

/// How many files are read between saves of what's been found, so a long first pass stopped half way (the app closed)
/// keeps most of its work.
const SAVE_EVERY: usize = 256;

/// A sound's print: SHA-256 of its decoded audio (with its sample rate, channels and length), or of the file's bytes
/// when its audio can't be decoded whole here. Two sounds with the same print are the same audio.
pub type Print = [u8; 32];

/// Groups of sounds (by id) whose audio is exactly the same, each of at least two, every group's ids in order and
/// the groups in the order of their first ids.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SameAudio {
    pub groups: Vec<Vec<i64>>,
}

impl SameAudio {
    /// How many sounds are copies of another (every group's sounds but one).
    pub fn copies(&self) -> usize {
        self.groups.iter().map(|g| g.len() - 1).sum()
    }

    fn of(prints: impl IntoIterator<Item = (i64, Print)>) -> SameAudio {
        let mut by: HashMap<Print, Vec<i64>> = HashMap::new();
        for (id, print) in prints {
            by.entry(print).or_default().push(id);
        }
        let mut groups: Vec<Vec<i64>> = by
            .into_values()
            .filter(|g| g.len() > 1)
            .map(|mut g| {
                g.sort_unstable();
                g.dedup();
                g
            })
            .filter(|g| g.len() > 1)
            .collect();
        groups.sort_unstable();
        SameAudio { groups }
    }
}

/// What the analysis says that identical audio always shares: length to the millisecond, sample rate, channels and
/// peak level to a tenth of a dB (rounded, so analyses made a little differently, by the C# app and this one, still
/// agree). Sounds not analysed have none and are never taken for copies.
type Key = (i64, i32, i32, i64);

fn key(s: &Sound) -> Option<Key> {
    let a = s.analysis.as_ref()?;
    let peak = if a.peak_db.is_finite() { (a.peak_db * 10.0).round() as i64 } else { i64::MIN };
    Some(((a.seconds * 1000.0).round() as i64, a.sample_rate, a.channel_count, peak))
}

/// The places in `sounds` of the ones that could be copies: their [`key`] is another sound's too. Every real copy is
/// one of these; most sounds aren't, so they're never read.
pub fn candidates(sounds: &[Sound]) -> Vec<usize> {
    let mut count: HashMap<Key, u32> = HashMap::new();
    for k in sounds.iter().filter_map(key) {
        *count.entry(k).or_default() += 1;
    }
    (0..sounds.len()).filter(|&i| key(&sounds[i]).is_some_and(|k| count[&k] > 1)).collect()
}

/// The print of the sound file at `path` ([`Print`]). An error only when the file couldn't be read (a drive gone, a
/// lock): a file that isn't a sound the app can decode is printed by its bytes.
pub fn print_of(path: &Path) -> Result<Print, AudioError> {
    match decoded_print(path) {
        Ok(Some(print)) => Ok(print),
        Ok(None) => bytes_print(path),
        Err(AudioError::Io(e)) => Err(AudioError::Io(e)),
        Err(_) => bytes_print(path),
    }
}

/// Puts decoded audio into the print: the frames in order, each channel's sample in turn, as little-endian floats,
/// through a small buffer (never a second copy of the whole sound).
fn add_frames(h: &mut Sha256, audio: &AudioData) {
    const FRAMES: usize = 4096;
    let mut buf = Vec::with_capacity(FRAMES * audio.channels.len() * 4);
    for from in (0..audio.len()).step_by(FRAMES) {
        buf.clear();
        for i in from..(from + FRAMES).min(audio.len()) {
            for ch in &audio.channels {
                buf.extend_from_slice(&ch[i].to_bits().to_le_bytes());
            }
        }
        h.update(&buf);
    }
}

/// The print of the file's decoded audio: `None` when it can't be decoded whole here (a long MP3 or similar), or its
/// samples or rate can't be read exactly ([`mo_audio::reads_exactly`]: 32-bit whole-number and 64-bit float WAVs).
fn decoded_print(path: &Path) -> Result<Option<Print>, AudioError> {
    let mut h = Sha256::new();
    h.update(b"MO same audio 1\0");
    let (rate, channels, frames) = if mo_audio::can_read_from(path) {
        if !mo_audio::reads_exactly(path) {
            return Ok(None);
        }
        let mut at = 0u64;
        let mut head: Option<(u32, usize)> = None;
        loop {
            let part = mo_audio::read_from(path, PART_SECONDS, at)?;
            let this = (part.sample_rate, part.channels.len());
            if *head.get_or_insert(this) != this {
                return Ok(None);
            }
            if part.is_empty() {
                break;
            }
            add_frames(&mut h, &part);
            at += part.len() as u64;
        }
        let (rate, channels) = head.unwrap_or_default();
        (rate, channels, at)
    } else {
        // Asked for a little more than the most, so a file longer than that is known to be.
        let audio = mo_audio::read(path, MOST_DECODED_SECONDS + 1.0)?;
        if audio.sample_rate == 0 || audio.seconds() > MOST_DECODED_SECONDS {
            return Ok(None);
        }
        add_frames(&mut h, &audio);
        (audio.sample_rate, audio.channels.len(), audio.len() as u64)
    };
    if frames == 0 {
        return Ok(None);
    }
    h.update(rate.to_le_bytes());
    h.update((channels as u64).to_le_bytes());
    h.update(frames.to_le_bytes());
    Ok(Some(h.finalize().into()))
}

/// The print of the file's bytes, read a megabyte at a time.
fn bytes_print(path: &Path) -> Result<Print, AudioError> {
    let mut h = Sha256::new();
    h.update(b"MO same file 1\0");
    let mut f = File::open(path)?;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().into())
}

/// A file as it was when printed: its size and when it was last changed (nanoseconds from 1970).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp {
    size: i64,
    changed: i64,
}

fn stamp_of(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok().filter(|m| m.is_file())?;
    let changed = meta.modified().ok()?;
    let changed = match changed.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_nanos()).unwrap_or(i64::MAX),
        Err(e) => -i64::try_from(e.duration().as_nanos()).unwrap_or(i64::MAX),
    };
    Some(Stamp { size: i64::try_from(meta.len()).unwrap_or(i64::MAX), changed })
}

/// `sound-copies.db`: each candidate's print, by path, with its file's stamp then.
struct Kept {
    conn: Connection,
}

fn open_error(path: &Path, e: impl std::fmt::Display) -> SoundsError {
    SoundsError::Open { path: path.to_owned(), why: e.to_string() }
}

impl Kept {
    /// Opens the file, making it when it isn't there. This app's file of another layout, or one that's damaged (or
    /// isn't a database at all), is made afresh: it only holds what can be worked out again. Another app's database,
    /// or a file that's only locked or can't be reached for now, is left as it is: the look fails this time.
    fn open(path: &Path) -> Result<Kept, SoundsError> {
        match Self::try_open(path) {
            Ok(Opened::Kept(kept)) => return Ok(kept),
            Ok(Opened::Remake) => {}
            Ok(Opened::Foreign) => return Err(open_error(path, "it's another app's database")),
            Err(e) => return Err(e),
        }
        for end in ["", "-journal", "-wal", "-shm"] {
            let file = with_end(path, end);
            if file.is_file() {
                std::fs::remove_file(&file).map_err(|e| open_error(&file, e))?;
            }
        }
        match Self::try_open(path)? {
            Opened::Kept(kept) => Ok(kept),
            _ => Err(open_error(path, "made afresh, but still not of this layout")),
        }
    }

    /// The file, if it's this app's of this layout (or new); else whether it may be made afresh.
    fn try_open(path: &Path) -> Result<Opened, SoundsError> {
        let conn = Connection::open(path).map_err(|e| open_error(path, e))?;
        conn.busy_timeout(std::time::Duration::from_secs(5)).map_err(SoundsError::Read)?;
        // Not a database (or a damaged one): SQLite says so on the first read.
        let damaged = |e: &rusqlite::Error| {
            matches!(
                e.sqlite_error_code(),
                Some(rusqlite::ErrorCode::NotADatabase | rusqlite::ErrorCode::DatabaseCorrupt)
            )
        };
        let id: i32 = match conn.query_row("PRAGMA application_id", [], |r| r.get(0)) {
            Ok(id) => id,
            Err(e) if damaged(&e) => return Ok(Opened::Remake),
            Err(e) => return Err(e.into()),
        };
        let layout: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        let tables: i64 = conn.query_row("SELECT COUNT(*) FROM sqlite_schema", [], |r| r.get(0))?;
        if id == 0 && layout == 0 && tables == 0 {
            conn.execute_batch(&format!(
                "PRAGMA application_id = {APP_ID};
                 PRAGMA user_version = {LAYOUT};
                 CREATE TABLE prints (path TEXT PRIMARY KEY, size INTEGER NOT NULL, changed INTEGER NOT NULL,
                                      print BLOB NOT NULL) WITHOUT ROWID;"
            ))?;
        } else if id != APP_ID {
            return Ok(Opened::Foreign);
        } else if layout != LAYOUT {
            return Ok(Opened::Remake);
        }
        let check: String = match conn.query_row("PRAGMA quick_check", [], |r| r.get(0)) {
            Ok(check) => check,
            Err(e) if damaged(&e) => return Ok(Opened::Remake),
            Err(e) => return Err(e.into()),
        };
        if check != "ok" {
            return Ok(Opened::Remake);
        }
        Ok(Opened::Kept(Kept { conn }))
    }

    fn known(&self) -> Result<HashMap<String, (Stamp, Print)>, SoundsError> {
        let mut st = self.conn.prepare("SELECT path, size, changed, print FROM prints")?;
        let rows = st.query_map([], |r| {
            let print: Vec<u8> = r.get(3)?;
            Ok((r.get::<_, String>(0)?, Stamp { size: r.get(1)?, changed: r.get(2)? }, print))
        })?;
        let mut known = HashMap::new();
        for row in rows {
            let (path, stamp, print) = row?;
            // A print of another length isn't one this app made: it's read again.
            if let Ok(print) = Print::try_from(print.as_slice()) {
                known.insert(path, (stamp, print));
            }
        }
        Ok(known)
    }

    fn put(&mut self, rows: &[(&str, Stamp, Print)]) -> Result<(), SoundsError> {
        let tx = self.conn.transaction()?;
        {
            let mut st = tx.prepare_cached(
                "INSERT INTO prints (path, size, changed, print) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(path) DO UPDATE SET size = ?2, changed = ?3, print = ?4",
            )?;
            for (path, stamp, print) in rows {
                st.execute(params![path, stamp.size, stamp.changed, &print[..]])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Forgets every file but `keep`.
    fn keep_only(&mut self, keep: &HashSet<&str>) -> Result<(), SoundsError> {
        let all: Vec<String> = {
            let mut st = self.conn.prepare("SELECT path FROM prints")?;
            st.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?
        };
        let gone: Vec<&String> = all.iter().filter(|p| !keep.contains(p.as_str())).collect();
        if gone.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        {
            let mut st = tx.prepare_cached("DELETE FROM prints WHERE path = ?1")?;
            for path in gone {
                st.execute([path])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
}

/// What [`Kept::try_open`] found.
enum Opened {
    Kept(Kept),
    /// This app's file of another layout, or damaged: it may be made afresh.
    Remake,
    /// Another app's database: never touched.
    Foreign,
}

/// `path` with `end` added to its name.
fn with_end(path: &Path, end: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(end);
    PathBuf::from(name)
}

/// How far a [`find`] has got.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Finding {
    /// The copies already known from `sound-copies.db` (files unchanged since they were read), before any file is
    /// read: `left` files are still to be read.
    Known { same: SameAudio, left: usize },
    /// `done` of `of` files read.
    Reading { done: usize, of: usize },
}

/// Finds the sounds in `sounds` whose audio is exactly the same, keeping what it reads in `kept_at`
/// (`sound-copies.db`). Reads in parallel on the pool it's called on (call it on the low-priority pool, never on the
/// window's thread). `said` is told how far it's got: first what was known already, then every second or so how many
/// files are read. `stop` ends it early: what's been read is kept, and the answer is what's known so far.
pub fn find(
    sounds: &[Sound],
    kept_at: &Path,
    stop: &AtomicBool,
    mut said: impl FnMut(Finding),
) -> Result<SameAudio, SoundsError> {
    let candidates = candidates(sounds);
    // Each one's file as it is now: one that can't be looked at (a drive not plugged in) isn't taken for a copy.
    let stamps: Vec<Option<Stamp>> = candidates.par_iter().map(|&i| stamp_of(Path::new(&sounds[i].path))).collect();
    let mut kept = Kept::open(kept_at)?;
    let known = kept.known()?;
    let mut prints: Vec<(i64, Print)> = Vec::with_capacity(candidates.len());
    let mut to_read: Vec<(usize, Stamp)> = Vec::new();
    for (&i, stamp) in candidates.iter().zip(&stamps) {
        let Some(stamp) = *stamp else { continue };
        let s = &sounds[i];
        match known.get(s.path.as_str()) {
            Some((was, print)) if *was == stamp => prints.push((s.id, *print)),
            _ => to_read.push((i, stamp)),
        }
    }
    drop(known);
    said(Finding::Known { same: SameAudio::of(prints.iter().copied()), left: to_read.len() });

    let of = to_read.len();
    let mut done = 0;
    let mut told = Instant::now();
    for part in to_read.chunks(SAVE_EVERY) {
        if stop.load(Ordering::Relaxed) {
            return Ok(SameAudio::of(prints));
        }
        let read: Vec<(usize, Stamp, Print)> = part
            .par_iter()
            .filter_map(|&(i, stamp)| {
                if stop.load(Ordering::Relaxed) {
                    return None;
                }
                let path = Path::new(&sounds[i].path);
                let print = mo_catch::catch(|| print_of(path)).ok()?.ok()?;
                // Changed while it was read: read again next time.
                (stamp_of(path) == Some(stamp)).then_some((i, stamp, print))
            })
            .collect();
        let rows: Vec<(&str, Stamp, Print)> =
            read.iter().map(|&(i, stamp, print)| (sounds[i].path.as_str(), stamp, print)).collect();
        kept.put(&rows)?;
        prints.extend(read.iter().map(|&(i, _, print)| (sounds[i].id, print)));
        done += part.len();
        if told.elapsed().as_secs_f64() >= 1.0 || done == of {
            said(Finding::Reading { done, of });
            told = Instant::now();
        }
    }
    if !stop.load(Ordering::Relaxed) {
        let keep: HashSet<&str> = candidates.iter().map(|&i| sounds[i].path.as_str()).collect();
        kept.keep_only(&keep)?;
    }
    Ok(SameAudio::of(prints))
}
