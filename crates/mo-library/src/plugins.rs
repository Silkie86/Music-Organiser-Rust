//! What the library knows about plugins, for the Plugins page: every plugin each save uses, the installed copies the
//! C# app's plugin scanner read, and how much room each installed plugin takes as the C# app last measured it.
//! Read-only, like the rest of this crate: nothing here can change `library.db`.

use std::collections::{HashMap, HashSet};

use chrono::NaiveDateTime;
use serde::Deserialize;

use crate::PluginKind;
use crate::db::{Library, LibraryError, int, local, plugin_kind, text};
use crate::read::name_key;

/// One plugin in one save, as the library stores it (a row of `plugins` with its save's DAW and date).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginUse {
    pub save: i64,
    pub name: String,
    /// [`name_key`] of the name, as the C# app stored it.
    pub key: String,
    pub vendor: String,
    pub kind: PluginKind,
    /// "VST2", "VST3", "CLAP", "AU", "Native" (the DAW's own device) …
    pub format: String,
    pub installed: bool,
    /// The save's DAW: "FL Studio", "Bitwig" …
    pub daw: String,
}

/// One plugin inside an installed plugin file, as the C# app's plugin scanner read it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScannedPlugin {
    /// The plugin file.
    pub path: String,
    /// "VST3", "CLAP" or "VST2".
    pub format: String,
    /// The exact id projects save it under: a VST3 class id, a CLAP id or a VST2's four letters.
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    /// "instrument", "effect", "note-effect", "analyzer", or "" when the file doesn't say.
    pub kind: String,
    /// The shell file it lives in (a WaveShell), if any.
    pub shell: String,
    /// How many parameters it has, once the scanner has loaded it to read them.
    pub parameters: Option<usize>,
    /// Why the scanner couldn't load it, if it couldn't.
    pub error: String,
}

/// How far the C# app's plugin scanner has got: plugin files read, the plugins inside them, files it couldn't read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScanCounts {
    pub files: u32,
    pub plugins: u32,
    pub failed: u32,
}

/// A plugin's room on disk as last measured (table `plugin_space`): by the app ([`crate::plugin_writes`]), or before
/// that by the C# app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredSpace {
    pub key: String,
    pub name: String,
    pub vendor: String,
    pub folders: Vec<StoredFolder>,
    pub measured: Option<NaiveDateTime>,
}

/// One of a measured plugin's files or folders.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredFolder {
    pub path: String,
    /// 0 the plugin's own files, 1 presets and settings, 2 sounds (the C# `SpaceKind`).
    pub kind: u8,
    pub bytes: u64,
    /// The other plugins (by name key) whose folder it also is.
    pub shared_with: Vec<String>,
    /// A folder you added to the plugin yourself.
    pub added: bool,
}

/// Settings where the C# app keeps your say on a plugin's folders.
pub fn added_folders_key(key: &str) -> String {
    format!("plugin_space_added:{key}")
}

pub fn ruled_out_key(key: &str) -> String {
    format!("plugin_space_not:{key}")
}

/// The setting for how many years without a save make a plugin unused (the C# `LibraryDb.UnusedYearsKey`).
pub const UNUSED_YEARS_KEY: &str = "space_unused_years";

/// The setting for how many Install these cards the Plugins page shows.
pub const NEEDED_COUNT_KEY: &str = "plugins_needed_cards";

