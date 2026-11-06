//! The app measuring your plugins itself (review X1-2): which plugins are installed, from the plugin files in your
//! plugin folders, and how much room each takes, with the folders you added and without the ones you said aren't its
//! (`plugin_space_added:<key>`, `plugin_space_not:<key>`). The C# app did this, and nothing had since it was retired,
//! so a new plugin was never found, an uninstalled one never went and sizes never changed.
//!
//! [`measure`] reads the disk and keeps nothing; [`mo_library::plugin_writes::write`] keeps what it found in the app's
//! own `songs.db`, in place of the last measure. Nothing here removes or moves anything.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use chrono::{NaiveDateTime, TimeDelta};
use mo_library::Library;
use mo_library::plugin_writes::Measure;
use mo_library::plugins::{StoredFolder, StoredSpace, added_folders_key, ruled_out_key};
use mo_space::plugin_space::{self, Choices, PluginSpace, SpaceKind};
use mo_space::plugins::{aax_folders, default_folders, scan_files};

/// A measure older than this is taken again at the app's next look at the plugins.
pub const AGAIN_AFTER: TimeDelta = TimeDelta::days(1);

/// When the app measures: what the pages say while nothing is measured.
pub const WHEN: &str = "the app measures them once a day, when it looks at your plugins, or when you press Measure \
                        again on the Plugins page.";

/// Whether the measure taken at `measured` (none: never) is due again at `now`.
pub fn due(measured: Option<NaiveDateTime>, now: NaiveDateTime) -> bool {
    measured.is_none_or(|t| now - t >= AGAIN_AFTER || t > now)
}

/// The system's folders a measure looks in, besides the ones Settings lists: given, so a test never reaches a real
/// one (rule 11).
pub struct Roots {
    /// The standard plugin folders, used when Settings lists none ([`default_folders`]).
    pub standard: Vec<PathBuf>,
    /// The AAX folders, never on Settings' list ([`aax_folders`]).
    pub aax: Vec<PathBuf>,
    /// Where plugins keep presets and sounds ([`plugin_space::data_roots`]).
    pub data: Vec<PathBuf>,
}

impl Roots {
    /// This computer's.
    pub fn system() -> Roots {
        Roots { standard: default_folders(), aax: aax_folders(), data: plugin_space::data_roots() }
    }
}

/// Where to look and what's yours, read from the library before the disk is.
struct Places {
    /// Every plugin folder, as Settings lists them (or the standard ones), and the AAX folders.
    plugin_roots: Vec<PathBuf>,
    /// Your project, sample and MIDI folders: never a plugin's.
    protected: Vec<String>,
    /// Each plugin's maker as the plugin scanner read it, by name key.
    vendors: HashMap<String, String>,
}

fn places(lib: &Library, roots: &Roots) -> Result<Places, String> {
    let chosen = lib.strict_list_setting("plugin_folders").map_err(|e| e.to_string())?;
    // A folder of yours that isn't there (a drive not plugged in) would take every plugin in it off the list.
    if let Some(gone) = chosen.iter().find(|c| !c.trim().is_empty() && !Path::new(c).is_dir()) {
        return Err(format!(
            "your plugin folder {gone} isn't there (a drive not plugged in?), so the last measure stays"
        ));
    }
    let plugin_roots = crate::space_spare::plugin_folders(&chosen, roots.standard.clone(), roots.aax.clone());
    let mut protected = lib.protected_roots().map_err(|e| e.to_string())?;
    protected.extend(lib.strict_list_setting("midi_folders").map_err(|e| e.to_string())?);
    let vendors = lib.scanned_vendors().map_err(|e| e.to_string())?;
    Ok(Places { plugin_roots, protected, vendors })
}

/// Your say on each plugin's folders, by name key: (added, ruled out). A list that can't be read is an error, never no
/// say: a folder you ruled out would be the plugin's again, and could be ticked in Clean up.
type Say = HashMap<String, (Vec<String>, Vec<String>)>;

fn say(lib: &Library, keys: impl Iterator<Item = String>) -> Result<Say, String> {
    let mut out = Say::new();
    for key in keys {
        let added = lib.strict_list_setting(&added_folders_key(&key)).map_err(|e| e.to_string())?;
        let not = lib.strict_list_setting(&ruled_out_key(&key)).map_err(|e| e.to_string())?;
        out.insert(key, (added, not));
    }
    Ok(out)
}

/// Measures the plugins installed now, for the library at `library` (read-only, and closed while the disk is read),
/// in your plugin folders and `roots`. With one of your plugin folders not there, or no plugin file found at all, it's
/// an error, so the last measure stays rather than those plugins going. Reads and measures every plugin folder: call
/// it away from the window's thread, on the bulk pool.
pub fn measure(library: &Path, roots: &Roots, now: NaiveDateTime, stop: &AtomicBool) -> Result<Measure, String> {
    let stopped = |_| "Measuring stopped.".to_owned();
    let places = places(&Library::open_read_only(library).map_err(|e| e.to_string())?, roots)?;
    let files = scan_files(&places.plugin_roots, stop).map_err(stopped)?;
    if files.is_empty() {
        return Err("no plugin files were found in your plugin folders, so the last measure stays".to_owned());
    }
    let roots_data = roots.data.clone();
    let roots: Vec<String> = places.plugin_roots.iter().map(|r| r.to_string_lossy().into_owned()).collect();
    let installed = plugin_space::group(&files, &roots, &places.vendors);
    let say =
        say(&Library::open_read_only(library).map_err(|e| e.to_string())?, installed.iter().map(|p| p.key.clone()))?;
    let added = |key: &str| say.get(key).map(|s| s.0.clone()).unwrap_or_default();
    let ruled_out = |key: &str| say.get(key).map(|s| s.1.clone()).unwrap_or_default();
    let choices = Choices { added: &added, ruled_out: &ruled_out };
    let spaces = plugin_space::measure(&installed, &roots_data, &roots, &places.protected, &choices, None, stop)
        .map_err(stopped)?;
    Ok(Measure {
        installed: installed.iter().map(|p| p.name.clone()).collect(),
        spaces: spaces.iter().map(|s| stored(s, now)).collect(),
    })
}

