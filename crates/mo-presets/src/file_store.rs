//! What the Presets page found in each preset file, kept between runs in `preset-files.db` (the app's own, in its data
//! folder), so a start reads only the preset files that are new or changed since. Before, every preset file was read
//! again each time the page first showed, and every Bitwig preset that loads a plugin was decoded twice (once for the
//! list, once more as a Send to Bitwig template).
//!
//! The folders are still walked each time (that's how new and removed files are found); only the reading is kept.
//! Each file is kept with its size and last-written time (to the nanosecond), and read again when either differs or
//! when what reads it changed ([`VERSION`] here, and the Bitwig reader's version). A file that couldn't be opened isn't
//! kept, so it's tried again next time. Files no longer found are forgotten, so the store never holds more than the
//! page lists (rule 28).
//!
//! It's only what was read before: a store that's damaged or from another layout is made again empty. A file with
//! another app's id in it is never thrown away; the page then reads every file, as before.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mo_formats::PluginKind;
use rayon::prelude::*;
use rusqlite::{Connection, OptionalExtension, params};

use crate::bitwig::{self as bw, Template, Templates};
use crate::files::{self, FileFormat, NotAPreset, PresetFile};
use crate::library::{FoundPreset, list_preset_files_and_unread};

/// The store's name, in the app's data folder.
pub const FILE_NAME: &str = "preset-files.db";

/// What the preset readers here find (`files`, and `bitwig::inspect` for templates). Bump it whenever a change there
/// would read a file differently (rule 7), so every file is read again. 2: a Bitwig preset's kind from Bitwig 6.1's
/// "note_to_audio" and its device (14 Nov 2026).
pub const VERSION: i64 = 2;

/// The tables' layout (`PRAGMA user_version`): another one is made again, empty.
const LAYOUT: i64 = 1;

/// Marks the file as this app's preset files store (`PRAGMA application_id`): "MOPF".
const APPLICATION_ID: i64 = 0x4D4F_5046;

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL) WITHOUT ROWID;
    CREATE TABLE IF NOT EXISTS files (path TEXT PRIMARY KEY NOT NULL, size INTEGER NOT NULL, modified INTEGER NOT NULL,
        error TEXT, format INTEGER, name TEXT NOT NULL, plugin TEXT NOT NULL, plugin_id TEXT NOT NULL,
        kind INTEGER NOT NULL, category TEXT NOT NULL, creator TEXT NOT NULL, tags TEXT NOT NULL,
        programs INTEGER NOT NULL, wraps INTEGER NOT NULL,
        t_plugin TEXT, t_state_field TEXT, t_state_file TEXT, t_preset_name TEXT, t_plugin_id TEXT) WITHOUT ROWID;";

/// Splits a preset's tags in the store (a character no tag holds).
const TAG_SPLIT: char = '\u{1f}';

/// The preset files found, with the Send to Bitwig templates among them.
pub struct Found {
    pub files: Vec<FoundPreset>,
    pub templates: Templates,
    /// How many were read from their files (the rest were as they were last time).
    pub read: usize,
    /// Why what was read couldn't be kept for next time, when it couldn't (the files are still all here).
    pub not_kept: Option<String>,
    /// When each of `files` was last changed, in the same order, as it was looked at to decide whether to read it
    /// (so the page needn't look again, X2-4); `None` when it couldn't be looked at.
    pub modified: Vec<Option<SystemTime>>,
}

/// One file as kept: what it says, and its template when it's one.
#[derive(Clone, Debug, PartialEq)]
struct Entry {
    file: Result<PresetFile, NotAPreset>,
    template: Option<Template>,
}

/// The preset files' readings kept between runs.
pub struct FileStore {
    conn: Connection,
    /// What the store held when opened, until the first [`FileStore::find`] takes it.
    loaded: Option<HashMap<String, (i64, i64, Entry)>>,
}

