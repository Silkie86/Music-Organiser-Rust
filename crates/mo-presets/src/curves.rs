//! The curves library (Solomon, 2026-11-02): every shape a setting was moved in, gathered from songs, to look at on
//! the Presets page and drop onto a Bitwig automation lane as a curve file (`.bwcurve`).
//!
//! Where curves come from ([`SourceKind`]):
//! - FL Studio's edit events: knob moves recorded into a pattern, one curve per place the pattern plays.
//! - Bitwig's automation clips, on any setting's lane whose values run 0 to 1 (not the tempo lane, in BPM).
//! - Curve files you saved in Bitwig's library (a pack's curves are locked and left out).
//! - Plugins' LFO shapes: Serum 2's and Serum 1's shape files, and the graphs in LFOTool's presets
//!   ([`mo_formats::serum`]), Vital's shape files and the LFOs drawn in its presets ([`mo_formats::vital`]), and
//!   Duck's shape files and its presets' envelopes ([`mo_formats::duck`]). A shape repeats, so it's kept as one bar.
//!
//! Each shape is kept once ([`shape_id`]): two clips are the same curve when their points match once stretched to the
//! same length and height (to three decimals), wherever they sit, whichever song, track or knob they're on. So a clip
//! copied across songs, or one that's only longer, shows up once, with every place it came from. A flat line or a
//! clip with a single point isn't a curve and is left out; so are the songs in the export folder (the converter's
//! copies would flood the list: sly_17 alone has 204 clips of 3 shapes).
//!
//! What's found is kept in the app's own `curves.db` beside `library.db`, so the page shows it at once; a save is read
//! again only when it changes ([`CurvesDb::out_of_date`], [`CurvesDb::scan`]). Your names and hidden curves are kept
//! apart from what a scan finds, so a rescan never loses them. Curve files are written on demand ([`file`]) into the
//! app's own `Curves` folder; [`save_to_bitwig`] copies one into Bitwig's library under a new name, never over a file.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rayon::prelude::*;
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

use mo_formats::bitwig::{self, automation, curve};
use mo_formats::fl::automation::FlAutomation;
use mo_formats::fl::events;
use mo_formats::fl::song::read_song;
use mo_formats::{duck, serum, vital};

use crate::fixes::{FixesError, is_there, make_new, text_at};
use crate::{export, names};

/// The file's name in the app's data folder.
pub const FILE_NAME: &str = "curves.db";
/// The folder in the app's data folder that curve files are written to.
pub const FOLDER: &str = "Curves";

/// What [`save_to_bitwig`] puts after a curve file's name while it makes it in Bitwig's folder:
/// "Rise.bwcurve.mo-staged".
const STAGED: &str = ".mo-staged";

/// How old a file [`save_to_bitwig`] left in Bitwig's folder must be before it's taken as left behind by a save that
/// was stopped part way: a save takes far less, so one another copy of the app is making right now is left alone.
const STAGED_LEFT_BEHIND: Duration = Duration::from_secs(60);

/// The tables, as made in a new database; only ever added to.
const SCHEMA: &str = "\
    CREATE TABLE IF NOT EXISTS curve_reads (path TEXT PRIMARY KEY NOT NULL, modified INTEGER NOT NULL, \
        size INTEGER NOT NULL, version INTEGER NOT NULL);
    CREATE TABLE IF NOT EXISTS curve_places (id TEXT NOT NULL, path TEXT NOT NULL, name TEXT NOT NULL, \
        device TEXT NOT NULL, setting TEXT NOT NULL, at REAL NOT NULL, length REAL NOT NULL, points TEXT NOT NULL);
    CREATE INDEX IF NOT EXISTS curve_places_path ON curve_places (path);
    CREATE INDEX IF NOT EXISTS curve_places_id ON curve_places (id);
    CREATE TABLE IF NOT EXISTS your_curves (id TEXT PRIMARY KEY NOT NULL, name TEXT, hidden INTEGER NOT NULL DEFAULT 0);";

/// Where a curve was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SourceKind {
    /// Knob moves recorded into an FL Studio pattern (edit events).
    FlKnobMoves,
    /// An automation clip in a Bitwig project.
    BitwigClip,
    /// A curve file in Bitwig's library.
    BitwigCurve,
    /// A plugin's LFO shape: a shape file (Serum, Vital, Duck), or one drawn in a preset (LFOTool, Vital, Duck).
    PluginShape,
}

impl SourceKind {
    /// The kind of curves a file of this name gives, by its ending; None for one that gives none.
    pub fn of(path: &str) -> Option<SourceKind> {
        let lower = path.to_lowercase();
        if lower.ends_with(".flp") {
            Some(SourceKind::FlKnobMoves)
        } else if lower.ends_with(".bwproject") {
            Some(SourceKind::BitwigClip)
        } else if lower.ends_with(".bwcurve") {
            Some(SourceKind::BitwigCurve)
        } else if serum::is_shape_file(&lower) || vital::is_shape_file(&lower) || duck::is_shape_file(&lower) {
            Some(SourceKind::PluginShape)
        } else {
            None
        }
    }

    /// Whether curves of this kind come from a song (a place in it to show), rather than a library of shapes.
    pub fn is_song(self) -> bool {
        matches!(self, SourceKind::FlKnobMoves | SourceKind::BitwigClip)
    }
}

/// How long a plugin's LFO shape is kept as, in beats: one bar (a shape has no length of its own; it's as long as the
/// LFO's rate makes it).
pub const SHAPE_BEATS: f64 = 4.0;

/// Goes up by one whenever this file finds other curves in the same saves (its filters and shapes; rule 7).
const CURVES_VERSION: i64 = 1;

/// What reads a save's curves: this file's version and its DAW reader's (for Bitwig, its clip or curve reader's and
/// the decoder's, as the project reader's version follows it), so a change to any of them reads it again.
fn read_version(path: &str) -> i64 {
    let reader = match SourceKind::of(path) {
        Some(SourceKind::FlKnobMoves) => mo_formats::fl::VERSION,
        Some(SourceKind::BitwigClip) => automation::VERSION * 1000 + bitwig::VERSION,
        Some(SourceKind::BitwigCurve) => curve::VERSION * 1000 + bitwig::VERSION,
        Some(SourceKind::PluginShape) if vital::is_shape_file(path) => vital::VERSION,
        Some(SourceKind::PluginShape) if duck::is_shape_file(path) => duck::VERSION,
        Some(SourceKind::PluginShape) => serum::VERSION,
        None => 0,
    };
    CURVES_VERSION * 1_000_000_000 + i64::from(reader)
}

