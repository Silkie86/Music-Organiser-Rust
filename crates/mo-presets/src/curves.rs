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
//! again only when it changes ([`CurvesDb::out_of_date`], [`CurvesDb::scan`]). A curve's points are kept once
//! (`curve_shapes`) however many places it was found in (`curve_places`): Solomon's `curves.db` was 857 MB with the
//! points kept with each of 51,049 places, about 2,300 different ones among them (19 Nov 2026; rule 23). One of that older
//! layout is moved into this one the first time it's opened ([`to_shapes`]). Your names and hidden curves are kept
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

/// The layout's number, kept in the file (`user_version`). 2: each curve's points once, in `curve_shapes` (the first
/// layout, with no number, kept them with every place). A file with a higher number is a newer app's, and is refused.
const VERSION: i64 = 2;

/// The tables, as made in a new database. `curve_shapes`: each curve as found (its id, [`shape_id`], and the points
/// it had there), once. `curve_places`: every place one was found, and how long it was there.
const SCHEMA: &str = "\
    CREATE TABLE IF NOT EXISTS curve_reads (path TEXT PRIMARY KEY NOT NULL, modified INTEGER NOT NULL, \
        size INTEGER NOT NULL, version INTEGER NOT NULL);
    CREATE TABLE IF NOT EXISTS curve_shapes (n INTEGER PRIMARY KEY, id TEXT NOT NULL, points TEXT NOT NULL);
    CREATE INDEX IF NOT EXISTS curve_shapes_id ON curve_shapes (id);
    CREATE TABLE IF NOT EXISTS curve_places (shape INTEGER NOT NULL, path TEXT NOT NULL, name TEXT NOT NULL, \
        device TEXT NOT NULL, setting TEXT NOT NULL, at REAL NOT NULL, length REAL NOT NULL);
    CREATE INDEX IF NOT EXISTS curve_places_path ON curve_places (path);
    CREATE INDEX IF NOT EXISTS curve_places_shape ON curve_places (shape);
    CREATE TABLE IF NOT EXISTS your_curves (id TEXT PRIMARY KEY NOT NULL, name TEXT, hidden INTEGER NOT NULL DEFAULT 0);
    CREATE TABLE IF NOT EXISTS made_curves (id TEXT PRIMARY KEY NOT NULL, name TEXT NOT NULL, length REAL NOT NULL, \
        points TEXT NOT NULL, made INTEGER NOT NULL);";

/// The most of the file that's kept for its free room to be given back at once: SQLite makes the new file in memory
/// first (the system's temp folder may be on a full drive), so a bigger one keeps its free room and fills it again.
const MOST_REWRITTEN: i64 = 256 * 1024 * 1024;

/// How long the move to this layout waits for another copy of the app that is writing the file.
const MOVE_WAIT: Duration = Duration::from_secs(30);

/// Free room in the file worth giving back to the disk ([`CurvesDb::give_back_room`]): this much, and half the file.
const ROOM_WORTH_GIVING_BACK: i64 = 16 * 1024 * 1024;

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
    /// Made in the app's curve editor and saved as a new curve: kept in `curves.db` itself, in no file.
    Made,
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
const CURVES_VERSION: i64 = 2;

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
        // No file gives a made curve: it's kept in the database itself.
        Some(SourceKind::Made) | None => 0,
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
    let id = id_at_speed(&c.id, speed);
    Curve { id, name: format!("{} ({words})", c.name), length_beats: c.length_beats / speed, ..c.clone() }
}