/// A plugin's room in the shape the library keeps it.
fn stored(s: &PluginSpace, now: NaiveDateTime) -> StoredSpace {
    StoredSpace {
        key: s.key.clone(),
        name: s.name.clone(),
        vendor: s.vendor.clone(),
        folders: s
            .folders
            .iter()
            .map(|f| StoredFolder {
                path: f.path.clone(),
                kind: match f.kind {
                    SpaceKind::Program => 0,
                    SpaceKind::Presets => 1,
                    SpaceKind::Sounds => 2,
                },
                bytes: f.bytes,
                shared_with: f.shared_with.clone(),
                added: f.added,
            })
            .collect(),
        measured: Some(now),
    }
}

/// What the page says once a measure is kept: how many plugins it found, and how much room they take.
pub fn done_text(measure: &Measure) -> String {
    let bytes = measure
        .spaces
        .iter()
        .flat_map(|s| &s.folders)
        .filter(|f| f.shared_with.is_empty())
        .fold(0u64, |a, f| a.saturating_add(f.bytes));
    format!(
        "Found {} installed, taking {} (folders two plugins share left out).",
        super::plural(measure.installed.len(), "plugin", "plugins"),
        mo_space::size_text(bytes)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn at(day: u32, hour: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 11, day).unwrap().and_hms_opt(hour, 0, 0).unwrap()
    }

    /// A plugin file, its guessed presets folder, a folder you added and one you ruled out: the measure finds the
    /// plugin, counts the first three and leaves the last out.
    #[test]
    fn a_measure_finds_the_plugins_and_follows_your_say() {
        let d = std::env::temp_dir().join(format!("mo-app-plugin-measure-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let (vst3, docs, mine) = (d.join("VST3"), d.join("Docs"), d.join("Elsewhere").join("My Bank"));
        for f in [vst3.join("Serum.vst3"), docs.join("Serum Presets"), docs.join("Serum Sounds"), mine.clone()] {
            std::fs::create_dir_all(&f).unwrap();
        }
        std::fs::write(vst3.join("Serum.vst3").join("Serum.dll"), [0u8; 100]).unwrap();
        std::fs::write(docs.join("Serum Presets").join("a.fxp"), [0u8; 200]).unwrap();
        std::fs::write(docs.join("Serum Sounds").join("b.wav"), [0u8; 300]).unwrap();
        std::fs::write(mine.join("c.fxp"), [0u8; 400]).unwrap();
        let s = |p: &Path| p.to_string_lossy().into_owned();
        let list = |p: &Path| serde_json::to_string(&[s(p)]).unwrap();
        let library = d.join("library.db");
        let conn = rusqlite::Connection::open(&library).unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
             CREATE TABLE projects (id INTEGER PRIMARY KEY, path TEXT, name TEXT, daw TEXT);",
        )
        .unwrap();
        for (k, v) in [
            ("plugin_folders".to_owned(), list(&vst3)),
            (added_folders_key("serum"), list(&mine)),
            (ruled_out_key("serum"), list(&docs.join("Serum Sounds"))),
        ] {
            conn.execute("INSERT INTO settings VALUES (?1, ?2)", [k, v]).unwrap();
        }
        drop(conn);
        let stop = AtomicBool::new(false);
        let roots = Roots { standard: Vec::new(), aax: Vec::new(), data: vec![docs.clone()] };
        let m = measure(&library, &roots, at(6, 20), &stop).unwrap();
        assert_eq!(m.installed, ["Serum"]);
        assert_eq!(m.spaces.len(), 1, "{m:?}");
        let serum = &m.spaces[0];
        assert_eq!(serum.measured, Some(at(6, 20)));
        let paths: Vec<(&str, u8, bool)> =
            serum.folders.iter().map(|f| (mo_formats::paths::file_name(&f.path), f.kind, f.added)).collect();
        assert_eq!(paths, [("Serum.vst3", 0, false), ("Serum Presets", 1, false), ("My Bank", 1, true)], "{m:?}");
        assert!(done_text(&m).starts_with("Found 1 plugin installed"), "{}", done_text(&m));
        // No plugin file found at all, or a plugin folder of yours not there: an error, so the last measure stays.
        std::fs::remove_dir_all(vst3.join("Serum.vst3")).unwrap();
        assert!(measure(&library, &roots, at(6, 20), &stop).unwrap_err().contains("no plugin files"));
        std::fs::remove_dir_all(&vst3).unwrap();
        assert!(measure(&library, &roots, at(6, 20), &stop).unwrap_err().contains("isn't there"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_measure_is_due_after_a_day_or_when_never_taken() {
        assert!(due(None, at(6, 20)));
        assert!(!due(Some(at(6, 9)), at(6, 20)));
        assert!(due(Some(at(5, 20)), at(6, 20)));
        // One from the future (the clock was wrong) is taken again.
        assert!(due(Some(at(8, 9)), at(6, 20)));
    }
}