/// A file as it is now: its last-written time (nanoseconds since 1970) and size; `None` when it can't be looked at.
fn stamp(path: &Path) -> Option<(i64, i64, SystemTime)> {
    let m = std::fs::metadata(path).ok()?;
    let modified = m.modified().ok()?;
    let at = modified.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO);
    Some((i64::try_from(at.as_nanos()).unwrap_or(i64::MAX), i64::try_from(m.len()).unwrap_or(i64::MAX), modified))
}

fn time_of(nanos: i64) -> SystemTime {
    UNIX_EPOCH + Duration::from_nanos(u64::try_from(nanos).unwrap_or(0))
}

fn format_number(f: Option<FileFormat>) -> Option<i64> {
    f.map(|f| match f {
        FileFormat::Fxp => 0,
        FileFormat::Fxb => 1,
        FileFormat::VstPreset => 2,
        FileFormat::BwPreset => 3,
    })
}

fn format_of(n: Option<i64>) -> Option<FileFormat> {
    match n? {
        0 => Some(FileFormat::Fxp),
        1 => Some(FileFormat::Fxb),
        2 => Some(FileFormat::VstPreset),
        3 => Some(FileFormat::BwPreset),
        _ => None,
    }
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

/// Reads one file, and its template when it may be one. A Bitwig preset is read from the disk and decoded once for both.
/// `None` when the file couldn't be read (open elsewhere, or a drive gone just now): it isn't kept, so it's read again
/// next time. A file that trips a reader is kept as it is: it reads the same way until it changes.
fn read_entry(path: &Path) -> Option<Entry> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if FileFormat::of(&name) != Some(FileFormat::BwPreset) {
        return match mo_catch::catch(|| files::read(path)) {
            Ok(Err(_)) => None,
            Ok(Ok(file)) => Some(Entry { file, template: None }),
            Err(_) => Some(Entry { file: Err(NotAPreset(TRIPPED.into())), template: None }),
        };
    }
    let bytes = std::fs::read(path).ok()?;
    let stem = mo_formats::paths::stem(&name).to_owned();
    let read = mo_catch::catch(move || {
        let doc = mo_formats::bitwig::decode(bytes)
            .map_err(|_| NotAPreset("it doesn't start as a Bitwig file does (\"BtWg\")".into()))?;
        let file = files::bwpreset_of(&doc, &stem);
        let template = if file.wraps_plugin { bw::inspect_doc(&doc) } else { None };
        Ok::<_, NotAPreset>((file, template))
    });
    Some(match read {
        Ok(Ok((file, template))) => {
            let template = template.map(|mut t| {
                t.path = path.to_path_buf();
                t
            });
            Entry { file: Ok(file), template }
        }
        Ok(Err(e)) => Entry { file: Err(e), template: None },
        Err(_) => Entry { file: Err(NotAPreset(TRIPPED.into())), template: None },
    })
}

/// What a file that trips its reader is listed with (as `library::read_preset_file` lists it).
const TRIPPED: &str = "the reader stopped on something it didn't expect in this file";

impl FileStore {
    /// Opens the store at `path`, making it (and its folder) if it isn't there. One that's damaged or from another
    /// layout is made again, empty; one kept by other readers is emptied. An error says why there's no store.
    pub fn open(path: &Path) -> Result<FileStore, String> {
        match Self::open_as_is(path) {
            Ok(store) => Ok(store),
            Err(Damage::Foreign) => {
                Err(format!("{} isn't the app's preset list, so it's left as it is", path.display()))
            }
            Err(Damage::Busy(e)) => Err(e),
            Err(Damage::Damaged(e)) => {
                for end in ["", "-wal", "-shm", "-journal"] {
                    let mut p = path.as_os_str().to_owned();
                    p.push(end);
                    let _ = std::fs::remove_file(PathBuf::from(p));
                }
                Self::open_as_is(path).map_err(|again| match again {
                    Damage::Foreign => "it couldn't be made again".to_owned(),
                    Damage::Busy(again) | Damage::Damaged(again) => format!("{e}; made again, {again}"),
                })
            }
        }
    }