/// The song's name a save gives, to show: its file's name without its ending.
fn song_of(path: &str) -> String {
    Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// One place a curve was found.
#[derive(Clone, Debug, PartialEq)]
pub struct CurveSource {
    pub kind: SourceKind,
    /// The save or curve file.
    pub path: String,
    /// The song's name, to show (the file's name without its ending).
    pub song: String,
    /// The clip's own name, or "".
    pub name: String,
    /// What it moves: the plugin or device, and its setting.
    pub device: String,
    pub setting: String,
    /// Where it starts in the song, in beats (0 for a curve file).
    pub at: f64,
    /// When the save was last changed (seconds since 1970), for newest first.
    pub modified: i64,
}

/// A curve in the library.
#[derive(Clone, Debug, PartialEq)]
pub struct Curve {
    /// Its shape's fingerprint ([`shape_id`]).
    pub id: String,
    /// Your name for it, or one made from where it was first found.
    pub name: String,
    /// Its points: across (0 to 1), value (0 to 1), bend (-1 to 1), as the first place it was found has them.
    pub points: Vec<curve::CurvePoint>,
    /// How long it was where it was first found, in beats.
    pub length_beats: f64,
    pub hidden: bool,
    /// Every place it was found, newest save first.
    pub sources: Vec<CurveSource>,
}

/// `c` played at `speed` times its speed (0.5 half, 2 double), as "Other speeds too" sends it: the same points over
/// a length divided by `speed`, named for the speed, under an id of its own (so its curve file sits beside the
/// curve's own and neither replaces the other). `c` itself at a speed of 1 (or a broken one).
pub fn at_speed(c: &Curve, speed: f64) -> Curve {
    if speed == 1.0 || !speed.is_finite() || speed <= 0.0 {
        return c.clone();
    }
    let words = if speed < 1.0 { "half speed" } else { "double speed" };
    let words = if speed == 0.5 || speed == 2.0 { words.to_owned() } else { format!("{speed}x speed") };
    let mut h = Sha256::new();
    h.update(c.id.as_bytes());
    h.update(speed.to_le_bytes());
    let id = h.finalize().iter().take(16).map(|b| format!("{b:02x}")).collect();
    Curve { id, name: format!("{} ({words})", c.name), length_beats: c.length_beats / speed, ..c.clone() }
}

/// A shape's fingerprint: the points stretched to the same length and height and rounded to three decimals, hashed
/// (SHA-256, the first 16 bytes as hex), so the same shape gives the same id on every computer.
pub fn shape_id(points: &[curve::CurvePoint]) -> String {
    let (low, high) = points.iter().fold((f64::MAX, f64::MIN), |(l, h), p| (l.min(p.y), h.max(p.y)));
    let span = (high - low).max(f64::EPSILON);
    let round = |v: f64| (v * 1000.0).round() as i64;
    let mut h = Sha256::new();
    h.update(b"mo-presets curve 1\0");
    for p in points {
        for v in [round(p.x), round((p.y - low) / span), round(p.bend)] {
            h.update(v.to_le_bytes());
        }
    }
    h.finalize().iter().take(16).map(|b| format!("{b:02x}")).collect()
}

/// A run of moves as a curve: beats from its start become 0 to 1 across `length`, values are kept within 0 to 1, and
/// points past the end are dropped. None when it's no curve at all: fewer than two points, or a flat line.
pub fn shape(points: &[(f64, f64, f64)], length: f64) -> Option<Vec<curve::CurvePoint>> {
    let last = points.iter().map(|p| p.0).fold(0.0, f64::max);
    let length = if length.is_finite() && length > 0.0 { length } else { last };
    if length <= 0.0 {
        return None;
    }
    let mut out: Vec<curve::CurvePoint> = points
        .iter()
        .filter(|p| p.0.is_finite() && p.1.is_finite() && p.0 >= 0.0 && p.0 <= length + 1e-6)
        .map(|&(beat, value, bend)| curve::CurvePoint {
            x: (beat / length).clamp(0.0, 1.0),
            y: value.clamp(0.0, 1.0),
            bend: if bend.is_finite() { bend.clamp(-1.0, 1.0) } else { 0.0 },
        })
        .collect();
    out.sort_by(|a, b| a.x.total_cmp(&b.x));
    let (low, high) = out.iter().fold((f64::MAX, f64::MIN), |(l, h), p| (l.min(p.y), h.max(p.y)));
    (out.len() >= 2 && high - low >= 1e-3).then_some(out)
}

/// A curve found in one save, before it's grouped with the same shape found elsewhere.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub id: String,
    pub points: Vec<curve::CurvePoint>,
    pub length: f64,
    pub source: CurveSource,
}

/// The curves in one save or curve file (FL Studio, Bitwig project or Bitwig curve), from its bytes. Never panics on a
/// damaged file: it gives what it could read, or nothing.
pub fn found_in(path: &str, raw: Vec<u8>, modified: i64) -> Vec<Found> {
    let place = Place { path, song: song_of(path), modified };
    let kind = SourceKind::of(path);
    let read = mo_catch::catch(std::panic::AssertUnwindSafe(|| {
        if kind == Some(SourceKind::FlKnobMoves) {
            let Ok(flp) = events::load(&raw) else { return Vec::new() };
            let song = read_song(&flp);
            let plugin_of = |channel: Option<u32>| {
                channel
                    .and_then(|c| song.channels.iter().find(|ch| ch.index == c))
                    .and_then(|ch| ch.instrument.as_ref().map(|p| p.name.clone()))
                    .unwrap_or_default()
            };
            fl_curves(&song.automation, &plugin_of, &place)
        } else if kind == Some(SourceKind::BitwigCurve) {
            let Some(c) = curve::read(raw) else { return Vec::new() };
            let points: Vec<(f64, f64, f64)> = c.points.iter().map(|p| (p.x, p.y, p.bend)).collect();
            let source = place.source(SourceKind::BitwigCurve, &c.name, "", "", 0.0);
            found(shape(&points, 1.0), c.beat_length.unwrap_or(4.0), source).into_iter().collect()
        } else if kind == Some(SourceKind::PluginShape) {
            let shapes = if vital::is_shape_file(path) {
                vital::read(path, &raw)
            } else if duck::is_shape_file(path) {
                duck::read(path, &raw)
            } else {
                serum::read(path, &raw)
            };
            shapes
                .into_iter()
                .filter_map(|s| {
                    let points: Vec<(f64, f64, f64)> = s.points.iter().map(|p| (p.x, p.value, p.bend)).collect();
                    // A shape file's name is the shape's; a preset's graphs are named by the preset and the graph
                    // ("Sidechain-1 - LFOTool - Graph 3", "Wob - Vital - LFO 2"), so each has its own.
                    let name = if s.part.is_empty() { place.song.clone() } else { String::new() };
                    let source = place.source(SourceKind::PluginShape, &name, &s.product, &s.part, 0.0);
                    found(shape(&points, 1.0), SHAPE_BEATS, source)
                })
                .collect()
        } else if kind == Some(SourceKind::BitwigClip)
            && let Ok(doc) = bitwig::decode(raw)
            && doc.body_error.is_none()
        {
            bitwig_curves(&automation::clips(&doc), &place)
        } else {
            Vec::new()
        }
    }));
    read.unwrap_or_default()
}

/// The save curves are being found in.
struct Place<'a> {
    path: &'a str,
    song: String,
    modified: i64,
}

impl Place<'_> {
    fn source(&self, kind: SourceKind, name: &str, device: &str, setting: &str, at: f64) -> CurveSource {
        CurveSource {
            kind,
            path: self.path.to_owned(),
            song: self.song.clone(),
            name: name.to_owned(),
            device: device.to_owned(),
            setting: setting.to_owned(),
            at,
            modified: self.modified,
        }
    }
}

fn found(points: Option<Vec<curve::CurvePoint>>, length: f64, source: CurveSource) -> Option<Found> {
    let points = points?;
    Some(Found { id: shape_id(&points), points, length, source })
}

/// FL's edit events: one curve per place a pattern with knob moves plays (its moves are joined by straight lines, as
/// the converter carries them). `plugin_of` names the plugin on a channel.
fn fl_curves(automation: &[FlAutomation], plugin_of: &dyn Fn(Option<u32>) -> String, place: &Place<'_>) -> Vec<Found> {
    let mut out = Vec::new();
    for a in automation.iter().filter(|a| !a.parts.is_empty()) {
        let plugin = plugin_of(a.channel);
        let setting = format!("setting {}", a.parameter);
        for part in &a.parts {
            let points: Vec<(f64, f64, f64)> = part.points.iter().map(|p| (p.beat, p.value, 0.0)).collect();
            let source = place.source(SourceKind::FlKnobMoves, "", &plugin, &setting, part.start);
            out.extend(found(shape(&points, part.length), part.length, source));
        }
    }
    out
}