/// The id [`at_speed`] gives the curve with id `id` at `speed`: `id` itself at a speed of 1 (or a broken one).
pub fn id_at_speed(id: &str, speed: f64) -> String {
    if speed == 1.0 || !speed.is_finite() || speed <= 0.0 {
        return id.to_owned();
    }
    let mut h = Sha256::new();
    h.update(id.as_bytes());
    h.update(speed.to_le_bytes());
    h.finalize().iter().take(16).map(|b| format!("{b:02x}")).collect()
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

/// The id of a curve made in the editor: its points as they are (not stretched to one height, as [`shape_id`]'s
/// are) and its length, hashed. The same curve saved twice is one curve; one that only differs from a found curve in
/// its height or length is still a curve of its own.
pub fn made_id(points: &[curve::CurvePoint], length_beats: f64) -> String {
    // To a millionth: two curves the eye can't tell apart are one, and one bent a little is another.
    let round = |v: f64| (v * 1_000_000.0).round() as i64;
    let mut h = Sha256::new();
    h.update(b"mo-presets made curve 1\0");
    h.update(round(length_beats).to_le_bytes());
    for p in points {
        for v in [round(p.x), round(p.y), round(p.bend)] {
            h.update(v.to_le_bytes());
        }
    }
    h.finalize().iter().take(16).map(|b| format!("{b:02x}")).collect()
}

/// A run of moves as a curve: beats from its start become 0 to 1 across `length`, values are kept within 0 to 1, and
/// points past the end are dropped. None when it's no curve at all: fewer than two points, or a flat line.
pub fn shape(points: &[(f64, f64, f64)], length: f64) -> Option<Vec<curve::CurvePoint>> {
    let length = span_of(points, length);
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

/// The longest curve kept at its own length, in beats (25,000 bars): a longer one is a damaged number.
const LONGEST: f64 = 100_000.0;

/// How long a run of moves is, in beats: the `length` its save gives, or with a damaged one (not a number, none or
/// less, absurdly long) as far as its last move; 0 when that's no good either. [`shape`] spreads the points over
/// this, and the curve is kept this long.
fn span_of(points: &[(f64, f64, f64)], length: f64) -> f64 {
    let good = |l: f64| l.is_finite() && l > 0.0 && l <= LONGEST;
    if good(length) {
        return length;
    }
    let last = points.iter().map(|p| p.0).filter(|b| b.is_finite()).fold(0.0, f64::max);
    if good(last) { last } else { 0.0 }
}

/// A curve found, with its length and where it starts as the library can keep them: a damaged number in a save
/// (not a number at all, none or less, or absurdly long) gives 4 beats, or a start at 0. SQLite keeps "not a number"
/// as nothing, which the places table refuses: one such clip stopped every later scan from being kept.
fn found(points: Option<Vec<curve::CurvePoint>>, length: f64, mut source: CurveSource) -> Option<Found> {
    let points = points?;
    let length = if length.is_finite() && length > 0.0 && length <= LONGEST { length } else { 4.0 };
    if !source.at.is_finite() {
        source.at = 0.0;
    }
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
            out.extend(found(shape(&points, part.length), span_of(&points, part.length), source));
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
            found(shape(&points, clip.length), span_of(&points, clip.length), source)
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
    /// What opening it did to the file ([`CurvesDb::notes`]).
    notes: Vec<String>,
}

/// Whether `conn`'s database keeps the points with each place, as the first layout did.
fn is_older_layout(conn: &Connection) -> Result<bool, CurvesError> {
    let with_points: i64 =
        conn.query_row("SELECT COUNT(*) FROM pragma_table_info('curve_places') WHERE name = 'points'", [], |r| {
            r.get(0)
        })?;
    Ok(with_points > 0)
}

/// A cell as the file keeps it, whatever its type and whether or not its text is good UTF-8: the move to this layout
/// carries a damaged line over as damaged as it was.
#[derive(Clone, PartialEq)]
enum Cell {
    Null,
    Integer(i64),
    Real(f64),
    Text(Vec<u8>),
    Blob(Vec<u8>),
}

impl Cell {
    fn at(row: &rusqlite::Row<'_>, at: usize) -> rusqlite::Result<Cell> {
        use rusqlite::types::ValueRef;
        Ok(match row.get_ref(at)? {
            ValueRef::Null => Cell::Null,
            ValueRef::Integer(n) => Cell::Integer(n),
            ValueRef::Real(n) => Cell::Real(n),
            ValueRef::Text(t) => Cell::Text(t.to_vec()),
            ValueRef::Blob(b) => Cell::Blob(b.to_vec()),
        })
    }
}

impl rusqlite::ToSql for Cell {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        use rusqlite::types::{ToSqlOutput, ValueRef};
        Ok(ToSqlOutput::Borrowed(match self {
            Cell::Null => ValueRef::Null,
            Cell::Integer(n) => ValueRef::Integer(*n),
            Cell::Real(n) => ValueRef::Real(*n),
            Cell::Text(t) => ValueRef::Text(t),
            Cell::Blob(b) => ValueRef::Blob(b),
        }))
    }
}

/// Moves a database of the first layout (the points kept with every place) into this one, where each curve as found
/// is kept once: how many places it has, and how many curves as found; None when it's moved already (another copy of
/// the app did it while this one waited). In one transaction, checked before it's kept: every place must come back
/// with the id, length and points it had, or nothing is changed (and the error says so). A power cut or a crash part
/// way leaves the file as it was, SQLite's own way, and the next opening moves it. Nothing is read from your saves
/// again. The old points' room stays free inside the file until [`CurvesDb::give_back_room`].
fn to_shapes(conn: &mut Connection) -> Result<Option<(i64, i64)>, CurvesError> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    // Looked at again now that nothing else can write.
    if !is_older_layout(&tx)? {
        return Ok(None);
    }
    tx.execute_batch(
        "CREATE TABLE curve_shapes (n INTEGER PRIMARY KEY, id TEXT NOT NULL, points TEXT NOT NULL);
         CREATE INDEX curve_shapes_id ON curve_shapes (id);
         CREATE TABLE curve_places_now (shape INTEGER NOT NULL, path TEXT NOT NULL, name TEXT NOT NULL, \
             device TEXT NOT NULL, setting TEXT NOT NULL, at REAL NOT NULL, length REAL NOT NULL);",
    )?;
    {
        // One place at a time, the places of one id together (by the id's index: nothing is sorted, and the points
        // of one curve at most are in memory at once), each given the shape it matches or a new one.
        let mut places = tx.prepare(
            "SELECT rowid, id, path, name, device, setting, at, length, points FROM curve_places ORDER BY id",
        )?;
        let mut new_shape = tx.prepare("INSERT INTO curve_shapes (id, points) VALUES (?1, ?2)")?;
        let mut new_place = tx.prepare(
            "INSERT INTO curve_places_now (rowid, shape, path, name, device, setting, at, length) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        let mut of_id: Option<Cell> = None;
        let mut shapes: Vec<(Cell, i64)> = Vec::new();
        let mut rows = places.query([])?;
        while let Some(r) = rows.next()? {
            let cell = |at: usize| Cell::at(r, at);
            let (id, points) = (cell(1)?, cell(8)?);
            if of_id.as_ref() != Some(&id) {
                shapes.clear();
                of_id = Some(id.clone());
            }
            let shape = match shapes.iter().find(|(p, _)| *p == points) {
                Some((_, n)) => *n,
                None => {
                    let n = new_shape.insert(params![id, points])?;
                    shapes.push((points, n));
                    n
                }
            };
            new_place.execute(params![
                r.get::<_, i64>(0)?,
                shape,
                cell(2)?,
                cell(3)?,
                cell(4)?,
                cell(5)?,
                cell(6)?,
                cell(7)?
            ])?;
        }
    }
    // Checked against the places as they were, each by its own line: all of them, each with what it had.
    let (were, same): (i64, i64) = tx.query_row(
        "SELECT (SELECT COUNT(*) FROM curve_places), \
                (SELECT COUNT(*) FROM curve_places o JOIN curve_places_now p ON p.rowid = o.rowid \
                     JOIN curve_shapes s ON s.n = p.shape \
                 WHERE s.id IS o.id AND p.length IS o.length AND s.points IS o.points AND p.path IS o.path \
                     AND p.name IS o.name AND p.device IS o.device AND p.setting IS o.setting AND p.at IS o.at)",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let now: i64 = tx.query_row("SELECT COUNT(*) FROM curve_places_now", [], |r| r.get(0))?;
    if were != same || were != now {
        return Err(CurvesError(format!(
            "couldn't keep each curve's points once: {same} of {were} places came back the same, so nothing was changed"
        )));
    }
    let shapes: i64 = tx.query_row("SELECT COUNT(*) FROM curve_shapes", [], |r| r.get(0))?;
    tx.execute_batch(
        "DROP TABLE curve_places;
         ALTER TABLE curve_places_now RENAME TO curve_places;
         CREATE INDEX curve_places_path ON curve_places (path);
         CREATE INDEX curve_places_shape ON curve_places (shape);",
    )?;
    tx.pragma_update(None, "user_version", VERSION)?;
    tx.commit()?;
    Ok(Some((were, shapes)))
}

/// The layout number `conn`'s file carries; an error for a newer app's.
fn version_of(conn: &Connection) -> Result<i64, CurvesError> {
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version > VERSION {
        return Err(CurvesError("it was made by a newer version of the app: update this one to read it".into()));
    }
    Ok(version)
}

impl CurvesDb {
    /// Opens the database at `path`, making it first when it isn't there (made beside it, checked, then put in place).
    pub fn open(path: &Path) -> Result<CurvesDb, CurvesError> {
        if !is_there(path)? {
            make_new(path, SCHEMA, "curve_places")?;
        }
        let mut conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(2))?;
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |r| r.get::<_, String>(0))?;
        // Sorting the places for a listing stays in memory, not in the system's temp folder on a full drive.
        conn.pragma_update(None, "temp_store", "MEMORY")?;
        let version = version_of(&conn)?;
        let mut notes = Vec::new();
        if is_older_layout(&conn)? {
            // Another copy of the app may be writing its scan, or moving the file itself: waited for.
            conn.busy_timeout(MOVE_WAIT)?;
            let moved = to_shapes(&mut conn);
            conn.busy_timeout(Duration::from_secs(2))?;
            if let Some((places, shapes)) = moved? {
                notes.push(format!(
                    "Moved curves.db to keeping each curve's points once: {places} places of {shapes} curves as found"
                ));
            }
        }
        conn.execute_batch(SCHEMA)?;
        if version != VERSION {
            conn.pragma_update(None, "user_version", VERSION)?;
        }
        Ok(CurvesDb { conn, notes })
    }

    /// What opening it (and [`CurvesDb::give_back_room`]) did to the file, in words for the log: nothing, most times.
    pub fn notes(&self) -> &[String] {
        &self.notes
    }

    /// Gives the file's free room back to the disk when there's a lot of it (after the move to this layout, which
    /// leaves the old points' room free inside the file): SQLite writes the file afresh, small, its own all-or-nothing
    /// way. The page's first read calls it, off the window's thread; what it did is added to [`CurvesDb::notes`].
    /// While something else reads the file (the plugin in a DAW) the disk gets the room once that has finished.
    pub fn give_back_room(&mut self) {
        let number = |name: &str| self.conn.pragma_query_value(None, name, |r| r.get::<_, i64>(0)).unwrap_or(0);
        let (free, all, page) = (number("freelist_count"), number("page_count"), number("page_size"));
        if free.saturating_mul(page) < ROOM_WORTH_GIVING_BACK || free.saturating_mul(2) < all {
            return;
        }
        let mb = |bytes: i64| bytes / 1_048_576;
        let kept = (all - free).saturating_mul(page);
        if kept > MOST_REWRITTEN {
            self.notes.push(format!(
                "curves.db keeps its {} MB of free room: new curves fill it (the {} MB kept are too much to write afresh)",
                mb(free.saturating_mul(page)),
                mb(kept)
            ));
            return;
        }
        // The new file is made in memory (`temp_store`, set at opening), then written over the old one.
        let given = self.conn.execute_batch("VACUUM").and_then(|()| {
            // Brings the write-ahead file back into the file and to nothing: held up while something else reads.
            self.conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get::<_, i64>(0))
        });
        let was = mb(all.saturating_mul(page));
        match given {
            Ok(0) => {
                let on_disk = self.conn.path().and_then(|p| std::fs::metadata(p).ok()).map(|m| m.len() / 1_048_576);
                let now = on_disk
                    .map_or_else(|| mb(number("page_count").saturating_mul(page)).to_string(), |n| n.to_string());
                self.notes.push(format!("curves.db is {now} MB now (it was {was} MB)"));
            }
            Ok(_) => self.notes.push(format!(
                "curves.db is written afresh, small (it was {was} MB): the disk gets the room back once nothing else \
                 is reading it"
            )),
            Err(e) => self.notes.push(format!("curves.db's free room stays for now ({e})")),
        }
    }

    /// Opens the database at `path` to keep a curve made in the CLAP plugin's editor ([`CurvesDb::save_made`]), the one
    /// thing the plugin writes in the app's files (Solomon, 20 Nov 2026: "Yes plugin too"). Nothing is made or moved
    /// here: an error when the file isn't there yet, is of an older layout or lacks the table for made curves (the app
    /// makes and moves it when its Curves are opened).
    pub fn open_to_save_made(path: &Path) -> Result<CurvesDb, CurvesError> {
        if !is_there(path)? {
            return Err(CurvesError(format!("{} isn't there yet", path.display())));
        }
        let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(path, flags)?;
        conn.busy_timeout(Duration::from_secs(2))?;
        conn.pragma_update(None, "temp_store", "MEMORY")?;
        let tables: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN ('made_curves', 'your_curves')",
            [],
            |r| r.get(0),
        )?;
        if version_of(&conn)? != VERSION || is_older_layout(&conn)? || tables != 2 {
            return Err(CurvesError(
                "it's from an older version of the app: update the app, then open its Curves once".into(),
            ));
        }
        Ok(CurvesDb { conn, notes: Vec::new() })
    }

    /// The ids of the curves made in an editor (the app's or the plugin's) and whether each is hidden, for a page to
    /// see one it doesn't list yet, or lists as it was. Only that is read: no names, no points.
    pub fn made_ids(&self) -> Result<Vec<(String, bool)>, CurvesError> {
        let tables: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN ('made_curves', 'your_curves')",
            [],
            |r| r.get(0),
        )?;
        if tables != 2 {
            return Ok(Vec::new());
        }
        let mut st = self
            .conn
            .prepare("SELECT m.id, COALESCE(y.hidden, 0) FROM made_curves m LEFT JOIN your_curves y ON y.id = m.id")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? != 0)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Opens the database at `path` only to read it, as the CLAP plugin's list does: nothing
    /// is made or changed, and an error when it isn't there yet (the app makes it when its Curves are first opened).
    pub fn open_read_only(path: &Path) -> Result<CurvesDb, CurvesError> {
        if !is_there(path)? {
            return Err(CurvesError(format!("{} isn't there yet", path.display())));
        }
        let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(path, flags)?;
        conn.busy_timeout(Duration::from_secs(2))?;
        conn.pragma_update(None, "query_only", true)?;
        conn.pragma_update(None, "temp_store", "MEMORY")?;
        version_of(&conn)?;
        // Only the app makes and moves its files: it moves an older layout into this one when its Curves are next read.
        if is_older_layout(&conn)? {
            return Err(CurvesError(
                "it's from an older version of the app: update the app, then open its Curves once".into(),
            ));
        }
        Ok(CurvesDb { conn, notes: Vec::new() })
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
        {
            let mut shape_of = tx.prepare("SELECT n FROM curve_shapes WHERE id = ?1 AND points = ?2 LIMIT 1")?;
            let mut new_shape = tx.prepare("INSERT INTO curve_shapes (id, points) VALUES (?1, ?2)")?;
            let mut new_place = tx.prepare(
                "INSERT INTO curve_places (shape, path, name, device, setting, at, length) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for (path, (modified, size), found) in &read {
                tx.execute(
                    "INSERT INTO curve_reads (path, modified, size, version) VALUES (?1, ?2, ?3, ?4)",
                    params![path, modified, size, read_version(path)],
                )?;
                for f in found {
                    // The curve as found here, kept once: the one it matches, or a new one.
                    let points = points_text(&f.points);
                    let known: Option<i64> = shape_of.query_row(params![f.id, points], |r| r.get(0)).optional()?;
                    let shape = match known {
                        Some(n) => n,
                        None => new_shape.insert(params![f.id, points])?,
                    };
                    let s = &f.source;
                    new_place.execute(params![shape, s.path, s.name, s.device, s.setting, s.at, f.length])?;
                }
            }
        }
        // A curve found nowhere any more goes with its last place.
        if !gone.is_empty() || !read.is_empty() {
            tx.execute("DELETE FROM curve_shapes WHERE n NOT IN (SELECT shape FROM curve_places)", [])?;
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
        // The save's date from curve_reads, the kind and song from the path (rule 23). Without the points: they're
        // read once for each curve below, not for every place.
        let mut st = self.conn.prepare(
            "SELECT s.id, c.path, c.name, c.device, c.setting, c.at, c.length, c.shape, r.modified \
             FROM curve_places c JOIN curve_shapes s ON s.n = c.shape JOIN curve_reads r ON r.path = c.path \
             WHERE ?1 IS NULL OR s.id = ?1 ORDER BY r.modified DESC, c.path, c.at",
        )?;
        type Row = (String, Option<SourceKind>, CurveSource, f64, i64);
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
                r.get::<_, i64>(7)?,
            ))
        })?;
        let mut by_id: BTreeMap<String, Vec<(CurveSource, f64, Option<String>)>> = BTreeMap::new();
        let mut points_of = self.conn.prepare("SELECT points FROM curve_shapes WHERE n = ?1")?;
        for row in rows {
            let (id, kind, mut source, length, shape): Row = row?;
            // A file of a kind a newer app reads is passed over.
            let Some(kind) = kind else { continue };
            source.kind = kind;
            let found = by_id.entry(id).or_default();
            // The curve's points are its newest place's: the first of its places here.
            let points = if found.is_empty() {
                points_of.query_row([shape], |r| text_at(r, 0)).optional()?.flatten()
            } else {
                None
            };
            found.push((source, length, points));
        }
        let yours = self.yours(only)?;
        let made = self.made(only)?;
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
        // The curves made in the editor: each its own curve, found in no file.
        out.extend(made.into_iter().map(|m| {
            let (name, hidden) = yours.get(&m.id).cloned().unwrap_or((None, false));
            let source = CurveSource {
                kind: SourceKind::Made,
                path: String::new(),
                song: m.name.clone(),
                name: String::new(),
                device: String::new(),
                setting: String::new(),
                at: 0.0,
                modified: m.made,
            };
            Curve {
                name: name.unwrap_or(m.name),
                id: m.id,
                points: m.points,
                length_beats: m.length,
                hidden,
                sources: vec![source],
            }
        }));
        out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.id.cmp(&b.id)));
        Ok(out)
    }

    /// The curves made in the editor (one, or all). None from a database made before there were any: the plugin opens
    /// it only to read, so the table isn't added for it.
    fn made(&self, only: Option<&str>) -> Result<Vec<Made>, CurvesError> {
        let there: Option<i64> = self
            .conn
            .query_row("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'made_curves'", [], |r| r.get(0))
            .optional()?;
        if there.is_none() {
            return Ok(Vec::new());
        }
        let mut st =
            self.conn.prepare("SELECT id, name, length, points, made FROM made_curves WHERE ?1 IS NULL OR id = ?1")?;
        let rows = st.query_map([only], |r| {
            Ok((r.get::<_, String>(0)?, text_at(r, 1)?, r.get::<_, f64>(2)?, text_at(r, 3)?, r.get::<_, i64>(4)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, name, length, points, made) = row?;
            // A line that can't be read (damaged) is passed over, as a damaged place is.
            let Some(points) = points.as_deref().and_then(parse_points) else { continue };
            out.push(Made { id, name: name.unwrap_or_default(), length, points, made });
        }
        Ok(out)
    }

    /// Keeps a curve made in the editor as a new curve of its own, named `name`, made at `now` (seconds since 1970);
    /// returns it as the list has it. The same points and length saved again are the same curve: it keeps its first
    /// name and date, and shows again if it was hidden. Refused when it's no curve (fewer than two points, a point out of place, no length).
    pub fn save_made(
        &self,
        name: &str,
        points: &[curve::CurvePoint],
        length_beats: f64,
        now: i64,
    ) -> Result<Curve, CurvesError> {
        let whole = |p: &curve::CurvePoint| {
            (0.0..=1.0).contains(&p.x) && (0.0..=1.0).contains(&p.y) && (-1.0..=1.0).contains(&p.bend)
        };
        let in_order = points.iter().zip(points.iter().skip(1)).all(|(a, b)| a.x <= b.x);
        if points.len() < 2 || !points.iter().all(whole) || !in_order {
            return Err(CurvesError("that isn't a curve that can be kept (its points are out of place)".into()));
        }
        if !length_beats.is_finite() || length_beats <= 0.0 {
            return Err(CurvesError("that curve has no length".into()));
        }
        let name = Some(name.trim()).filter(|n| !n.is_empty()).unwrap_or("New curve");
        let id = made_id(points, length_beats);
        // All of it or none of it, with the file held from the first step: the plugin keeps curves too, from another
        // program, and a write of the app's coming between the steps must not leave a curve kept and called failed.
        let all = rusqlite::Transaction::new_unchecked(&self.conn, rusqlite::TransactionBehavior::Immediate)?;
        self.conn.execute(
            "INSERT INTO made_curves (id, name, length, points, made) VALUES (?1, ?2, ?3, ?4, ?5) \
             ON CONFLICT(id) DO NOTHING",
            params![id, name, length_beats, points_text(points), now],
        )?;
        // The same shape saved before and hidden since: saving it again shows it again.
        self.conn.execute("UPDATE your_curves SET hidden = 0 WHERE id = ?1", [&id])?;
        let kept =
            self.get(&id)?.ok_or_else(|| CurvesError("the curve can't be read back, so it isn't kept".into()))?;
        all.commit()?;
        Ok(kept)
    }

    /// Forgets a curve made in the editor (and your name for it); whether there was one. Found curves aren't touched:
    /// they come from your files.
    pub fn remove_made(&self, id: &str) -> Result<bool, CurvesError> {
        let gone = self.conn.execute("DELETE FROM made_curves WHERE id = ?1", [id])? > 0;
        if gone {
            self.conn.execute("DELETE FROM your_curves WHERE id = ?1", [id])?;
        }
        Ok(gone)
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
        let found = "SELECT 1 FROM curve_shapes s JOIN curve_places c ON c.shape = s.n WHERE s.id = ?1 LIMIT 1";
        Ok(self.conn.query_row(found, [id], |_| Ok(())).optional()?.is_some() || !self.made(Some(id))?.is_empty())
    }
}

/// A line of `made_curves`.
struct Made {
    id: String,
    name: String,
    length: f64,
    points: Vec<curve::CurvePoint>,
    made: i64,
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

/// Who a curve file says made it, as Bitwig's browser shows it: the app's name.
pub const CREATOR: &str = "DAWmate";

/// `c` as a Bitwig curve file's bytes (`.bwcurve`), as [`file`] writes it: what a drag out of the plugin carries.
pub fn curve_bytes(c: &Curve) -> Result<Vec<u8>, String> {
    let periodic =
        c.sources.iter().all(|s| !s.kind.is_song()) && c.points.first().map(|p| p.y) == c.points.last().map(|p| p.y);
    curve::write(&curve::Curve {
        name: c.name.clone(),
        creator: CREATOR.into(),
        comment: String::new(),
        category: if periodic { "Periodic" } else { "Sequence" }.into(),
        tags: String::new(),
        beat_length: Some(c.length_beats).filter(|l| *l > 0.0),
        points: c.points.clone(),
    })
    .ok_or_else(|| "This curve has no points".to_owned())
}

/// Whether a curve file already holds this curve as it is now: `Some(true)` with the app's name as its creator,
/// `Some(false)` with an older name (written before the rename: written again, but still good to use), `None` when it
/// doesn't hold it (or isn't there).
fn holds(path: &Path, c: &Curve) -> Option<bool> {
    let back = std::fs::read(path).ok().and_then(curve::read)?;
    (back.points == c.points && back.beat_length == Some(c.length_beats).filter(|l| *l > 0.0))
        .then(|| back.creator == CREATOR)
}

/// The curve file for `curve` in the app's own `Curves` folder (`folder`): written the first time it's asked for, and
/// again when the curve has changed since (copy, check, rename, as every file the app writes); returns its path.
/// Safe on the worker pool; never on the window's thread.
pub fn file(c: &Curve, folder: &Path) -> Result<PathBuf, String> {
    let id = c.id.get(..ID_IN_NAME).unwrap_or(&c.id);
    let path = folder.join(format!("{} {id}.{}", names::safe_file_name(&c.name), curve::EXTENSION));
    let held = holds(&path, c);
    if held == Some(true) {
        // Used again: its time says so, and the start-up sweep (which clears the files here that are a month old)
        // counts from its last use, never from when it was first made. (Best effort: an old time only means it's
        // made again one day.)
        if let Ok(file) = std::fs::File::options().write(true).open(&path) {
            let _ = file.set_modified(std::time::SystemTime::now());
        }
        return Ok(path);
    }
    let written = std::fs::create_dir_all(folder)
        .map_err(|e| format!("Couldn't make the Curves folder: {e}"))
        .and_then(|()| curve_bytes(c))
        .and_then(|bytes| export::write_checked(&path, &bytes).map_err(|e| format!("Couldn't write the curve: {e}")));
    match written {
        Ok(()) => {}
        // Only its maker's name is older: the file there still holds the curve (something has it open, say).
        Err(_) if held == Some(false) => return Ok(path),
        Err(e) => return Err(e),
    }
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
/// ([`id_in_file_name`]). Nothing else is listed.
fn curve_files(folder: &Path) -> Vec<(PathBuf, String)> {
    let Ok(entries) = std::fs::read_dir(folder) else { return Vec::new() };
    entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_owned();
            let id = id_in_file_name(&name)?.to_owned();
            Some((PathBuf::from(&name), id))
        })
        .collect()
}

/// Whether a file's whole name is one [`file`] gives now: "<name> <16 hex>.bwcurve". The start-up sweep clears such
/// files once they haven't been used for a month (`mo_app::tidy`): [`file`] makes one again when it's asked for.
/// (Not the 8 digits of the first test builds: eight digits are a date too, and a file named so may be yours.)
pub fn is_made_file_name(name: &str) -> bool {
    id_in_file_name(name).is_some_and(|id| id.len() == ID_IN_NAME)
}

/// The curve id's start that a file's name ends with, when the name is one [`file`] gives: "<name> <16 hex>.bwcurve"
/// (8 in the first test builds).
fn id_in_file_name(name: &str) -> Option<&str> {
    let stem = name.strip_suffix(curve::EXTENSION)?.strip_suffix('.')?;
    let (_, id) = stem.rsplit_once(' ')?;
    let hex = (id.len() == ID_IN_NAME || id.len() == 8) && id.bytes().all(|b| b.is_ascii_hexdigit());
    hex.then_some(id)
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

    #[test]
    fn a_damaged_length_or_start_is_kept_as_a_number_the_library_takes() {
        let source = |at: f64| CurveSource {
            kind: SourceKind::BitwigClip,
            path: "a.bwproject".into(),
            song: "a".into(),
            name: String::new(),
            device: String::new(),
            setting: String::new(),
            at,
            modified: 0,
        };
        let rise = || shape(&[(0.0, 0.0, 0.0), (4.0, 1.0, 0.0)], 4.0);
        for bad in [f64::NAN, f64::INFINITY, -1.0, 0.0, 1e15] {
            let f = found(rise(), bad, source(f64::NAN)).expect("a curve");
            assert_eq!((f.length, f.source.at), (4.0, 0.0), "{bad}");
            // A clip's moves say how long it is then: as far as the last one, which is what they're spread over.
            let moves = [(0.0, 0.0, 0.0), (8.0, 1.0, 0.0), (16.0, 0.5, 0.0)];
            assert_eq!(span_of(&moves, bad), 16.0, "{bad}");
            assert_eq!(shape(&moves, bad).expect("a curve")[1].x, 0.5, "{bad}");
        }
        let f = found(rise(), 512.0, source(64.0)).expect("a curve");
        assert_eq!((f.length, f.source.at), (512.0, 64.0));
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mo-curves-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_curve_saved_from_the_editor_is_a_curve_of_its_own() {
        let dir = temp_dir("made");
        let found = library_curve(&dir, "a.bwcurve", "Rise", &[0.0, 1.0, 0.0]);
        let mut db = CurvesDb::open(&dir.join(FILE_NAME)).unwrap();
        db.scan(std::slice::from_ref(&found), &Progress::default()).unwrap();
        // The found curve at half its height: the same shape to the scan, but a curve of its own once saved.
        let points = [p(0.0, 0.0, 0.0), p(0.5, 0.5, 0.25), p(1.0, 0.0, 0.0)];
        let made = db.save_made("  Half rise ", &points, 8.0, 1_800_000_000).unwrap();
        assert_eq!((made.name.as_str(), made.length_beats, made.points.as_slice()), ("Half rise", 8.0, &points[..]));
        assert_eq!(made.sources.len(), 1);
        assert_eq!((made.sources[0].kind, made.sources[0].modified), (SourceKind::Made, 1_800_000_000));
        assert!(!made.sources[0].kind.is_song());
        assert_eq!(db.list().unwrap().len(), 2);
        assert!(db.has(&made.id).unwrap());
        // Saved again it's the same curve, with its first name and date; another length is another curve.
        let again = db.save_made("Other name", &points, 8.0, 1_900_000_000).unwrap();
        assert_eq!(again, made);
        assert_ne!(db.save_made("Longer", &points, 16.0, 1).unwrap().id, made.id);
        assert_eq!(db.list().unwrap().len(), 3);
        // It's renamed and hidden as any curve, and a scan leaves it be.
        assert_eq!(db.rename(&made.id, "Mine").unwrap().unwrap().name, "Mine");
        assert!(db.set_hidden(&made.id, true).unwrap().unwrap().hidden);
        db.scan(&[], &Progress::default()).unwrap();
        let kept = db.get(&made.id).unwrap().unwrap();
        assert_eq!((kept.name.as_str(), kept.hidden), ("Mine", true));
        // Saved again while hidden, it shows again (under the name you gave it).
        let shown = db.save_made("Third name", &points, 8.0, 2).unwrap();
        assert_eq!((shown.name.as_str(), shown.hidden), ("Mine", false));
        assert_eq!(db.list().unwrap().len(), 2, "the found curve went with its file, the made ones stay");
        // No name gets one; what's no curve is refused; the file it's dragged out as is made as for any curve.
        assert_eq!(db.save_made(" ", &[p(0.0, 1.0, 0.0), p(1.0, 0.0, 0.0)], 4.0, 2).unwrap().name, "New curve");
        assert!(db.save_made("One point", &points[..1], 4.0, 2).is_err());
        assert!(db.save_made("Backwards", &[p(1.0, 0.0, 0.0), p(0.0, 1.0, 0.0)], 4.0, 2).is_err());
        assert!(db.save_made("Too high", &[p(0.0, 0.0, 0.0), p(1.0, 1.5, 0.0)], 4.0, 2).is_err());
        assert!(db.save_made("No length", &points, 0.0, 2).is_err());
        assert_eq!(
            curve::read(std::fs::read(file(&kept, &dir.join(FOLDER)).unwrap()).unwrap()).unwrap().points,
            points
        );
        // The plugin reads them too; and a database from before there were any still opens to read.
        let read_only = CurvesDb::open_read_only(&dir.join(FILE_NAME)).unwrap();
        assert_eq!(read_only.list().unwrap(), db.list().unwrap());
        assert!(db.remove_made(&made.id).unwrap() && !db.remove_made(&made.id).unwrap());
        assert!(!db.has(&made.id).unwrap());
        // The plugin keeps one too, in a file that's there and whole; it makes and changes nothing else.
        let plugin = CurvesDb::open_to_save_made(&dir.join(FILE_NAME)).unwrap();
        let ramp = [p(0.0, 0.0, 0.0), p(1.0, 0.25, 0.0)];
        let from_plugin = plugin.save_made("From the plugin", &ramp, 2.0, 5).unwrap();
        assert!(db.made_ids().unwrap().contains(&(from_plugin.id.clone(), false)));
        assert_eq!(db.get(&from_plugin.id).unwrap().unwrap().name, "From the plugin");
        drop(plugin);
        assert!(CurvesDb::open_to_save_made(&dir.join("none.db")).is_err());
        assert!(!dir.join("none.db").exists(), "nothing is made for the plugin");
        db.conn.execute_batch("DROP TABLE made_curves").unwrap();
        assert!(
            CurvesDb::open_to_save_made(&dir.join(FILE_NAME)).is_err(),
            "no table for made curves: the app's to make"
        );
        let old = CurvesDb::open_read_only(&dir.join(FILE_NAME)).unwrap();
        assert_eq!(old.list().unwrap().len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
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

        // Each curve as found is kept once: Rise, its taller copy (the same curve, other points) and Fall.
        let shapes = |db: &CurvesDb| db.conn.query_row("SELECT COUNT(*) FROM curve_shapes", [], |r| r.get::<_, i64>(0));
        assert_eq!(shapes(&db).unwrap(), 3);

        // A save left out of the list is forgotten; your name stays for when it comes back.
        db.scan(&[a.clone(), c.clone()], &progress).unwrap();
        assert_eq!(shapes(&db).unwrap(), 2, "a curve found nowhere any more goes with its last place");
        let rise = db.get(&rise.id).unwrap().unwrap();
        assert_eq!((rise.name.as_str(), rise.hidden, rise.sources.len()), ("My rise", true, 1));

        // Written on demand, once; and into Bitwig's library without replacing a file.
        let curves = dir.join(FOLDER);
        let written = file(&rise, &curves).unwrap();
        assert_eq!(file(&rise, &curves).unwrap(), written);
        assert_eq!(curve::read(std::fs::read(&written).unwrap()).unwrap().points, rise.points);
        // Written before the app was called DAWmate, it's written again with the app's name as its maker.
        let mut old = curve::read(std::fs::read(&written).unwrap()).unwrap();
        assert_eq!(old.creator, CREATOR);
        old.creator = "Music Organiser".into();
        std::fs::write(&written, curve::write(&old).unwrap()).unwrap();
        assert_eq!(file(&rise, &curves).unwrap(), written);
        assert_eq!(curve::read(std::fs::read(&written).unwrap()).unwrap().creator, CREATOR);
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

    /// The first layout's tables: the points kept with every place.
    const FIRST_LAYOUT: &str = "\
        CREATE TABLE curve_reads (path TEXT PRIMARY KEY NOT NULL, modified INTEGER NOT NULL, \
            size INTEGER NOT NULL, version INTEGER NOT NULL);
        CREATE TABLE curve_places (id TEXT NOT NULL, path TEXT NOT NULL, name TEXT NOT NULL, \
            device TEXT NOT NULL, setting TEXT NOT NULL, at REAL NOT NULL, length REAL NOT NULL, points TEXT NOT NULL);
        CREATE INDEX curve_places_path ON curve_places (path);
        CREATE INDEX curve_places_id ON curve_places (id);
        CREATE TABLE your_curves (id TEXT PRIMARY KEY NOT NULL, name TEXT, hidden INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE made_curves (id TEXT PRIMARY KEY NOT NULL, name TEXT NOT NULL, length REAL NOT NULL, \
            points TEXT NOT NULL, made INTEGER NOT NULL);";

    #[test]
    fn a_database_with_the_points_kept_at_every_place_is_moved_to_keeping_them_once() {
        let dir = temp_dir("first-layout");
        let path = dir.join(FILE_NAME);
        // A long curve (knob moves: hundreds of points) found at 1,500 places in 30 saves, in two lengths; a short
        // one found once; a line that can't be read; your name for one, and a curve of your own making.
        let long: Vec<curve::CurvePoint> =
            (0..800).map(|i| p(f64::from(i) / 799.0, f64::from(i % 7) / 6.0, 0.0)).collect();
        let short = [p(0.0, 0.0, 0.0), p(1.0, 1.0, 0.5)];
        let (long_id, short_id) = (shape_id(&long), shape_id(&short));
        {
            let mut conn = Connection::open(&path).unwrap();
            conn.pragma_update_and_check(None, "journal_mode", "WAL", |r| r.get::<_, String>(0)).unwrap();
            conn.execute_batch(FIRST_LAYOUT).unwrap();
            let tx = conn.transaction().unwrap();
            for save in 0..30 {
                let song = format!("/songs/Tune {save}.flp");
                tx.execute("INSERT INTO curve_reads VALUES (?1, ?2, 10, 1)", params![song, 1_700_000_000 + save])
                    .unwrap();
                for at in 0..50 {
                    let length = if at % 2 == 0 { 4.0 } else { 8.0 };
                    tx.execute(
                        "INSERT INTO curve_places VALUES (?1, ?2, '', 'Serum', 'Cutoff', ?3, ?4, ?5)",
                        params![long_id, song, f64::from(at) * 4.0, length, points_text(&long)],
                    )
                    .unwrap();
                }
            }
            tx.execute(
                "INSERT INTO curve_places VALUES (?1, '/songs/Tune 3.flp', 'Rise', 'Mixer', 'Volume', 0, 2, ?2)",
                params![short_id, points_text(&short)],
            )
            .unwrap();
            // An older place of the long curve whose text is damaged (no good UTF-8): carried over as it is.
            tx.execute(
                "INSERT INTO curve_places VALUES (?1, '/songs/Tune 0.flp', '', 'Serum', 'Cutoff', 999, 4, \
                     CAST(x'30ff' AS TEXT))",
                params![long_id],
            )
            .unwrap();
            tx.execute(
                "INSERT INTO curve_places VALUES ('bad', '/songs/Tune 4.flp', '', '', '', 0, 1, 'not points')",
                [],
            )
            .unwrap();
            tx.execute("INSERT INTO your_curves VALUES (?1, 'My wobble', 0)", [&long_id]).unwrap();
            tx.execute("INSERT INTO made_curves VALUES ('made1', 'Made', 4, ?1, 1800000000)", [points_text(&short)])
                .unwrap();
            tx.commit().unwrap();
        }
        let size = |path: &Path| std::fs::metadata(path).unwrap().len();
        let before = size(&path);
        assert!(before > 30_000_000, "{before}");

        // The plugin never changes the app's files: it says the app must open it first.
        let old = CurvesDb::open_read_only(&path).err().unwrap().to_string();
        assert!(old.contains("older version"), "{old}");

        let mut db = CurvesDb::open(&path).unwrap();
        db.give_back_room();
        assert_eq!(db.notes().len(), 2, "{:?}", db.notes());
        assert_eq!(
            db.notes()[0],
            "Moved curves.db to keeping each curve's points once: 1503 places of 4 curves as found"
        );
        assert!(db.notes()[1].starts_with("curves.db is 0 MB now (it was "), "{:?}", db.notes());
        let list = db.list().unwrap();
        assert_eq!(list.len(), 3, "the two curves and the one made; the line that can't be read is passed over");
        let wobble = list.iter().find(|c| c.id == long_id).unwrap();
        assert_eq!((wobble.name.as_str(), wobble.sources.len(), &wobble.points), ("My wobble", 1501, &long));
        // Its newest place's length: the newest save's first place.
        assert_eq!((wobble.sources[0].song.as_str(), wobble.length_beats), ("Tune 29", 4.0));
        let rise = list.iter().find(|c| c.id == short_id).unwrap();
        assert_eq!((rise.name.as_str(), rise.length_beats, rise.points.as_slice()), ("Rise", 2.0, &short[..]));
        assert_eq!(rise.sources[0].device, "Mixer");
        assert!(list.iter().any(|c| c.id == "made1" && c.name == "Made"));
        assert!(db.has(&long_id).unwrap() && !db.has("none").unwrap());
        drop(db);
        let after = size(&path);
        assert!(after < before / 50, "{after} of {before}");

        // Another copy of the app that saw the first layout and then waited for this one's move finds it done.
        assert_eq!(to_shapes(&mut Connection::open(&path).unwrap()).unwrap(), None);

        // Opened again: nothing to do, and the plugin reads the same.
        let mut db = CurvesDb::open(&path).unwrap();
        db.give_back_room();
        assert!(db.notes().is_empty(), "{:?}", db.notes());
        assert_eq!(CurvesDb::open_read_only(&path).unwrap().list().unwrap(), list);
        drop(db);

        // A newer app's file is left alone, by the app and by the plugin.
        Connection::open(&path).unwrap().pragma_update(None, "user_version", VERSION + 1).unwrap();
        for newer in [CurvesDb::open(&path).err(), CurvesDb::open_read_only(&path).err()] {
            assert!(newer.unwrap().to_string().contains("newer version"));
        }
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