    fn open_as_is(path: &Path) -> Result<FileStore, Damage> {
        let err = |e: rusqlite::Error| {
            // A row that doesn't read back as written (no SQLite code) is damage too.
            let damaged = matches!(
                e.sqlite_error_code(),
                None | Some(rusqlite::ErrorCode::NotADatabase | rusqlite::ErrorCode::DatabaseCorrupt)
            );
            if damaged { Damage::Damaged(e.to_string()) } else { Damage::Busy(e.to_string()) }
        };
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(|e| Damage::Busy(e.to_string()))?;
        }
        let conn = Connection::open(path).map_err(err)?;
        conn.busy_timeout(Duration::from_secs(2)).map_err(err)?;
        let id: i64 = conn.pragma_query_value(None, "application_id", |r| r.get(0)).map_err(err)?;
        let tables: i64 = conn.query_row("SELECT COUNT(*) FROM sqlite_master", [], |r| r.get(0)).map_err(err)?;
        if id != APPLICATION_ID && (id != 0 || tables > 0) {
            return Err(Damage::Foreign);
        }
        conn.pragma_update(None, "journal_mode", "WAL").map_err(err)?;
        // Only what was read before: a write lost to a power cut is read again, so no wait for the disk at each one.
        conn.pragma_update(None, "synchronous", "NORMAL").map_err(err)?;
        conn.pragma_update(None, "temp_store", "MEMORY").map_err(err)?;
        conn.pragma_update(None, "cache_size", -32000).map_err(err)?;
        let layout: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0)).map_err(err)?;
        if layout != LAYOUT {
            conn.execute_batch(&format!(
                "BEGIN IMMEDIATE; PRAGMA application_id = {APPLICATION_ID}; DROP TABLE IF EXISTS files; DROP TABLE IF EXISTS meta; {SCHEMA}
                 PRAGMA user_version = {LAYOUT}; COMMIT;"
            ))
            .map_err(err)?;
        }
        let version = format!("{VERSION}/bitwig {}", mo_formats::bitwig::VERSION);
        let kept: Option<String> =
            conn.query_row("SELECT value FROM meta WHERE key = 'version'", [], |r| r.get(0)).optional().map_err(err)?;
        if kept.as_deref() != Some(version.as_str()) {
            conn.execute_batch("BEGIN IMMEDIATE; DELETE FROM files;").map_err(err)?;
            conn.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('version', ?1)", [&version]).map_err(err)?;
            conn.execute_batch("COMMIT;").map_err(err)?;
        }
        // Reads every row once here, so a damaged page shows now (and the store is made again) rather than later.
        let mut store = FileStore { conn, loaded: None };
        store.loaded = Some(store.load().map_err(err)?);
        Ok(store)
    }

    fn load(&self) -> rusqlite::Result<HashMap<String, (i64, i64, Entry)>> {
        load_from(&self.conn)
    }

    /// Every preset file under `roots` (walked as `library::find_preset_files` walks) with what each says, and the
    /// Send to Bitwig templates among them: a file unchanged since it was last read is taken from the store, the rest
    /// are read over every core of the current rayon pool and kept for next time. Files no longer found are forgotten.
    /// Call it off the window's thread. A file under a folder that couldn't be looked in now (a drive that's away, a
    /// link to it, a folder that can't be read) is kept as it was. With `every_root` these are all the page's folders,
    /// so a file under none of them (a folder no longer looked in) is forgotten too (rule 28); without it, only some
    /// of them may be, and those files are left.
    pub fn find(&mut self, roots: &[PathBuf], every_root: bool) -> Found {
        let (paths, unread) = list_preset_files_and_unread(roots);
        let kept = match self.loaded.take() {
            Some(kept) => kept,
            None => self.load().unwrap_or_default(),
        };
        let done = look(&paths, &kept);
        let read = read_now(&done);
        // A kept file is gone only when its folder was looked through: one on a drive that's away now (unplugged or
        // asleep) is kept as it was, as curves.db keeps what it can't reach, and not read again when it's back (H-13).
        // The same goes for a folder under it that couldn't be looked in, a link to a moved folder among them (M-4).
        let walked: Vec<&PathBuf> = roots.iter().filter(|r| r.is_dir()).collect();
        let listed: std::collections::HashSet<&str> = done.iter().map(|d| d.0.path.as_str()).collect();
        let gone: Vec<&String> = kept
            .keys()
            .filter(|p| !listed.contains(p.as_str()))
            .filter(|p| {
                let p = Path::new(p.as_str());
                // Under a folder looked through and not one that couldn't be; or under no folder at all now (X1-2).
                (under_any(p, walked.iter().copied()) && !under_any(p, &unread)) || (every_root && !under_any(p, roots))
            })
            .collect();
        drop(listed);
        // Nothing read and nothing gone: the store is as it should be, so it isn't written.
        let not_kept =
            if read == 0 && gone.is_empty() { None } else { self.write(&done, &gone).err().map(|e| e.to_string()) };
        found_of(done, read, not_kept)
    }

    /// Keeps what was read now, and forgets the files no longer found, in one go.
    fn write(&mut self, done: &[Done], gone: &[&String]) -> rusqlite::Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut forget = tx.prepare_cached("DELETE FROM files WHERE path = ?1")?;
            for path in gone {
                forget.execute([path])?;
            }
            let mut put = tx.prepare_cached(
                "INSERT OR REPLACE INTO files (path, size, modified, error, format, name, plugin, plugin_id, kind,
                    category, creator, tags, programs, wraps, t_plugin, t_state_field, t_state_file, t_preset_name,
                    t_plugin_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
            )?;
            for (found, template, keep, _, _) in done {
                let Some((at, size)) = keep else { continue };
                let blank = PresetFile::default();
                let (error, file) = match &found.file {
                    Ok(f) => (None, f),
                    Err(e) => (Some(e.0.as_str()), &blank),
                };
                let tags = file.tags.join(&TAG_SPLIT.to_string());
                let t = template.as_ref();
                put.execute(params![
                    found.path,
                    size,
                    at,
                    error,
                    format_number(file.format),
                    file.name,
                    file.plugin,
                    file.plugin_id,
                    kind_number(file.kind),
                    file.category,
                    file.creator,
                    tags,
                    i64::from(file.programs),
                    file.wraps_plugin,
                    t.map(|t| t.plugin.as_str()),
                    t.map(|t| t.state_field.as_str()),
                    t.map(|t| t.state_file.as_str()),
                    t.map(|t| t.preset_name.as_str()),
                    t.map(|t| t.plugin_id.as_str()),
                ])?;
            }
        }
        tx.commit()
    }
}