/// Bitwig's automation clips, on lanes whose values run 0 to 1 (a volume lane goes a little over 1, by Bitwig's
/// cube-root gain, and is scaled to fit; the tempo lane, in BPM, is left out).
fn bitwig_curves(clips: &[automation::AutomationClip], place: &Place<'_>) -> Vec<Found> {
    clips
        .iter()
        .filter(|c| c.points.iter().all(|p| p.value >= -1e-6 && p.value <= 1.26))
        .filter_map(|clip| {
            // A volume lane that goes over 0 dB is scaled down to fit, rather than cut flat at the top.
            let top = clip.points.iter().map(|p| p.value).fold(1.0, f64::max);
            let points: Vec<(f64, f64, f64)> = clip.points.iter().map(|p| (p.beat, p.value / top, p.bend)).collect();
            let source = place.source(SourceKind::BitwigClip, &clip.name, &clip.device, &clip.setting, clip.start);
            found(shape(&points, clip.length), clip.length, source)
        })
        .collect()
}

/// Whether a save is the converter's own copy (in the export folder), which the library leaves out. Big and small
/// letters count the same, as Windows takes them (and the Songs list).
pub fn is_export(path: &Path, export_root: Option<&Path>) -> bool {
    export_root.is_some_and(|root| {
        let mut path = path.components();
        root.components().all(|r| path.next().is_some_and(|p| p.as_os_str().eq_ignore_ascii_case(r.as_os_str())))
    })
}

/// What went wrong with `curves.db`.
#[derive(Debug)]
pub struct CurvesError(pub String);

impl std::fmt::Display for CurvesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "curves.db: {}", self.0)
    }
}

impl std::error::Error for CurvesError {}

impl From<rusqlite::Error> for CurvesError {
    fn from(e: rusqlite::Error) -> CurvesError {
        CurvesError(e.to_string())
    }
}

impl From<FixesError> for CurvesError {
    fn from(e: FixesError) -> CurvesError {
        CurvesError(e.0)
    }
}

/// How far a scan has got, for the page's header: saves read so far, of how many.
#[derive(Debug, Default)]
pub struct Progress {
    pub done: AtomicUsize,
    pub total: AtomicUsize,
}

/// What a look at a save on the disk found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Look {
    /// Its last change (seconds since 1970, before it for an older date) and size.
    There(i64, i64),
    /// Not there, though the folder it was in is: deleted, moved or renamed, so forgotten.
    Gone,
    /// Can't be looked at now (a drive unplugged, a network share away, no permission): what was found in it before
    /// is kept, and it's tried again next time.
    Away,
}

