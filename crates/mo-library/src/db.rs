//! Reading `library.db`, the C# app's database, without ever changing it.

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{Local, NaiveDateTime};
use mo_formats::case;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row, Statement};
use serde::de::{Deserializer, MapAccess, Visitor};

use crate::read::Installed;
use crate::scan::KnownFile;
use crate::songs::SongChoices;
use crate::{Confidence, PluginKind, STATUSES, Save};

/// What went wrong reading the library.
#[derive(Debug)]
pub enum LibraryError {
    /// The file isn't there, can't be opened, or isn't a Music Organiser library.
    Open { path: PathBuf, why: String },
    /// A read failed (the file is damaged, or locked for longer than the wait).
    Read(rusqlite::Error),
}

impl fmt::Display for LibraryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LibraryError::Open { path, why } => write!(f, "Couldn't open the library {}: {why}", path.display()),
            LibraryError::Read(e) => write!(f, "Couldn't read the library: {e}"),
        }
    }
}

impl std::error::Error for LibraryError {}

impl From<rusqlite::Error> for LibraryError {
    fn from(e: rusqlite::Error) -> LibraryError {
        LibraryError::Read(e)
    }
}

/// A plugin one save uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectPlugin {
    pub name: String,
    pub vendor: String,
    pub kind: PluginKind,
    /// "VST", "VST3", "CLAP", "AU", "Native" (a DAW's own device) …
    pub format: String,
    pub path: String,
    pub installed: bool,
}

/// A sound file one save uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectSample {
    pub path: String,
    /// Whether it was where the project says when the save was read.
    pub found: bool,
    /// It's loaded but no note plays it.
    pub unplayed: bool,
}

/// Everything the library keeps on one save, for the song panel.
#[derive(Clone, Debug, Default)]
pub struct SaveDetail {
    pub save: Save,
    /// The song title the project itself gives, if any.
    pub title: String,
    /// Instruments first, then by name.
    pub plugins: Vec<ProjectPlugin>,
    /// Missing ones first, then by path.
    pub samples: Vec<ProjectSample>,
    pub tracks: Vec<String>,
    /// What the reader found, in its order: ("Time signature", "4/4") …
    pub details: Vec<(String, String)>,
    pub warnings: Vec<String>,
    pub can_extract_midi: bool,
    pub can_extract_presets: bool,
}

/// File paths, matched ignoring letter case as the library matches them.
#[derive(Clone, Debug, Default)]
pub struct PathSet(HashSet<String>);

