//! Reading one project file into a save as the library lists it, as the C# app's `Scanner.Analyse` does: what the
//! DAW's reader finds, which of its plugins are installed, and which of its samples are on this computer.
//!
//! This changes nothing on disk: the save it gives is held by the app until the library itself can be written. Its
//! key and note counts aren't worked out yet (they come with the notes reader), so they read as unknown.

use std::collections::HashSet;
use std::path::Path;
use std::sync::LazyLock;

use chrono::{DateTime, Local};
use mo_formats::paths::{file_name, stem};
use mo_formats::plugins::{elektron_device, is_fl_studio_bundled, vst2_id_in};
use mo_formats::{PluginKind, PluginRef, ProjectInfo, bitwig, fl, studio_one};
use regex::Regex;

use crate::samples::SampleLocator;
use crate::scan::{daw_of, mtime};
use crate::{ProjectPlugin, ProjectSample, Save, SaveDetail};

/// A plugin's name as the library matches names: lower-case letters and digits only, without the common endings, so
/// "Serum (x64)" matches "Serum" (the C# app's `NameKey`).
pub fn name_key(name: &str) -> String {
    static NOISE: LazyLock<Option<Regex>> = LazyLock::new(|| {
        Regex::new(
            r"\((x64|x86|64[- ]?bit|32[- ]?bit|vst3?|clap|au|mono|stereo|automap)\)|\b(x64|x86|vst3?|clap)\b|\bvst_(x64|x86)\b|_(x64|x86|64)\b",
        )
        .ok()
    });
    // The C# pattern's last part, "x64" straight after a letter ("Serumx64"), needs a look back the regex crate
    // doesn't have: the letter is kept and put back.
    static GLUED: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"([a-z])x64\b").ok());
    let mut n = name.to_lowercase();
    if let Some(re) = NOISE.as_ref() {
        n = re.replace_all(&n, "").into_owned();
    }
    if let Some(re) = GLUED.as_ref() {
        n = re.replace_all(&n, "$1").into_owned();
    }
    n.chars().filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit()).collect()
}

fn is_waves(name: &str, path: &str) -> bool {
    let lower = name.to_lowercase();
    lower.starts_with("waves ") || lower.starts_with("waveshell") || path.to_lowercase().contains("waveshell")
}

/// The plugins on this computer, as the library last found them.
#[derive(Clone, Debug, Default)]
pub struct Installed {
    /// Each installed plugin's [`name_key`].
    pub keys: HashSet<String>,
    /// The ids the plugin scanner found inside plugin files.
    pub ids: HashSet<String>,
    /// The VST2 ids inside Waves shells, which a project may save by its four letters.
    pub waves: HashSet<String>,
}

impl Installed {
    /// The ids found by the plugin scanner, from rows of (id, plugin file).
    pub fn with_scans(mut self, scans: impl IntoIterator<Item = (String, String)>) -> Installed {
        for (id, path) in scans.into_iter().filter(|(id, _)| !id.is_empty()) {
            if is_waves("", &path) {
                let vst2 = if id.len() == 32 { vst2_id_in(&id) } else { Some(id.clone()) };
                self.waves.extend(vst2);
            }
            self.ids.insert(id);
        }
        self
    }

    fn has_id(&self, id: &str) -> bool {
        self.ids.contains(id)
            || (if id.len() == 32 { vst2_id_in(id) } else { Some(id.to_owned()) })
                .is_some_and(|v| self.waves.contains(&v))
    }

    /// Whether the plugin is on this computer, by the C# app's rules: the DAW's own, its file there, part of FL
    /// Studio, its id found by the plugin scanner, or its name among the installed ones ("Vendor Name" against
    /// "Name" too). With no plugin scan yet, every plugin counts as installed rather than crying wolf. Asks the disk.
    pub fn has(&self, p: &PluginRef) -> bool {
        if p.format.eq_ignore_ascii_case("Native") {
            return true;
        }
        if !p.path.is_empty() && Path::new(&p.path).exists() {
            return true;
        }
        if is_fl_studio_bundled(&p.name, &p.vendor, &p.path) || self.keys.is_empty() {
            return true;
        }
        if !p.id.is_empty() && self.has_id(&p.id) {
            return true;
        }
        let key = name_key(&p.name);
        if self.keys.contains(&key) {
            return true;
        }
        if is_waves(&p.name, &p.path) && self.keys.iter().any(|k| k.starts_with("waveshell")) {
            return true;
        }
        self.keys
            .iter()
            .any(|k| k.len() >= 4 && (key.ends_with(k.as_str()) || (k.ends_with(key.as_str()) && key.len() >= 4)))
    }
}