/// A save's [`Look`].
fn look(path: &Path) -> Look {
    match std::fs::metadata(path) {
        Ok(m) => {
            let since = |t: std::time::SystemTime| match t.duration_since(UNIX_EPOCH) {
                Ok(d) => d.as_secs() as i64,
                Err(before) => -(before.duration().as_secs() as i64),
            };
            match m.modified() {
                Ok(t) => Look::There(since(t), m.len() as i64),
                Err(_) => Look::Away,
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Only when its own folder is there: a drive that's unplugged (K:\\, or /Volumes/Music on a Mac, whose
            // /Volumes stays) takes the folder with it, and the save may be back. A whole folder deleted is forgotten
            // once the library stops listing its saves.
            match path.parent() {
                Some(folder) if !folder.as_os_str().is_empty() && folder.is_dir() => Look::Gone,
                _ => Look::Away,
            }
        }
        Err(_) => Look::Away,
    }
}

/// A save read in a scan: its path, date and size, and its curves.
type ReadSave = (String, (i64, i64), Vec<Found>);

/// The curves library, open on `curves.db`. Asks the disk, so never on the window's thread.
pub struct CurvesDb {
    conn: Connection,
}

impl CurvesDb {
    /// Opens the database at `path`, making it first when it isn't there (made beside it, checked, then put in place).
    pub fn open(path: &Path) -> Result<CurvesDb, CurvesError> {
        if !is_there(path)? {
            make_new(path, SCHEMA, "curve_places")?;
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(2))?;
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |r| r.get::<_, String>(0))?;
        conn.execute_batch(SCHEMA)?;
        Ok(CurvesDb { conn })
    }

    /// Opens the database at `path` only to read it, as the CLAP plugin does (it never writes the app's files): nothing
    /// is made or changed, and an error when it isn't there yet (the app makes it when its Curves are first opened).
    pub fn open_read_only(path: &Path) -> Result<CurvesDb, CurvesError> {
        if !is_there(path)? {
            return Err(CurvesError(format!("{} isn't there yet", path.display())));
        }
        let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(path, flags)?;
        conn.busy_timeout(Duration::from_secs(2))?;
        conn.pragma_update(None, "query_only", true)?;
        Ok(CurvesDb { conn })
    }

    /// Each save read before: its date, size and what read it.
    fn known(&self) -> Result<HashMap<String, (i64, i64, i64)>, CurvesError> {
        let mut st = self.conn.prepare("SELECT path, modified, size, version FROM curve_reads")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?, r.get(3)?))))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The saves among `paths` that changed since they were read (or were never read, or were read by an older
    /// reader), each once, and whether any read before are gone from `paths` or from the disk. Quick: it only looks at
    /// each file's date and size. A save that can't be looked at now isn't counted either way.
    pub fn out_of_date(&self, paths: &[String]) -> Result<(Vec<String>, bool), CurvesError> {
        let (changed, gone) = self.changed(paths)?;
        let gone_from_disk = changed.iter().any(|(_, now)| *now == Look::Gone);
        let changed = changed.into_iter().filter(|(_, now)| *now != Look::Gone).map(|(p, _)| p).collect();
        Ok((changed, !gone.is_empty() || gone_from_disk))
    }

    /// The changed saves with what a look at them found now (never [`Look::Away`]: those are kept as they are), and
    /// the saves no longer in `paths`.
    #[allow(clippy::type_complexity)]
    fn changed(&self, paths: &[String]) -> Result<(Vec<(String, Look)>, Vec<String>), CurvesError> {
        let known = self.known()?;
        let mut seen = HashSet::new();
        let changed = paths
            .iter()
            .filter(|p| seen.insert(p.as_str()))
            .map(|p| (p.clone(), look(Path::new(p))))
            .filter(|(p, now)| match (*now, known.get(p.as_str())) {
                (Look::Away, _) => false,
                (Look::Gone, before) => before.is_some(),
                (Look::There(modified, size), Some(&before)) => before != (modified, size, read_version(p)),
                (Look::There(..), None) => true,
            })
            .collect();
        let gone = known.into_keys().filter(|k| !seen.contains(k.as_str())).collect();
        Ok((changed, gone))
    }

    /// Reads the saves among `paths` that changed (over every core, at low priority as the caller's pool is), and
    /// forgets saves no longer in `paths` or no longer there. A save that can't be read or looked at now (locked by
    /// its DAW, a drive unplugged) keeps what was found in it before and is tried again next time. `progress` counts the
    /// saves read. Returns how many were read.
    pub fn scan(&mut self, paths: &[String], progress: &Progress) -> Result<usize, CurvesError> {
        let (changed, mut gone) = self.changed(paths)?;
        progress.total.store(changed.len(), Ordering::Relaxed);
        progress.done.store(0, Ordering::Relaxed);
        let results: Vec<Result<ReadSave, (String, bool)>> = changed
            .par_iter()
            .map(|(p, now)| {
                let read = match now {
                    Look::There(modified, size) => match std::fs::read(p) {
                        Ok(raw) => Ok((p.clone(), (*modified, *size), found_in(p, raw, *modified))),
                        // There but not readable: kept as it was.
                        Err(_) => Err((p.clone(), false)),
                    },
                    // Not there any more: forgotten.
                    Look::Gone | Look::Away => Err((p.clone(), *now == Look::Gone)),
                };
                progress.done.fetch_add(1, Ordering::Relaxed);
                read
            })
            .collect();
        let mut read = Vec::new();
        for r in results {
            match r {
                Ok(save) => read.push(save),
                Err((path, true)) => gone.push(path),
                Err(_) => {}
            }
        }
        let tx = self.conn.transaction()?;
        for p in gone.iter().chain(read.iter().map(|r| &r.0)) {
            tx.execute("DELETE FROM curve_places WHERE path = ?1", [p])?;
            tx.execute("DELETE FROM curve_reads WHERE path = ?1", [p])?;
        }
        for (path, (modified, size), found) in &read {
            tx.execute(
                "INSERT INTO curve_reads (path, modified, size, version) VALUES (?1, ?2, ?3, ?4)",
                params![path, modified, size, read_version(path)],
            )?;
            for f in found {
                let s = &f.source;
                tx.execute(
                    "INSERT INTO curve_places (id, path, name, device, setting, at, length, points) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![f.id, s.path, s.name, s.device, s.setting, s.at, f.length, points_text(&f.points)],
                )?;
            }
        }
        tx.commit()?;
        Ok(read.len())
    }

    /// Every curve, each with every place it was found (newest save first), in name order. A curve's points and length
    /// are its newest place's.
    pub fn list(&self) -> Result<Vec<Curve>, CurvesError> {
        self.curves(None)
    }

    /// One curve, by its id.
    pub fn get(&self, id: &str) -> Result<Option<Curve>, CurvesError> {
        Ok(self.curves(Some(id))?.into_iter().next())
    }

    fn curves(&self, only: Option<&str>) -> Result<Vec<Curve>, CurvesError> {
        // The save's date from curve_reads, the kind and song from the path (rule 23).
        let mut st = self.conn.prepare(
            "SELECT c.id, c.path, c.name, c.device, c.setting, c.at, c.length, c.points, s.modified \
             FROM curve_places c JOIN curve_reads s ON s.path = c.path \
             WHERE ?1 IS NULL OR c.id = ?1 ORDER BY s.modified DESC, c.path, c.at",
        )?;
        type Row = (String, Option<SourceKind>, CurveSource, f64, Option<String>);
        let rows = st.query_map([only], |r| {
            let path: String = r.get(1)?;
            Ok((
                r.get::<_, String>(0)?,
                SourceKind::of(&path),
                CurveSource {
                    kind: SourceKind::BitwigClip,
                    song: song_of(&path),
                    path,
                    name: r.get(2)?,
                    device: r.get(3)?,
                    setting: r.get(4)?,
                    at: r.get(5)?,
                    modified: r.get(8)?,
                },
                r.get::<_, f64>(6)?,
                text_at(r, 7)?,
            ))
        })?;
        let mut by_id: BTreeMap<String, Vec<(CurveSource, f64, Option<String>)>> = BTreeMap::new();
        for row in rows {
            let (id, kind, mut source, length, points): Row = row?;
            // A file of a kind a newer app reads is passed over.
            let Some(kind) = kind else { continue };
            source.kind = kind;
            by_id.entry(id).or_default().push((source, length, points));
        }
        let yours = self.yours(only)?;
        let mut out: Vec<Curve> = by_id
            .into_iter()
            .filter_map(|(id, found)| {
                let (first, length, points) = found.first()?;
                let points = parse_points(points.as_deref()?)?;
                let made = made_name(first);
                let length_beats = *length;
                let (name, hidden) = yours.get(&id).cloned().unwrap_or((None, false));
                Some(Curve {
                    name: name.unwrap_or(made),
                    id,
                    points,
                    length_beats,
                    hidden,
                    sources: found.into_iter().map(|(s, _, _)| s).collect(),
                })
            })
            .collect();
        out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.id.cmp(&b.id)));
        Ok(out)
    }

    fn yours(&self, only: Option<&str>) -> Result<HashMap<String, (Option<String>, bool)>, CurvesError> {
        let mut st = self.conn.prepare("SELECT id, name, hidden FROM your_curves WHERE ?1 IS NULL OR id = ?1")?;
        let rows =
            st.query_map([only], |r| Ok((r.get::<_, String>(0)?, (text_at(r, 1)?, r.get::<_, i64>(2)? != 0))))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Gives a curve your own name ("" goes back to the one made for it); returns it as it now is.
    pub fn rename(&self, id: &str, name: &str) -> Result<Option<Curve>, CurvesError> {
        let name = Some(name.trim()).filter(|n| !n.is_empty());
        self.conn.execute(
            "INSERT INTO your_curves (id, name) VALUES (?1, ?2) ON CONFLICT(id) DO UPDATE SET name = excluded.name",
            params![id, name],
        )?;
        self.get(id)
    }

    /// Hides a curve in the app, or shows it again (nothing on disk changes); returns it as it now is.
    pub fn set_hidden(&self, id: &str, hidden: bool) -> Result<Option<Curve>, CurvesError> {
        self.conn.execute(
            "INSERT INTO your_curves (id, hidden) VALUES (?1, ?2) ON CONFLICT(id) DO UPDATE SET hidden = excluded.hidden",
            params![id, i64::from(hidden)],
        )?;
        self.get(id)
    }

    /// Whether a curve with this id is found anywhere now (for a page holding an old id).
    pub fn has(&self, id: &str) -> Result<bool, CurvesError> {
        Ok(self.conn.query_row("SELECT 1 FROM curve_places WHERE id = ?1", [id], |_| Ok(())).optional()?.is_some())
    }
}

/// The name made for a curve from where it was found: its clip's name, else the song, plugin and setting.
fn made_name(s: &CurveSource) -> String {
    if !s.name.is_empty() {
        return s.name.clone();
    }
    [s.song.as_str(), s.device.as_str(), s.setting.as_str()]
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" - ")
}

fn points_text(points: &[curve::CurvePoint]) -> String {
    points.iter().map(|p| format!("{} {} {}", p.x, p.y, p.bend)).collect::<Vec<_>>().join(";")
}

fn parse_points(text: &str) -> Option<Vec<curve::CurvePoint>> {
    text.split(';')
        .map(|p| {
            let mut n = p.split(' ').map(|v| v.parse::<f64>().ok());
            Some(curve::CurvePoint { x: n.next()??, y: n.next()??, bend: n.next()?? })
        })
        .collect()
}

/// The curve file's bytes for `c`.
/// `c` as a Bitwig curve file's bytes (`.bwcurve`), as [`file`] writes it: what a drag out of the plugin carries.
pub fn curve_bytes(c: &Curve) -> Result<Vec<u8>, String> {
    let periodic =
        c.sources.iter().all(|s| !s.kind.is_song()) && c.points.first().map(|p| p.y) == c.points.last().map(|p| p.y);
    curve::write(&curve::Curve {
        name: c.name.clone(),
        creator: "Music Organiser".into(),
        comment: String::new(),
        category: if periodic { "Periodic" } else { "Sequence" }.into(),
        tags: String::new(),
        beat_length: Some(c.length_beats).filter(|l| *l > 0.0),
        points: c.points.clone(),
    })
    .ok_or_else(|| "This curve has no points".to_owned())
}

/// Whether a curve file already holds this curve as it is now (its points and length).
fn holds(path: &Path, c: &Curve) -> bool {
    std::fs::read(path)
        .ok()
        .and_then(curve::read)
        .is_some_and(|back| back.points == c.points && back.beat_length == Some(c.length_beats).filter(|l| *l > 0.0))
}