impl PathSet {
    pub fn contains(&self, path: &str) -> bool {
        self.0.contains(&case::key(path))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The library, open for reading only: SQLite itself refuses any change, so the C# app's data is safe.
///
/// SQLite may leave `library.db-wal` and `library.db-shm` beside the file after reading (it needs them to read while
/// the C# app writes). They hold nothing of their own, and the C# app goes on using them as it always does.
pub struct Library {
    conn: Connection,
    path: PathBuf,
    /// The tables this library has: one made by an older C# app may lack some, which then read as empty.
    tables: HashSet<String>,
    /// The list query, naming the columns this library has (all but the big `info`, which only the detail needs).
    summaries: String,
    /// The columns `projects` has: an older C# app's lacks some.
    columns: HashSet<String>,
}

impl Library {
    /// Opens the library at `path` for reading. It must already exist: this never makes one.
    pub fn open_read_only(path: &Path) -> Result<Library, LibraryError> {
        let open_error = |why: String| LibraryError::Open { path: path.to_owned(), why };
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(path, flags).map_err(|e| open_error(e.to_string()))?;
        // The C# app may be saving a scan while this reads: wait a little for it rather than fail.
        conn.busy_timeout(Duration::from_secs(2)).map_err(|e| open_error(e.to_string()))?;
        conn.pragma_update(None, "query_only", true).map_err(|e| open_error(e.to_string()))?;
        let names = |sql: &str| -> rusqlite::Result<Vec<String>> {
            let mut st = conn.prepare(sql)?;
            st.query_map([], |r| r.get::<_, String>(0))?.collect()
        };
        let tables: HashSet<String> = names("SELECT name FROM sqlite_master WHERE type = 'table'")
            .map_err(|e| open_error(format!("it isn't a Music Organiser library ({e})")))?
            .into_iter()
            .collect();
        if !tables.contains("projects") {
            return Err(open_error("it isn't a Music Organiser library (it has no projects table)".into()));
        }
        let columns = names("SELECT name FROM pragma_table_info('projects')").map_err(|e| open_error(e.to_string()))?;
        let summaries = summaries_query(&columns, &tables);
        let columns = columns.into_iter().collect();
        Ok(Library { conn, path: path.to_owned(), tables, summaries, columns })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn has(&self, table: &str) -> bool {
        self.tables.contains(table)
    }

    /// Every save's file as the library last read it, for a scan to tell what's new or changed. As the C# app, a save
    /// whose key or notes were never worked out, or whose file was missing, is read again whatever its date.
    pub fn known_files(&self) -> Result<Vec<KnownFile>, LibraryError> {
        let has = |c: &str| self.columns.contains(c);
        // A column the library doesn't have reads as nothing, so its files are read again.
        let or_zero = |c: &str| if has(c) { format!("COALESCE({c}, 0)") } else { "0".to_owned() };
        let reread = if has("detected_key") && has("note_count") && has("file_missing") {
            "(detected_key IS NULL OR note_count IS NULL OR file_missing = 1)"
        } else {
            "1"
        };
        let sql = format!(
            "SELECT id, path, {}, {}, {}, {reread} FROM projects",
            or_zero("size"),
            or_zero("mtime"),
            or_zero("file_missing")
        );
        let mut st = self.conn.prepare(&sql)?;
        let rows = st.query_map([], |r| {
            Ok(KnownFile {
                id: r.get(0)?,
                path: r.get(1)?,
                size: r.get(2)?,
                mtime: r.get(3)?,
                missing: r.get::<_, i64>(4)? != 0,
                reread: r.get::<_, i64>(5)? != 0,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The plugins on this computer as the C# app last found them: their names, and the ids its plugin scanner found
    /// inside plugin files. Empty when it hasn't looked yet.
    pub fn installed(&self) -> Result<Installed, LibraryError> {
        let mut installed = Installed::default();
        if self.has("installed_plugins") {
            let mut st = self.conn.prepare("SELECT name, name_key FROM installed_plugins")?;
            let rows: Vec<(String, String)> =
                st.query_map([], |r| Ok((text(r, "name"), text(r, "name_key"))))?.collect::<Result<_, _>>()?;
            for (name, key) in rows {
                installed.names.insert(name.to_lowercase());
                installed.keys.insert(key);
            }
        }
        if self.has("scanned_plugins") {
            let mut st = self.conn.prepare("SELECT id, path FROM scanned_plugins")?;
            let rows: Vec<(String, String)> =
                st.query_map([], |r| Ok((text(r, "id"), text(r, "path"))))?.collect::<Result<_, _>>()?;
            installed = installed.with_scans(rows);
        }
        Ok(installed)
    }

    /// Every save, the most recently written first.
    pub fn saves(&self) -> Result<Vec<Save>, LibraryError> {
        self.summaries("", [])
    }

    /// The save at this path (ignoring case, as the library does), if the library has it.
    pub fn save_at(&self, path: &str) -> Result<Option<Save>, LibraryError> {
        Ok(self.summaries("WHERE p.path = ?1", [path])?.into_iter().next())
    }

    fn summaries<P: rusqlite::Params>(&self, filter: &str, args: P) -> Result<Vec<Save>, LibraryError> {
        let mut st = self.conn.prepare(&format!("{} {filter} ORDER BY p.mtime DESC", self.summaries))?;
        let cols = SaveColumns::of(&st);
        let rows = st.query_map(args, |r| Ok(cols.save(r)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// One save with its plugins, samples and what its reader found; `None` when there's no save with this id.
    pub fn detail(&self, id: i64) -> Result<Option<SaveDetail>, LibraryError> {
        // One read for all of it, so a scan the C# app saves meanwhile can't mix old counts with new lists.
        let read = self.conn.unchecked_transaction()?;
        let Some(save) = self.summaries("WHERE p.id = ?1", [id])?.into_iter().next() else { return Ok(None) };
        let row = self
            .conn
            .query_row("SELECT * FROM projects WHERE id = ?1", [id], |r| {
                let mut d = SaveDetail {
                    title: text(r, "title"),
                    can_extract_midi: int(r, "can_midi") == Some(1),
                    can_extract_presets: int(r, "can_presets") == Some(1),
                    ..SaveDetail::default()
                };
                read_info(&text(r, "info"), &mut d);
                Ok(d)
            })
            .optional()?;
        let Some(mut d) = row else { return Ok(None) };
        d.save = save;
        if self.has("plugins") {
            let mut st = self.conn.prepare("SELECT * FROM plugins WHERE project_id = ?1 ORDER BY kind, name")?;
            d.plugins = st
                .query_map([id], |r| {
                    Ok(ProjectPlugin {
                        name: text(r, "name"),
                        vendor: text(r, "vendor"),
                        kind: plugin_kind(int(r, "kind").unwrap_or(0)),
                        format: text(r, "format"),
                        path: text(r, "path"),
                        installed: int(r, "installed") == Some(1),
                    })
                })?
                .collect::<Result<_, _>>()?;
        }
        if self.has("samples") {
            let mut st = self.conn.prepare("SELECT * FROM samples WHERE project_id = ?1 ORDER BY found, path")?;
            d.samples = st
                .query_map([id], |r| {
                    Ok(ProjectSample {
                        path: text(r, "path"),
                        found: int(r, "found") == Some(1),
                        unplayed: int(r, "unplayed") == Some(1),
                    })
                })?
                .collect::<Result<_, _>>()?;
        }
        drop(read);
        Ok(Some(d))
    }

    /// Your say on which saves are one song (table `song_choices`).
    pub fn song_choices(&self) -> Result<SongChoices, LibraryError> {
        if !self.has("song_choices") {
            return Ok(SongChoices::default());
        }
        let mut st = self.conn.prepare("SELECT kind, a, b FROM song_choices")?;
        let rows: Vec<(String, String, String)> =
            st.query_map([], |r| Ok((text(r, "kind"), text(r, "a"), text(r, "b"))))?.collect::<Result<_, _>>()?;
        Ok(SongChoices::from_rows(rows.iter().map(|(k, a, b)| (k.as_str(), a.as_str(), b.as_str()))))
    }

    /// A setting the C# app keeps, if it's set.
    pub fn setting(&self, key: &str) -> Result<Option<String>, LibraryError> {
        if !self.has("settings") {
            return Ok(None);
        }
        let mut st = self.conn.prepare("SELECT value FROM settings WHERE key = ?1")?;
        let mut rows = st.query([key])?;
        // A value of NULL is no value, as in the C# app.
        Ok(rows.next()?.and_then(|r| match r.get_ref("value") {
            Ok(ValueRef::Text(t)) => Some(String::from_utf8_lossy(t).into_owned()),
            _ => None,
        }))
    }

    /// A setting that holds a list (kept as JSON); empty when it isn't set or can't be read.
    pub fn list_setting(&self, key: &str) -> Result<Vec<String>, LibraryError> {
        let Some(json) = self.setting(key)? else { return Ok(Vec::new()) };
        Ok(serde_json::from_str::<Vec<String>>(&json).unwrap_or_default())
    }

    /// The project folders the app scans.
    pub fn roots(&self) -> Result<Vec<String>, LibraryError> {
        if !self.has("roots") {
            return Ok(Vec::new());
        }
        let mut st = self.conn.prepare("SELECT path FROM roots ORDER BY path")?;
        Ok(st.query_map([], |r| Ok(text(r, "path")))?.collect::<Result<_, _>>()?)
    }

    /// Fix's copies still waiting for Keep or Delete. Until Keep, a song opens its original, so the Songs list leaves
    /// these out.
    pub fn pending_fixes(&self) -> Result<PathSet, LibraryError> {
        if !self.has("settings") {
            return Ok(PathSet::default());
        }
        let mut st = self.conn.prepare("SELECT value FROM settings WHERE key LIKE 'fix_check:%' AND value <> ''")?;
        let paths = st.query_map([], |r| Ok(case::key(&text(r, "value"))))?.collect::<Result<_, _>>()?;
        Ok(PathSet(paths))
    }

    /// Every tag and how many saves have it, the most used first.
    pub fn all_tags(&self) -> Result<Vec<(String, u32)>, LibraryError> {
        if !self.has("tags") {
            return Ok(Vec::new());
        }
        let mut st = self.conn.prepare("SELECT tag, COUNT(*) AS n FROM tags GROUP BY tag ORDER BY n DESC, tag")?;
        let tags =
            st.query_map([], |r| Ok((text(r, "tag"), int(r, "n").and_then(|n| u32::try_from(n).ok()).unwrap_or(0))))?;
        Ok(tags.collect::<Result<_, _>>()?)
    }
}

/// The C# app's list query: each save's own columns (as many as this library has, leaving out `info`) with its
/// counts, tags and plugin names. A count from a table the library lacks is none.
fn summaries_query(columns: &[String], tables: &HashSet<String>) -> String {
    let mut list: Vec<String> =
        columns.iter().filter(|c| c.as_str() != "info").map(|c| format!("p.\"{}\"", c.replace('"', "\"\""))).collect();
    let mut sub = |table: &str, sql: &str, name: &str| {
        list.push(if tables.contains(table) { format!("({sql}) AS {name}") } else { format!("NULL AS {name}") });
    };
    sub("plugins", "SELECT COUNT(*) FROM plugins WHERE project_id = p.id", "plugin_count");
    sub("plugins", "SELECT COUNT(*) FROM plugins WHERE project_id = p.id AND installed = 0", "missing_plugins");
    sub("samples", "SELECT COUNT(*) FROM samples WHERE project_id = p.id", "sample_count");
    sub("samples", "SELECT COUNT(*) FROM samples WHERE project_id = p.id AND found = 0", "missing_samples");
    sub("tags", "SELECT group_concat(tag, char(31)) FROM tags WHERE project_id = p.id", "tag_list");
    sub("plugins", "SELECT group_concat(name, char(31)) FROM plugins WHERE project_id = p.id", "plugin_list");
    format!("SELECT {} FROM projects p", list.join(", "))
}

// A column that's missing (a library made before it was added) or holds something else reads as empty, as in the C#
// app, rather than failing the whole list.

fn text(r: &Row<'_>, col: &str) -> String {
    match r.get_ref(col) {
        Ok(ValueRef::Text(t)) => String::from_utf8_lossy(t).into_owned(),
        _ => String::new(),
    }
}

fn int(r: &Row<'_>, col: &str) -> Option<i64> {
    match r.get_ref(col) {
        Ok(ValueRef::Integer(i)) => Some(i),
        _ => None,
    }
}

/// Unix seconds as local time, as the C# app shows them.
pub(crate) fn local(seconds: i64) -> Option<NaiveDateTime> {
    chrono::DateTime::from_timestamp(seconds, 0).map(|t| t.with_timezone(&Local).naive_local())
}

fn split_list(s: &str) -> Vec<String> {
    if s.is_empty() { Vec::new() } else { s.split('\u{1f}').map(str::to_owned).collect() }
}

fn confidence(n: i64) -> Confidence {
    match n {
        0 => Confidence::Full,
        1 => Confidence::Partial,
        2 => Confidence::Heuristic,
        _ => Confidence::Catalogue,
    }
}

fn plugin_kind(n: i64) -> PluginKind {
    match n {
        1 => PluginKind::Instrument,
        2 => PluginKind::Effect,
        3 => PluginKind::NoteEffect,
        _ => PluginKind::Unknown,
    }
}

/// Where each of a save's columns is in a list query, found once per query rather than by name for every field of
/// every row; `None` for a column this library doesn't have.
struct SaveColumns {
    id: Option<usize>,
    path: Option<usize>,
    name: Option<usize>,
    daw: Option<usize>,
    folder: Option<usize>,
    size: Option<usize>,
    mtime: Option<usize>,
    tempo: Option<usize>,
    version: Option<usize>,
    confidence: Option<usize>,
    file_missing: Option<usize>,
    error: Option<usize>,
    last_opened: Option<usize>,
    plugin_count: Option<usize>,
    sample_count: Option<usize>,
    missing_plugins: Option<usize>,
    missing_samples: Option<usize>,
    tag_list: Option<usize>,
    plugin_list: Option<usize>,
    hardware: Option<usize>,
    status: Option<usize>,
    rating: Option<usize>,
    favourite: Option<usize>,
    musical_key: Option<usize>,
    detected_key: Option<usize>,
    created: Option<usize>,
    note_count: Option<usize>,
    length_bars: Option<usize>,
    track_count: Option<usize>,
    notes: Option<usize>,
}

impl SaveColumns {
    fn of(st: &Statement<'_>) -> SaveColumns {
        let at = |name: &str| st.column_index(name).ok();
        SaveColumns {
            id: at("id"),
            path: at("path"),
            name: at("name"),
            daw: at("daw"),
            folder: at("folder"),
            size: at("size"),
            mtime: at("mtime"),
            tempo: at("tempo"),
            version: at("version"),
            confidence: at("confidence"),
            file_missing: at("file_missing"),
            error: at("error"),
            last_opened: at("last_opened"),
            plugin_count: at("plugin_count"),
            sample_count: at("sample_count"),
            missing_plugins: at("missing_plugins"),
            missing_samples: at("missing_samples"),
            tag_list: at("tag_list"),
            plugin_list: at("plugin_list"),
            hardware: at("hardware"),
            status: at("status"),
            rating: at("rating"),
            favourite: at("favourite"),
            musical_key: at("musical_key"),
            detected_key: at("detected_key"),
            created: at("created"),
            note_count: at("note_count"),
            length_bars: at("length_bars"),
            track_count: at("track_count"),
            notes: at("notes"),
        }
    }

    fn save(&self, r: &Row<'_>) -> Save {
        let value = |c: Option<usize>| c.and_then(|c| r.get_ref(c).ok());
        let text_or = |c: Option<usize>| match value(c) {
            Some(ValueRef::Text(t)) => Some(String::from_utf8_lossy(t).into_owned()),
            _ => None,
        };
        let text = |c: Option<usize>| text_or(c).unwrap_or_default();
        let int = |c: Option<usize>| match value(c) {
            Some(ValueRef::Integer(i)) => Some(i),
            _ => None,
        };
        let count = |c: Option<usize>| int(c).and_then(|n| u32::try_from(n).ok()).unwrap_or(0);
        // A count that's -1 when unknown.
        let known = |c: Option<usize>| int(c).and_then(|n| i32::try_from(n).ok()).unwrap_or(-1);
        let mtime = int(self.mtime).unwrap_or(0);
        let status = text(self.status);
        let mut tags = split_list(&text(self.tag_list));
        tags.sort();
        Save {
            id: int(self.id).unwrap_or(0),
            path: text(self.path),
            name: text(self.name),
            daw: text(self.daw),
            folder: text(self.folder),
            size: int(self.size).unwrap_or(0),
            modified: local(mtime).unwrap_or_default(),
            mtime,
            tempo: match value(self.tempo) {
                Some(ValueRef::Real(t)) => Some(t),
                _ => None,
            },
            version: text(self.version),
            confidence: confidence(int(self.confidence).unwrap_or(0)),
            file_missing: int(self.file_missing) == Some(1),
            error: text_or(self.error),
            last_opened: int(self.last_opened).and_then(local),
            plugin_count: count(self.plugin_count),
            sample_count: count(self.sample_count),
            missing_plugins: count(self.missing_plugins),
            missing_samples: count(self.missing_samples),
            tags,
            plugin_names: split_list(&text(self.plugin_list)),
            hardware: text(self.hardware).split(", ").filter(|h| !h.is_empty()).map(str::to_owned).collect(),
            // An empty status is an idea, as in the C# app.
            status: if status.is_empty() { STATUSES[0].to_owned() } else { status },
            rating: int(self.rating).and_then(|n| i32::try_from(n).ok()).unwrap_or(0),
            favourite: int(self.favourite) == Some(1),
            musical_key: text(self.musical_key),
            detected_key: text(self.detected_key),
            created: int(self.created).and_then(local),
            note_count: known(self.note_count),
            length_bars: known(self.length_bars),
            track_count: known(self.track_count),
            notes: text(self.notes),
        }
    }
}

/// The reader's findings the C# app keeps as JSON: `{"Tracks": [...], "Details": {...}, "Warnings": [...]}`. As there,
/// one that doesn't read as that shape leaves them all empty.
fn read_info(json: &str, d: &mut SaveDetail) {
    #[derive(serde::Deserialize)]
    struct Info {
        #[serde(rename = "Tracks", default)]
        tracks: Option<Vec<String>>,
        #[serde(rename = "Details", default)]
        details: Option<InOrder>,
        #[serde(rename = "Warnings", default)]
        warnings: Option<Vec<String>>,
    }
    let Ok(info) = serde_json::from_str::<Info>(json) else { return };
    d.tracks = info.tracks.unwrap_or_default();
    d.details = info.details.map(|o| o.0).unwrap_or_default();
    d.warnings = info.warnings.unwrap_or_default();
}

/// A JSON object of text, kept in the order it was written (serde_json's own map would sort it).
struct InOrder(Vec<(String, String)>);

impl<'de> serde::Deserialize<'de> for InOrder {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<InOrder, D::Error> {
        struct Pairs;
        impl<'de> Visitor<'de> for Pairs {
            type Value = InOrder;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an object of text")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<InOrder, M::Error> {
                let mut pairs = Vec::new();
                while let Some(pair) = map.next_entry::<String, String>()? {
                    pairs.push(pair);
                }
                Ok(InOrder(pairs))
            }
        }
        d.deserialize_map(Pairs)
    }
}