/// Everything a store holds, by path: each file's last-written time, size and what was read from it.
fn load_from(conn: &Connection) -> rusqlite::Result<HashMap<String, (i64, i64, Entry)>> {
    let mut st = conn.prepare(
        "SELECT path, size, modified, error, format, name, plugin, plugin_id, kind, category, creator, tags,
            programs, wraps, t_plugin, t_state_field, t_state_file, t_preset_name, t_plugin_id FROM files",
    )?;
    let rows = st.query_map([], |r| {
        let path: String = r.get(0)?;
        let (size, modified): (i64, i64) = (r.get(1)?, r.get(2)?);
        let error: Option<String> = r.get(3)?;
        let file = match error {
            Some(e) => Err(NotAPreset(e)),
            None => Ok(PresetFile {
                format: format_of(r.get(4)?),
                name: r.get(5)?,
                plugin: r.get(6)?,
                plugin_id: r.get(7)?,
                kind: kind_of(r.get(8)?),
                category: r.get(9)?,
                creator: r.get(10)?,
                tags: r.get::<_, String>(11)?.split(TAG_SPLIT).filter(|t| !t.is_empty()).map(str::to_owned).collect(),
                programs: u32::try_from(r.get::<_, i64>(12)?).unwrap_or(1),
                wraps_plugin: r.get(13)?,
            }),
        };
        let template = match r.get::<_, Option<String>>(14)? {
            Some(plugin) => Some(Template {
                path: PathBuf::from(&path),
                plugin,
                state_field: r.get::<_, Option<String>>(15)?.unwrap_or_default(),
                state_file: r.get::<_, Option<String>>(16)?.unwrap_or_default(),
                preset_name: r.get::<_, Option<String>>(17)?.unwrap_or_default(),
                plugin_id: r.get::<_, Option<String>>(18)?.unwrap_or_default(),
                // The file's own kind: a template's is read by the same rules (`bitwig::kind_of`, then its settings).
                kind: kind_of(r.get(8)?),
                modified: Some(time_of(modified)),
            }),
            None => None,
        };
        Ok((path, (modified, size, Entry { file, template })))
    })?;
    rows.collect()
}