/// The curve file for `curve` in the app's own `Curves` folder (`folder`): written the first time it's asked for, and
/// again when the curve has changed since (copy, check, rename, as every file the app writes); returns its path.
/// Safe on the worker pool; never on the window's thread.
pub fn file(c: &Curve, folder: &Path) -> Result<PathBuf, String> {
    let id = c.id.get(..ID_IN_NAME).unwrap_or(&c.id);
    let path = folder.join(format!("{} {id}.{}", names::safe_file_name(&c.name), curve::EXTENSION));
    if holds(&path, c) {
        return Ok(path);
    }
    std::fs::create_dir_all(folder).map_err(|e| format!("Couldn't make the Curves folder: {e}"))?;
    export::write_checked(&path, &curve_bytes(c)?).map_err(|e| format!("Couldn't write the curve: {e}"))?;
    // The folder is a cache (rule 28): this curve's file under an older name (before it was renamed) goes. Names
    // that differ only in letter case are one file on Windows and a Mac, so that one stays.
    let own = path.file_name().map(|n| n.to_string_lossy().to_lowercase());
    for (old, of) in curve_files(folder) {
        if of == id && Some(old.to_string_lossy().to_lowercase()) != own {
            let _ = std::fs::remove_file(folder.join(old));
        }
    }
    Ok(path)
}

/// How much of a curve's id its file's name ends with: 16 hex digits, so two curves never share one.
const ID_IN_NAME: usize = 16;

/// The plain curve files in the app's Curves folder, by name, each with the curve id's start its name ends with
/// ("<name> <16 hex>.bwcurve", as [`file`] names them; 8 in the first test builds). Nothing else is listed.
fn curve_files(folder: &Path) -> Vec<(PathBuf, String)> {
    let Ok(entries) = std::fs::read_dir(folder) else { return Vec::new() };
    entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_owned();
            let stem = name.strip_suffix(&format!(".{}", curve::EXTENSION))?;
            let (_, id) = stem.rsplit_once(' ')?;
            let hex = (id.len() == ID_IN_NAME || id.len() == 8) && id.bytes().all(|b| b.is_ascii_hexdigit());
            hex.then(|| (PathBuf::from(&name), id.to_owned()))
        })
        .collect()
}

/// Copies `curve` into Bitwig's curves library (`folder`, such as `K:\Bitwig Studio\Library\Curves`) under its name,
/// with " (2)", " (3)"... when that name is taken: written and checked under a name of its own, then linked to the
/// real name, which never replaces a file (where the disk can't link, the file is made new, which can't either).
/// A save stopped part way (the app closed by force) leaves its own files there (`Rise.bwcurve.mo-staged`, and the
/// same with `.mo-part-<app>-<write>` after it while it's written): each save clears those a minute old or more, and
/// nothing else (rule 28). Returns the path written.
pub fn save_to_bitwig(c: &Curve, folder: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(folder).map_err(|e| format!("Couldn't make the folder: {e}"))?;
    let bytes = curve_bytes(c)?;
    let base = names::safe_file_name(&c.name);
    let base = if base.is_empty() { "Curve".to_owned() } else { base };
    let staged = folder.join(format!("{base}.{}{STAGED}", curve::EXTENSION));
    export::write_checked(&staged, &bytes).map_err(|e| format!("Couldn't write the curve: {e}"))?;
    let result = (|| {
        for n in 1..1000 {
            let name = if n == 1 { base.clone() } else { format!("{base} ({n})") };
            let path = folder.join(format!("{name}.{}", curve::EXTENSION));
            match std::fs::hard_link(&staged, &path) {
                Ok(()) => return Ok(path),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(_) => match new_file(&path, &bytes) {
                    Ok(true) => return Ok(path),
                    Ok(false) => continue,
                    Err(e) => return Err(format!("Couldn't save the curve: {e}")),
                },
            }
        }
        Err("Too many curves with that name in the folder".to_owned())
    })();
    let _ = std::fs::remove_file(&staged);
    // The file just saved says what time it is by the clock that stamps this folder's files (a network drive's own),
    // so a leftover's age is measured on that clock too.
    if let Ok(path) = &result
        && let Ok(now) = std::fs::metadata(path).and_then(|m| m.modified())
    {
        clear_staged(folder, now);
    }
    result
}