/// Reads the project at `path` into a save and its detail, with `installed` saying which plugins are here and
/// `samples` finding its samples. A file no reader knows gives `None`; one that can't be read gives a save that says
/// why, as the C# app lists it. Asks the disk, so never on the window's thread.
pub fn read_save(path: &str, installed: &Installed, samples: &SampleLocator) -> Option<(Save, SaveDetail)> {
    let daw = daw_of(path)?;
    let meta = std::fs::metadata(path).ok()?;
    let seconds = mtime(&meta);
    let modified =
        DateTime::from_timestamp(seconds, 0).map(|t| t.with_timezone(&Local).naive_local()).unwrap_or_default();
    let mut save = Save {
        path: path.to_owned(),
        name: stem(file_name(path)).to_owned(),
        daw: daw.to_owned(),
        folder: Path::new(path).parent().and_then(|p| p.to_str()).unwrap_or_default().to_owned(),
        size: i64::try_from(meta.len()).unwrap_or(i64::MAX),
        modified,
        mtime: seconds,
        ..Save::default()
    };
    let read = match daw {
        "FL Studio" => fl::read(Path::new(path)),
        "Bitwig Studio" => bitwig::read(Path::new(path)),
        _ => studio_one::read(Path::new(path)),
    };
    let info = match read {
        Ok(info) => info.dedupe(),
        Err(e) => {
            save.error = Some(format!("{e}"));
            return Some((save.clone(), SaveDetail { save, ..SaveDetail::default() }));
        }
    };
    let detail = fill(&mut save, info, installed, samples);
    Some((save, detail))
}

fn fill(save: &mut Save, info: ProjectInfo, installed: &Installed, samples: &SampleLocator) -> SaveDetail {
    save.tempo = info.tempo;
    save.version = info.version.clone();
    save.confidence = info.confidence;
    save.created = info.created.or(save.created);
    save.track_count = i32::try_from(info.tracks.len()).unwrap_or(i32::MAX);
    let mut plugins: Vec<ProjectPlugin> = info
        .plugins
        .iter()
        .map(|p| ProjectPlugin {
            name: p.name.clone(),
            vendor: p.vendor.clone(),
            kind: p.kind,
            format: p.format.clone(),
            path: p.path.clone(),
            installed: installed.has(p),
        })
        .collect();
    // Instruments first, then by name, as the library lists them.
    plugins.sort_by_cached_key(|p| (p.kind != PluginKind::Instrument, p.name.to_lowercase()));
    let mut found: Vec<ProjectSample> = info
        .samples
        .iter()
        .map(|s| ProjectSample {
            path: s.clone(),
            found: samples.found(s, &save.path),
            unplayed: info.unplayed.contains_key(s),
        })
        .collect();
    // Missing ones first, then by path.
    found.sort_by_cached_key(|s| (s.found, s.path.to_lowercase()));
    let mut hardware: Vec<String> = info
        .plugins
        .iter()
        .map(|p| p.name.as_str())
        .chain(info.hardware.iter().map(String::as_str))
        .filter_map(elektron_device)
        .map(str::to_owned)
        .collect();
    hardware.sort();
    hardware.dedup();
    save.plugin_count = u32::try_from(plugins.len()).unwrap_or(u32::MAX);
    save.missing_plugins = u32::try_from(plugins.iter().filter(|p| !p.installed).count()).unwrap_or(u32::MAX);
    save.sample_count = u32::try_from(found.len()).unwrap_or(u32::MAX);
    save.missing_samples = u32::try_from(found.iter().filter(|s| !s.found).count()).unwrap_or(u32::MAX);
    save.plugin_names = plugins.iter().map(|p| p.name.clone()).collect();
    save.hardware = hardware;
    SaveDetail {
        save: save.clone(),
        title: info.title,
        plugins,
        samples: found,
        tracks: info.tracks,
        details: info.details.into_iter().collect(),
        warnings: info.warnings,
        can_extract_midi: info.can_extract_midi,
        can_extract_presets: info.can_extract_presets,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_names_match_without_their_endings() {
        assert_eq!(name_key("Serum (x64)"), "serum");
        assert_eq!(name_key("Serum_x64"), "serum");
        assert_eq!(name_key("Serumx64"), "serum");
        assert_eq!(name_key("Pro-Q 3 (VST3)"), "proq3");
        assert_eq!(name_key("Kontakt 7"), "kontakt7");
    }

    fn plugin(name: &str) -> PluginRef {
        PluginRef { name: name.into(), format: "VST3".into(), ..PluginRef::default() }
    }

    #[test]
    fn installed_as_the_csharp_app_decides() {
        let none = Installed::default();
        assert!(none.has(&plugin("Anything")), "no plugin scan yet: nothing counts as missing");
        let some = Installed { keys: ["serum".into(), "fabfilterproq3".into()].into(), ..Installed::default() }
            .with_scans([("ABCD".to_owned(), r"C:\VST\Renamed.dll".to_owned())]);
        assert!(some.has(&plugin("Serum (x64)")));
        assert!(some.has(&plugin("Pro-Q 3")), "the project says Name, the file Vendor Name");
        assert!(some.has(&PluginRef { id: "ABCD".into(), ..plugin("Something Else") }));
        assert!(some.has(&PluginRef { format: "Native".into(), ..plugin("Fruity Reverb 2") }));
        assert!(!some.has(&plugin("Gone Synth")));
    }
}
