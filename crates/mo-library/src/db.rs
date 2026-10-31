//! Reading `library.db`, the C# app's database, without ever changing it.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{Local, NaiveDateTime};
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, Row, params};

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

/// The library, open for reading only: SQLite itself refuses any change, so the C# app's data is safe.
pub struct Library {
    conn: Connection,
    path: PathBuf,
}

/// Each save's counts, tags and plugin names come with it, as the C# app's list reads them.
const SUMMARIES: &str = "SELECT p.*,
  (SELECT COUNT(*) FROM plugins WHERE project_id = p.id) AS plugin_count,
  (SELECT COUNT(*) FROM plugins WHERE project_id = p.id AND installed = 0) AS missing_plugins,
  (SELECT COUNT(*) FROM samples WHERE project_id = p.id) AS sample_count,
  (SELECT COUNT(*) FROM samples WHERE project_id = p.id AND found = 0) AS missing_samples,
  (SELECT group_concat(tag, char(31)) FROM tags WHERE project_id = p.id) AS tag_list,
  (SELECT group_concat(name, char(31)) FROM plugins WHERE project_id = p.id) AS plugin_list
FROM projects p";

impl Library {
    /// Opens the library at `path` for reading. It must already exist: this never makes one.
    pub fn open_read_only(path: &Path) -> Result<Library, LibraryError> {
        let open_error = |why: String| LibraryError::Open { path: path.to_owned(), why };
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(path, flags).map_err(|e| open_error(e.to_string()))?;
        // The C# app may be saving a scan while this reads: wait a little for it rather than fail.
        conn.busy_timeout(Duration::from_secs(2)).map_err(|e| open_error(e.to_string()))?;
        conn.pragma_update(None, "query_only", true).map_err(|e| open_error(e.to_string()))?;
        conn.query_row("SELECT COUNT(*) FROM projects WHERE 0", [], |_| Ok(()))
            .map_err(|e| open_error(format!("it isn't a Music Organiser library ({e})")))?;
        Ok(Library { conn, path: path.to_owned() })
    }