impl Library {
    /// Every plugin every save uses, leaving out saves whose file has gone (as the C# Plugins page does). One row per
    /// plugin in a save, so a save with Serum twice has it twice.
    pub fn plugin_uses(&self) -> Result<Vec<PluginUse>, LibraryError> {
        if !self.has("plugins") {
            return Ok(Vec::new());
        }
        let present = if self.columns.contains("file_missing") { "COALESCE(p.file_missing, 0) = 0" } else { "1" };
        // An older app's plugins table may lack some columns (name_key came later); those read as empty.
        let have: HashSet<String> = self
            .conn
            .prepare("SELECT name FROM pragma_table_info('plugins')")?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<_, _>>()?;
        if !have.contains("project_id") || !have.contains("name") {
            return Ok(Vec::new());
        }
        let picked: Vec<String> = ["project_id", "name", "name_key", "vendor", "kind", "format", "installed"]
            .into_iter()
            .filter(|c| have.contains(*c))
            .map(|c| format!("pl.{c}"))
            .collect();
        let mut st = self.conn.prepare(&format!(
            "SELECT {}, p.daw FROM plugins pl JOIN projects p ON p.id = pl.project_id WHERE {present}",
            picked.join(", ")
        ))?;
        let rows = st.query_map([], |r| {
            let name = text(r, "name");
            // A row an older app wrote without its key gets one, worked out the same way.
            let key = match text(r, "name_key") {
                k if k.is_empty() => name_key(&name),
                k => k,
            };
            Ok(PluginUse {
                save: int(r, "project_id").unwrap_or(0),
                key,
                name,
                vendor: text(r, "vendor"),
                kind: plugin_kind(int(r, "kind").unwrap_or(0)),
                format: text(r, "format"),
                installed: int(r, "installed") == Some(1),
                daw: text(r, "daw"),
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Your own folders, which Clean up never touches (nor a folder holding one, nor one inside): the project folders
    /// and the sample folders, as the C# app's `ProtectedRoots`.
    pub fn protected_roots(&self) -> Result<Vec<String>, LibraryError> {
        let mut out = self.roots()?;
        out.extend(self.strict_list_setting("sample_folders")?);
        let mut seen = HashSet::new();
        out.retain(|p| !p.trim().is_empty() && seen.insert(crate::scan::path_key(p)));
        Ok(out)
    }

    /// Every sample any project uses, for Clean up: a folder holding one never goes. Reads the samples table once, so
    /// call it away from the window's thread.
    pub fn sample_index(&self) -> Result<SampleIndex, LibraryError> {
        let mut index = SampleIndex::default();
        if !self.has("samples") {
            return Ok(index);
        }
        let mut st = self.conn.prepare("SELECT DISTINCT path FROM samples")?;
        for path in st.query_map([], |r| Ok(text(r, "path")))? {
            index.add(&path?);
        }
        Ok(index)
    }

    /// The names of the plugins on this computer, as the app last found them in the plugin folders
    /// ([`crate::plugin_writes`]; before that, the C# app).
    pub fn installed_names(&self) -> Result<Vec<String>, LibraryError> {
        if !self.has("installed_plugins") {
            return Ok(Vec::new());
        }
        let mut st = self.conn.prepare("SELECT name FROM installed_plugins ORDER BY name")?;
        Ok(st.query_map([], |r| Ok(text(r, "name")))?.collect::<Result<_, _>>()?)
    }

    /// When the oldest plugin's room was measured, if any was.
    pub fn oldest_plugin_measure(&self) -> Result<Option<NaiveDateTime>, LibraryError> {
        if !self.has("plugin_space") {
            return Ok(None);
        }
        let oldest: Option<i64> =
            self.conn.query_row("SELECT MIN(measured) FROM plugin_space WHERE measured > 0", [], |r| r.get(0))?;
        Ok(oldest.and_then(local))
    }

    /// Each plugin's maker as the plugin scanner read it from its files, by name key (the first one found).
    pub fn scanned_vendors(&self) -> Result<HashMap<String, String>, LibraryError> {
        if !self.has("scanned_plugins") {
            return Ok(HashMap::new());
        }
        let mut st = self.conn.prepare("SELECT name_key, vendor FROM scanned_plugins WHERE vendor != ''")?;
        let mut out = HashMap::new();
        for row in st.query_map([], |r| Ok((text(r, "name_key"), text(r, "vendor"))))? {
            let (key, vendor) = row?;
            out.entry(key).or_insert(vendor);
        }
        Ok(out)
    }

    /// The installed plugins a project's plugin name refers to, as the C# app's `ScannedPluginsNamed`: the same name,
    /// or the same name with or without its maker in front ("FabFilter Pro-Q 3" and "Pro-Q 3"). The same name first,
    /// then by format, name and file.
    pub fn scanned_named(&self, name: &str) -> Result<Vec<ScannedPlugin>, LibraryError> {
        let key = name_key(name);
        if key.is_empty() || !self.has("scanned_plugins") {
            return Ok(Vec::new());
        }
        // The keys first, from the index alone: reading every row's parameters to match them froze the C# app.
        let mut st = self.conn.prepare("SELECT DISTINCT name_key FROM scanned_plugins")?;
        let keys: Vec<String> = st
            .query_map([], |r| Ok(text(r, "name_key")))?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|k| keys_match(k, &key))
            .collect();
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let marks = (2..keys.len() + 2).map(|i| format!("?{i}")).collect::<Vec<_>>().join(", ");
        let mut st = self.conn.prepare(&format!(
            "SELECT path, format, id, name, vendor, version, kind,
                    CASE WHEN json_valid(json) THEN COALESCE(json_extract(json, '$.shell'), '') ELSE '' END AS shell,
                    CASE WHEN json_valid(json) AND json_type(json, '$.params') = 'array'
                         THEN json_array_length(json, '$.params') END AS params,
                    CASE WHEN json_valid(json) THEN COALESCE(json_extract(json, '$.error'), '') ELSE '' END AS error
             FROM scanned_plugins WHERE name_key IN ({marks})
             ORDER BY name_key = ?1 DESC, format, name, path"
        ))?;
        let args: Vec<&dyn rusqlite::ToSql> = std::iter::once(&key as &dyn rusqlite::ToSql)
            .chain(keys.iter().map(|k| k as &dyn rusqlite::ToSql))
            .collect();
        let rows = st.query_map(args.as_slice(), |r| {
            Ok(ScannedPlugin {
                path: text(r, "path"),
                format: text(r, "format"),
                id: text(r, "id"),
                name: text(r, "name"),
                vendor: text(r, "vendor"),
                version: text(r, "version"),
                kind: text(r, "kind"),
                shell: text(r, "shell"),
                parameters: int(r, "params").and_then(|n| usize::try_from(n).ok()),
                error: text(r, "error"),
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// How far the plugin scanner has got, as the C# app's `PluginScanCounts` (other DLLs in VST2 folders, which
    /// aren't plugins, don't count as failed).
    pub fn scan_counts(&self) -> Result<ScanCounts, LibraryError> {
        if !self.has("plugin_scans") || !self.has("scanned_plugins") {
            return Ok(ScanCounts::default());
        }
        let n = |v: Option<i64>| v.and_then(|n| u32::try_from(n).ok()).unwrap_or(0);
        Ok(self.conn.query_row(
            "SELECT (SELECT COUNT(*) FROM plugin_scans) AS files, (SELECT COUNT(*) FROM scanned_plugins) AS plugins,
                    (SELECT COUNT(*) FROM plugin_scans WHERE error != '' AND error NOT LIKE 'not a VST2 plugin%'
                       AND path NOT IN (SELECT path FROM scanned_plugins)) AS failed",
            [],
            |r| {
                Ok(ScanCounts { files: n(int(r, "files")), plugins: n(int(r, "plugins")), failed: n(int(r, "failed")) })
            },
        )?)
    }

    /// Every installed plugin's room on disk as last measured, by name key. A row whose folders can't be read has
    /// none (it reads as not measured), rather than failing the page.
    pub fn plugin_spaces(&self) -> Result<HashMap<String, StoredSpace>, LibraryError> {
        if !self.has("plugin_space") {
            return Ok(HashMap::new());
        }
        let mut st = self.conn.prepare("SELECT * FROM plugin_space")?;
        let rows = st.query_map([], |r| {
            let measured = int(r, "measured").filter(|&t| t > 0).and_then(local);
            Ok(StoredSpace {
                key: text(r, "name_key"),
                name: text(r, "name"),
                vendor: text(r, "vendor"),
                folders: folders(&text(r, "folders")),
                measured,
            })
        })?;
        let mut out = HashMap::new();
        for row in rows {
            let s = row?;
            out.insert(s.key.clone(), s);
        }
        Ok(out)
    }
}

/// Whether a scanned plugin's key is the project's plugin's, as the C# app matches them: the same, or one ends with
/// the other (the maker's name in front), the shorter at least four letters.
fn keys_match(scanned: &str, wanted: &str) -> bool {
    scanned == wanted
        || (scanned.len() >= 4 && wanted.len() > scanned.len() && wanted.ends_with(scanned))
        || (wanted.len() >= 4 && scanned.len() > wanted.len() && scanned.ends_with(wanted))
}

/// The C# app's folder list, written by System.Text.Json: `[{"Path": …, "Kind": 0, "Bytes": …, "SharedWith": […],
/// "Added": false}]`. The kind may be a number or its name.
fn folders(json: &str) -> Vec<StoredFolder> {
    #[derive(Deserialize)]
    struct Row {
        #[serde(rename = "Path", default)]
        path: String,
        #[serde(rename = "Kind", default)]
        kind: serde_json::Value,
        #[serde(rename = "Bytes", default)]
        bytes: i64,
        #[serde(rename = "SharedWith", default)]
        shared_with: Option<Vec<String>>,
        #[serde(rename = "Added", default)]
        added: bool,
    }
    let Ok(rows) = serde_json::from_str::<Vec<Row>>(json) else { return Vec::new() };
    rows.into_iter()
        .map(|r| StoredFolder {
            kind: match &r.kind {
                serde_json::Value::Number(n) => n.as_u64().and_then(|n| u8::try_from(n).ok()).unwrap_or(0).min(2),
                serde_json::Value::String(s) => match s.as_str() {
                    "Presets" => 1,
                    "Sounds" => 2,
                    _ => 0,
                },
                _ => 0,
            },
            path: r.path,
            bytes: u64::try_from(r.bytes).unwrap_or(0),
            shared_with: r.shared_with.unwrap_or_default(),
            added: r.added,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sample_holds_its_folder_on_any_drive_letter_and_by_name() {
        let mut index = SampleIndex::default();
        index.add(r"F:\Splice\Kicks\Kick 1.WAV");
        index.add("/Users/me/Samples/snare.wav");
        assert_eq!(index.inside(r"K:\Splice").as_deref(), Some(r"F:\Splice\Kicks\Kick 1.WAV"));
        assert!(index.inside(r"K:\Splice\Kicks\").is_some());
        // Not a folder that only starts the same.
        assert!(index.inside(r"K:\Spl").is_none());
        assert!(index.inside(r"K:\Splice\Kicks\Kick 1.WAV\x").is_none());
        assert!(index.inside("/Users/me/Samples").is_some());
        assert!(index.inside("/Users/me/Sample").is_none());
        assert!(index.named("kick 1.wav") && index.named("SNARE.WAV") && !index.named("hat.wav"));
        assert!(index.inside("").is_none());
        // Windows' long-path prefix is the same folder.
        assert!(index.inside(r"\\?\D:\Splice\Kicks").is_some());
    }

    #[test]
    fn keys_match_with_or_without_the_maker() {
        assert!(keys_match("serum", "serum"));
        assert!(keys_match("proq3", "fabfilterproq3"));
        assert!(keys_match("fabfilterproq3", "proq3"));
        // Too short to trust.
        assert!(!keys_match("q3", "fabfilterproq3"));
        assert!(!keys_match("serum2", "serum"));
    }

    #[test]
    fn folders_read_as_the_csharp_app_writes_them() {
        let f = folders(
            r#"[{"Path":"C:\\VST\\Serum.dll","Kind":0,"Bytes":1000,"SharedWith":[],"Added":false},
                {"Path":"D:\\Serum Presets","Kind":"Presets","Bytes":50,"SharedWith":["serum2"],"Added":true},
                {"Path":"E:\\Sounds","Kind":2,"Bytes":-1}]"#,
        );
        assert_eq!(f.len(), 3);
        assert_eq!((f[0].kind, f[0].bytes, f[0].added), (0, 1000, false));
        assert_eq!((f[1].kind, f[1].shared_with.as_slice(), f[1].added), (1, ["serum2".to_owned()].as_slice(), true));
        assert_eq!((f[2].kind, f[2].bytes), (2, 0));
        assert!(folders("not json").is_empty());
    }
}

/// The samples projects use, as Clean up asks about them: is one inside this folder, and does any project use a file
/// of this name (projects keep the paths of the PC that saved them, so a name counts too).
#[derive(Clone, Debug, Default)]
pub struct SampleIndex {
    /// Each sample's path in [`sample_key`]'s form, with its path as stored.
    paths: Vec<(String, String)>,
    /// Each sample's file name, in small letters.
    names: HashSet<String>,
}

impl SampleIndex {
    pub fn add(&mut self, path: &str) {
        let k = sample_key(path);
        if k.is_empty() {
            return;
        }
        if let Some(name) = k.rsplit('/').next().filter(|n| !n.is_empty()) {
            self.names.insert(name.to_owned());
        }
        self.paths.push((k, path.to_owned()));
    }

    pub fn len(&self) -> usize {
        self.paths.len()
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// A sample some project uses that's inside `folder`, if any, as the C# `SampleInside`: the same folder on another
    /// drive letter counts too (a project saved when the drive was F: finds its samples now it's K:).
    pub fn inside(&self, folder: &str) -> Option<String> {
        let f = sample_key(folder);
        if f.is_empty() {
            return None;
        }
        let f = format!("{f}/");
        self.paths.iter().find(|(k, _)| k.starts_with(&f)).map(|(_, p)| p.clone())
    }

    /// Whether any project uses a file of this name (in any letter case).
    pub fn named(&self, name: &str) -> bool {
        self.names.contains(&mo_formats::case::key(name))
    }
}

/// A path to compare samples by: as the library compares paths (`/` between folders, no `/` at the end, letter case
/// ignored), without Windows' long-path prefix, and with a drive letter left out (`c:/beats/kick.wav` and
/// `k:/beats/kick.wav` are both `:/beats/kick.wav`).
fn sample_key(path: &str) -> String {
    let slashes = path.trim().replace('\\', "/");
    let plain = match (slashes.strip_prefix("//?/UNC/"), slashes.strip_prefix("//?/")) {
        (Some(network), _) => format!("//{network}"),
        (None, Some(local)) => local.to_owned(),
        (None, None) => slashes,
    };
    let mut k = mo_formats::case::key(&plain);
    while k.len() > 1 && k.ends_with('/') {
        k.pop();
    }
    let b = k.as_bytes();
    if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        k.remove(0);
    }
    k
}