/// Each of `paths` with what it says: from `kept` when it's unchanged since, else read now (over every core of the
/// current rayon pool).
fn look(paths: &[PathBuf], kept: &HashMap<String, (i64, i64, Entry)>) -> Vec<Done> {
    paths
        .par_iter()
        .map(|path| {
            let text = path.to_string_lossy().into_owned();
            let now = stamp(path);
            if let (Some((at, size, _)), Some((k_at, k_size, entry))) = (now, kept.get(&text))
                && at == *k_at
                && size == *k_size
            {
                let found = FoundPreset { path: text, file: entry.file.clone() };
                return (found, entry.template.clone(), None, false, now.map(|n| n.2));
            }
            let Some(entry) = read_entry(path) else {
                // Couldn't be read just now: listed as the page always listed it, and read again next time.
                let found = crate::library::read_preset_file(path);
                return (found, None, None, true, now.map(|n| n.2));
            };
            // A file that couldn't be looked at, or changed while it was read, is read again next time. Its time
            // is the one after the read, so a file changed meanwhile gets its newer time.
            let after = stamp(path);
            let keep = now.filter(|(at, size, _)| after.is_some_and(|(a, s, _)| a == *at && s == *size));
            let template = entry.template.map(|mut t| {
                t.modified = now.map(|(_, _, m)| m).or(t.modified);
                t
            });
            let found = FoundPreset { path: text, file: entry.file };
            (found, template, keep.map(|(at, size, _)| (at, size)), true, after.or(now).map(|n| n.2))
        })
        .collect()
}

/// How many of `done` were read from their files now.
fn read_now(done: &[Done]) -> usize {
    done.iter().filter(|d| d.3).count()
}

/// What was found, as [`Found`] gives it: `read` is [`read_now`].
fn found_of(done: Vec<Done>, read: usize, not_kept: Option<String>) -> Found {
    let mut files = Vec::with_capacity(done.len());
    let mut modified = Vec::with_capacity(done.len());
    let mut templates = Vec::new();
    for (found, template, _, _, at) in done {
        files.push(found);
        modified.push(at);
        templates.extend(template);
    }
    Found { files, templates: bw::newest_templates(templates), read, not_kept, modified }
}

/// One file found: what it says, its template, its stamp when what was read now is to be kept, and whether it was read
/// now.
/// A file found: what it says, its template, its stamp to keep (when it's to be kept), whether it was read now, and
/// when it was last changed.
type Done = (FoundPreset, Option<Template>, Option<(i64, i64)>, bool, Option<SystemTime>);

/// Why the store couldn't be opened as it is.
enum Damage {
    /// It's another app's file: left as it is.
    Foreign,
    /// Damaged (SQLite says so): made again, empty.
    Damaged(String),
    /// Busy, or couldn't be read just now: left as it is.
    Busy(String),
}

