//! What the library knows about plugins, for the Plugins page: every plugin each save uses, the installed copies the
//! C# app's plugin scanner read, and how much room each installed plugin takes as the C# app last measured it.
//! Read-only, like the rest of this crate: nothing here can change `library.db`.

use std::collections::HashMap;

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
    /// When the save was last written, as Unix seconds.
    pub mtime: i64,
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

/// A plugin's room on disk as the C# app measured it (table `plugin_space`).
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
        let mut st = self.conn.prepare(&format!(
            "SELECT pl.project_id, pl.name, pl.name_key, pl.vendor, pl.kind, pl.format, pl.installed, p.daw, p.mtime
             FROM plugins pl JOIN projects p ON p.id = pl.project_id WHERE {present}"
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
                mtime: int(r, "mtime").unwrap_or(0),
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The names of the plugins on this computer, as the C# app last found them in the plugin folders.
    pub fn installed_names(&self) -> Result<Vec<String>, LibraryError> {
        if !self.has("installed_plugins") {
            return Ok(Vec::new());
        }
        let mut st = self.conn.prepare("SELECT name FROM installed_plugins ORDER BY name")?;
        Ok(st.query_map([], |r| Ok(text(r, "name")))?.collect::<Result<_, _>>()?)
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