/// Clears what [`save_to_bitwig`] leaves in Bitwig's curves folder when it's stopped part way (the app closed by force,
/// or a power cut), once it's [`STAGED_LEFT_BEHIND`] older than `now` (the time by the clock that stamps this
/// folder's files). Only plain files right in this folder with a name [`is_staged`] knows (rule 28): never a folder or
/// a link of that name, nor anything else in your Bitwig library. One in use stays; on most drives a read-only one goes
/// too, which takes away only that name (a staged file can be a second name of your saved curve): your curve keeps
/// its own.
fn clear_staged(folder: &Path, now: SystemTime) {
    let Ok(entries) = std::fs::read_dir(folder) else { return };
    // A plain file (the entry itself, never followed: a link of that name is a link) left long enough ago.
    let left = |meta: &std::fs::Metadata| {
        meta.is_file()
            && meta
                .modified()
                .ok()
                .and_then(|t| now.duration_since(t).ok())
                .is_some_and(|age| age >= STAGED_LEFT_BEHIND)
    };
    for entry in entries.flatten() {
        // Only a name a save gives its files is looked at any further, so a big library costs one listing.
        if !entry.file_name().to_str().is_some_and(is_staged) {
            continue;
        }
        let path = entry.path();
        // Looked at again just before it goes: another copy of the app may be saving that name now.
        if std::fs::symlink_metadata(&path).is_ok_and(|meta| left(&meta)) {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Whether `name` is one [`save_to_bitwig`] gives a file on its way and removes when it's done: the curve under its
/// staged name ("Rise.bwcurve.mo-staged"), or that file while it's written ("Rise.bwcurve.mo-staged.mo-part-12-0",
/// [`export::write_checked`]'s name for it). Nothing else: never "Rise.bwcurve", "Rise.bwcurve.mo-staged.bak" or
/// "Rise.bwcurve.mo-part-12-0".
fn is_staged(name: &str) -> bool {
    let staged = |n: &str| {
        n.strip_suffix(STAGED)
            .and_then(|n| n.strip_suffix(curve::EXTENSION))
            .and_then(|n| n.strip_suffix('.'))
            .is_some_and(|base| !base.is_empty())
    };
    // `write_checked` writes under the name with ".mo-part-<app>-<write>" after it.
    staged(name) || (export::is_half_written(name) && name.rsplit_once(".mo-part-").is_some_and(|(s, _)| staged(s)))
}

/// Makes `path` new with `bytes` (false when it's taken), on the disk and checked before it's left there.
fn new_file(path: &Path, bytes: &[u8]) -> std::io::Result<bool> {
    use std::io::Write as _;
    let mut f = match std::fs::OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
        Err(e) => return Err(e),
    };
    let written = f.write_all(bytes).and_then(|()| f.sync_all());
    drop(f);
    if written.is_err() || std::fs::read(path).ok().as_deref() != Some(bytes) {
        // Ours, half made: it goes.
        let _ = std::fs::remove_file(path);
        return Err(written.err().unwrap_or_else(|| std::io::Error::other("it didn't read back as written")));
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64, bend: f64) -> curve::CurvePoint {
        curve::CurvePoint { x, y, bend }
    }

    #[test]
    fn the_converters_copies_are_known_whatever_their_letters() {
        let root = Path::new("/Music/Exports");
        assert!(is_export(Path::new("/music/EXPORTS/Night/Bitwig/Night.bwproject"), Some(root)));
        assert!(is_export(Path::new("/Music/Exports"), Some(root)));
        assert!(!is_export(Path::new("/Music/Exports 2/Night.bwproject"), Some(root)));
        assert!(!is_export(Path::new("/Music/Night.flp"), Some(root)));
        assert!(!is_export(Path::new("/Music/Exports/Night.flp"), None));
    }

    #[test]
    fn a_stretched_copy_is_the_same_curve_and_a_flat_line_none() {
        let short = shape(&[(0.0, 0.2, 0.0), (1.0, 0.4, 0.5), (2.0, 0.2, 0.0)], 2.0).unwrap();
        let long = shape(&[(0.0, 0.0, 0.0), (4.0, 1.0, 0.5), (8.0, 0.0, 0.0)], 8.0).unwrap();
        assert_eq!(shape_id(&short), shape_id(&long), "same shape, longer and taller");
        let other = shape(&[(0.0, 0.0, 0.0), (4.0, 1.0, -0.5), (8.0, 0.0, 0.0)], 8.0).unwrap();
        assert_ne!(shape_id(&long), shape_id(&other), "bent the other way");
        assert_eq!(short, [p(0.0, 0.2, 0.0), p(0.5, 0.4, 0.5), p(1.0, 0.2, 0.0)]);
        assert_eq!(shape(&[(0.0, 0.5, 0.0), (4.0, 0.5, 0.0)], 4.0), None);
        assert_eq!(shape(&[(0.0, 0.5, 0.0)], 4.0), None);
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mo-curves-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn library_curve(dir: &Path, file: &str, name: &str, ys: &[f64]) -> String {
        let n = ys.len() as f64 - 1.0;
        let points = ys.iter().enumerate().map(|(i, &y)| p(i as f64 / n, y, 0.0)).collect();
        let c = curve::Curve {
            name: name.into(),
            creator: "Solomon".into(),
            comment: String::new(),
            category: "Sequence".into(),
            tags: String::new(),
            beat_length: Some(4.0),
            points,
        };
        let path = dir.join(file);
        std::fs::write(&path, curve::write(&c).unwrap()).unwrap();
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn a_scan_keeps_each_shape_once_with_every_place_and_your_names_survive_a_rescan() {
        let dir = temp_dir("scan");
        let a = library_curve(&dir, "a.bwcurve", "Rise", &[0.0, 1.0, 0.0]);
        let b = library_curve(&dir, "b.bwcurve", "Rise again", &[0.2, 0.6, 0.2]);
        let c = library_curve(&dir, "c.bwcurve", "Fall", &[1.0, 0.0]);
        let mut db = CurvesDb::open(&dir.join(FILE_NAME)).unwrap();
        let progress = Progress::default();
        let paths = vec![a.clone(), b.clone(), c.clone()];
        assert_eq!(db.scan(&paths, &progress).unwrap(), 3);
        assert_eq!(progress.done.load(Ordering::Relaxed), 3);
        let list = db.list().unwrap();
        assert_eq!(list.len(), 2, "Rise and its taller copy are one curve");
        let rise = list.iter().find(|c| c.sources.len() == 2).unwrap();
        assert!(rise.name == "Rise" || rise.name == "Rise again");

        // Nothing changed: nothing to read again.
        assert_eq!(db.out_of_date(&paths).unwrap(), (Vec::new(), false));
        assert_eq!(db.scan(&paths, &progress).unwrap(), 0);

        let renamed = db.rename(&rise.id, "My rise").unwrap().unwrap();
        assert_eq!(renamed.name, "My rise");
        assert!(db.set_hidden(&rise.id, true).unwrap().unwrap().hidden);

        // A save left out of the list is forgotten; your name stays for when it comes back.
        db.scan(&[a.clone(), c.clone()], &progress).unwrap();
        let rise = db.get(&rise.id).unwrap().unwrap();
        assert_eq!((rise.name.as_str(), rise.hidden, rise.sources.len()), ("My rise", true, 1));

        // Written on demand, once; and into Bitwig's library without replacing a file.
        let curves = dir.join(FOLDER);
        let written = file(&rise, &curves).unwrap();
        assert_eq!(file(&rise, &curves).unwrap(), written);
        assert_eq!(curve::read(std::fs::read(&written).unwrap()).unwrap().points, rise.points);
        let library = dir.join("Library").join("Curves");
        let first = save_to_bitwig(&rise, &library).unwrap();
        let second = save_to_bitwig(&rise, &library).unwrap();
        assert_eq!(first.file_name().unwrap(), "My rise.bwcurve");
        assert_eq!(second.file_name().unwrap(), "My rise (2).bwcurve");
        // The plugin reads it without changing it: the same curves, and a write refused.
        let read_only = CurvesDb::open_read_only(&dir.join(FILE_NAME)).unwrap();
        assert_eq!(read_only.list().unwrap(), db.list().unwrap());
        assert!(read_only.rename(&rise.id, "Not mine").is_err());
        assert!(CurvesDb::open_read_only(&dir.join("none.db")).is_err());
        assert!(!dir.join("none.db").exists(), "nothing is made");
        assert_eq!(curve::read(curve_bytes(&rise).unwrap()).unwrap().points, rise.points);
        // Windows won't remove a folder while the database in it is open.
        drop(read_only);
        drop(db);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_save_to_bitwig_clears_what_one_stopped_part_way_left_and_nothing_else() {
        let dir = temp_dir("staged");
        let library = dir.join("Library").join("Curves");
        std::fs::create_dir_all(&library).unwrap();
        let hour_ago = SystemTime::now() - Duration::from_secs(3600);
        // Left an hour ago by a save of "Fall" that was stopped part way: the curve under its staged name, and the
        // same half written. (A number no process can have, so never this test's own.)
        let gone = ["Fall.bwcurve.mo-staged", "Fall.bwcurve.mo-staged.mo-part-9999999901-0"];
        // As old, but only named like them: yours.
        let yours = [
            "Fall.bwcurve",
            "Fall.bwcurve.mo-staged.bak",
            "Fall.bwcurve.mo-staged.mo-part-1-x",
            "Fall.bwcurve.mo-part-1-2",
            "Fall.mo-staged",
            ".bwcurve.mo-staged",
        ];
        for name in gone.iter().chain(&yours) {
            let path = library.join(name);
            std::fs::write(&path, b"left").unwrap();
            std::fs::File::options().write(true).open(&path).unwrap().set_modified(hour_ago).unwrap();
        }
        // Another copy of the app saving "Swell" right now.
        let being_made = ["Swell.bwcurve.mo-staged", "Swell.bwcurve.mo-staged.mo-part-9999999902-0"];
        for name in being_made {
            std::fs::write(library.join(name), b"being made").unwrap();
        }
        let rise = Curve {
            id: "ab".repeat(16),
            name: "Rise".into(),
            points: vec![p(0.0, 0.0, 0.0), p(1.0, 1.0, 0.0)],
            length_beats: 4.0,
            hidden: false,
            sources: Vec::new(),
        };
        assert_eq!(save_to_bitwig(&rise, &library).unwrap(), library.join("Rise.bwcurve"));
        let mut there: Vec<String> =
            std::fs::read_dir(&library).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        there.sort();
        let mut kept: Vec<&str> = yours.iter().chain(&being_made).copied().chain(["Rise.bwcurve"]).collect();
        kept.sort();
        assert_eq!(there, kept, "the save's leftovers gone, everything else as it was");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_a_plain_file_a_save_left_is_cleared_never_a_folder_or_a_link_of_that_name() {
        let dir = temp_dir("staged-links");
        let staged = dir.join("Fall.bwcurve.mo-staged");
        std::fs::write(&staged, b"left").unwrap();
        let folder = dir.join("Rise.bwcurve.mo-staged");
        std::fs::create_dir_all(&folder).unwrap();
        // Nothing in a folder inside is looked at.
        let inside = folder.join("Fall.bwcurve.mo-staged");
        std::fs::write(&inside, b"left").unwrap();
        // Links of those names, to a curve of yours elsewhere.
        #[cfg(unix)]
        let (links, yours) = {
            let yours = folder.join("Swell.bwcurve");
            std::fs::write(&yours, b"yours").unwrap();
            let links = [dir.join("Swell.bwcurve.mo-staged"), dir.join("Swell.bwcurve.mo-staged.mo-part-9999999901-0")];
            for link in &links {
                std::os::unix::fs::symlink(&yours, link).unwrap();
            }
            (links, yours)
        };
        // An hour on, each of them is old enough to go, were it a file a save left.
        clear_staged(&dir, SystemTime::now() + Duration::from_secs(3600));
        assert!(!staged.exists(), "a plain file a save left goes");
        assert!(folder.is_dir() && inside.exists(), "a folder of that name stays, and what's in it");
        #[cfg(unix)]
        {
            for link in &links {
                let link_there = std::fs::symlink_metadata(link).is_ok_and(|m| m.file_type().is_symlink());
                assert!(link_there, "{} stays", link.display());
            }
            assert_eq!(std::fs::read(&yours).unwrap(), b"yours", "and so does what they lead to");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn fl_knob_moves_give_a_curve_per_placement_and_bitwig_clips_one_each() {
        use mo_formats::fl::automation::{FlControl, FlPart, FlPoint};
        let place = Place { path: "sly_17.flp", song: "sly_17".into(), modified: 5 };
        let moves = vec![FlPoint { beat: 0.0, value: 0.2 }, FlPoint { beat: 2.0, value: 0.8 }];
        let part = |start| FlPart { start, length: 4.0, points: moves.clone() };
        let knob = FlAutomation {
            name: "Keys - setting 0 (edit events)".into(),
            channel: Some(3),
            control: FlControl::PluginParameter,
            parameter: 0,
            points: Vec::new(),
            insert: None,
            slot: None,
            starts: vec![64.0, 72.0],
            parts: vec![part(64.0), part(72.0)],
        };
        let plugin_of = |c: Option<u32>| if c == Some(3) { "LegacyCell".to_owned() } else { String::new() };
        let found = fl_curves(&[knob], &plugin_of, &place);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].id, found[1].id, "one shape, placed twice");
        let s = &found[1].source;
        assert_eq!(
            (s.device.as_str(), s.setting.as_str(), s.at, s.kind),
            ("LegacyCell", "setting 0", 72.0, SourceKind::FlKnobMoves)
        );
        assert_eq!(found[0].points, [p(0.0, 0.2, 0.0), p(0.5, 0.8, 0.0)]);
        assert_eq!(made_name(s), "sly_17 - LegacyCell - setting 0");

        let point = |beat, value, bend| automation::ClipPoint { beat, value, bend };
        let clip = |name: &str, values: [f64; 2]| automation::AutomationClip {
            name: name.into(),
            device: "Acid V".into(),
            setting: "Cutoff".into(),
            start: 28.0,
            length: 4.0,
            muted: false,
            points: vec![point(0.0, values[0], 0.82), point(4.0, values[1], 0.0)],
        };
        // The tempo lane, in BPM, isn't a curve.
        let found = bitwig_curves(&[clip("Basic Sequence #1", [0.0, 1.0]), clip("", [120.0, 140.0])], &place);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].points, [p(0.0, 0.0, 0.82), p(1.0, 1.0, 0.0)]);
        assert_eq!(made_name(&found[0].source), "Basic Sequence #1");
    }

    #[test]
    fn a_better_reader_reads_each_save_again() {
        let dir = temp_dir("version");
        let a = library_curve(&dir, "a.bwcurve", "Rise", &[0.0, 1.0, 0.0]);
        let mut db = CurvesDb::open(&dir.join(FILE_NAME)).unwrap();
        let progress = Progress::default();
        assert_eq!(db.scan(std::slice::from_ref(&a), &progress).unwrap(), 1);
        // As if an older reader had read it.
        db.conn.execute("UPDATE curve_reads SET version = 1", []).unwrap();
        assert_eq!(db.out_of_date(std::slice::from_ref(&a)).unwrap(), (vec![a.clone()], false));
        assert_eq!(db.scan(std::slice::from_ref(&a), &progress).unwrap(), 1);
        assert_eq!(db.out_of_date(std::slice::from_ref(&a)).unwrap(), (Vec::new(), false));
        // Kind and song come from the path, the date from the save.
        let s = &db.list().unwrap()[0].sources[0];
        assert_eq!((s.kind, s.song.as_str()), (SourceKind::BitwigCurve, "a"));
        drop(db);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_save_on_a_drive_that_isnt_there_keeps_its_curves_and_a_deleted_one_is_forgotten() {
        let dir = temp_dir("away");
        let a = library_curve(&dir, "a.bwcurve", "Rise", &[0.0, 1.0, 0.0]);
        let b = library_curve(&dir, "b.bwcurve", "Fall", &[1.0, 0.0]);
        let mut db = CurvesDb::open(&dir.join(FILE_NAME)).unwrap();
        let progress = Progress::default();
        db.scan(&[a.clone(), b.clone()], &progress).unwrap();
        // A save on a drive that's unplugged (its folder isn't there either): kept as it was. A folder of the test's
        // own, never a made-up drive or network share, so nothing is asked of the network.
        let away = dir.join("unplugged").join("Songs").join("c.bwcurve").to_string_lossy().into_owned();
        db.conn
            .execute(
                "INSERT INTO curve_reads (path, modified, size, version) VALUES (?1, 1, 1, ?2)",
                params![away, read_version(&away)],
            )
            .unwrap();
        assert_eq!(look(Path::new(&away)), Look::Away);
        std::fs::remove_file(&b).unwrap();
        assert_eq!(db.out_of_date(&[a.clone(), b.clone(), away.clone()]).unwrap(), (Vec::new(), true));
        db.scan(&[a.clone(), b.clone(), away.clone()], &progress).unwrap();
        let saves: Vec<String> = db.known().unwrap().into_keys().collect();
        assert!(saves.contains(&away) && saves.contains(&a) && !saves.contains(&b), "{saves:?}");
        assert_eq!(db.list().unwrap().len(), 1);
        drop(db);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_save_dated_before_1970_is_read_once() {
        let dir = temp_dir("old-date");
        let a = library_curve(&dir, "a.bwcurve", "Rise", &[0.0, 1.0, 0.0]);
        let before = UNIX_EPOCH - Duration::from_secs(86_400);
        std::fs::File::options().write(true).open(&a).unwrap().set_modified(before).unwrap();
        assert!(matches!(look(Path::new(&a)), Look::There(-86_400, _)));
        let mut db = CurvesDb::open(&dir.join(FILE_NAME)).unwrap();
        let progress = Progress::default();
        assert_eq!(db.scan(std::slice::from_ref(&a), &progress).unwrap(), 1);
        assert_eq!(db.scan(std::slice::from_ref(&a), &progress).unwrap(), 0);
        drop(db);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_curves_folder_keeps_one_file_a_curve() {
        let dir = temp_dir("tidy");
        let a = library_curve(&dir, "a.bwcurve", "Rise", &[0.0, 1.0, 0.0]);
        let mut db = CurvesDb::open(&dir.join(FILE_NAME)).unwrap();
        db.scan(std::slice::from_ref(&a), &Progress::default()).unwrap();
        let curves = dir.join(FOLDER);
        let rise = db.list().unwrap().remove(0);
        let first = file(&rise, &curves).unwrap();
        // Renamed: its file under the old name goes.
        let renamed = db.rename(&rise.id, "Filter rise").unwrap().unwrap();
        let second = file(&renamed, &curves).unwrap();
        assert!(!first.exists() && second.exists());
        drop(db);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_damaged_or_unknown_file_gives_no_curves() {
        assert!(found_in("x.flp", b"FLhd garbage".to_vec(), 0).is_empty());
        assert!(found_in("x.bwproject", b"BtWg garbage".to_vec(), 0).is_empty());
        assert!(found_in("x.txt", b"hello".to_vec(), 0).is_empty());
        assert!(found_in("x.shp", vec![0; 2056], 0).is_empty());
        assert!(found_in("x.XferShape", b"XferJson\0garbage".to_vec(), 0).is_empty());
    }

    /// A Serum 1 shape file (`.shp`): points in its pixels (388 across, 240 high, 0 at the top) with their bends.
    fn shp(points: &[(f64, f64, f64)]) -> Vec<u8> {
        let mut lists = [[0.0f64; 64]; 4];
        for (i, (x, y, c)) in points.iter().enumerate() {
            (lists[0][i], lists[1][i], lists[2][i]) = (*c, *x, *y);
        }
        let mut out: Vec<u8> = lists.iter().flatten().flat_map(|v| v.to_le_bytes()).collect();
        out.push(points.len() as u8);
        out.extend([0; 7]);
        out
    }

    #[test]
    fn a_plugin_shape_is_a_one_bar_curve_found_in_no_song() {
        let dir = temp_dir("shapes");
        // Serum's Triangle: bottom, top half way, and back down, kept once though it's in two files.
        let triangle = shp(&[(0.0, 240.0, 0.5), (194.0, 0.0, 0.5)]);
        let (a, b) = (dir.join("triangle.shp"), dir.join("Copy.SHP"));
        std::fs::write(&a, &triangle).unwrap();
        std::fs::write(&b, &triangle).unwrap();
        // A flat line isn't a curve.
        std::fs::write(dir.join("flat 25.shp"), shp(&[(0.0, 180.0, 0.5)])).unwrap();
        let paths: Vec<String> =
            ["triangle.shp", "Copy.SHP", "flat 25.shp"].map(|f| dir.join(f).to_string_lossy().into_owned()).into();
        let mut db = CurvesDb::open(&dir.join(FILE_NAME)).unwrap();
        assert_eq!(db.scan(&paths, &Progress::default()).unwrap(), 3);
        let list = db.list().unwrap();
        assert_eq!(list.len(), 1);
        let c = &list[0];
        assert_eq!(c.points, [p(0.0, 0.0, 0.0), p(0.5, 1.0, 0.0), p(1.0, 0.0, 0.0)]);
        assert_eq!(c.length_beats, SHAPE_BEATS);
        assert_eq!(c.sources.len(), 2);
        assert!(c.sources.iter().all(|s| s.kind == SourceKind::PluginShape && !s.kind.is_song()));
        assert!(c.sources.iter().all(|s| s.device == "Serum" && s.setting.is_empty()));
        assert!(["triangle", "Copy"].contains(&c.name.as_str()), "named after its file: {}", c.name);
        // Its curve file says it repeats.
        let file = file(c, &dir.join(FOLDER)).unwrap();
        assert_eq!(curve::read(std::fs::read(file).unwrap()).unwrap().category, "Periodic");
        drop(db);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn vitals_shapes_and_a_presets_drawn_lfos_are_plugin_shapes() {
        let dir = temp_dir("vital");
        // Vital's Saw Up, and a preset whose second LFO was drawn (the others are Vital's own triangle, passed over).
        let saw = r#"{"num_points":3,"points":[0.0,1.0,1.0,0.0,1.0,1.0],"powers":[0.0,0.0,0.0],"smooth":false}"#;
        let triangle =
            r#"{"name":"Triangle","num_points":3,"points":[0.0,1.0,0.5,0.0,1.0,1.0],"powers":[0.0,0.0,0.0]}"#;
        let drawn = r#"{"num_points":2,"points":[0.0,1.0,0.25,0.0],"powers":[0.0,0.0]}"#;
        let preset = format!(r#"{{"settings":{{"lfos":[{triangle},{drawn},{triangle}]}}}}"#);
        std::fs::write(dir.join("Saw Up.vitallfo"), saw).unwrap();
        std::fs::write(dir.join("Wob.vital"), preset).unwrap();
        let paths: Vec<String> =
            ["Saw Up.vitallfo", "Wob.vital"].map(|f| dir.join(f).to_string_lossy().into_owned()).into();
        assert!(paths.iter().all(|p| SourceKind::of(p) == Some(SourceKind::PluginShape)));
        assert_ne!(read_version(&paths[0]), 0);
        let mut db = CurvesDb::open(&dir.join(FILE_NAME)).unwrap();
        assert_eq!(db.scan(&paths, &Progress::default()).unwrap(), 2);
        let mut list = db.list().unwrap();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        let names: Vec<&str> = list.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Saw Up", "Wob - Vital - LFO 2"]);
        assert_eq!(list[0].points, [p(0.0, 0.0, 0.0), p(1.0, 1.0, 0.0), p(1.0, 0.0, 0.0)]);
        assert!(list.iter().all(|c| c.sources[0].device == "Vital" && c.length_beats == SHAPE_BEATS));
        drop(db);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ducks_shapes_and_preset_bands_are_plugin_shapes() {
        let dir = temp_dir("duck");
        let machines = dir.join("DeviousMachines").join("Duck 2");
        std::fs::create_dir_all(&machines).unwrap();
        let env = r#"<MultipointEnvelopeData><node x="0.0" value="0.0" curve="0.5" shape="0"/>
            <node x="0.5" value="1.0" curve="0.5" shape="0"/><node x="1.0" value="1.0" curve="0.5" shape="0"/>
            </MultipointEnvelopeData>"#;
        std::fs::write(machines.join("4.dmlfo"), env).unwrap();
        // The preset's band falls back half way instead: another shape.
        let band = env.replace(r#"x="1.0" value="1.0""#, r#"x="1.0" value="0.5""#);
        let preset = format!(
            r#"<state><parameters/><data><Duck2EnvelopeData><MultibandEnvelopeData>{band}</MultibandEnvelopeData>
            </Duck2EnvelopeData></data></state>"#
        );
        std::fs::write(machines.join("Pump.preset"), preset).unwrap();
        let paths: Vec<String> =
            ["4.dmlfo", "Pump.preset"].map(|f| machines.join(f).to_string_lossy().into_owned()).into();
        assert!(paths.iter().all(|p| SourceKind::of(p) == Some(SourceKind::PluginShape)));
        assert_eq!(SourceKind::of("C:/Presets/Lead.preset"), None, "another plugin's preset");
        let mut db = CurvesDb::open(&dir.join(FILE_NAME)).unwrap();
        assert_eq!(db.scan(&paths, &Progress::default()).unwrap(), 2);
        let mut names: Vec<String> = db.list().unwrap().into_iter().map(|c| c.name).collect();
        names.sort();
        assert_eq!(names, ["4", "Pump - Duck - Band 1"]);
        drop(db);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_curve_at_another_speed_is_stretched_under_its_own_id() {
        let c = Curve {
            id: "0123456789abcdef0123456789abcdef".into(),
            name: "Pump".into(),
            points: vec![
                curve::CurvePoint { x: 0.0, y: 1.0, bend: 0.0 },
                curve::CurvePoint { x: 1.0, y: 0.0, bend: 0.0 },
            ],
            length_beats: 1.0,
            hidden: false,
            sources: Vec::new(),
        };
        assert_eq!(at_speed(&c, 1.0), c);
        let half = at_speed(&c, 0.5);
        assert_eq!((half.name.as_str(), half.length_beats, &half.points), ("Pump (half speed)", 2.0, &c.points));
        assert_eq!(half.id.len(), 32);
        assert_ne!(half.id[..16], c.id[..16], "its file mustn't replace the curve's own");
        assert_ne!(at_speed(&c, 2.0).id, half.id);
        assert_eq!(at_speed(&c, 2.0).length_beats, 0.5);
    }
}