fn under_any<'a>(p: &Path, folders: impl IntoIterator<Item = &'a PathBuf>) -> bool {
    folders.into_iter().any(|f| p.starts_with(f))
}

/// The preset files under `roots` and their templates, through the store at `store` when it can be opened (`Err`
/// says why not: every file is then read, as before). Files kept from folders not in `roots` are left as they are.
pub fn find(roots: &[PathBuf], store: &Path) -> (Found, Option<String>) {
    find_in(roots, store, false)
}

/// [`find`] when `roots` are every folder the page looks in: files kept from any other folder are forgotten.
pub fn find_every(roots: &[PathBuf], store: &Path) -> (Found, Option<String>) {
    find_in(roots, store, true)
}

/// [`find_every`] without writing anything, for the CLAP plugin (which reads the app's files and never writes them):
/// a file unchanged since the store at `store` kept it is taken from it, the rest are read now and not kept. With no
/// store, or one that can't be used as it is (another layout or reader version, busy or damaged), every file is read,
/// and the second half says why. Call it off the window's thread (from `mo_app::background::on_pool`, as it reads over
/// every core of the current rayon pool).
pub fn find_read_only(roots: &[PathBuf], store: &Path) -> (Found, Option<String>) {
    let (paths, _) = list_preset_files_and_unread(roots);
    let (kept, why) = match read_kept(store) {
        Ok(kept) => (kept, None),
        Err(why) => (HashMap::new(), Some(why)),
    };
    let done = look(&paths, &kept);
    let read = read_now(&done);
    (found_of(done, read, None), why)
}

/// What the store at `path` holds, opened read-only (nothing made, emptied or changed), when it's this app's store of
/// this layout read by these readers.
fn read_kept(path: &Path) -> Result<HashMap<String, (i64, i64, Entry)>, String> {
    if !path.is_file() {
        return Err(format!("{} isn't there yet", path.display()));
    }
    let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(path, flags).map_err(|e| e.to_string())?;
    let set_up = || -> rusqlite::Result<(i64, i64)> {
        conn.busy_timeout(Duration::from_secs(2))?;
        conn.pragma_update(None, "query_only", true)?;
        conn.pragma_update(None, "temp_store", "MEMORY")?;
        let id: i64 = conn.pragma_query_value(None, "application_id", |r| r.get(0))?;
        let layout: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        Ok((id, layout))
    };
    let (id, layout) = set_up().map_err(|e| e.to_string())?;
    if id != APPLICATION_ID || layout != LAYOUT {
        return Err(format!("{} isn't the app's preset list of this layout", path.display()));
    }
    let version = format!("{VERSION}/bitwig {}", mo_formats::bitwig::VERSION);
    let kept: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key = 'version'", [], |r| r.get(0))
        .optional()
        .map_err(|e| e.to_string())?;
    if kept.as_deref() != Some(version.as_str()) {
        return Err(format!("{} was read by other readers", path.display()));
    }
    load_from(&conn).map_err(|e| e.to_string())
}

