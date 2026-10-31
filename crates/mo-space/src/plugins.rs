//! The plugins installed on this computer, from the plugin files in the plugin folders: the C# app's
//! `PluginCatalog.ScanFiles`, `DefaultFolders`, `Family` and `IsWaves`, as Space needs them.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};

use mo_formats::case;
use mo_formats::paths::{extension, stem};
use mo_formats::plugins::{format_from_path, name_from_path};
use mo_formats::textscan::is_plugin_path;
use rayon::prelude::*;
use regex::Regex;

use crate::key;
use crate::size::Stopped;

/// A plugin file on this computer: its name, format ("VST2", "VST3", "CLAP", "AU", "AAX") and where it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledPlugin {
    pub name: String,
    pub format: String,
    pub path: String,
}

/// A plugin's name without its version: "Ozone 8" and "Ozone 12" are both "ozone", "Serum_x64" is "serum".
pub fn family(name: &str) -> String {
    static TAIL: LazyLock<Option<Regex>> =
        LazyLock::new(|| Regex::new(r"(?i)[\s_-]*(v?\d+(\.\d+)*|mk\s*[ivx\d]+|x64|x86|vst3?|\(.*\))$|\s+v$").ok());
    let mut n = name.trim().to_owned();
    if let Some(tail) = TAIL.as_ref() {
        loop {
            let next = tail.replace(&n, "").trim().to_owned();
            if next == n || next.is_empty() {
                n = next;
                break;
            }
            n = next;
        }
    }
    norm(&n)
}

/// A name in small letters with only its letters and digits (as .NET's `char.IsLetterOrDigit` keeps them).
pub(crate) fn norm(s: &str) -> String {
    case::to_lower(s).chars().filter(|c| c.is_alphabetic() || c.is_numeric()).collect()
}

/// Waves plugins all live inside one WaveShell file whose version changes with every update.
pub fn is_waves(name: &str, path: &str) -> bool {
    let starts = |s: &str, start: &str| s.get(..start.len()).is_some_and(|p| case::eq_ignore_case(p, start));
    starts(name, "Waves ") || starts(name, "WaveShell") || case::key(path).contains("WAVESHELL")
}

/// The standard plugin folders for this operating system.
pub fn default_folders() -> Vec<PathBuf> {
    let home = crate::home_folder();
    let mut out = Vec::new();
    if cfg!(target_os = "macos") {
        let mut roots = vec![PathBuf::from("/Library/Audio/Plug-Ins")];
        roots.extend(home.map(|h| h.join("Library/Audio/Plug-Ins")));
        for root in roots {
            for kind in ["VST3", "CLAP", "VST", "Components"] {
                out.push(root.join(kind));
            }
        }
    } else if cfg!(windows) {
        let env = |name: &str| std::env::var_os(name).map(PathBuf::from);
        if let Some(common) = env("CommonProgramFiles") {
            out.extend(["VST3", "CLAP", "VST2"].map(|k| common.join(k)));
        }
        if let Some(pf) = env("ProgramFiles") {
            out.push(pf.join("VSTPlugins"));
            out.push(pf.join("Steinberg").join("VSTPlugins"));
            out.push(pf.join("Common Files").join("VST3"));
        }
    } else {
        if let Some(h) = home {
            out.extend([".vst3", ".clap", ".vst"].map(|k| h.join(k)));
        }
        for root in ["/usr/lib", "/usr/local/lib"] {
            out.extend(["vst3", "clap", "vst"].map(|k| Path::new(root).join(k)));
        }
    }
    out
}

/// Where AAX plugins live (they aren't among the folders the app scans for your DAWs).
pub fn aax_folders() -> Vec<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("CommonProgramFiles")
            .map(|c| PathBuf::from(c).join("Avid").join("Audio").join("Plug-Ins"))
            .into_iter()
            .collect()
    } else if cfg!(target_os = "macos") {
        vec![PathBuf::from("/Library/Application Support/Avid/Audio/Plug-Ins")]
    } else {
        Vec::new()
    }
}

/// Whether Pro Tools is installed, so AAX plugins are in use.
pub fn pro_tools_installed() -> bool {
    if cfg!(target_os = "macos") {
        return Path::new("/Applications/Pro Tools.app").is_dir();
    }
    std::env::var_os("ProgramFiles").is_some_and(|pf| PathBuf::from(pf).join("Avid").join("Pro Tools").is_dir())
}

/// Plugin bundles are folders (a .vst3 on Windows; on a Mac .component, .clap and .vst too): what's inside one isn't a
/// plugin of its own.
const BUNDLES: [&str; 5] = [".vst3", ".component", ".clap", ".vst", ".aaxplugin"];

/// How deep a plugin folder is walked: far deeper than any real one.
const DEEPEST: usize = 32;