    pub fn path(&self) -> &Path {
        &self.path
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
        let mut st = self.conn.prepare(&format!("{SUMMARIES} {filter} ORDER BY p.mtime DESC"))?;
        let rows = st.query_map(args, |r| Ok(save_from(r)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// One save with its plugins, samples and what its reader found; `None` when there's no save with this id.
    pub fn detail(&self, id: i64) -> Result<Option<SaveDetail>, LibraryError> {
        let Some(save) = self.summaries("WHERE p.id = ?1", [id])?.into_iter().next() else { return Ok(None) };
        let mut d = self.conn.query_row(
            "SELECT title, info, can_midi, can_presets FROM projects WHERE id = ?1",
            [id],
            |r| {
                let mut d = SaveDetail {
                    title: text(r, "title"),
                    can_extract_midi: int(r, "can_midi") == Some(1),
                    can_extract_presets: int(r, "can_presets") == Some(1),
                    ..SaveDetail::default()
                };
                read_info(&text(r, "info"), &mut d);
                Ok(d)
            },
        )?;
        d.save = save;
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
        Ok(Some(d))
    }

    /// Your say on which saves are one song (table `song_choices`).
    pub fn song_choices(&self) -> Result<SongChoices, LibraryError> {
        let mut st = self.conn.prepare("SELECT kind, a, b FROM song_choices")?;
        let rows: Vec<(String, String, String)> =
            st.query_map([], |r| Ok((text(r, "kind"), text(r, "a"), text(r, "b"))))?.collect::<Result<_, _>>()?;
        Ok(SongChoices::from_rows(rows.iter().map(|(k, a, b)| (k.as_str(), a.as_str(), b.as_str()))))
    }

    /// A setting the C# app keeps, if it's set.
    pub fn setting(&self, key: &str) -> Result<Option<String>, LibraryError> {
        let mut st = self.conn.prepare("SELECT value FROM settings WHERE key = ?1")?;
        let mut rows = st.query([key])?;
        Ok(rows.next()?.map(|r| text(r, "value")))
    }

    /// A setting that holds a list (kept as JSON); empty when it isn't set or can't be read.
    pub fn list_setting(&self, key: &str) -> Result<Vec<String>, LibraryError> {
        let Some(json) = self.setting(key)? else { return Ok(Vec::new()) };
        Ok(serde_json::from_str::<Vec<String>>(&json).unwrap_or_default())
    }

    /// The project folders the app scans.
    pub fn roots(&self) -> Result<Vec<String>, LibraryError> {
        let mut st = self.conn.prepare("SELECT path FROM roots ORDER BY path")?;
        Ok(st.query_map([], |r| Ok(text(r, "path")))?.collect::<Result<_, _>>()?)
    }

    /// Fix's copies still waiting for Keep or Delete. Until Keep, a song opens its original, so the Songs list leaves
    /// these out.
    pub fn pending_fixes(&self) -> Result<Vec<String>, LibraryError> {
        let mut st = self.conn.prepare("SELECT value FROM settings WHERE key LIKE 'fix_check:%' AND value <> ''")?;
        Ok(st.query_map([], |r| Ok(text(r, "value")))?.collect::<Result<_, _>>()?)
    }

    /// Every tag and how many saves have it, the most used first.
    pub fn all_tags(&self) -> Result<Vec<(String, u32)>, LibraryError> {
        let mut st = self.conn.prepare("SELECT tag, COUNT(*) AS n FROM tags GROUP BY tag ORDER BY n DESC, tag")?;
        Ok(st.query_map(params![], |r| Ok((text(r, "tag"), count(r, "n"))))?.collect::<Result<_, _>>()?)
    }
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

fn count(r: &Row<'_>, col: &str) -> u32 {
    int(r, col).and_then(|n| u32::try_from(n).ok()).unwrap_or(0)
}

/// A count that's -1 when unknown.
fn known(r: &Row<'_>, col: &str) -> i32 {
    int(r, col).and_then(|n| i32::try_from(n).ok()).unwrap_or(-1)
}

/// Unix seconds as local time, as the C# app shows them.
fn local(seconds: i64) -> Option<NaiveDateTime> {
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

fn save_from(r: &Row<'_>) -> Save {
    let mtime = int(r, "mtime").unwrap_or(0);
    let status = text(r, "status");
    let mut tags = split_list(&text(r, "tag_list"));
    tags.sort();
    Save {
        id: int(r, "id").unwrap_or(0),
        path: text(r, "path"),
        name: text(r, "name"),
        daw: text(r, "daw"),
        folder: text(r, "folder"),
        size: int(r, "size").unwrap_or(0),
        modified: local(mtime).unwrap_or_default(),
        mtime,
        tempo: match r.get_ref("tempo") {
            Ok(ValueRef::Real(t)) => Some(t),
            _ => None,
        },
        version: text(r, "version"),
        confidence: confidence(int(r, "confidence").unwrap_or(0)),
        file_missing: int(r, "file_missing") == Some(1),
        error: match r.get_ref("error") {
            Ok(ValueRef::Text(t)) => Some(String::from_utf8_lossy(t).into_owned()),
            _ => None,
        },
        last_opened: int(r, "last_opened").and_then(local),
        plugin_count: count(r, "plugin_count"),
        sample_count: count(r, "sample_count"),
        missing_plugins: count(r, "missing_plugins"),
        missing_samples: count(r, "missing_samples"),
        tags,
        plugin_names: split_list(&text(r, "plugin_list")),
        hardware: text(r, "hardware").split(", ").filter(|h| !h.is_empty()).map(str::to_owned).collect(),
        status: if status.is_empty() { STATUSES[0].to_owned() } else { status },
        rating: int(r, "rating").and_then(|n| i32::try_from(n).ok()).unwrap_or(0),
        favourite: int(r, "favourite") == Some(1),
        musical_key: text(r, "musical_key"),
        detected_key: text(r, "detected_key"),
        created: int(r, "created").and_then(local),
        note_count: known(r, "note_count"),
        length_bars: known(r, "length_bars"),
        track_count: known(r, "track_count"),
        notes: text(r, "notes"),
    }
}

/// The reader's findings the C# app keeps as JSON: `{"Tracks": [...], "Details": {...}, "Warnings": [...]}`. A damaged
/// one leaves them empty, as there.
fn read_info(json: &str, d: &mut SaveDetail) {
    let Ok(serde_json::Value::Object(info)) = serde_json::from_str::<serde_json::Value>(json) else { return };
    let strings = |v: Option<&serde_json::Value>| -> Vec<String> {
        v.and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect())
            .unwrap_or_default()
    };
    d.tracks = strings(info.get("Tracks"));
    d.warnings = strings(info.get("Warnings"));
    if let Some(details) = info.get("Details").and_then(|v| v.as_object()) {
        d.details = details.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned()))).collect();
    }
}