fn find_in(roots: &[PathBuf], store: &Path, every_root: bool) -> (Found, Option<String>) {
    match FileStore::open(store) {
        Ok(mut s) => (s.find(roots, every_root), None),
        Err(e) => {
            let files = crate::library::find_preset_files(roots);
            let templates = bw::templates(&files);
            let read = files.len();
            let modified = files.iter().map(|f| stamp(Path::new(&f.path)).map(|s| s.2)).collect();
            (Found { files, templates, read, not_kept: None, modified }, Some(e))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mo-presets-file-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("test folder");
        dir
    }

    /// A VST2 program: "CcnK", size, "FxCk", version, the plugin's four letters, its version, parameter count, name.
    fn fxp(plugin: &[u8; 4], name: &str) -> Vec<u8> {
        let mut b = b"CcnK".to_vec();
        b.extend(0u32.to_be_bytes());
        b.extend(b"FxCk");
        b.extend(1u32.to_be_bytes());
        b.extend(plugin);
        b.extend(1u32.to_be_bytes());
        b.extend(0u32.to_be_bytes());
        let mut n = name.as_bytes().to_vec();
        n.resize(28, 0);
        b.extend(n);
        b
    }

    #[test]
    fn a_second_run_reads_only_new_and_changed_files_and_forgets_gone_ones() {
        let dir = folder("runs");
        let presets = dir.join("presets");
        std::fs::create_dir_all(presets.join("Synth")).expect("folders");
        std::fs::write(presets.join("Synth/Lead.fxp"), fxp(b"Abcd", "Lead")).expect("write");
        std::fs::write(presets.join("Synth/Pad.fxp"), fxp(b"Abcd", "Pad")).expect("write");
        std::fs::write(presets.join("Synth/Broken.vstpreset"), b"nope").expect("write");
        let store = dir.join("data").join(FILE_NAME);
        let roots = [presets.clone()];

        let (first, why) = find(&roots, &store);
        assert_eq!(why, None);
        assert_eq!((first.files.len(), first.read, first.not_kept.as_deref()), (3, 3, None));
        assert_eq!(first.files, crate::library::find_preset_files(&roots), "the same as reading every file");

        let (second, _) = find(&roots, &store);
        assert_eq!(second.read, 0, "nothing changed");
        assert_eq!(second.files, first.files);

        std::fs::write(presets.join("Synth/Pad.fxp"), fxp(b"Abcd", "Pad two")).expect("write");
        std::fs::remove_file(presets.join("Synth/Lead.fxp")).expect("remove");
        let (third, _) = find(&roots, &store);
        assert_eq!(third.read, 1);
        assert_eq!(third.files, crate::library::find_preset_files(&roots));
        let rows: i64 = Connection::open(&store)
            .and_then(|c| c.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0)))
            .expect("rows");
        assert_eq!(rows, 2, "the removed file is forgotten");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reading_without_writing_uses_what_was_kept_and_changes_nothing() {
        let dir = folder("read-only");
        let presets = dir.join("presets");
        std::fs::create_dir_all(&presets).expect("folders");
        std::fs::write(presets.join("Lead.fxp"), fxp(b"Abcd", "Lead")).expect("write");
        let store = dir.join("data").join(FILE_NAME);
        let roots = [presets.clone()];

        // No store yet: every file is read, and none is made.
        let (first, why) = find_read_only(&roots, &store);
        assert!(why.is_some(), "it says why the files were all read");
        assert_eq!((first.files.len(), first.read), (1, 1));
        assert!(!store.exists(), "no store is made");

        let (_, why) = find(&roots, &store);
        assert_eq!(why, None);
        std::fs::write(presets.join("Pad.fxp"), fxp(b"Abcd", "Pad")).expect("write");
        let before = std::fs::read(&store).expect("store");
        let (second, why) = find_read_only(&roots, &store);
        assert_eq!(why, None);
        assert_eq!(second.read, 1, "only the new file is read");
        assert_eq!(second.files, crate::library::find_preset_files(&roots));
        assert_eq!(std::fs::read(&store).expect("store"), before, "the store is as it was");
        let rows: i64 = Connection::open(&store)
            .and_then(|c| c.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0)))
            .expect("rows");
        assert_eq!(rows, 1, "the new file isn't kept");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn files_on_a_folder_thats_away_are_kept_and_not_read_again() {
        let dir = folder("away");
        let (here, away) = (dir.join("here"), dir.join("away"));
        std::fs::create_dir_all(&here).expect("folders");
        std::fs::create_dir_all(&away).expect("folders");
        std::fs::write(here.join("Lead.fxp"), fxp(b"Abcd", "Lead")).expect("write");
        std::fs::write(away.join("Pad.fxp"), fxp(b"Abcd", "Pad")).expect("write");
        let store = dir.join("data").join(FILE_NAME);
        let roots = [here.clone(), away.clone()];
        let (first, _) = find(&roots, &store);
        assert_eq!(first.read, 2);
        assert_eq!(first.modified.len(), 2);
        assert!(first.modified.iter().all(Option::is_some), "each file's time comes with it");

        // The drive is unplugged: its file isn't listed, but it isn't forgotten either.
        let hidden = dir.join("away-unplugged");
        std::fs::rename(&away, &hidden).expect("rename");
        let (second, _) = find(&roots, &store);
        assert_eq!((second.files.len(), second.read), (1, 0));
        std::fs::rename(&hidden, &away).expect("rename back");
        let (third, _) = find(&roots, &store);
        assert_eq!((third.files.len(), third.read), (2, 0), "back, and not read again");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_no_longer_looked_in_is_forgotten_only_when_every_folder_is_given() {
        let dir = folder("dropped");
        let (one, two) = (dir.join("one"), dir.join("two"));
        std::fs::create_dir_all(&one).expect("folders");
        std::fs::create_dir_all(&two).expect("folders");
        std::fs::write(one.join("Lead.fxp"), fxp(b"Abcd", "Lead")).expect("write");
        std::fs::write(two.join("Pad.fxp"), fxp(b"Abcd", "Pad")).expect("write");
        let store = dir.join("data").join(FILE_NAME);
        let rows = || -> i64 {
            Connection::open(&store)
                .and_then(|c| c.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0)))
                .expect("rows")
        };
        find_every(&[one.clone(), two.clone()], &store);
        assert_eq!(rows(), 2);
        // Some of the folders only (Send to Bitwig's look): the other's files stay.
        find(std::slice::from_ref(&one), &store);
        assert_eq!(rows(), 2);
        // Every folder the page looks in, and two isn't one now: its file goes (X1-2).
        find_every(std::slice::from_ref(&one), &store);
        assert_eq!(rows(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn files_behind_a_link_to_a_folder_thats_away_are_kept() {
        let dir = folder("link");
        let (root, moved) = (dir.join("root"), dir.join("moved"));
        std::fs::create_dir_all(&root).expect("folders");
        std::fs::create_dir_all(&moved).expect("folders");
        std::fs::write(moved.join("Pad.fxp"), fxp(b"Abcd", "Pad")).expect("write");
        std::os::unix::fs::symlink(&moved, root.join("Synth")).expect("link");
        let store = dir.join("data").join(FILE_NAME);
        let roots = [root.clone()];
        let (first, _) = find_every(&roots, &store);
        assert_eq!(first.files.len(), 1);
        // The drive the folder moved to is away: the link leads nowhere, and its file is kept (M-4).
        let away = dir.join("moved-away");
        std::fs::rename(&moved, &away).expect("rename");
        let (second, _) = find_every(&roots, &store);
        assert_eq!(second.files.len(), 0);
        std::fs::rename(&away, &moved).expect("rename back");
        let (third, _) = find_every(&roots, &store);
        assert_eq!((third.files.len(), third.read), (1, 0), "back, and not read again");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_damaged_store_is_made_again_and_another_apps_file_is_left() {
        let dir = folder("damaged");
        let store = dir.join(FILE_NAME);
        std::fs::write(&store, vec![9u8; 4096]).expect("write");
        assert!(FileStore::open(&store).is_ok());

        let other = dir.join("other.db");
        let conn = Connection::open(&other).expect("open");
        conn.execute_batch("CREATE TABLE mine (x); INSERT INTO mine VALUES (1);").expect("make");
        drop(conn);
        assert!(FileStore::open(&other).is_err());
        let n: i64 = Connection::open(&other)
            .and_then(|c| c.query_row("SELECT COUNT(*) FROM mine", [], |r| r.get(0)))
            .expect("still there");
        assert_eq!(n, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