/// Every plugin file under the given folders, with its format; each file once. Folders moved to another drive with a
/// link in their place are followed (their plugins are still installed), each real folder once, so a link back up
/// can't go round for ever. Folders that can't be read are left out. A level of folders at a time over every core of
/// the pool it's called in.
pub fn scan_files(folders: &[PathBuf], stop: &AtomicBool) -> Result<Vec<InstalledPlugin>, Stopped> {
    let mut found = Vec::new();
    let mut seen_files: HashSet<String> = HashSet::new();
    let mut seen_dirs: HashSet<String> = HashSet::new();
    let mut level: Vec<PathBuf> = folders.to_vec();
    for _ in 0..DEEPEST {
        if level.is_empty() {
            break;
        }
        if stop.load(Ordering::Relaxed) {
            return Err(Stopped);
        }
        let real: Vec<String> = level
            .par_iter()
            .map(|d| fs::canonicalize(d).map_or_else(|_| key(&d.to_string_lossy()), |r| key(&r.to_string_lossy())))
            .collect();
        let todo: Vec<PathBuf> =
            level.into_iter().zip(real).filter(|(_, r)| seen_dirs.insert(r.clone())).map(|(d, _)| d).collect();
        let read: Vec<(Vec<InstalledPlugin>, Vec<PathBuf>)> = todo.par_iter().map(|d| list_plugins(d)).collect();
        level = Vec::new();
        for (plugins, dirs) in read {
            found.extend(plugins.into_iter().filter(|p| seen_files.insert(key(&p.path))));
            level.extend(dirs);
        }
    }
    if stop.load(Ordering::Relaxed) { Err(Stopped) } else { Ok(found) }
}

/// The plugin files in one folder, and the folders in it to look in next (a bundle's insides aren't looked in).
fn list_plugins(dir: &Path) -> (Vec<InstalledPlugin>, Vec<PathBuf>) {
    let (mut plugins, mut dirs) = (Vec::new(), Vec::new());
    let Ok(entries) = fs::read_dir(dir) else { return (plugins, dirs) };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        let path = entry.path();
        // A link counts as what it leads to.
        let is_dir = if kind.is_symlink() { fs::metadata(&path).is_ok_and(|m| m.is_dir()) } else { kind.is_dir() };
        let text = path.to_string_lossy().into_owned();
        if let Some((name, format)) = plugin_file(&text) {
            // Two letters is a real name: KORG's M1 is M1.vst3 and M1.dll.
            if name.chars().count() >= 2 {
                plugins.push(InstalledPlugin { name, format, path: text.clone() });
            }
            if BUNDLES.iter().any(|b| case::eq_ignore_case(extension(&text), b)) {
                continue;
            }
        }
        if is_dir {
            dirs.push(path);
        }
    }
    (plugins, dirs)
}

/// Whether a file is a plugin (a plugin file's ending, or a `.so` on Linux).
pub(crate) fn is_plugin_file(path: &str) -> bool {
    plugin_file(path).is_some()
}

/// A plugin file's format ("VST3"), when it is one.
pub(crate) fn format_of(path: &str) -> Option<String> {
    plugin_file(path).map(|(_, format)| format)
}

/// A plugin file's name and format. On Linux a VST2 plugin is a `.so` file (in ~/.vst and the like).
fn plugin_file(path: &str) -> Option<(String, String)> {
    if is_plugin_path(path) {
        return Some((name_from_path(path), format_from_path(path).to_owned()));
    }
    if cfg!(target_os = "linux") && path.ends_with(".so") {
        return Some((stem(path).to_owned(), "VST2".to_owned()));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_family_is_the_name_without_its_version() {
        assert_eq!(family("Ozone 8"), "ozone");
        assert_eq!(family("Ozone 12"), "ozone");
        assert_eq!(family("Serum_x64"), "serum");
        assert_eq!(family("Serum 2"), "serum");
        assert_eq!(family("FabFilter Pro-Q 3"), "fabfilterproq");
        assert_eq!(family("Minimoog V"), "minimoog");
        assert_eq!(family("Jupiter-8 V4"), "jupiter");
        assert_eq!(family("Prophet-5 MkII"), "prophet");
        assert_eq!(family("Kontakt 7 (64 bit)"), "kontakt");
        assert_eq!(family("M1"), "m");
        assert_eq!(family(""), "");
    }

    #[test]
    fn waves_live_in_a_waveshell() {
        assert!(is_waves("Waves L2", ""));
        assert!(is_waves("WaveShell1-VST3 15.0", ""));
        assert!(is_waves("L2", r"C:\Program Files\Common Files\VST3\WaveShell1-VST3 15.0_x64.vst3"));
        assert!(!is_waves("Wavetable", ""));
    }
}
